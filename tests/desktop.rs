//! Tests of the desktop integration files in `data/`: the `.desktop` file
//! and the application icons, and that both packages install all of them.
//!
//! Desktops match a window to `<app ID>.desktop` by its application ID and
//! take the icon named there from the icon theme, so the file names, the
//! `Icon=` key and `md_viewer::APP_ID` must all agree; otherwise GNOME shows
//! a generic icon.

use md_viewer::{APP_ID, APP_NAME, MARKDOWN_MIME_TYPES};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root.
fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The `.desktop` file's path, relative to the repository root.
fn desktop_file() -> String {
    format!("data/{APP_ID}.desktop")
}

/// The full-colour icon's path, relative to the repository root.
fn scalable_icon() -> String {
    format!("data/icons/hicolor/scalable/apps/{APP_ID}.svg")
}

/// The symbolic icon's path, relative to the repository root.
fn symbolic_icon() -> String {
    format!("data/icons/hicolor/symbolic/apps/{APP_ID}-symbolic.svg")
}

/// Reads a file relative to the repository root.
fn read(relative: &str) -> String {
    fs::read_to_string(root().join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

/// Parses the `.desktop` file's `[Desktop Entry]` group into key/value
/// pairs, checking that it is the only group and that no key repeats.
fn desktop_entry() -> HashMap<String, String> {
    let text = read(&desktop_file());
    let mut lines = text
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    assert_eq!(lines.next(), Some("[Desktop Entry]"));
    let mut entry = HashMap::new();
    for line in lines {
        assert!(!line.starts_with('['), "unexpected group {line}");
        let (key, value) = line.split_once('=').expect("key=value line");
        let previous = entry.insert(key.to_string(), value.to_string());
        assert!(previous.is_none(), "duplicate key {key}");
    }
    entry
}

/// Every regular file under `data/`, relative to the repository root.
fn data_files() -> Vec<String> {
    /// Collects the files under `dir` into `files`, recursively.
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, files);
            } else {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root().join("data"), &mut files);
    let mut files: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    files
}

/// The `.desktop` file is named after the application ID and names the
/// application and its icon by it.
#[test]
fn desktop_file_matches_app_id() {
    assert!(root().join(desktop_file()).is_file());
    let entry = desktop_entry();
    assert_eq!(entry["Icon"], APP_ID);
    assert_eq!(entry["Name"], APP_NAME);
    assert_eq!(entry["Type"], "Application");
}

/// The entry starts the installed binary with the chosen file (`%f`: at
/// most one local file, which is what the command line accepts).
#[test]
fn desktop_file_launches_binary_with_one_file() {
    let entry = desktop_entry();
    assert_eq!(entry["Exec"], format!("{} %f", env!("CARGO_PKG_NAME")));
    assert_eq!(entry["Terminal"], "false");
}

/// The entry is listed in the app grid (the viewer works without a file)
/// and offered for Markdown files, with the same MIME types the file
/// chooser filters on.
#[test]
fn desktop_file_is_visible_and_handles_markdown() {
    let entry = desktop_entry();
    assert!(!entry.contains_key("NoDisplay"));
    assert!(!entry.contains_key("Hidden"));
    let mime_types: Vec<&str> = entry["MimeType"]
        .strip_suffix(';')
        .expect("MimeType ends with ;")
        .split(';')
        .collect();
    assert_eq!(mime_types, MARKDOWN_MIME_TYPES);
    assert!(entry["Categories"].split(';').any(|c| c == "Viewer"));
}

/// If `desktop-file-validate` is installed, the `.desktop` file passes it;
/// otherwise the test says it is skipped.
#[test]
fn desktop_file_validates() {
    let path = root().join(desktop_file());
    match Command::new("desktop-file-validate").arg(&path).output() {
        Ok(output) => assert!(
            output.status.success() && output.stdout.is_empty(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(_) => eprintln!("skipped: desktop-file-validate is not installed"),
    }
}

/// The full-colour icon is a square SVG that renders, and draws something.
#[test]
fn scalable_icon_renders_square() {
    let image = md_viewer::graphics::rasterize(&read(&scalable_icon()), 16.0, 1.0).unwrap();
    assert_eq!((image.width, image.height), (128, 128));
    assert!(image.pixels.chunks(4).any(|pixel| pixel[3] > 0));
}

/// The symbolic icon is 16x16, renders, and uses fills only: GTK recolours
/// symbolic icons by their fill, so a stroke would keep its own colour.
#[test]
fn symbolic_icon_renders_16px_with_fills_only() {
    let svg = read(&symbolic_icon());
    let image = md_viewer::graphics::rasterize(&svg, 16.0, 1.0).unwrap();
    assert_eq!((image.width, image.height), (16, 16));
    assert!(image.pixels.chunks(4).any(|pixel| pixel[3] > 0));
    assert!(!svg.contains("stroke"), "symbolic icon uses a stroke");
}

/// `data/` holds exactly the `.desktop` file and the two icons, so the
/// packaging checks below cover everything there.
#[test]
fn data_holds_desktop_file_and_icons() {
    let mut expected = vec![desktop_file(), scalable_icon(), symbolic_icon()];
    expected.sort();
    assert_eq!(data_files(), expected);
}

/// The `.deb` installs every file under `data/` (cargo-deb's `assets` in
/// `Cargo.toml`) into the matching directory under `/usr/share`.
#[test]
fn deb_installs_every_data_file() {
    let cargo_toml = read("Cargo.toml");
    for file in data_files() {
        let relative = file.strip_prefix("data/").unwrap();
        let directory = match relative.rsplit_once('/') {
            Some((dir, _)) => format!("usr/share/{dir}/"),
            None => "usr/share/applications/".to_string(),
        };
        let asset = format!("[\"{file}\", \"{directory}\", \"644\"]");
        assert!(cargo_toml.contains(&asset), "Cargo.toml lacks {asset}");
    }
}

/// The FreeBSD package stages every file under `data/` into the matching
/// directory under `/usr/local/share` (`.woodpecker/release.yaml`).
#[test]
fn freebsd_pkg_stages_every_data_file() {
    let release = read(".woodpecker/release.yaml");
    for file in data_files() {
        let relative = file.strip_prefix("data/").unwrap();
        let directory = match relative.rsplit_once('/') {
            Some((dir, _)) => format!("stage-root/usr/local/share/{dir}/"),
            None => "stage-root/usr/local/share/applications/".to_string(),
        };
        let step = format!("cp {file} {directory}");
        assert!(release.contains(&step), "release.yaml lacks `{step}`");
    }
}

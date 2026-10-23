//! End-to-end tests of the `md_viewer` binary: command line handling, and,
//! when a display is available, starting the GUI.
//!
//! Argument errors, `--help` and `--version` are handled before GTK starts,
//! so those tests always run. Tests that open a window need a display; they
//! are skipped (and say so) when neither `DISPLAY` nor `WAYLAND_DISPLAY` is
//! set, so the suite still passes on headless CI.

use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Runs the binary with `args` and waits for it to exit.
fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_md_viewer"))
        .args(args)
        .output()
        .expect("run md_viewer")
}

/// True if a display is available to open windows on; otherwise prints why
/// the calling test is skipped.
fn have_display() -> bool {
    let available = ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
    if !available {
        eprintln!("skipped: no DISPLAY or WAYLAND_DISPLAY");
    }
    available
}

/// `--version` prints the program name and the `VERSION` file's version.
#[test]
fn version_flag() {
    let output = run(&["--version"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("md_viewer {}", md_viewer::version())
    );
}

/// `--help` describes the program and its optional `FILE` argument.
#[test]
fn help_flag() {
    let output = run(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("View a Markdown document"), "{help}");
    assert!(help.contains("[FILE]"), "{help}");
}

/// More than one file is an argument error: usage and status 2.
#[test]
fn too_many_arguments() {
    let output = run(&["a.md", "b.md"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("[FILE]"));
}

/// With a display, a file that cannot be read is reported on stderr and
/// the program exits with status 1.
#[test]
fn unreadable_file_exits_with_error() {
    if !have_display() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.md");
    let output = run(&[missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cannot open"), "{stderr}");
    assert!(stderr.contains("missing.md"), "{stderr}");
}

/// Starts the viewer with `args`, lets it run for a few seconds, then sends
/// SIGTERM and checks that the viewer stayed up, shut down cleanly (status
/// 0) and printed no GTK criticals or panics.
fn run_and_close(args: &[&str]) {
    run_for_and_close(args, Duration::from_secs(5));
}

/// [`run_and_close`], sending SIGTERM after `time` instead.
fn run_for_and_close(args: &[&str], time: Duration) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_md_viewer"))
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start md_viewer");
    let started = Instant::now();
    while started.elapsed() < time {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("md_viewer exited early with {status}");
        }
        sleep(Duration::from_millis(100));
    }
    // SIGTERM, not kill(): the viewer must shut down cleanly.
    let pid = child.id().to_string();
    let terminated = Command::new("kill").args(["-TERM", &pid]).status().unwrap();
    assert!(terminated.success());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "exit status {}", output.status);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("CRITICAL"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// Opens `fixture` (relative to `tests/fixtures`) with `run_and_close`.
fn open_and_close(fixture: &str) {
    let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
    run_and_close(&[&path]);
}

/// With a display and no file, the viewer opens its empty start window,
/// keeps running and exits cleanly on SIGTERM.
#[test]
fn no_file_opens_empty_window() {
    if have_display() {
        run_and_close(&[]);
    }
}

/// With a display, the showcase document (every feature) opens, keeps
/// running and exits cleanly on SIGTERM.
#[test]
fn showcase_opens() {
    if have_display() {
        open_and_close("showcase.md");
    }
}

/// With a display, the human-facing showcase at the repository root
/// (`showcase.md`, the demo for people trying the viewer) opens, keeps
/// running, prints no GTK criticals and exits cleanly on SIGTERM.
#[test]
fn root_showcase_opens() {
    if have_display() {
        run_and_close(&[concat!(env!("CARGO_MANIFEST_DIR"), "/showcase.md")]);
    }
}

/// With a display, the fallbacks fixture (formulas and diagrams that cannot
/// be rendered, math in centred and right-aligned cells) opens and exits
/// cleanly.
#[test]
fn fallbacks_open() {
    if have_display() {
        open_and_close("fallbacks.md");
    }
}

/// Writes a large document to `dir`: many sections, each with inline and
/// display math, a diagram, a table with a formula and a code block, so
/// loading it takes a while and goes through every loading phase.
fn large_document(dir: &std::path::Path) -> std::path::PathBuf {
    let mut text = String::new();
    for i in 0..40 {
        text += &format!("## Section {i}\n\nInline $x_{{{i}}}^2$ and $\\ce{{H2O}}$.\n\n");
        text += &format!("$$\\int_0^{{{i}}} e^{{-t}}\\,dt$$\n\n");
        text += &format!("```mermaid\ngraph TD;\n  A{i}[Start {i}] --> B{i}[End {i}];\n```\n\n");
        text += &format!("| n | value |\n|---|---|\n| {i} | $\\sqrt{{{i}}}$ |\n\n");
        text += &format!("```rust\nlet n = {i};\n```\n\n");
    }
    let path = dir.join("large.md");
    std::fs::write(&path, text).unwrap();
    path
}

/// With a display, a large document (graphics rendered in the background,
/// blocks appended as they complete) opens and exits cleanly.
#[test]
fn large_document_opens() {
    if have_display() {
        let dir = tempfile::tempdir().unwrap();
        let path = large_document(dir.path());
        run_and_close(&[path.to_str().unwrap()]);
    }
}

/// With a display, closing the viewer while a large document is still
/// loading (workers busy, the loading spinner up) exits cleanly too.
#[test]
fn closing_while_loading() {
    if have_display() {
        let dir = tempfile::tempdir().unwrap();
        let path = large_document(dir.path());
        for millis in [1500, 2500] {
            run_for_and_close(&[path.to_str().unwrap()], Duration::from_millis(millis));
        }
    }
}

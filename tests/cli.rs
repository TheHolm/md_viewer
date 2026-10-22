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
    let mut child = Command::new(env!("CARGO_BIN_EXE_md_viewer"))
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start md_viewer");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
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

/// With a display, the fallbacks fixture (formulas and diagrams that cannot
/// be rendered, math in centred and right-aligned cells) opens and exits
/// cleanly.
#[test]
fn fallbacks_open() {
    if have_display() {
        open_and_close("fallbacks.md");
    }
}

//! Tests that the `VERSION` file is well formed, that `md_viewer::version`
//! reports it, and that the copy in `Cargo.toml` has not drifted from it.

use md_viewer::version;
use std::fs;
use std::path::Path;

/// Reads the `VERSION` file from the repository root at test run time (not
/// the copy embedded at compile time), so these tests see the file as it is.
fn version_file() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("VERSION");
    fs::read_to_string(path).expect("read VERSION")
}

/// Returns true if `text` is a plain `X.Y.Z` version: three dot-separated,
/// non-empty runs of ASCII digits.
fn is_semver_triple(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// The helper accepts `X.Y.Z` and rejects anything else, so the tests below
/// really do catch a malformed `VERSION`.
#[test]
fn semver_triple_helper() {
    assert!(is_semver_triple("0.1.0"));
    assert!(is_semver_triple("10.20.30"));
    for bad in [
        "", "1", "1.2", "1.2.3.4", "v1.2.3", "1.2.x", "1..3", " 1.2.3",
    ] {
        assert!(!is_semver_triple(bad), "{bad:?} should be rejected");
    }
}

/// `VERSION` is exactly one `X.Y.Z` line, terminated by a single newline.
#[test]
fn version_file_is_one_xyz_line() {
    let content = version_file();
    let line = content
        .strip_suffix('\n')
        .expect("VERSION must end with a newline");
    assert!(!line.contains('\n'), "VERSION must be a single line");
    assert!(is_semver_triple(line), "VERSION {line:?} is not X.Y.Z");
}

/// `version()` reports exactly the contents of `VERSION`.
#[test]
fn version_matches_version_file() {
    assert_eq!(version(), version_file().trim());
}

/// The `version` in `Cargo.toml` (seen here as `CARGO_PKG_VERSION`) is a copy
/// of `VERSION` and must match it; this fails when one is bumped without the
/// other.
#[test]
fn cargo_toml_matches_version_file() {
    assert_eq!(
        env!("CARGO_PKG_VERSION"),
        version_file().trim(),
        "Cargo.toml version differs from VERSION; update Cargo.toml to match"
    );
}

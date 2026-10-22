//! Integration tests for reading documents from disk, the error message,
//! the window title, and the decisions made when opening a document (where
//! it opens, where the file chooser starts, how failures are reported).

use md_viewer::{
    chooser_folder, error_message, open_document, open_target, read_document, window_title, Block,
    Inline, Origin, Span, Style, Target,
};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

/// Writes `content` to a fresh temporary file and returns it, keeping it
/// alive for as long as the returned handle lives.
fn temp_file(content: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create temp file");
    file.write_all(content).expect("write temp file");
    file
}

/// `read_document` returns the file's contents unchanged.
#[test]
fn read_document_returns_contents() {
    let file = temp_file(b"# Hello\n\nworld\n");
    assert_eq!(read_document(file.path()).unwrap(), "# Hello\n\nworld\n");
}

/// Reading a document never modifies the file.
#[test]
fn read_document_does_not_modify_file() {
    let file = temp_file(b"text");
    let before = fs::metadata(file.path()).unwrap().modified().unwrap();
    read_document(file.path()).unwrap();
    open_document(file.path()).unwrap();
    assert_eq!(fs::read(file.path()).unwrap(), b"text");
    assert_eq!(
        fs::metadata(file.path()).unwrap().modified().unwrap(),
        before
    );
}

/// An empty file reads as an empty string.
#[test]
fn read_document_empty_file() {
    let file = temp_file(b"");
    assert_eq!(read_document(file.path()).unwrap(), "");
}

/// A missing file is reported as `NotFound`.
#[test]
fn read_document_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let error = read_document(&dir.path().join("missing.md")).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
}

/// A directory cannot be read as a document.
#[test]
fn read_document_directory_fails() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_document(dir.path()).is_err());
}

/// A file that is not valid UTF-8 is reported as `InvalidData`.
#[test]
fn read_document_invalid_utf8() {
    let file = temp_file(&[0x66, 0xff, 0xfe, 0x6f]);
    let error = read_document(file.path()).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidData);
}

/// `open_document` reads and renders in one step.
#[test]
fn open_document_renders() {
    let file = temp_file(b"# Hi");
    assert_eq!(
        open_document(file.path()).unwrap(),
        vec![Block::Text(vec![Inline::Text(Span {
            text: "Hi".to_string(),
            styles: vec![Style::Heading(1)],
        })])]
    );
}

/// `open_document` passes read errors through.
#[test]
fn open_document_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let error = open_document(&dir.path().join("missing.md")).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotFound);
}

/// The error message names the file and includes the underlying error.
#[test]
fn error_message_names_file() {
    let error = std::io::Error::new(ErrorKind::NotFound, "gone");
    assert_eq!(
        error_message(Path::new("docs/a.md"), &error),
        "Cannot open docs/a.md: gone"
    );
}

/// The window title is the file name (not the full path) and the app name.
#[test]
fn window_title_uses_file_name() {
    assert_eq!(
        window_title(Path::new("some/dir/notes.md")),
        "notes.md — MD Viewer"
    );
}

/// A path without a file name component falls back to the app name alone.
#[test]
fn window_title_without_file_name() {
    assert_eq!(window_title(Path::new("/")), "MD Viewer");
    assert_eq!(window_title(Path::new("")), "MD Viewer");
}

/// A failure to open the command line's file ends the program; one picked
/// in the file chooser is shown in a dialog instead.
#[test]
fn only_command_line_errors_exit() {
    assert!(Origin::CommandLine.exits_on_error());
    assert!(!Origin::Chooser.exits_on_error());
}

/// An empty window is reused for the chosen document; a window already
/// showing one keeps it, and the new document gets a new window.
#[test]
fn open_target_reuses_only_empty_windows() {
    assert_eq!(open_target(false), Target::SameWindow);
    assert_eq!(open_target(true), Target::NewWindow);
}

/// The chooser starts in the shown document's folder.
#[test]
fn chooser_folder_is_document_folder() {
    assert_eq!(
        chooser_folder(Some(Path::new("/some/dir/notes.md"))),
        Some(Path::new("/some/dir").to_path_buf())
    );
}

/// A relative document path is resolved against the current directory, so
/// a bare file name starts the chooser there.
#[test]
fn chooser_folder_resolves_relative_paths() {
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(
        chooser_folder(Some(Path::new("notes.md"))),
        Some(cwd.clone())
    );
    assert_eq!(
        chooser_folder(Some(Path::new("docs/notes.md"))),
        Some(cwd.join("docs"))
    );
}

/// An empty window (or a path with no parent) leaves the chooser's folder
/// to GTK.
#[test]
fn chooser_folder_without_document() {
    assert_eq!(chooser_folder(None), None);
    assert_eq!(chooser_folder(Some(Path::new("/"))), None);
    assert_eq!(chooser_folder(Some(Path::new(""))), None);
}

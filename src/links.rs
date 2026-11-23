//! Link destinations and the clickable ranges of a document, pure and
//! tested.
//!
//! The renderer keeps each link's destination on its [`Span`](crate::Span)
//! (`span.link`); the GUI turns those into clickable text. This module
//! resolves a destination against the document being viewed
//! ([`resolve_link`]), generates the anchor a heading links to
//! ([`heading_slug`]), and tracks the character ranges that are links
//! ([`LinkRanges`]).

use crate::ImageState;
use std::path::{Path, PathBuf};
use url::Url;

/// Where a Markdown link destination leads, resolved against the document
/// being viewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// Open in the desktop's default handler (`http`, `https`, `mailto`,
    /// `file` to a non-Markdown file).
    Web(String),
    /// A local Markdown document to open in the viewer, with an optional
    /// `#fragment` to scroll to once it is shown.
    Markdown(PathBuf, Option<String>),
    /// Another local file to open in its default handler.
    File(PathBuf),
    /// A same-document `#fragment`, scrolled to within the current document.
    Anchor(String),
    /// Nothing to do: an empty, unsafe or unsupported destination.
    Ignored,
}

/// Resolves `dest`, a Markdown link destination, against `base` (the path of
/// the document being viewed).
///
/// Web and mail links are kept verbatim; `file:` and relative destinations
/// are resolved to local paths; a fragment naming the current document is an
/// anchor within it; any other scheme (`javascript:`, `data:`, ...) is
/// ignored.
pub fn resolve_link(base: &Path, dest: &str) -> Resolved {
    if dest.is_empty() {
        return Resolved::Ignored;
    }
    let Some(base_url) = file_url(base) else {
        return Resolved::Ignored;
    };
    let Ok(joined) = base_url.join(dest) else {
        return Resolved::Ignored;
    };
    let fragment = joined.fragment().map(str::to_string);
    match joined.scheme() {
        "http" | "https" | "mailto" => Resolved::Web(joined.to_string()),
        "file" => {
            let Some(path) = joined.to_file_path().ok() else {
                return Resolved::Ignored;
            };
            let same = same_file(&path, base);
            if same {
                return fragment.map_or(Resolved::Ignored, Resolved::Anchor);
            }
            if let Some(fragment) = fragment {
                if is_markdown(&path) {
                    Resolved::Markdown(path, Some(fragment))
                } else {
                    Resolved::File(path)
                }
            } else if is_markdown(&path) {
                Resolved::Markdown(path, None)
            } else {
                Resolved::File(path)
            }
        }
        _ => Resolved::Ignored,
    }
}

/// Resolves `src`, a Markdown image destination, against `document` (the path
/// of the document being viewed).
///
/// Only local files are ever loaded: a relative or absolute path (or a
/// `file:` URL) becomes [`ImageState::Local`]; `http`/`https` (and a
/// protocol-relative `//host/...`) become [`ImageState::Remote`], which the
/// viewer does not support; any other scheme or an empty destination becomes
/// [`ImageState::Unsupported`] with the reason to show.
pub fn resolve_image(document: &Path, src: &str) -> ImageState {
    let src = src.trim();
    if src.is_empty() {
        return ImageState::Unsupported("empty source".to_string());
    }
    if src.starts_with("//") {
        return ImageState::Remote;
    }
    let Some(base) = file_url(document) else {
        return ImageState::Unsupported("cannot resolve path".to_string());
    };
    let Ok(joined) = base.join(src) else {
        return ImageState::Unsupported(format!("invalid destination: {src}"));
    };
    match joined.scheme() {
        "http" | "https" => ImageState::Remote,
        "file" => {
            let mut joined = joined;
            joined.set_fragment(None);
            match joined.to_file_path() {
                Ok(path) => ImageState::Local(path),
                Err(()) => ImageState::Unsupported(format!("unsupported file URL: {src}")),
            }
        }
        scheme => ImageState::Unsupported(format!("unsupported scheme: {scheme}")),
    }
}

/// Whether two paths name the same file, compared lexically after making
/// both absolute (the target may not exist yet, so they are not
/// canonicalized).
fn same_file(a: &Path, b: &Path) -> bool {
    let a = std::path::absolute(a);
    let b = std::path::absolute(b);
    match (a, b) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A `file:` URL for `path`, made absolute first so `Url::from_file_path`
/// accepts it.
fn file_url(path: &Path) -> Option<Url> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    Url::from_file_path(absolute).ok()
}

/// The `file:` URI for a local file, for handing to the desktop's default
/// handler.
pub fn file_uri(path: &Path) -> Option<String> {
    file_url(path).map(|url| url.to_string())
}

/// Whether `path` has a Markdown file extension (`*.md` or `*.markdown`),
/// case-insensitively.
fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

/// The anchor a heading with `text` is addressed by: GitHub-style
/// lower-casing, spaces and other separators collapse to a single hyphen,
/// and anything that is not a letter, digit, underscore or hyphen is
/// dropped. Duplicate suffixes (`-1`, `-2`, ...) are added by the caller,
/// which knows the order the headings appear in.
pub fn heading_slug(text: &str) -> String {
    let mut slug = String::new();
    let mut pending_hyphen = false;
    for ch in text.trim().chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_alphanumeric() || ch == '_' {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            pending_hyphen = false;
            slug.push(ch);
        } else if ch == '-' {
            pending_hyphen = false;
            slug.push(ch);
        } else if ch.is_whitespace() {
            if !slug.is_empty() {
                pending_hyphen = true;
            }
        }
        // Any other character (punctuation) is dropped, and is not a
        // separator by itself.
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// A clickable range of the document's text, in character offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkRange {
    /// The first character offset of the link text.
    pub start: usize,
    /// The offset one past the last character of the link text.
    pub end: usize,
    /// The link destination.
    pub dest: String,
}

/// The clickable ranges of a document, accumulated in document order as
/// blocks are appended to the page.
#[derive(Debug, Default)]
pub struct LinkRanges {
    ranges: Vec<LinkRange>,
}

impl LinkRanges {
    /// Records a link over `start..end` (character offsets).
    pub fn push(&mut self, start: usize, end: usize, dest: String) {
        if start < end {
            self.ranges.push(LinkRange { start, end, dest });
        }
    }

    /// The destination of the link covering character `offset`, if any.
    pub fn dest_at(&self, offset: usize) -> Option<&str> {
        self.ranges
            .iter()
            .find(|range| range.start <= offset && offset < range.end)
            .map(|range| range.dest.as_str())
    }
}

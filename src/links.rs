//! Link destinations and the clickable ranges of a document, pure and
//! tested.
//!
//! The renderer keeps each link's destination on its [`Span`](crate::Span)
//! (`span.link`); the GUI turns those into clickable text. This module
//! resolves a destination against the document being viewed
//! ([`resolve_link`]), finds the headings of a text block
//! ([`heading_starts`]), generates the anchor a heading links to
//! ([`heading_slug`], [`Anchors`]), decides what a click on a link does and
//! how its tooltip describes that ([`LinkAction`], [`local_file_risk`],
//! [`link_tooltip`]), and tracks the character ranges that are links
//! ([`LinkRanges`]).
//!
//! Nothing here fetches anything: web links are only ever handed to the
//! user's browser (see "Hard decisions" in `AGENTS.md`).

use crate::{ImageState, Inline, Style};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use url::Url;

/// Where a Markdown link destination leads, resolved against the document
/// being viewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// An `http`/`https` URL, handed to the browser (never fetched by the
    /// viewer, even for a Markdown file).
    Web(String),
    /// A `mailto:` URL, handed to the mail client.
    Mail(String),
    /// A local Markdown document to open in the viewer, with an optional
    /// `#fragment` to scroll to once it is shown.
    Markdown(PathBuf, Option<String>),
    /// Another local file to open in its default handler.
    File(PathBuf),
    /// A same-document `#fragment`, scrolled to within the current document.
    Anchor(String),
    /// Never opened, for the reason given (an unsupported scheme, a file on
    /// another computer).
    Refused(String),
    /// Nothing to do: an empty destination, or the current document itself.
    Ignored,
}

/// Resolves `dest`, a Markdown link destination, against `base` (the path of
/// the document being viewed).
///
/// Web and mail links are kept verbatim; `file:` and relative destinations
/// are resolved to local paths; a fragment naming the current document is an
/// anchor within it; any other scheme (`javascript:`, `data:`, `ftp:`, ...)
/// and a `file:` URL naming another computer are refused.
pub fn resolve_link(base: &Path, dest: &str) -> Resolved {
    if dest.is_empty() {
        return Resolved::Ignored;
    }
    let Some(base_url) = file_url(base) else {
        return Resolved::Ignored;
    };
    let Ok(joined) = base_url.join(dest) else {
        return Resolved::Refused("not a valid link".to_string());
    };
    let fragment = joined.fragment().map(decode_fragment);
    match joined.scheme() {
        "http" | "https" => Resolved::Web(joined.to_string()),
        "mailto" => Resolved::Mail(joined.to_string()),
        "file" => {
            let Some(path) = joined.to_file_path().ok() else {
                return Resolved::Refused("files on other computers are not opened".to_string());
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
        scheme => Resolved::Refused(format!("{scheme}: links are not supported")),
    }
}

/// What is known about a local file a link names, as the GUI finds it out
/// (GIO's content types are MIME types on Linux and FreeBSD).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFacts {
    /// False for a FIFO, device, directory or socket.
    pub regular: bool,
    /// True if any executable permission bit is set.
    pub executable_bit: bool,
    /// The content type its name suggests (its extension).
    pub name_type: String,
    /// The content type its first bytes suggest, `None` if they say nothing
    /// definite (`application/octet-stream`, or nothing could be read).
    pub data_type: Option<String>,
    /// True if GIO says either content type can be executable (programs,
    /// scripts, launchers).
    pub can_be_executable: bool,
}

/// Content types a local file may have to be opened without asking:
/// whole families (`image/`, `audio/`, `video/`) or exact types.
const SAFE_TYPE_PREFIXES: &[&str] = &["image/", "audio/", "video/"];

/// Exact content types that are safe to open without asking.
const SAFE_TYPES: &[&str] = &["application/pdf", "text/plain"];

/// Content types that are never safe despite their family: an SVG can carry
/// scripts (and often opens in a browser).
const UNSAFE_TYPES: &[&str] = &["image/svg+xml", "image/svg+xml-compressed"];

/// True if `content_type` is on the safe list.
fn is_safe_type(content_type: &str) -> bool {
    !UNSAFE_TYPES.contains(&content_type)
        && (SAFE_TYPES.contains(&content_type)
            || SAFE_TYPE_PREFIXES
                .iter()
                .any(|prefix| content_type.starts_with(prefix)))
}

/// Why opening the local file `name` (described by `facts`) is potentially
/// dangerous, or `None` if it may be opened without asking: it is a regular
/// file, not executable, and of a safe type (images, PDF, plain text, audio,
/// video) by both its name and its content.
pub fn local_file_risk(name: &str, facts: &FileFacts) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if !facts.regular {
        return Some(
            "it is not a regular file (a device, pipe or folder), and opening it could do anything"
                .to_string(),
        );
    }
    if lower.ends_with(".desktop") || facts.name_type == "application/x-desktop" {
        return Some("it is a desktop launcher (.desktop), which can run any command".to_string());
    }
    if facts.executable_bit {
        return Some(
            "it is marked as executable and could run a program on your computer".to_string(),
        );
    }
    if facts.can_be_executable {
        let shown = facts.data_type.as_deref().unwrap_or(&facts.name_type);
        return Some(format!(
            "it is a program or script ({shown}) and could run code on your computer"
        ));
    }
    if let Some(data_type) = &facts.data_type {
        if !is_safe_type(data_type) && is_safe_type(&facts.name_type) {
            return Some(format!(
                "its name says {} but its content is {data_type}",
                facts.name_type
            ));
        }
        if !is_safe_type(data_type) {
            return Some(format!(
                "its type ({data_type}) is not known to be safe; the application that opens it could run code"
            ));
        }
    }
    if !is_safe_type(&facts.name_type) {
        return Some(format!(
            "its type ({}) is not known to be safe; the application that opens it could run code",
            facts.name_type
        ));
    }
    None
}

/// What clicking a link does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkAction {
    /// Hand the URL to the browser.
    Browse(String),
    /// Hand the `mailto:` URL to the mail client.
    Mail(String),
    /// Open the Markdown file in the viewer, then go to the fragment.
    Markdown(PathBuf, Option<String>),
    /// Go to a heading of this document.
    Anchor(String),
    /// Open a safe local file in its default application.
    OpenFile(PathBuf),
    /// Ask before opening a potentially dangerous local file, for this
    /// reason.
    AskFile(PathBuf, String),
    /// The local file does not exist.
    Missing(PathBuf),
    /// Never opened, for this reason.
    Refused(String),
    /// Nothing to do.
    Nothing,
}

/// Decides what a click on a link resolved to `resolved` does; `facts`
/// describes a local file (`None` if it does not exist).
pub fn link_action(
    resolved: Resolved,
    facts: impl FnOnce(&Path) -> Option<FileFacts>,
) -> LinkAction {
    match resolved {
        Resolved::Web(url) => LinkAction::Browse(url),
        Resolved::Mail(url) => LinkAction::Mail(url),
        Resolved::Markdown(path, fragment) => LinkAction::Markdown(path, fragment),
        Resolved::Anchor(fragment) => LinkAction::Anchor(fragment),
        Resolved::Refused(reason) => LinkAction::Refused(reason),
        Resolved::Ignored => LinkAction::Nothing,
        Resolved::File(path) => match facts(&path) {
            None => LinkAction::Missing(path),
            Some(facts) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match local_file_risk(&name, &facts) {
                    Some(reason) => LinkAction::AskFile(path, reason),
                    None => LinkAction::OpenFile(path),
                }
            }
        },
    }
}

/// The tooltip of a link whose click does `action`: what will happen, so
/// refused and dangerous links are clear before clicking. `dest` is the
/// destination as written.
pub fn link_tooltip(dest: &str, action: &LinkAction) -> String {
    match action {
        LinkAction::Browse(url) => format!("Open in browser: {url}"),
        LinkAction::Mail(url) => format!(
            "Write an email to {}",
            url.strip_prefix("mailto:").unwrap_or(url)
        ),
        LinkAction::Markdown(path, _) => format!("Open in MD Viewer: {}", path.display()),
        LinkAction::Anchor(_) => format!("Go to section: {dest}"),
        LinkAction::OpenFile(path) => {
            format!("Open with the default application: {}", path.display())
        }
        LinkAction::AskFile(path, reason) => format!(
            "⚠ Potentially dangerous: {reason}. Asks before opening.\n{}",
            path.display()
        ),
        LinkAction::Missing(path) => format!("File not found: {}", path.display()),
        LinkAction::Refused(reason) => format!("Not opened: {reason}\n{dest}"),
        LinkAction::Nothing => dest.to_string(),
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

/// Percent-decodes a URL `fragment` back to the text it was written as: the
/// URL parser encodes non-ASCII characters and spaces (`#über` becomes
/// `#%C3%BCber`), while heading anchors are plain Unicode. Invalid UTF-8
/// after decoding is replaced rather than rejected.
fn decode_fragment(fragment: &str) -> String {
    percent_encoding::percent_decode_str(fragment)
        .decode_utf8_lossy()
        .into_owned()
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

/// The anchor a heading with `text` is addressed by: lower-cased, runs of
/// whitespace collapse to a single hyphen, and anything that is not a
/// letter, digit, underscore or hyphen is dropped (close to GitLab's rule).
/// Duplicate suffixes (`-1`, `-2`, ...) are added by the caller, which
/// knows the order the headings appear in. [`github_slug`] is the other
/// spelling a heading is found by.
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

/// The anchor GitHub gives a heading with `text` (github-slugger): trimmed
/// and lower-cased, anything other than a letter, digit, `_`, `-` or
/// whitespace dropped, and every whitespace character (not runs of them)
/// turned into a hyphen, a trailing one included. `Install - Linux` is
/// `install---linux` here and `install--linux` in [`heading_slug`];
/// tables of contents written for GitHub use this spelling.
pub fn github_slug(text: &str) -> String {
    text.trim()
        .chars()
        .flat_map(char::to_lowercase)
        .filter_map(|ch| {
            if ch.is_whitespace() {
                Some('-')
            } else if ch.is_alphanumeric() || ch == '_' || ch == '-' {
                Some(ch)
            } else {
                None
            }
        })
        .collect()
}

/// The headings in a text block's `inlines`: for each, the index of its
/// first inline and its text (what [`heading_slug`] is computed from).
///
/// A text block holds many paragraphs, lists and headings in a row, so a
/// heading is a run of text spans carrying a [`Style::Heading`] of the same
/// level. Formulas and images carry no styles; one next to heading text (at
/// its start, inside it or at its end) belongs to the heading, but a heading
/// needs at least one span of heading text. Any other text (the blank lines
/// between blocks, a paragraph, a list marker) ends a heading.
pub fn heading_starts(inlines: &[Inline]) -> Vec<(usize, String)> {
    /// A heading being gathered: its first inline, text so far, and level
    /// (`None` while only formulas or images have been seen).
    struct Run {
        start: usize,
        text: String,
        level: Option<u8>,
    }
    let mut headings = Vec::new();
    let mut run: Option<Run> = None;
    let finish = |run: Option<Run>, headings: &mut Vec<(usize, String)>| {
        if let Some(run) = run.filter(|run| run.level.is_some()) {
            headings.push((run.start, run.text));
        }
    };
    for (index, inline) in inlines.iter().enumerate() {
        match inline {
            Inline::Text(span) => match heading_level(&span.styles) {
                Some(level) => {
                    let continues = run
                        .as_ref()
                        .is_some_and(|run| run.level.is_none_or(|l| l == level));
                    if !continues {
                        finish(run.take(), &mut headings);
                        run = Some(Run {
                            start: index,
                            text: String::new(),
                            level: None,
                        });
                    }
                    let current = run.as_mut().expect("run was just started");
                    current.level = Some(level);
                    current.text.push_str(&span.text);
                }
                None => finish(run.take(), &mut headings),
            },
            Inline::Math(_) | Inline::Image(_) => {
                let current = run.get_or_insert_with(|| Run {
                    start: index,
                    text: String::new(),
                    level: None,
                });
                current.text.push_str(&inline.text());
            }
        }
    }
    finish(run, &mut headings);
    headings
}

/// The heading level among `styles`, if the text is part of a heading.
fn heading_level(styles: &[Style]) -> Option<u8> {
    styles.iter().find_map(|style| match style {
        Style::Heading(level) => Some(*level),
        _ => None,
    })
}

/// The headings of a document, by anchor, as they are appended to the page.
///
/// Each heading gets the anchor [`heading_slug`] gives its text, and is
/// also found by its GitHub spelling ([`github_slug`]); in each spelling a
/// repeated anchor gets `-1`, `-2`, ... appended in document order, like
/// GitHub. Our own spelling wins when both name different headings, so a
/// link that worked keeps its meaning. `offset` is wherever the caller wants
/// to scroll to (the GUI uses the heading's first buffer character).
#[derive(Debug, Default)]
pub struct Anchors {
    offsets: HashMap<String, i32>,
    github: HashMap<String, i32>,
}

/// Adds `base` (with the first free `-N` suffix if it is taken) to `map`
/// for `offset`, returning the anchor it got, or `None` for an empty base.
fn register_unique(map: &mut HashMap<String, i32>, base: String, offset: i32) -> Option<String> {
    if base.is_empty() {
        return None;
    }
    let mut slug = base.clone();
    let mut n = 1;
    while map.contains_key(&slug) {
        slug = format!("{base}-{n}");
        n += 1;
    }
    map.insert(slug.clone(), offset);
    Some(slug)
}

impl Anchors {
    /// Registers a heading with `text` at `offset`, returning its anchor, or
    /// `None` if the text has no anchor (it is only punctuation or spaces).
    ///
    /// The heading is also registered under its GitHub spelling.
    pub fn register(&mut self, text: &str, offset: i32) -> Option<String> {
        register_unique(&mut self.github, github_slug(text), offset);
        register_unique(&mut self.offsets, heading_slug(text), offset)
    }

    /// The offset of the heading `fragment` names (a decoded link fragment,
    /// as [`resolve_link`] returns it), if it has been registered.
    pub fn lookup(&self, fragment: &str) -> Option<i32> {
        self.offsets
            .get(fragment)
            .or_else(|| self.github.get(fragment))
            .copied()
    }
}

/// Every heading of a parsed document under the anchor it will have, with
/// the index of its block as the offset: the same anchors (and duplicate
/// suffixes) the GUI registers as the blocks are appended, known before
/// they are, so a link to a heading not on the page yet can render up to
/// it.
pub fn heading_blocks(blocks: &[crate::Block]) -> Anchors {
    let mut anchors = Anchors::default();
    for (index, block) in blocks.iter().enumerate() {
        if let crate::Block::Text(inlines) = block {
            for (_, text) in heading_starts(inlines) {
                anchors.register(&text, i32::try_from(index).unwrap_or(i32::MAX));
            }
        }
    }
    anchors
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

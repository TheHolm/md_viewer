//! Integration tests for `md_viewer::links`: resolving link destinations
//! against a document, generating heading anchors, tracking clickable
//! ranges, and the links inside label text.

use md_viewer::links::{heading_slug, resolve_link, LinkRanges, Resolved};
use md_viewer::{label_text, Inline, Style};
use std::path::Path;

/// A document path the tests resolve links against.
fn base() -> &'static Path {
    Path::new("/home/user/docs/readme.md")
}

// --- resolve_link ---------------------------------------------------------

/// Web and mail destinations are handed over verbatim.
#[test]
fn web_and_mail_destinations() {
    assert_eq!(
        resolve_link(base(), "https://example.com/a?b#c"),
        Resolved::Web("https://example.com/a?b#c".to_string())
    );
    assert_eq!(
        resolve_link(base(), "mailto:someone@example.com"),
        Resolved::Web("mailto:someone@example.com".to_string())
    );
}

/// Empty, unsafe and unsupported destinations are ignored.
#[test]
fn ignored_destinations() {
    assert_eq!(resolve_link(base(), ""), Resolved::Ignored);
    assert_eq!(
        resolve_link(base(), "javascript:alert(1)"),
        Resolved::Ignored
    );
    assert_eq!(
        resolve_link(base(), "data:text/plain,hi"),
        Resolved::Ignored
    );
    assert_eq!(
        resolve_link(base(), "ftp://example.com/x"),
        Resolved::Ignored
    );
}

/// A fragment alone names a heading in the current document.
#[test]
fn same_document_fragment() {
    assert_eq!(
        resolve_link(base(), "#intro"),
        Resolved::Anchor("intro".to_string())
    );
    // The same file, addressed by name, plus a fragment, is also an anchor.
    assert_eq!(
        resolve_link(base(), "readme.md#intro"),
        Resolved::Anchor("intro".to_string())
    );
    // A link back to the current file with no fragment is pointless.
    assert_eq!(resolve_link(base(), "readme.md"), Resolved::Ignored);
}

/// A relative Markdown file resolves against the document's directory.
#[test]
fn relative_markdown() {
    assert_eq!(
        resolve_link(base(), "guide.md"),
        Resolved::Markdown(Path::new("/home/user/docs/guide.md").to_path_buf(), None)
    );
    assert_eq!(
        resolve_link(base(), "sub/guide.md#part"),
        Resolved::Markdown(
            Path::new("/home/user/docs/sub/guide.md").to_path_buf(),
            Some("part".to_string()),
        )
    );
}

/// A relative non-Markdown file opens in the default handler.
#[test]
fn relative_file() {
    assert_eq!(
        resolve_link(base(), "image.png"),
        Resolved::File(Path::new("/home/user/docs/image.png").to_path_buf())
    );
    // A fragment on a non-Markdown file is dropped.
    assert_eq!(
        resolve_link(base(), "image.png#frag"),
        Resolved::File(Path::new("/home/user/docs/image.png").to_path_buf())
    );
}

/// Absolute paths and percent-encoded names resolve like the shell.
#[test]
fn absolute_and_percent_encoded() {
    assert_eq!(
        resolve_link(base(), "/etc/readme.md"),
        Resolved::Markdown(Path::new("/etc/readme.md").to_path_buf(), None)
    );
    assert_eq!(
        resolve_link(base(), "my%20doc.md"),
        Resolved::Markdown(Path::new("/home/user/docs/my doc.md").to_path_buf(), None)
    );
}

// --- heading_slug ---------------------------------------------------------

/// Slugs lowercase and collapse whitespace to hyphens.
#[test]
fn slug_basic() {
    assert_eq!(heading_slug("Hello World"), "hello-world");
    assert_eq!(heading_slug("  Hello   World  "), "hello-world");
    assert_eq!(heading_slug("Multi\nline"), "multi-line");
}

/// Punctuation is dropped rather than becoming a separator.
#[test]
fn slug_punctuation() {
    assert_eq!(heading_slug("3.14 Pi"), "314-pi");
    assert_eq!(heading_slug("!!!Punctuation!!!"), "punctuation");
    assert_eq!(heading_slug("What? Yes."), "what-yes");
}

/// Underscores and hyphens are kept, unicode letters survive lowercased.
#[test]
fn slug_unicode_and_separators() {
    assert_eq!(heading_slug("A_B C"), "a_b-c");
    assert_eq!(heading_slug("Already-Hyphenated"), "already-hyphenated");
    assert_eq!(heading_slug("Café Olé"), "café-olé");
}

/// A heading with no usable characters yields an empty slug.
#[test]
fn slug_empty() {
    assert_eq!(heading_slug("!!!"), "");
    assert_eq!(heading_slug("   "), "");
}

// --- LinkRanges -----------------------------------------------------------

/// Ranges resolve an offset to its destination and gaps to nothing.
#[test]
fn link_ranges_find() {
    let mut ranges = LinkRanges::default();
    ranges.push(0, 4, "a".to_string());
    ranges.push(10, 14, "b".to_string());
    assert_eq!(ranges.dest_at(0), Some("a"));
    assert_eq!(ranges.dest_at(3), Some("a"));
    assert_eq!(ranges.dest_at(4), None);
    assert_eq!(ranges.dest_at(9), None);
    assert_eq!(ranges.dest_at(10), Some("b"));
    assert_eq!(ranges.dest_at(13), Some("b"));
    assert_eq!(ranges.dest_at(14), None);
}

/// Empty ranges are not recorded.
#[test]
fn link_ranges_skip_empty() {
    let mut ranges = LinkRanges::default();
    ranges.push(5, 5, "x".to_string());
    assert_eq!(ranges.dest_at(5), None);
}

// --- label_text links -----------------------------------------------------

/// `label_text` reports each link's destination over the label's byte range.
#[test]
fn label_text_links() {
    let inlines = vec![
        Inline::Text(md_viewer::Span {
            text: "a".to_string(),
            styles: vec![Style::Link],
            link: Some("u".to_string()),
        }),
        Inline::Text(md_viewer::Span {
            text: " ".to_string(),
            styles: Vec::new(),
            link: None,
        }),
        Inline::Text(md_viewer::Span {
            text: "b".to_string(),
            styles: vec![Style::Strong, Style::Link],
            link: Some("v".to_string()),
        }),
    ];
    let content = label_text(&inlines);
    assert_eq!(content.text, "a b");
    assert_eq!(
        content.links,
        vec![
            md_viewer::LabelLink {
                range: 0..1,
                dest: "u".to_string(),
            },
            md_viewer::LabelLink {
                range: 2..3,
                dest: "v".to_string(),
            },
        ]
    );
}

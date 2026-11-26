//! Integration tests for `md_viewer::links`: resolving link destinations
//! against a document, generating heading anchors, tracking clickable
//! ranges, and the links inside label text.

use md_viewer::links::{
    file_uri, github_slug, heading_blocks, heading_slug, heading_starts, link_action, link_tooltip,
    local_file_risk, resolve_image, resolve_link, Anchors, FileFacts, LinkAction, LinkRanges,
    Resolved,
};
use md_viewer::{label_text, render, Block, ImageState, Inline, Style};
use std::path::{Path, PathBuf};

/// A document path the tests resolve links against.
fn base() -> &'static Path {
    Path::new("/home/user/docs/readme.md")
}

// --- resolve_link ---------------------------------------------------------

/// Web and mail destinations are handed over verbatim, each to its own
/// application; a remote Markdown file is a web page too (never fetched).
#[test]
fn web_and_mail_destinations() {
    assert_eq!(
        resolve_link(base(), "https://example.com/a?b#c"),
        Resolved::Web("https://example.com/a?b#c".to_string())
    );
    assert_eq!(
        resolve_link(base(), "http://example.com/notes.md"),
        Resolved::Web("http://example.com/notes.md".to_string())
    );
    assert_eq!(
        resolve_link(base(), "mailto:someone@example.com"),
        Resolved::Mail("mailto:someone@example.com".to_string())
    );
}

/// An empty destination does nothing; unsafe and unsupported schemes, and
/// a `file:` URL naming another computer, are refused with the reason.
#[test]
fn refused_destinations() {
    assert_eq!(resolve_link(base(), ""), Resolved::Ignored);
    for (dest, reason) in [
        ("javascript:alert(1)", "javascript: links are not supported"),
        ("data:text/plain,hi", "data: links are not supported"),
        ("ftp://example.com/x", "ftp: links are not supported"),
        ("smb://server/share/x", "smb: links are not supported"),
        (
            "file://server/share/x.md",
            "files on other computers are not opened",
        ),
    ] {
        assert_eq!(
            resolve_link(base(), dest),
            Resolved::Refused(reason.to_string()),
            "{dest}"
        );
    }
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

/// Fragments are percent-decoded back to the text they were written as, so
/// they can match a heading's Unicode anchor: the URL parser encodes
/// non-ASCII characters and spaces. This holds for same-document anchors and
/// for fragments on links to other documents.
#[test]
fn fragments_are_percent_decoded() {
    assert_eq!(
        resolve_link(base(), "#über-uns"),
        Resolved::Anchor("über-uns".to_string())
    );
    assert_eq!(
        resolve_link(base(), "#%C3%BCber-uns"),
        Resolved::Anchor("über-uns".to_string())
    );
    assert_eq!(
        resolve_link(base(), "#日本語"),
        Resolved::Anchor("日本語".to_string())
    );
    assert_eq!(
        resolve_link(base(), "guide.md#caf%C3%A9"),
        Resolved::Markdown(
            PathBuf::from("/home/user/docs/guide.md"),
            Some("café".to_string())
        )
    );
    // Invalid UTF-8 is replaced rather than dropping the link.
    assert_eq!(
        resolve_link(base(), "#a%FFb"),
        Resolved::Anchor("a\u{fffd}b".to_string())
    );
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

/// A trailing hyphen (from a literal `-` or trailing whitespace) is dropped,
/// so a slug never ends in one.
#[test]
fn slug_drops_trailing_hyphen() {
    assert_eq!(heading_slug("foo-"), "foo");
    assert_eq!(heading_slug("a - "), "a");
    assert_eq!(heading_slug("Hello World-"), "hello-world");
}

/// GitHub's spelling keeps every whitespace character as a hyphen (no
/// collapsing, a trailing one kept), drops punctuation and keeps letters,
/// digits, `_` and `-`.
#[test]
fn github_slugs() {
    assert_eq!(github_slug("Install - Linux"), "install---linux");
    assert_eq!(github_slug("  Hello   World  "), "hello---world");
    assert_eq!(github_slug("foo -"), "foo--");
    assert_eq!(github_slug("What? Yes."), "what-yes");
    assert_eq!(github_slug("A_B C"), "a_b-c");
    assert_eq!(github_slug("Café Olé"), "café-olé");
    assert_eq!(github_slug("!!!"), "");
}

/// A heading is found by our anchor and by GitHub's; duplicates count up
/// in each spelling; our spelling wins when it names another heading.
#[test]
fn anchors_accept_github_spelling() {
    let mut anchors = Anchors::default();
    assert_eq!(
        anchors.register("Install - Linux", 10).as_deref(),
        Some("install--linux")
    );
    assert_eq!(anchors.lookup("install--linux"), Some(10));
    assert_eq!(anchors.lookup("install---linux"), Some(10));
    anchors.register("Install - Linux", 20);
    assert_eq!(anchors.lookup("install--linux-1"), Some(20));
    assert_eq!(anchors.lookup("install---linux-1"), Some(20));
    // `a  b` is `a-b` here and `a--b` on GitHub; a later heading that is
    // `a--b` in our spelling keeps that anchor for itself.
    anchors.register("a  b", 30);
    anchors.register("a--b", 40);
    assert_eq!(anchors.lookup("a-b"), Some(30));
    assert_eq!(anchors.lookup("a--b"), Some(40));
    assert_eq!(anchors.lookup("nowhere"), None);
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

// --- resolve_image --------------------------------------------------------

/// Relative and absolute paths, and `file:` URLs, become local files resolved
/// against the document's folder.
#[test]
fn image_local_paths() {
    assert_eq!(
        resolve_image(base(), "pics/one.png"),
        ImageState::Local(PathBuf::from("/home/user/docs/pics/one.png"))
    );
    assert_eq!(
        resolve_image(base(), "/tmp/two.png"),
        ImageState::Local(PathBuf::from("/tmp/two.png"))
    );
    assert_eq!(
        resolve_image(base(), "file:///tmp/three.png"),
        ImageState::Local(PathBuf::from("/tmp/three.png"))
    );
}

/// Network destinations are remote: the viewer never loads them.
#[test]
fn image_remote() {
    assert_eq!(
        resolve_image(base(), "https://example.com/a.png"),
        ImageState::Remote
    );
    assert_eq!(
        resolve_image(base(), "http://example.com/a.png"),
        ImageState::Remote
    );
    assert_eq!(
        resolve_image(base(), "//example.com/a.png"),
        ImageState::Remote
    );
}

/// Other schemes and an empty source are unsupported, with a reason.
#[test]
fn image_unsupported() {
    for src in ["data:image/png;base64,AA", "javascript:alert(1)", "   "] {
        assert!(
            matches!(resolve_image(base(), src), ImageState::Unsupported(_)),
            "{src:?} should be unsupported"
        );
    }
    // A `file:` URL naming a remote host has no local path to read.
    match resolve_image(base(), "file://server/share/x.png") {
        ImageState::Unsupported(reason) => assert!(reason.contains("file URL"), "{reason}"),
        other => panic!("expected unsupported, got {other:?}"),
    }
}

/// A local destination is percent-decoded and a fragment is ignored.
#[test]
fn image_percent_decoding_and_fragment() {
    assert_eq!(
        resolve_image(base(), "my%20pic.png#part"),
        ImageState::Local(PathBuf::from("/home/user/docs/my pic.png"))
    );
}

// --- heading_starts / Anchors --------------------------------------------

/// The inlines of the single text block `markdown` renders to.
fn text_block(markdown: &str) -> Vec<Inline> {
    match render(markdown).as_slice() {
        [Block::Text(inlines)] => inlines.clone(),
        other => panic!("expected one text block, got {other:?}"),
    }
}

/// The text of each heading `heading_starts` finds in `markdown`.
fn heading_texts(markdown: &str) -> Vec<String> {
    heading_starts(&text_block(markdown))
        .into_iter()
        .map(|(_, text)| text)
        .collect()
}

/// Several headings rendered into one text block (headings, paragraphs and
/// lists share a block) are found one by one, each with only its own text,
/// and each starts at the inline holding its first character.
#[test]
fn heading_starts_separates_headings_in_one_block() {
    let inlines = text_block("# Intro\n\nSome text.\n\n## Usage\n\nMore.\n\n### Usage\n");
    let headings = heading_starts(&inlines);
    let texts: Vec<&str> = headings.iter().map(|(_, text)| text.as_str()).collect();
    assert_eq!(texts, ["Intro", "Usage", "Usage"]);
    for (start, text) in &headings {
        assert!(inlines[*start].text().starts_with(text.as_str()), "{text}");
    }
}

/// Inline markup and links inside a heading stay part of it.
#[test]
fn heading_starts_keeps_inline_markup() {
    assert_eq!(
        heading_texts("# Hello *big* [world](x.md) `code`\n\ntext"),
        ["Hello big world code"]
    );
}

/// A formula or image at the start, middle or end of a heading belongs to
/// it; one that is alone (not next to any heading text) is no heading.
#[test]
fn heading_starts_includes_formulas_and_images() {
    assert_eq!(heading_texts("# $x$ squared\n\ntext"), ["$x$ squared"]);
    assert_eq!(heading_texts("# Euler $e$ number\n"), ["Euler $e$ number"]);
    assert_eq!(heading_texts("# Logo ![cat](cat.png)\n"), ["Logo cat"]);
    // A paragraph formula after a heading is not part of it.
    assert_eq!(heading_texts("# Title\n\n$x$ text"), ["Title"]);
    assert!(heading_texts("text $x$ more").is_empty());
}

/// Headings inside lists and block quotes are found, without the list
/// marker or indentation that precede them.
#[test]
fn heading_starts_in_lists_and_quotes() {
    assert_eq!(
        heading_texts("- # In list\n- item\n\n> ## Quoted\n> text\n"),
        ["In list", "Quoted"]
    );
}

/// A document without headings has none; nor does an empty block.
#[test]
fn heading_starts_without_headings() {
    assert!(heading_texts("just *text*\n\n- a list").is_empty());
    assert!(heading_starts(&[]).is_empty());
}

/// Each heading is registered under its slug at its offset; repeats get
/// `-1`, `-2`, ... in order; text with no anchor (only punctuation) is not
/// registered; unknown fragments are not found.
#[test]
fn anchors_register_and_lookup() {
    let mut anchors = Anchors::default();
    assert_eq!(anchors.register("Intro", 0), Some("intro".to_string()));
    assert_eq!(anchors.register("Usage", 10), Some("usage".to_string()));
    assert_eq!(anchors.register("Usage", 20), Some("usage-1".to_string()));
    assert_eq!(anchors.register("Usage", 30), Some("usage-2".to_string()));
    assert_eq!(anchors.register("!!!", 40), None);
    assert_eq!(anchors.lookup("intro"), Some(0));
    assert_eq!(anchors.lookup("usage"), Some(10));
    assert_eq!(anchors.lookup("usage-1"), Some(20));
    assert_eq!(anchors.lookup("usage-2"), Some(30));
    assert_eq!(anchors.lookup("missing"), None);
}

/// A heading whose own slug looks like a duplicate suffix does not collide
/// with the generated suffixes: the next free one is taken.
#[test]
fn anchors_skip_taken_suffixes() {
    let mut anchors = Anchors::default();
    anchors.register("A", 0);
    anchors.register("A-1", 1);
    assert_eq!(anchors.register("A", 2), Some("a-2".to_string()));
}

/// End to end: a link fragment resolved from a Markdown link finds the
/// right heading of a rendered document, including a non-ASCII one.
#[test]
fn link_fragments_find_rendered_headings() {
    let inlines = text_block("# Intro\n\ntext\n\n## Über uns\n\nmore\n");
    let mut anchors = Anchors::default();
    for (start, text) in heading_starts(&inlines) {
        anchors.register(&text, start as i32);
    }
    for (dest, start) in [("#intro", 0), ("#über-uns", 2), ("#%C3%9Cber-uns", -1)] {
        let Resolved::Anchor(fragment) = resolve_link(base(), dest) else {
            panic!("{dest} is not an anchor");
        };
        let found = anchors.lookup(&fragment);
        if start < 0 {
            // Upper-case `Ü` decodes to a different anchor than `ü`.
            assert_eq!(found, None, "{dest}");
        } else {
            assert_eq!(found, Some(start), "{dest}");
        }
    }
}

// --- file_uri -------------------------------------------------------------

/// `file_uri` turns an absolute path into a `file:` URI the desktop handler
/// can take, and returns `None` for a path that cannot be made absolute
/// (here the empty path).
#[test]
fn file_uri_for_absolute_path() {
    assert_eq!(
        file_uri(Path::new("/tmp/three.png")),
        Some("file:///tmp/three.png".to_string())
    );
    assert_eq!(file_uri(Path::new("")), None);
}

// --- Link actions and safety -----------------------------------------------

/// Facts for a regular, non-executable file of type `name_type` by name and
/// `data_type` by content.
fn facts(name_type: &str, data_type: Option<&str>) -> FileFacts {
    FileFacts {
        regular: true,
        executable_bit: false,
        name_type: name_type.to_string(),
        data_type: data_type.map(str::to_string),
        can_be_executable: false,
    }
}

/// Images, PDF, plain text, audio and video open without asking, by name
/// alone or confirmed by their content.
#[test]
fn safe_file_types() {
    for (name, name_type, data_type) in [
        ("a.png", "image/png", Some("image/png")),
        ("a.jpg", "image/jpeg", None),
        ("a.pdf", "application/pdf", Some("application/pdf")),
        ("a.txt", "text/plain", Some("text/plain")),
        ("a.ogg", "audio/ogg", None),
        ("a.mp4", "video/mp4", Some("video/mp4")),
    ] {
        assert_eq!(
            local_file_risk(name, &facts(name_type, data_type)),
            None,
            "{name}"
        );
    }
}

/// Every other local file is potentially dangerous, and the reason says
/// why: executable bit, program or script, launcher, a name that lies about
/// the content, a type not known to be safe (SVG included), or no regular
/// file at all.
#[test]
fn dangerous_file_reasons() {
    let risk = |name: &str, facts: FileFacts| local_file_risk(name, &facts).expect(name);
    let executable = FileFacts {
        executable_bit: true,
        ..facts("image/png", Some("image/png"))
    };
    assert!(risk("a.png", executable).contains("marked as executable"));
    let script = FileFacts {
        can_be_executable: true,
        ..facts(
            "application/x-shellscript",
            Some("application/x-shellscript"),
        )
    };
    assert_eq!(
        risk("run.sh", script),
        "it is a program or script (application/x-shellscript) and could run code on your computer"
    );
    assert!(risk("app.desktop", facts("application/x-desktop", None)).contains("desktop launcher"));
    assert!(risk("APP.DESKTOP", facts("text/plain", None)).contains("desktop launcher"));
    assert_eq!(
        risk(
            "cat.png",
            facts("image/png", Some("application/x-executable"))
        ),
        "its name says image/png but its content is application/x-executable"
    );
    assert!(risk("a.zip", facts("application/zip", None))
        .contains("(application/zip) is not known to be safe"));
    assert!(risk("a.svg", facts("image/svg+xml", None)).contains("image/svg+xml"));
    assert!(risk(
        "x.bin",
        facts("text/plain", Some("application/x-sharedlib"))
    )
    .contains("application/x-sharedlib"));
    let pipe = FileFacts {
        regular: false,
        ..facts("image/png", None)
    };
    assert!(risk("pipe.png", pipe).contains("not a regular file"));
}

/// A click's action follows from the resolved link; a local file is
/// checked, and a missing one does nothing.
#[test]
fn link_actions() {
    let none = |_: &Path| -> Option<FileFacts> { None };
    let png = |_: &Path| Some(facts("image/png", Some("image/png")));
    let zip = |_: &Path| Some(facts("application/zip", None));
    let resolve = |dest| resolve_link(base(), dest);
    assert_eq!(
        link_action(resolve("https://a.example/"), none),
        LinkAction::Browse("https://a.example/".to_string())
    );
    assert_eq!(
        link_action(resolve("mailto:a@b.example"), none),
        LinkAction::Mail("mailto:a@b.example".to_string())
    );
    assert_eq!(
        link_action(resolve("#x"), none),
        LinkAction::Anchor("x".to_string())
    );
    assert!(matches!(
        link_action(resolve("b.md"), none),
        LinkAction::Markdown(_, None)
    ));
    let file = PathBuf::from("/home/user/docs/f.png");
    assert_eq!(
        link_action(resolve("f.png"), png),
        LinkAction::OpenFile(file.clone())
    );
    assert_eq!(
        link_action(resolve("f.png"), none),
        LinkAction::Missing(file)
    );
    assert!(matches!(
        link_action(resolve("f.zip"), zip),
        LinkAction::AskFile(_, _)
    ));
    assert!(matches!(
        link_action(resolve("ftp://x/"), none),
        LinkAction::Refused(_)
    ));
    assert_eq!(link_action(resolve(""), none), LinkAction::Nothing);
}

/// Each tooltip says what a click does; refused and dangerous links say so
/// clearly, with the reason.
#[test]
fn link_tooltips() {
    let path = PathBuf::from("/d/f.zip");
    assert_eq!(
        link_tooltip(
            "https://a.example/",
            &LinkAction::Browse("https://a.example/".into())
        ),
        "Open in browser: https://a.example/"
    );
    assert_eq!(
        link_tooltip(
            "mailto:a@b.example",
            &LinkAction::Mail("mailto:a@b.example".into())
        ),
        "Write an email to a@b.example"
    );
    assert_eq!(
        link_tooltip("x.md", &LinkAction::Markdown("/d/x.md".into(), None)),
        "Open in MD Viewer: /d/x.md"
    );
    assert_eq!(
        link_tooltip("#intro", &LinkAction::Anchor("intro".into())),
        "Go to section: #intro"
    );
    assert_eq!(
        link_tooltip("f.png", &LinkAction::OpenFile("/d/f.png".into())),
        "Open with the default application: /d/f.png"
    );
    assert_eq!(
        link_tooltip(
            "f.zip",
            &LinkAction::AskFile(path.clone(), "its type is odd".into())
        ),
        "⚠ Potentially dangerous: its type is odd. Asks before opening.\n/d/f.zip"
    );
    assert_eq!(
        link_tooltip("f.zip", &LinkAction::Missing(path)),
        "File not found: /d/f.zip"
    );
    assert_eq!(
        link_tooltip(
            "ftp://x/",
            &LinkAction::Refused("ftp: links are not supported".into())
        ),
        "Not opened: ftp: links are not supported\nftp://x/"
    );
    assert_eq!(link_tooltip("", &LinkAction::Nothing), "");
}

/// Every heading of a parsed document is known by anchor, with its block,
/// before anything is on the page: duplicates suffixed in order, headings
/// after a table or code block in later blocks.
#[test]
fn heading_blocks_maps_anchors_to_blocks() {
    let blocks =
        render("# Intro\n\n| a |\n|---|\n| b |\n\n## Usage\n\n```\ncode\n```\n\n## Usage\n");
    let anchors = heading_blocks(&blocks);
    assert_eq!(anchors.lookup("intro"), Some(0));
    assert_eq!(anchors.lookup("usage"), Some(2));
    assert_eq!(anchors.lookup("usage-1"), Some(4));
    assert_eq!(anchors.lookup("missing"), None);
    assert_eq!(heading_blocks(&[]).lookup("intro"), None);
}

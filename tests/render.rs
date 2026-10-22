//! Integration tests for the Markdown renderer (`md_viewer::render`), grouped
//! by block type and by inline style.

use md_viewer::{render, Span, Style, RULE_TEXT};

/// Concatenates the text of every span, i.e. what the user sees.
fn plain_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// Returns the styles of the span whose text is exactly `text`, panicking with
/// the full span list if there is no such span.
fn styles_of(spans: &[Span], text: &str) -> Vec<Style> {
    spans
        .iter()
        .find(|span| span.text == text)
        .unwrap_or_else(|| panic!("no span {text:?} in {spans:#?}"))
        .styles
        .clone()
}

/// Builds a span, for brevity in exact-output assertions.
fn span(text: &str, styles: &[Style]) -> Span {
    Span {
        text: text.to_string(),
        styles: styles.to_vec(),
    }
}

// --- Document level -------------------------------------------------------

/// An empty document renders to no spans at all.
#[test]
fn empty_document() {
    assert!(render("").is_empty());
}

/// Whitespace-only input renders to no visible text.
#[test]
fn whitespace_only_document() {
    assert_eq!(plain_text(&render("   \n\n  \n")), "");
}

/// Adjacent text with identical styles is merged into a single span, so plain
/// text broken across source lines is one span.
#[test]
fn adjacent_identical_styles_are_merged() {
    assert_eq!(render("one\ntwo"), vec![span("one two", &[])]);
}

/// The output never ends with a trailing newline.
#[test]
fn no_trailing_newline() {
    assert!(!plain_text(&render("# a\n\ntext\n\n")).ends_with('\n'));
}

// --- Paragraphs -----------------------------------------------------------

/// Paragraphs are separated by exactly one blank line.
#[test]
fn paragraphs_separated_by_blank_line() {
    assert_eq!(
        plain_text(&render("first\n\n\n\nsecond")),
        "first\n\nsecond"
    );
}

/// A soft line break inside a paragraph becomes a space.
#[test]
fn soft_break_becomes_space() {
    assert_eq!(plain_text(&render("a\nb")), "a b");
}

/// A hard line break (two trailing spaces, or a backslash) is kept as a
/// newline.
#[test]
fn hard_break_becomes_newline() {
    assert_eq!(plain_text(&render("a  \nb")), "a\nb");
    assert_eq!(plain_text(&render("a\\\nb")), "a\nb");
}

// --- Headings -------------------------------------------------------------

/// Each ATX heading level maps to the matching `Heading` style.
#[test]
fn heading_levels() {
    for level in 1..=6u8 {
        let source = format!("{} Title", "#".repeat(level as usize));
        assert_eq!(
            render(&source),
            vec![span("Title", &[Style::Heading(level)])]
        );
    }
}

/// Setext headings are recognised too.
#[test]
fn setext_heading() {
    assert_eq!(
        styles_of(&render("Title\n====="), "Title"),
        vec![Style::Heading(1)]
    );
    assert_eq!(
        styles_of(&render("Title\n-----"), "Title"),
        vec![Style::Heading(2)]
    );
}

/// Inline styles inside a heading combine with the heading style.
#[test]
fn heading_with_inline_style() {
    let spans = render("## A **b**");
    assert_eq!(styles_of(&spans, "A "), vec![Style::Heading(2)]);
    assert_eq!(
        styles_of(&spans, "b"),
        vec![Style::Heading(2), Style::Strong]
    );
}

/// A heading is separated from the following paragraph by a blank line and
/// the paragraph does not inherit the heading style.
#[test]
fn heading_then_paragraph() {
    assert_eq!(
        render("# T\nbody"),
        vec![span("T", &[Style::Heading(1)]), span("\n\nbody", &[])]
    );
}

// --- Lists ----------------------------------------------------------------

/// Unordered list items get a bullet and are separated by single newlines.
#[test]
fn unordered_list() {
    assert_eq!(plain_text(&render("- a\n- b\n* c")), "• a\n• b\n\n• c");
}

/// Ordered list items are numbered from the list's start number.
#[test]
fn ordered_list_numbering() {
    assert_eq!(plain_text(&render("1. a\n1. b\n1. c")), "1. a\n2. b\n3. c");
    assert_eq!(plain_text(&render("7. a\n8. b")), "7. a\n8. b");
}

/// Nested list items are indented four spaces per level, and numbering of the
/// outer list continues after the nested list.
#[test]
fn nested_lists() {
    let source = "1. a\n   - b\n     - c\n2. d";
    assert_eq!(
        plain_text(&render(source)),
        "1. a\n    • b\n        • c\n2. d"
    );
}

/// Task list items show `[x]` or `[ ]` after the bullet.
#[test]
fn task_list() {
    assert_eq!(
        plain_text(&render("- [x] done\n- [ ] todo")),
        "• [x] done\n• [ ] todo"
    );
}

/// A loose list (blank lines between items) still separates items by a
/// single newline.
#[test]
fn loose_list() {
    assert_eq!(plain_text(&render("- a\n\n- b")), "• a\n• b");
}

/// A second paragraph inside a list item starts on a new, indented line.
#[test]
fn list_item_with_two_paragraphs() {
    assert_eq!(
        plain_text(&render("- a\n\n  more\n- b")),
        "• a\n    more\n• b"
    );
}

/// A list is separated from surrounding paragraphs by blank lines.
#[test]
fn list_between_paragraphs() {
    assert_eq!(
        plain_text(&render("before\n\n- a\n\nafter")),
        "before\n\n• a\n\nafter"
    );
}

/// Inline styles inside list items are preserved; the marker is unstyled.
#[test]
fn list_item_with_inline_style() {
    let spans = render("- **x**");
    assert_eq!(styles_of(&spans, "• "), Vec::<Style>::new());
    assert_eq!(styles_of(&spans, "x"), vec![Style::Strong]);
}

// --- Block quotes ---------------------------------------------------------

/// Block quote text carries the `Quote` style.
#[test]
fn block_quote() {
    assert_eq!(render("> quoted"), vec![span("quoted", &[Style::Quote])]);
}

/// Paragraphs inside a block quote are separated by a blank line and text
/// after the quote is no longer quoted.
#[test]
fn block_quote_paragraphs_and_end() {
    let spans = render("> a\n>\n> b\n\nafter");
    assert_eq!(plain_text(&spans), "a\n\nb\n\nafter");
    assert_eq!(styles_of(&spans, "a"), vec![Style::Quote]);
    assert_eq!(styles_of(&spans, "b"), vec![Style::Quote]);
    assert_eq!(styles_of(&spans, "\n\nafter"), Vec::<Style>::new());
}

/// Nested quotes keep the single `Quote` style (styles are deduplicated).
#[test]
fn nested_block_quote() {
    assert_eq!(render("> > deep"), vec![span("deep", &[Style::Quote])]);
}

// --- Code blocks ----------------------------------------------------------

/// A fenced code block keeps its line breaks and indentation verbatim, drops
/// the final newline, and does not interpret Markdown inside it.
#[test]
fn fenced_code_block() {
    let source = "```rust\nfn main() {\n    *x*\n}\n```";
    assert_eq!(
        render(source),
        vec![span("fn main() {\n    *x*\n}", &[Style::CodeBlock])]
    );
}

/// An indented code block is rendered the same way as a fenced one.
#[test]
fn indented_code_block() {
    assert_eq!(
        render("    a\n    b"),
        vec![span("a\nb", &[Style::CodeBlock])]
    );
}

/// A code block is separated from surrounding blocks by blank lines.
#[test]
fn code_block_between_paragraphs() {
    assert_eq!(
        plain_text(&render("a\n\n```\ncode\n```\n\nb")),
        "a\n\ncode\n\nb"
    );
}

/// A code block inside a block quote carries both styles.
#[test]
fn code_block_in_quote() {
    assert_eq!(
        styles_of(&render("> ```\n> x\n> ```"), "x"),
        vec![Style::CodeBlock, Style::Quote]
    );
}

// --- Horizontal rules -----------------------------------------------------

/// A thematic break renders as the rule text with the `Rule` style, separated
/// by blank lines.
#[test]
fn horizontal_rule() {
    let spans = render("a\n\n---\n\nb");
    assert_eq!(plain_text(&spans), format!("a\n\n{RULE_TEXT}\n\nb"));
    assert_eq!(styles_of(&spans, RULE_TEXT), vec![Style::Rule]);
}

// --- Tables ---------------------------------------------------------------

/// A table is laid out as aligned monospace text: a strong header line, a
/// `-` separator line and the body rows, honouring each column's alignment.
#[test]
fn table_layout_and_alignment() {
    let source = "| l | c | r |\n|:--|:-:|--:|\n| 1 | 22 | 333 |\n| abcd | e | f |";
    assert_eq!(
        render(source),
        vec![
            span("l    | c  |   r", &[Style::Strong, Style::Table]),
            span(
                "\n-----+----+----\n1    | 22 | 333\nabcd | e  |   f",
                &[Style::Table]
            ),
        ]
    );
}

/// Rows with missing cells are padded out to the full column count.
#[test]
fn table_short_row() {
    let spans = render("| a | b |\n|---|---|\n| x |");
    assert_eq!(plain_text(&spans), "a | b\n--+--\nx |");
}

/// Inline Markdown inside cells is reduced to its text, and a link in a cell
/// shows only its text, not its destination.
#[test]
fn table_cells_with_inline_markup() {
    let spans = render("| **a** | `b` |\n|---|---|\n| [c](u) | ![d](i) |");
    assert_eq!(plain_text(&spans), "a | b\n--+-----------\nc | [image: d]");
}

/// A table is separated from surrounding paragraphs by blank lines.
#[test]
fn table_between_paragraphs() {
    let text = plain_text(&render("before\n\n| a |\n|---|\n| b |\n\nafter"));
    assert_eq!(text, "before\n\na\n-\nb\n\nafter");
}

/// Column widths count characters, not bytes, so non-ASCII text lines up.
#[test]
fn table_non_ascii_width() {
    let text = plain_text(&render("| ä | b |\n|---|---|\n| xy | z |"));
    assert_eq!(text, "ä  | b\n---+--\nxy | z");
}

// --- Emphasis, strong, strikethrough -------------------------------------

/// `*x*` and `_x_` are emphasis.
#[test]
fn emphasis() {
    assert_eq!(styles_of(&render("a *b* c"), "b"), vec![Style::Emphasis]);
    assert_eq!(styles_of(&render("a _b_ c"), "b"), vec![Style::Emphasis]);
}

/// `**x**` and `__x__` are strong.
#[test]
fn strong() {
    assert_eq!(styles_of(&render("**b**"), "b"), vec![Style::Strong]);
    assert_eq!(styles_of(&render("__b__"), "b"), vec![Style::Strong]);
}

/// `~~x~~` is struck through.
#[test]
fn strikethrough() {
    assert_eq!(styles_of(&render("~~b~~"), "b"), vec![Style::Strikethrough]);
}

/// Nested inline styles accumulate and are removed again when they end.
#[test]
fn nested_inline_styles() {
    let spans = render("***a*** ~~**b**~~ c");
    assert_eq!(styles_of(&spans, "a"), vec![Style::Emphasis, Style::Strong]);
    assert_eq!(
        styles_of(&spans, "b"),
        vec![Style::Strong, Style::Strikethrough]
    );
    assert_eq!(styles_of(&spans, " c"), Vec::<Style>::new());
}

// --- Inline code ----------------------------------------------------------

/// Inline code carries the `Code` style and is not interpreted as Markdown.
#[test]
fn inline_code() {
    let spans = render("use `*raw*` here");
    assert_eq!(plain_text(&spans), "use *raw* here");
    assert_eq!(styles_of(&spans, "*raw*"), vec![Style::Code]);
}

/// Inline code inside strong text carries both styles.
#[test]
fn inline_code_in_strong() {
    assert_eq!(
        styles_of(&render("**`x`**"), "x"),
        vec![Style::Strong, Style::Code]
    );
}

// --- Links ----------------------------------------------------------------

/// A link shows its text with the `Link` style; the URL is not shown.
#[test]
fn link_text() {
    let spans = render("see [docs](https://example.com) now");
    assert_eq!(plain_text(&spans), "see docs now");
    assert_eq!(styles_of(&spans, "docs"), vec![Style::Link]);
}

/// An autolink shows the URL itself as the link text.
#[test]
fn autolink() {
    let spans = render("<https://example.com>");
    assert_eq!(styles_of(&spans, "https://example.com"), vec![Style::Link]);
}

/// A link with empty text shows its destination instead, so it is not
/// silently dropped.
#[test]
fn empty_link_shows_destination() {
    let spans = render("[](https://example.com)");
    assert_eq!(spans, vec![span("https://example.com", &[Style::Link])]);
}

/// Styled text inside a link carries both styles.
#[test]
fn styled_link_text() {
    assert_eq!(
        styles_of(&render("[*a*](u)"), "a"),
        vec![Style::Emphasis, Style::Link]
    );
}

// --- Images ---------------------------------------------------------------

/// An image renders as an `[image: <alt text>]` placeholder.
#[test]
fn image_placeholder() {
    assert_eq!(
        render("![a cat](cat.png)"),
        vec![span("[image: a cat]", &[Style::Image])]
    );
}

/// An image without alt text still shows a placeholder.
#[test]
fn image_without_alt() {
    assert_eq!(plain_text(&render("![](cat.png)")), "[image: ]");
}

/// An image inside a link carries both styles.
#[test]
fn image_in_link() {
    assert_eq!(
        styles_of(&render("[![x](i.png)](u)"), "[image: x]"),
        vec![Style::Link, Style::Image]
    );
}

// --- Things shown as plain text ------------------------------------------

/// Inline HTML is shown verbatim as plain text.
#[test]
fn inline_html_is_plain_text() {
    assert_eq!(render("a <b>b</b> c"), vec![span("a <b>b</b> c", &[])]);
}

/// An HTML block is shown verbatim as plain text, without its trailing
/// newline, separated from other blocks by blank lines.
#[test]
fn html_block_is_plain_text() {
    assert_eq!(
        plain_text(&render("a\n\n<div>\nx\n</div>\n\nb")),
        "a\n\n<div>\nx\n</div>\n\nb"
    );
}

/// Footnote syntax (the extension is not enabled) is shown as plain text.
#[test]
fn footnote_reference_is_plain_text() {
    assert_eq!(plain_text(&render("text[^1]")), "text[^1]");
}

// --- Style tag names ------------------------------------------------------

/// Every style has a distinct tag name, so each maps to its own GTK tag.
#[test]
fn tag_names_are_unique() {
    let mut names: Vec<&str> = Style::ALL.iter().map(|style| style.tag_name()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), Style::ALL.len());
}

/// `Style::ALL` covers every heading level the renderer can produce, so the
/// GUI never applies a tag it did not create.
#[test]
fn all_styles_cover_headings() {
    for level in 1..=6u8 {
        assert!(Style::ALL.contains(&Style::Heading(level)));
    }
}

/// Out-of-range heading levels are clamped to the nearest valid tag.
#[test]
fn heading_tag_names_clamp() {
    assert_eq!(Style::Heading(0).tag_name(), "h1");
    assert_eq!(Style::Heading(1).tag_name(), "h1");
    assert_eq!(Style::Heading(6).tag_name(), "h6");
    assert_eq!(Style::Heading(9).tag_name(), "h6");
}

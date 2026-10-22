//! Integration tests for the Markdown renderer (`md_viewer::render`), grouped
//! by block type and by inline style.

use md_viewer::{render, Align, Block, Cell, Inline, Span, Style, Table, RULE_TEXT};

/// Renders `source` and flattens every text block into one list of spans,
/// skipping non-text blocks; most tests only care about flowing text.
fn r(source: &str) -> Vec<Span> {
    render(source)
        .into_iter()
        .filter_map(|block| match block {
            Block::Text(inlines) => Some(inlines),
            _ => None,
        })
        .flatten()
        .map(|inline| match inline {
            Inline::Text(span) => span,
        })
        .collect()
}

/// Concatenates the text of every span, i.e. what the user sees.
fn plain_text(spans: &[Span]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// Describes a whole rendered document as text: text blocks verbatim and
/// each table as `<table>`, to check how tables sit between other blocks.
fn outline(source: &str) -> String {
    render(source)
        .iter()
        .map(|block| match block {
            Block::Text(inlines) => inlines.iter().map(Inline::text).collect(),
            Block::Table(_) => "<table>".to_string(),
        })
        .collect()
}

/// Returns the only table in the rendered `source`, panicking otherwise.
fn only_table(source: &str) -> Table {
    let tables: Vec<Table> = render(source)
        .into_iter()
        .filter_map(|block| match block {
            Block::Table(table) => Some(table),
            _ => None,
        })
        .collect();
    assert_eq!(tables.len(), 1, "expected exactly one table");
    tables.into_iter().next().unwrap()
}

/// The visible text of a table cell.
fn cell_text(cell: &Cell) -> String {
    cell.iter().map(Inline::text).collect()
}

/// The visible text of every cell of a row.
fn row_text(row: &[Cell]) -> Vec<String> {
    row.iter().map(cell_text).collect()
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

/// Builds a cell holding one span, for exact-output assertions.
fn cell(text: &str, styles: &[Style]) -> Cell {
    vec![Inline::Text(span(text, styles))]
}

// --- Document level -------------------------------------------------------

/// An empty document renders to no spans at all.
#[test]
fn empty_document() {
    assert!(r("").is_empty());
}

/// Whitespace-only input renders to no visible text.
#[test]
fn whitespace_only_document() {
    assert_eq!(plain_text(&r("   \n\n  \n")), "");
}

/// Adjacent text with identical styles is merged into a single span, so plain
/// text broken across source lines is one span.
#[test]
fn adjacent_identical_styles_are_merged() {
    assert_eq!(r("one\ntwo"), vec![span("one two", &[])]);
}

/// The output never ends with a trailing newline.
#[test]
fn no_trailing_newline() {
    assert!(!plain_text(&r("# a\n\ntext\n\n")).ends_with('\n'));
}

// --- Paragraphs -----------------------------------------------------------

/// Paragraphs are separated by exactly one blank line.
#[test]
fn paragraphs_separated_by_blank_line() {
    assert_eq!(plain_text(&r("first\n\n\n\nsecond")), "first\n\nsecond");
}

/// A soft line break inside a paragraph becomes a space.
#[test]
fn soft_break_becomes_space() {
    assert_eq!(plain_text(&r("a\nb")), "a b");
}

/// A hard line break (two trailing spaces, or a backslash) is kept as a
/// newline.
#[test]
fn hard_break_becomes_newline() {
    assert_eq!(plain_text(&r("a  \nb")), "a\nb");
    assert_eq!(plain_text(&r("a\\\nb")), "a\nb");
}

// --- Headings -------------------------------------------------------------

/// Each ATX heading level maps to the matching `Heading` style.
#[test]
fn heading_levels() {
    for level in 1..=6u8 {
        let source = format!("{} Title", "#".repeat(level as usize));
        assert_eq!(r(&source), vec![span("Title", &[Style::Heading(level)])]);
    }
}

/// Setext headings are recognised too.
#[test]
fn setext_heading() {
    assert_eq!(
        styles_of(&r("Title\n====="), "Title"),
        vec![Style::Heading(1)]
    );
    assert_eq!(
        styles_of(&r("Title\n-----"), "Title"),
        vec![Style::Heading(2)]
    );
}

/// Inline styles inside a heading combine with the heading style.
#[test]
fn heading_with_inline_style() {
    let spans = r("## A **b**");
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
        r("# T\nbody"),
        vec![span("T", &[Style::Heading(1)]), span("\n\nbody", &[])]
    );
}

// --- Lists ----------------------------------------------------------------

/// Unordered list items get a bullet and are separated by single newlines.
#[test]
fn unordered_list() {
    assert_eq!(plain_text(&r("- a\n- b\n* c")), "• a\n• b\n\n• c");
}

/// Ordered list items are numbered from the list's start number.
#[test]
fn ordered_list_numbering() {
    assert_eq!(plain_text(&r("1. a\n1. b\n1. c")), "1. a\n2. b\n3. c");
    assert_eq!(plain_text(&r("7. a\n8. b")), "7. a\n8. b");
}

/// Nested list items are indented four spaces per level, and numbering of the
/// outer list continues after the nested list.
#[test]
fn nested_lists() {
    let source = "1. a\n   - b\n     - c\n2. d";
    assert_eq!(plain_text(&r(source)), "1. a\n    • b\n        • c\n2. d");
}

/// Task list items show `[x]` or `[ ]` after the bullet.
#[test]
fn task_list() {
    assert_eq!(
        plain_text(&r("- [x] done\n- [ ] todo")),
        "• [x] done\n• [ ] todo"
    );
}

/// A loose list (blank lines between items) still separates items by a
/// single newline.
#[test]
fn loose_list() {
    assert_eq!(plain_text(&r("- a\n\n- b")), "• a\n• b");
}

/// A second paragraph inside a list item starts on a new, indented line.
#[test]
fn list_item_with_two_paragraphs() {
    assert_eq!(plain_text(&r("- a\n\n  more\n- b")), "• a\n    more\n• b");
}

/// A list is separated from surrounding paragraphs by blank lines.
#[test]
fn list_between_paragraphs() {
    assert_eq!(
        plain_text(&r("before\n\n- a\n\nafter")),
        "before\n\n• a\n\nafter"
    );
}

/// Inline styles inside list items are preserved; the marker is unstyled.
#[test]
fn list_item_with_inline_style() {
    let spans = r("- **x**");
    assert_eq!(styles_of(&spans, "• "), Vec::<Style>::new());
    assert_eq!(styles_of(&spans, "x"), vec![Style::Strong]);
}

// --- Block quotes ---------------------------------------------------------

/// Block quote text carries the `Quote` style.
#[test]
fn block_quote() {
    assert_eq!(r("> quoted"), vec![span("quoted", &[Style::Quote])]);
}

/// Paragraphs inside a block quote are separated by a blank line and text
/// after the quote is no longer quoted.
#[test]
fn block_quote_paragraphs_and_end() {
    let spans = r("> a\n>\n> b\n\nafter");
    assert_eq!(plain_text(&spans), "a\n\nb\n\nafter");
    assert_eq!(styles_of(&spans, "a"), vec![Style::Quote]);
    assert_eq!(styles_of(&spans, "b"), vec![Style::Quote]);
    assert_eq!(styles_of(&spans, "\n\nafter"), Vec::<Style>::new());
}

/// Nested quotes keep the single `Quote` style (styles are deduplicated).
#[test]
fn nested_block_quote() {
    assert_eq!(r("> > deep"), vec![span("deep", &[Style::Quote])]);
}

// --- Code blocks ----------------------------------------------------------

/// A fenced code block keeps its line breaks and indentation verbatim, drops
/// the final newline, and does not interpret Markdown inside it.
#[test]
fn fenced_code_block() {
    let source = "```rust\nfn main() {\n    *x*\n}\n```";
    assert_eq!(
        r(source),
        vec![span("fn main() {\n    *x*\n}", &[Style::CodeBlock])]
    );
}

/// An indented code block is rendered the same way as a fenced one.
#[test]
fn indented_code_block() {
    assert_eq!(r("    a\n    b"), vec![span("a\nb", &[Style::CodeBlock])]);
}

/// A code block is separated from surrounding blocks by blank lines.
#[test]
fn code_block_between_paragraphs() {
    assert_eq!(plain_text(&r("a\n\n```\ncode\n```\n\nb")), "a\n\ncode\n\nb");
}

/// A code block inside a block quote carries both styles.
#[test]
fn code_block_in_quote() {
    assert_eq!(
        styles_of(&r("> ```\n> x\n> ```"), "x"),
        vec![Style::CodeBlock, Style::Quote]
    );
}

// --- Horizontal rules -----------------------------------------------------

/// A thematic break renders as the rule text with the `Rule` style, separated
/// by blank lines.
#[test]
fn horizontal_rule() {
    let spans = r("a\n\n---\n\nb");
    assert_eq!(plain_text(&spans), format!("a\n\n{RULE_TEXT}\n\nb"));
    assert_eq!(styles_of(&spans, RULE_TEXT), vec![Style::Rule]);
}

// --- Tables ---------------------------------------------------------------

/// A table becomes a structured block: header, rows and per-column
/// alignment; header cells are strong.
#[test]
fn table_structure_and_alignment() {
    let table = only_table("| l | c | r | n |\n|:--|:-:|--:|---|\n| 1 | 22 | 333 | 4 |");
    assert_eq!(
        table,
        Table {
            alignments: vec![Align::Left, Align::Center, Align::Right, Align::None],
            header: vec![
                cell("l", &[Style::Strong]),
                cell("c", &[Style::Strong]),
                cell("r", &[Style::Strong]),
                cell("n", &[Style::Strong]),
            ],
            rows: vec![vec![
                cell("1", &[]),
                cell("22", &[]),
                cell("333", &[]),
                cell("4", &[]),
            ]],
        }
    );
}

/// Rows with missing cells are padded out to the full column count with
/// empty cells.
#[test]
fn table_short_row() {
    let table = only_table("| a | b |\n|---|---|\n| x |");
    assert_eq!(table.columns(), 2);
    assert_eq!(row_text(&table.rows[0]), vec!["x", ""]);
    assert!(table.rows[0][1].is_empty());
}

/// A table with a header only has no body rows.
#[test]
fn table_header_only() {
    let table = only_table("| a | b |\n|---|---|");
    assert_eq!(row_text(&table.header), vec!["a", "b"]);
    assert!(table.rows.is_empty());
}

/// Inline styles inside cells are kept: strong, emphasis, code,
/// strikethrough, and the header's own strong style on top.
#[test]
fn table_cells_keep_inline_styles() {
    let table = only_table("| *h* |\n|---|\n| **a** `b` ~~c~~ |");
    assert_eq!(
        table.header[0],
        cell("h", &[Style::Emphasis, Style::Strong])
    );
    assert_eq!(
        table.rows[0][0],
        vec![
            Inline::Text(span("a", &[Style::Strong])),
            Inline::Text(span(" ", &[])),
            Inline::Text(span("b", &[Style::Code])),
            Inline::Text(span(" ", &[])),
            Inline::Text(span("c", &[Style::Strikethrough])),
        ]
    );
}

/// A link in a cell shows its text with the link style (not its
/// destination), and an image shows its placeholder.
#[test]
fn table_cells_with_links_and_images() {
    let table = only_table("| a |\n|---|\n| [c](u) ![d](i) |");
    assert_eq!(
        table.rows[0][0],
        vec![
            Inline::Text(span("c", &[Style::Link])),
            Inline::Text(span(" ", &[])),
            Inline::Text(span("[image: d]", &[Style::Image])),
        ]
    );
}

/// A table is a block of its own, separated from surrounding paragraphs by
/// blank lines, and its cell text does not leak into the text blocks.
#[test]
fn table_between_paragraphs() {
    assert_eq!(
        outline("before\n\n| a |\n|---|\n| b |\n\nafter"),
        "before\n\n<table>\n\nafter"
    );
}

/// Two consecutive tables are separate blocks with a blank line between.
#[test]
fn consecutive_tables() {
    assert_eq!(
        outline("| a |\n|---|\n\n| b |\n|---|"),
        "<table>\n\n<table>"
    );
}

/// A document that is only a table renders to just the table block.
#[test]
fn table_only_document() {
    assert_eq!(render("| a |\n|---|\n| b |").len(), 1);
}

/// A table inside a list item stays in the list: the marker comes first.
#[test]
fn table_in_list_item() {
    assert_eq!(
        outline("- item\n\n  | a |\n  |---|\n  | b |"),
        "• item\n    <table>"
    );
}

// --- Emphasis, strong, strikethrough -------------------------------------

/// `*x*` and `_x_` are emphasis.
#[test]
fn emphasis() {
    assert_eq!(styles_of(&r("a *b* c"), "b"), vec![Style::Emphasis]);
    assert_eq!(styles_of(&r("a _b_ c"), "b"), vec![Style::Emphasis]);
}

/// `**x**` and `__x__` are strong.
#[test]
fn strong() {
    assert_eq!(styles_of(&r("**b**"), "b"), vec![Style::Strong]);
    assert_eq!(styles_of(&r("__b__"), "b"), vec![Style::Strong]);
}

/// `~~x~~` is struck through.
#[test]
fn strikethrough() {
    assert_eq!(styles_of(&r("~~b~~"), "b"), vec![Style::Strikethrough]);
}

/// Nested inline styles accumulate and are removed again when they end.
#[test]
fn nested_inline_styles() {
    let spans = r("***a*** ~~**b**~~ c");
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
    let spans = r("use `*raw*` here");
    assert_eq!(plain_text(&spans), "use *raw* here");
    assert_eq!(styles_of(&spans, "*raw*"), vec![Style::Code]);
}

/// Inline code inside strong text carries both styles.
#[test]
fn inline_code_in_strong() {
    assert_eq!(
        styles_of(&r("**`x`**"), "x"),
        vec![Style::Strong, Style::Code]
    );
}

// --- Links ----------------------------------------------------------------

/// A link shows its text with the `Link` style; the URL is not shown.
#[test]
fn link_text() {
    let spans = r("see [docs](https://example.com) now");
    assert_eq!(plain_text(&spans), "see docs now");
    assert_eq!(styles_of(&spans, "docs"), vec![Style::Link]);
}

/// An autolink shows the URL itself as the link text.
#[test]
fn autolink() {
    let spans = r("<https://example.com>");
    assert_eq!(styles_of(&spans, "https://example.com"), vec![Style::Link]);
}

/// A link with empty text shows its destination instead, so it is not
/// silently dropped.
#[test]
fn empty_link_shows_destination() {
    let spans = r("[](https://example.com)");
    assert_eq!(spans, vec![span("https://example.com", &[Style::Link])]);
}

/// Styled text inside a link carries both styles.
#[test]
fn styled_link_text() {
    assert_eq!(
        styles_of(&r("[*a*](u)"), "a"),
        vec![Style::Emphasis, Style::Link]
    );
}

// --- Images ---------------------------------------------------------------

/// An image renders as an `[image: <alt text>]` placeholder.
#[test]
fn image_placeholder() {
    assert_eq!(
        r("![a cat](cat.png)"),
        vec![span("[image: a cat]", &[Style::Image])]
    );
}

/// An image without alt text still shows a placeholder.
#[test]
fn image_without_alt() {
    assert_eq!(plain_text(&r("![](cat.png)")), "[image: ]");
}

/// An image inside a link carries both styles.
#[test]
fn image_in_link() {
    assert_eq!(
        styles_of(&r("[![x](i.png)](u)"), "[image: x]"),
        vec![Style::Link, Style::Image]
    );
}

// --- Things shown as plain text ------------------------------------------

/// Inline HTML is shown verbatim as plain text.
#[test]
fn inline_html_is_plain_text() {
    assert_eq!(r("a <b>b</b> c"), vec![span("a <b>b</b> c", &[])]);
}

/// An HTML block is shown verbatim as plain text, without its trailing
/// newline, separated from other blocks by blank lines.
#[test]
fn html_block_is_plain_text() {
    assert_eq!(
        plain_text(&r("a\n\n<div>\nx\n</div>\n\nb")),
        "a\n\n<div>\nx\n</div>\n\nb"
    );
}

/// Footnote syntax (the extension is not enabled) is shown as plain text.
#[test]
fn footnote_reference_is_plain_text() {
    assert_eq!(plain_text(&r("text[^1]")), "text[^1]");
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

// --- Pango markup (table cells) ------------------------------------------

/// Plain text is escaped, and each styled span becomes a `<span>` with the
/// attributes of all its styles.
#[test]
fn pango_markup_styles_and_escaping() {
    let cell = vec![
        Inline::Text(span("a < b & \"c\"", &[])),
        Inline::Text(span("x", &[Style::Emphasis, Style::Strong])),
    ];
    assert_eq!(
        md_viewer::pango_markup(&cell),
        "a &lt; b &amp; &quot;c&quot;<span style=\"italic\" weight=\"bold\">x</span>"
    );
}

/// Every style produces markup Pango accepts: one span per style, each with
/// at least one attribute, and the text inside escaped.
#[test]
fn pango_markup_every_style() {
    for style in Style::ALL {
        let markup = md_viewer::pango_markup(&[Inline::Text(span("<t>", &[style]))]);
        assert!(markup.starts_with("<span "), "{style:?}: {markup}");
        assert!(markup.ends_with(">&lt;t&gt;</span>"), "{style:?}: {markup}");
    }
}

/// Inline code in a cell gets a monospace font and a translucent
/// background; links get the link colour and an underline.
#[test]
fn pango_markup_code_and_link() {
    let code = md_viewer::pango_markup(&[Inline::Text(span("c", &[Style::Code]))]);
    assert!(code.contains("font_family=\"monospace\""));
    assert!(code.contains(&format!("bgalpha=\"{}\"", md_viewer::CODE_BACKGROUND_ALPHA)));
    let link = md_viewer::pango_markup(&[Inline::Text(span("l", &[Style::Link]))]);
    assert!(link.contains(&format!("foreground=\"{}\"", md_viewer::LINK_COLOR)));
    assert!(link.contains("underline=\"single\""));
}

/// An empty cell produces empty markup.
#[test]
fn pango_markup_empty() {
    assert_eq!(md_viewer::pango_markup(&[]), "");
}

/// `escape_markup` escapes all five XML special characters and nothing else.
#[test]
fn escape_markup_specials() {
    assert_eq!(
        md_viewer::escape_markup("<a href='x'>&\"é"),
        "&lt;a href=&#39;x&#39;&gt;&amp;&quot;é"
    );
}

/// The colour constants are valid `#rrggbb` colours.
#[test]
fn colour_constants_are_hex() {
    for colour in [
        md_viewer::LINK_COLOR,
        md_viewer::MUTED_COLOR,
        md_viewer::CODE_BACKGROUND,
    ] {
        assert_eq!(colour.len(), 7, "{colour}");
        assert!(colour.starts_with('#'));
        assert!(
            colour[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "{colour}"
        );
    }
}

//! Non-GUI logic for `md_viewer`: reading a document from disk and rendering
//! Markdown into a list of blocks.
//!
//! Everything here is independent of GTK so it can be exercised by the
//! integration tests in `tests/` without a display. The GUI in `main.rs` only
//! walks the blocks in order: text blocks are inserted into a text buffer with
//! one text tag per [`Style`], and tables become grid widgets.

use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd};
use std::fs;
use std::io;
use std::path::Path;

/// The application name shown in the window title.
pub const APP_NAME: &str = "MD Viewer";

/// The raw contents of the repository's `VERSION` file, embedded at compile
/// time (cargo rebuilds the crate whenever the file changes).
const VERSION_FILE: &str = include_str!("../VERSION");

/// Returns the application version, read from the `VERSION` file at the
/// repository root - the single source of the version. Surrounding
/// whitespace (the trailing newline) is stripped.
pub fn version() -> &'static str {
    VERSION_FILE.trim()
}

/// The text a horizontal rule is rendered as.
pub const RULE_TEXT: &str = "────────────────────────────────────────";

/// A visual style applied to a piece of rendered text.
///
/// Each style corresponds to exactly one GTK text tag, named by
/// [`Style::tag_name`]. A span may carry several styles at once (for example
/// strong text inside a heading).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Style {
    /// A heading of the given level, 1 (largest) to 6 (smallest).
    Heading(u8),
    /// Emphasised (italic) text.
    Emphasis,
    /// Strong (bold) text.
    Strong,
    /// Struck-through text.
    Strikethrough,
    /// An inline code span.
    Code,
    /// The contents of a fenced or indented code block.
    CodeBlock,
    /// Text inside a block quote.
    Quote,
    /// The text of a link.
    Link,
    /// An image placeholder (`[image: <alt text>]`).
    Image,
    /// A horizontal rule.
    Rule,
}

impl Style {
    /// Every style, so the GUI can create one text tag for each up front.
    pub const ALL: [Style; 15] = [
        Style::Heading(1),
        Style::Heading(2),
        Style::Heading(3),
        Style::Heading(4),
        Style::Heading(5),
        Style::Heading(6),
        Style::Emphasis,
        Style::Strong,
        Style::Strikethrough,
        Style::Code,
        Style::CodeBlock,
        Style::Quote,
        Style::Link,
        Style::Image,
        Style::Rule,
    ];

    /// The name of the GTK text tag implementing this style.
    ///
    /// Heading levels outside 1-6 cannot be produced by the renderer; they are
    /// clamped to the nearest valid level.
    pub fn tag_name(self) -> &'static str {
        match self {
            Style::Heading(level) => match level {
                0 | 1 => "h1",
                2 => "h2",
                3 => "h3",
                4 => "h4",
                5 => "h5",
                _ => "h6",
            },
            Style::Emphasis => "emphasis",
            Style::Strong => "strong",
            Style::Strikethrough => "strikethrough",
            Style::Code => "code",
            Style::CodeBlock => "code-block",
            Style::Quote => "quote",
            Style::Link => "link",
            Style::Image => "image",
            Style::Rule => "rule",
        }
    }
}

/// A piece of rendered text and the styles that apply to all of it.
///
/// `styles` is kept sorted and free of duplicates, so two spans with the same
/// set of styles compare equal regardless of the order the styles were
/// applied in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// The text to display.
    pub text: String,
    /// The styles applied to `text`, sorted and deduplicated.
    pub styles: Vec<Style>,
}

/// One element of flowing text: a run of styled text.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Styled text.
    Text(Span),
}

impl Inline {
    /// The text this inline element displays.
    pub fn text(&self) -> &str {
        match self {
            Inline::Text(span) => &span.text,
        }
    }
}

/// The horizontal alignment of a table column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// No alignment was given (shown left-aligned).
    None,
    /// `:---`
    Left,
    /// `:---:`
    Center,
    /// `---:`
    Right,
}

impl From<Alignment> for Align {
    /// Converts the parser's alignment into the library's own type, so the
    /// parser crate does not leak into the public API.
    fn from(alignment: Alignment) -> Self {
        match alignment {
            Alignment::None => Align::None,
            Alignment::Left => Align::Left,
            Alignment::Center => Align::Center,
            Alignment::Right => Align::Right,
        }
    }
}

/// The content of one table cell: styled inline elements.
pub type Cell = Vec<Inline>;

/// A table, with every row padded to the same number of columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The alignment of each column; as long as every row.
    pub alignments: Vec<Align>,
    /// The header row. Its cells additionally carry [`Style::Strong`].
    pub header: Vec<Cell>,
    /// The body rows.
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// The number of columns.
    pub fn columns(&self) -> usize {
        self.alignments.len()
    }
}

/// One block of a rendered document.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// Flowing text, including the newlines that separate it from the blocks
    /// before and after it.
    Text(Vec<Inline>),
    /// A table, shown as a grid widget on a line of its own.
    Table(Table),
}

/// The colour of link text, as `#rrggbb`.
pub const LINK_COLOR: &str = "#3584e4";

/// The colour of de-emphasised text (quotes, image placeholders, rules).
pub const MUTED_COLOR: &str = "#808080";

/// The background colour of inline code; drawn at [`CODE_BACKGROUND_ALPHA`]
/// so it works on light and dark backgrounds.
pub const CODE_BACKGROUND: &str = "#808080";

/// The opacity of the inline code background, as a Pango percentage.
pub const CODE_BACKGROUND_ALPHA: &str = "15%";

/// Escapes `text` for use inside Pango markup.
pub fn escape_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\'' => escaped.push_str("&#39;"),
            '"' => escaped.push_str("&quot;"),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// Converts inline elements into Pango markup, for widgets that show text
/// with a label rather than a text buffer (table cells).
///
/// Each span becomes one `<span>` element carrying the attributes of all its
/// styles; unstyled spans are emitted as escaped text. Styles that only make
/// sense for whole blocks (headings, quotes, code blocks, rules) get the
/// closest inline equivalent.
pub fn pango_markup(inlines: &[Inline]) -> String {
    let mut markup = String::new();
    for inline in inlines {
        let Inline::Text(span) = inline;
        let text = escape_markup(&span.text);
        let attributes: Vec<String> = span.styles.iter().map(|s| markup_attributes(*s)).collect();
        if attributes.is_empty() {
            markup.push_str(&text);
        } else {
            markup.push_str(&format!("<span {}>{text}</span>", attributes.join(" ")));
        }
    }
    markup
}

/// The Pango markup attributes implementing `style`.
fn markup_attributes(style: Style) -> String {
    match style {
        Style::Heading(_) | Style::Strong => "weight=\"bold\"".to_string(),
        Style::Emphasis => "style=\"italic\"".to_string(),
        Style::Strikethrough => "strikethrough=\"true\"".to_string(),
        Style::Code | Style::CodeBlock => {
            format!(
                "font_family=\"monospace\" background=\"{CODE_BACKGROUND}\" \
                 bgalpha=\"{CODE_BACKGROUND_ALPHA}\""
            )
        }
        Style::Quote | Style::Image => format!("style=\"italic\" foreground=\"{MUTED_COLOR}\""),
        Style::Link => format!("underline=\"single\" foreground=\"{LINK_COLOR}\""),
        Style::Rule => format!("foreground=\"{MUTED_COLOR}\""),
    }
}

/// Reads the document at `path` into a string.
///
/// The file is only ever opened for reading. Fails if the file cannot be read
/// or is not valid UTF-8.
pub fn read_document(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// Reads the document at `path` and renders it, ready for display.
pub fn open_document(path: &Path) -> io::Result<Vec<Block>> {
    read_document(path).map(|text| render(&text))
}

/// Builds the user-facing message for a document that could not be opened.
pub fn error_message(path: &Path, error: &io::Error) -> String {
    format!("Cannot open {}: {}", path.display(), error)
}

/// Returns the window title for a document at `path`: the file name followed
/// by the application name, or just the application name if the path has no
/// file name component.
pub fn window_title(path: &Path) -> String {
    match path.file_name() {
        Some(name) => format!("{} — {}", name.to_string_lossy(), APP_NAME),
        None => APP_NAME.to_string(),
    }
}

/// Renders Markdown `source` into a list of blocks.
///
/// Never fails: anything the renderer does not specifically understand (raw
/// HTML, for instance) is shown as plain text rather than dropped. Blocks are
/// separated by a blank line, list items by a single newline, and adjacent
/// text with identical styles is merged into one span. See the "Rendering
/// model" section of `AGENTS.md` for the full set of rules.
pub fn render(source: &str) -> Vec<Block> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::default();
    for event in Parser::new_ext(source, options) {
        renderer.event(event);
    }
    renderer.blocks
}

/// A table being collected cell by cell until its end tag arrives.
#[derive(Debug, Default)]
struct TableBuilder {
    /// The alignment of each column, as parsed.
    alignments: Vec<Align>,
    /// The header row's cells.
    header: Vec<Cell>,
    /// The body rows' cells.
    rows: Vec<Vec<Cell>>,
    /// The cells of the row currently being collected.
    current_row: Vec<Cell>,
    /// The cell currently being collected, if inside a cell.
    current_cell: Option<Cell>,
    /// True while the header row is being collected.
    in_head: bool,
}

impl TableBuilder {
    /// Finishes the table, padding the alignments and every row to the
    /// widest row's column count.
    fn finish(self) -> Table {
        let columns = self
            .rows
            .iter()
            .map(Vec::len)
            .chain([self.header.len(), self.alignments.len()])
            .max()
            .unwrap_or(0);
        let pad = |mut row: Vec<Cell>| {
            row.resize_with(columns, Vec::new);
            row
        };
        let mut alignments = self.alignments;
        alignments.resize(columns, Align::None);
        Table {
            alignments,
            header: pad(self.header),
            rows: self.rows.into_iter().map(pad).collect(),
        }
    }
}

/// A block whose text is buffered until its end tag, so the trailing newline
/// the parser includes can be trimmed.
#[derive(Debug)]
struct BufferedBlock {
    /// The text collected so far.
    text: String,
    /// The styles applied to the whole block.
    styles: Vec<Style>,
}

/// The state of an in-progress rendering of one document.
#[derive(Debug, Default)]
struct Renderer {
    /// The output produced so far.
    blocks: Vec<Block>,
    /// Styles currently in effect, innermost last.
    style_stack: Vec<Style>,
    /// One entry per open list: the next item number for ordered lists, or
    /// `None` for unordered lists.
    lists: Vec<Option<u64>>,
    /// True right after a list item's marker was written, so the item's first
    /// block continues on the marker's line instead of starting a new one.
    item_fresh: bool,
    /// The table being collected, if inside one.
    table: Option<TableBuilder>,
    /// The code or HTML block being collected, if inside one.
    block: Option<BufferedBlock>,
    /// The destination of the link being rendered and whether any of its text
    /// has been written yet, if inside a link.
    link: Option<(String, bool)>,
}

impl Renderer {
    /// Handles one parser event.
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(text) => {
                self.style_stack.push(Style::Code);
                self.text(&text);
                self.style_stack.pop();
            }
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            Event::InlineMath(text) | Event::DisplayMath(text) => self.text(&text),
            Event::FootnoteReference(label) => self.text(&format!("[^{label}]")),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => {
                self.block_break();
                self.style_stack.push(Style::Rule);
                self.text(RULE_TEXT);
                self.style_stack.pop();
            }
            Event::TaskListMarker(checked) => self.text(if checked { "[x] " } else { "[ ] " }),
        }
    }

    /// Handles the start of a block or inline element.
    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.block_break(),
            Tag::Heading { level, .. } => {
                self.block_break();
                self.style_stack.push(Style::Heading(level as u8));
            }
            Tag::BlockQuote(_) => {
                self.block_break();
                self.style_stack.push(Style::Quote);
            }
            Tag::CodeBlock(_) => {
                self.block_break();
                let mut styles = self.current_styles();
                styles.push(Style::CodeBlock);
                normalize(&mut styles);
                self.block = Some(BufferedBlock {
                    text: String::new(),
                    styles,
                });
            }
            Tag::HtmlBlock => {
                self.block_break();
                self.block = Some(BufferedBlock {
                    text: String::new(),
                    styles: self.current_styles(),
                });
            }
            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.block_break();
                }
                self.lists.push(first);
            }
            Tag::Item => self.start_item(),
            Tag::Table(alignments) => {
                self.block_break();
                self.table = Some(TableBuilder {
                    alignments: alignments.into_iter().map(Align::from).collect(),
                    ..TableBuilder::default()
                });
            }
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.in_head = true;
                }
            }
            Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.current_cell = Some(Vec::new());
                }
            }
            Tag::Emphasis => self.style_stack.push(Style::Emphasis),
            Tag::Strong => self.style_stack.push(Style::Strong),
            Tag::Strikethrough => self.style_stack.push(Style::Strikethrough),
            Tag::Link { dest_url, .. } => {
                self.style_stack.push(Style::Link);
                self.link = Some((dest_url.to_string(), false));
            }
            Tag::Image { .. } => {
                self.style_stack.push(Style::Image);
                self.text("[image: ");
            }
            // Extensions that are not enabled; their content still arrives as
            // ordinary events and is shown as text.
            _ => {}
        }
    }

    /// Handles the end of a block or inline element.
    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough => {
                self.style_stack.pop();
            }
            TagEnd::CodeBlock | TagEnd::HtmlBlock => {
                if let Some(block) = self.block.take() {
                    let text = block.text.strip_suffix('\n').unwrap_or(&block.text);
                    self.push(text, block.styles);
                }
            }
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::Item => self.item_fresh = false,
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    let cell = table.current_cell.take().unwrap_or_default();
                    table.current_row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.header = std::mem::take(&mut table.current_row);
                    table.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    let row = std::mem::take(&mut table.current_row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.blocks.push(Block::Table(table.finish()));
                }
            }
            TagEnd::Link => {
                if let Some((dest, has_text)) = self.link.take() {
                    if !has_text {
                        self.text(&dest);
                    }
                }
                self.style_stack.pop();
            }
            TagEnd::Image => {
                self.text("]");
                self.style_stack.pop();
            }
            _ => {}
        }
    }

    /// Writes a list item's indentation and marker on a new line.
    fn start_item(&mut self) {
        self.ensure_newlines(1);
        let depth = self.lists.len().saturating_sub(1);
        let marker = match self.lists.last_mut() {
            Some(Some(number)) => {
                let marker = format!("{number}. ");
                *number += 1;
                marker
            }
            _ => "• ".to_string(),
        };
        let indent = "    ".repeat(depth);
        self.push(&format!("{indent}{marker}"), Vec::new());
        self.item_fresh = true;
    }

    /// Routes text to the buffered block or to the output (which may be the
    /// current table cell), with the styles currently in effect.
    ///
    /// Any non-empty text written while inside a link (wherever it is routed)
    /// marks the link as having text, so its destination is not shown.
    fn text(&mut self, text: &str) {
        if let Some((_, has_text)) = self.link.as_mut() {
            if !text.is_empty() {
                *has_text = true;
            }
        }
        if let Some(block) = self.block.as_mut() {
            block.text.push_str(text);
            return;
        }
        let styles = self.current_styles();
        self.push(text, styles);
    }

    /// Separates the block about to start from whatever came before it.
    ///
    /// The first block of a list item continues on the marker's line; any
    /// other block inside a list starts on a new line; everything else is
    /// separated by one blank line. Nothing is written before the first block
    /// of the document.
    fn block_break(&mut self) {
        if self.item_fresh {
            self.item_fresh = false;
        } else if !self.lists.is_empty() {
            self.ensure_newlines(1);
            let indent = "    ".repeat(self.lists.len());
            self.push(&indent, Vec::new());
        } else {
            self.ensure_newlines(2);
        }
    }

    /// Makes the output end with at least `count` newlines, unless the output
    /// is still empty.
    fn ensure_newlines(&mut self, count: usize) {
        if self.blocks.is_empty() {
            return;
        }
        let have = trailing_newlines(&self.blocks);
        if have < count {
            self.push(&"\n".repeat(count - have), Vec::new());
        }
    }

    /// The styles currently in effect, sorted and deduplicated. Inside a
    /// table's header row this includes [`Style::Strong`].
    fn current_styles(&self) -> Vec<Style> {
        let mut styles = self.style_stack.clone();
        if self.table.as_ref().is_some_and(|table| table.in_head) {
            styles.push(Style::Strong);
        }
        normalize(&mut styles);
        styles
    }

    /// Appends `text` with `styles` to the current table cell if inside one,
    /// otherwise to the trailing text block (starting a new one if the last
    /// block is not text). Empty text is ignored.
    fn push(&mut self, text: &str, styles: Vec<Style>) {
        if text.is_empty() {
            return;
        }
        if let Some(cell) = self.table.as_mut().and_then(|t| t.current_cell.as_mut()) {
            push_inline(cell, text, styles);
            return;
        }
        if !matches!(self.blocks.last(), Some(Block::Text(_))) {
            self.blocks.push(Block::Text(Vec::new()));
        }
        if let Some(Block::Text(inlines)) = self.blocks.last_mut() {
            push_inline(inlines, text, styles);
        }
    }
}

/// Appends `text` with `styles` to `inlines`, merging it into the last
/// element when that is text with identical styles.
fn push_inline(inlines: &mut Vec<Inline>, text: &str, styles: Vec<Style>) {
    if let Some(Inline::Text(last)) = inlines.last_mut() {
        if last.styles == styles {
            last.text.push_str(text);
            return;
        }
    }
    inlines.push(Inline::Text(Span {
        text: text.to_string(),
        styles,
    }));
}

/// Sorts and deduplicates a style list so equal sets compare equal.
fn normalize(styles: &mut Vec<Style>) {
    styles.sort();
    styles.dedup();
}

/// Counts the newlines at the very end of the rendered output, looking back
/// across span boundaries. A trailing non-text block (a table) counts as
/// content, i.e. zero newlines.
fn trailing_newlines(blocks: &[Block]) -> usize {
    let Some(Block::Text(inlines)) = blocks.last() else {
        return 0;
    };
    let mut count = 0;
    for inline in inlines.iter().rev() {
        for c in inline.text().chars().rev() {
            if c == '\n' {
                count += 1;
            } else {
                return count;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a text block of unstyled spans, for brevity.
    fn text_block(parts: &[&str]) -> Block {
        Block::Text(
            parts
                .iter()
                .map(|text| {
                    Inline::Text(Span {
                        text: text.to_string(),
                        styles: Vec::new(),
                    })
                })
                .collect(),
        )
    }

    /// An empty table, for blocks that are not text.
    fn empty_table() -> Block {
        Block::Table(Table {
            alignments: Vec::new(),
            header: Vec::new(),
            rows: Vec::new(),
        })
    }

    /// `trailing_newlines` counts newlines across span boundaries, stops at
    /// the first other character, and treats a trailing table as content.
    #[test]
    fn trailing_newlines_spans_boundaries() {
        assert_eq!(trailing_newlines(&[]), 0);
        assert_eq!(trailing_newlines(&[text_block(&["a"])]), 0);
        assert_eq!(trailing_newlines(&[text_block(&["a\n", "\n"])]), 2);
        assert_eq!(trailing_newlines(&[text_block(&["\n", "\n\n"])]), 3);
        assert_eq!(trailing_newlines(&[text_block(&["a\nb"])]), 0);
        assert_eq!(trailing_newlines(&[text_block(&["\n"]), empty_table()]), 0);
    }

    /// `normalize` sorts and deduplicates so equal style sets compare equal.
    #[test]
    fn normalize_sorts_and_dedups() {
        let mut styles = vec![Style::Strong, Style::Emphasis, Style::Strong];
        normalize(&mut styles);
        assert_eq!(styles, vec![Style::Emphasis, Style::Strong]);
    }

    /// `push_inline` merges text with identical styles and starts a new
    /// element when the styles differ.
    #[test]
    fn push_inline_merges_equal_styles() {
        let mut inlines = Vec::new();
        push_inline(&mut inlines, "a", Vec::new());
        push_inline(&mut inlines, "b", Vec::new());
        push_inline(&mut inlines, "c", vec![Style::Strong]);
        assert_eq!(inlines.len(), 2);
        assert_eq!(inlines[0].text(), "ab");
        assert_eq!(inlines[1].text(), "c");
    }

    /// `TableBuilder::finish` pads the alignments, the header and short rows
    /// to the widest row.
    #[test]
    fn table_builder_pads_rows() {
        let cell = |text: &str| {
            vec![Inline::Text(Span {
                text: text.to_string(),
                styles: Vec::new(),
            })]
        };
        let builder = TableBuilder {
            alignments: vec![Align::Right],
            header: vec![cell("h")],
            rows: vec![vec![cell("a"), cell("b"), cell("c")], vec![]],
            ..TableBuilder::default()
        };
        let table = builder.finish();
        assert_eq!(table.columns(), 3);
        assert_eq!(
            table.alignments,
            vec![Align::Right, Align::None, Align::None]
        );
        assert_eq!(table.header.len(), 3);
        assert!(table.rows.iter().all(|row| row.len() == 3));
        assert!(table.rows[1].iter().all(Vec::is_empty));
    }

    /// A table with no cells at all finishes as a zero-column table.
    #[test]
    fn table_builder_empty() {
        let table = TableBuilder::default().finish();
        assert_eq!(table.columns(), 0);
        assert!(table.header.is_empty());
    }

    /// The parser's alignments map one-to-one onto [`Align`].
    #[test]
    fn align_from_parser() {
        assert_eq!(Align::from(Alignment::None), Align::None);
        assert_eq!(Align::from(Alignment::Left), Align::Left);
        assert_eq!(Align::from(Alignment::Center), Align::Center);
        assert_eq!(Align::from(Alignment::Right), Align::Right);
    }
}

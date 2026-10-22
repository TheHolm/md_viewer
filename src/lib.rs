//! Non-GUI logic for `md_viewer`: reading a document from disk and rendering
//! Markdown into a flat list of styled text spans.
//!
//! Everything here is independent of GTK so it can be exercised by the
//! integration tests in `tests/` without a display. The GUI in `main.rs` only
//! inserts the spans in order and applies one text tag per [`Style`].

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
    /// A table laid out as aligned monospace text.
    Table,
    /// A horizontal rule.
    Rule,
}

impl Style {
    /// Every style, so the GUI can create one text tag for each up front.
    pub const ALL: [Style; 16] = [
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
        Style::Table,
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
            Style::Table => "table",
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

/// Reads the document at `path` into a string.
///
/// The file is only ever opened for reading. Fails if the file cannot be read
/// or is not valid UTF-8.
pub fn read_document(path: &Path) -> io::Result<String> {
    fs::read_to_string(path)
}

/// Reads the document at `path` and renders it, ready for display.
pub fn open_document(path: &Path) -> io::Result<Vec<Span>> {
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

/// Renders Markdown `source` into a flat list of styled spans.
///
/// Never fails: anything the renderer does not specifically understand (raw
/// HTML, for instance) is shown as plain text rather than dropped. Blocks are
/// separated by a blank line, list items by a single newline, and adjacent
/// text with identical styles is merged into one span. See the "Rendering
/// model" section of `AGENTS.md` for the full set of rules.
pub fn render(source: &str) -> Vec<Span> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut renderer = Renderer::default();
    for event in Parser::new_ext(source, options) {
        renderer.event(event);
    }
    renderer.spans
}

/// A table being collected cell by cell until its end tag arrives.
#[derive(Debug, Default)]
struct TableBuilder {
    /// The alignment of each column.
    alignments: Vec<Alignment>,
    /// The header row's cells.
    header: Vec<String>,
    /// The body rows' cells.
    rows: Vec<Vec<String>>,
    /// The cells of the row currently being collected.
    current_row: Vec<String>,
    /// The text of the cell currently being collected, if inside a cell.
    current_cell: Option<String>,
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
    spans: Vec<Span>,
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
                    alignments,
                    ..TableBuilder::default()
                });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.current_cell = Some(String::new());
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
                    table.current_row.push(cell.trim().to_string());
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.header = std::mem::take(&mut table.current_row);
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
                    self.emit_table(&table);
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

    /// Routes text to the current table cell, the buffered block, or the
    /// output, in that order of precedence.
    ///
    /// Any non-empty text written while inside a link (wherever it is routed)
    /// marks the link as having text, so its destination is not shown.
    fn text(&mut self, text: &str) {
        if let Some((_, has_text)) = self.link.as_mut() {
            if !text.is_empty() {
                *has_text = true;
            }
        }
        if let Some(cell) = self.table.as_mut().and_then(|t| t.current_cell.as_mut()) {
            cell.push_str(text);
            return;
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
        if self.spans.is_empty() {
            return;
        }
        let have = trailing_newlines(&self.spans);
        if have < count {
            self.push(&"\n".repeat(count - have), Vec::new());
        }
    }

    /// The styles currently in effect, sorted and deduplicated.
    fn current_styles(&self) -> Vec<Style> {
        let mut styles = self.style_stack.clone();
        normalize(&mut styles);
        styles
    }

    /// Appends `text` with `styles`, merging it into the previous span when
    /// the styles are identical. Empty text is ignored.
    fn push(&mut self, text: &str, styles: Vec<Style>) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.styles == styles {
                last.text.push_str(text);
                return;
            }
        }
        self.spans.push(Span {
            text: text.to_string(),
            styles,
        });
    }

    /// Writes a collected table: the header line in strong text, then the
    /// separator line and the body rows.
    fn emit_table(&mut self, table: &TableBuilder) {
        let lines = layout_table(&table.alignments, &table.header, &table.rows);
        let mut base = self.current_styles();
        base.push(Style::Table);
        normalize(&mut base);
        let mut header_styles = base.clone();
        header_styles.push(Style::Strong);
        normalize(&mut header_styles);

        let mut lines = lines.into_iter();
        if let Some(header) = lines.next() {
            self.push(&header, header_styles);
        }
        for line in lines {
            self.push("\n", base.clone());
            self.push(&line, base.clone());
        }
    }
}

/// Sorts and deduplicates a style list so equal sets compare equal.
fn normalize(styles: &mut Vec<Style>) {
    styles.sort();
    styles.dedup();
}

/// Counts the newlines at the very end of the rendered output, looking back
/// across span boundaries.
fn trailing_newlines(spans: &[Span]) -> usize {
    let mut count = 0;
    for span in spans.iter().rev() {
        for c in span.text.chars().rev() {
            if c == '\n' {
                count += 1;
            } else {
                return count;
            }
        }
    }
    count
}

/// Pads `text` to `width` characters according to `alignment`. Unaligned
/// (`None`) columns are left-aligned; centred text that cannot be split evenly
/// gets the extra space on the right.
fn pad_cell(text: &str, width: usize, alignment: Alignment) -> String {
    let gap = width.saturating_sub(text.chars().count());
    let (left, right) = match alignment {
        Alignment::Right => (gap, 0),
        Alignment::Center => (gap / 2, gap - gap / 2),
        Alignment::Left | Alignment::None => (0, gap),
    };
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

/// Lays a table out as aligned monospace lines: the header, a `-` separator
/// line, then one line per body row.
///
/// Every column is padded to the width of its widest cell (at least one
/// character), and cells are joined with ` | ` (`-+-` on the separator line).
/// Rows with fewer cells than the header are padded with empty cells. Trailing
/// spaces are trimmed from every line.
fn layout_table(alignments: &[Alignment], header: &[String], rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows
        .iter()
        .map(Vec::len)
        .chain([header.len(), alignments.len()])
        .max()
        .unwrap_or(0);
    let cell = |row: &[String], index: usize| row.get(index).cloned().unwrap_or_default();
    let widths: Vec<usize> = (0..columns)
        .map(|index| {
            std::iter::once(header)
                .chain(rows.iter().map(Vec::as_slice))
                .map(|row| cell(row, index).chars().count())
                .max()
                .unwrap_or(0)
                .max(1)
        })
        .collect();
    let format_row = |row: &[String]| {
        let cells: Vec<String> = (0..columns)
            .map(|index| {
                let alignment = alignments.get(index).copied().unwrap_or(Alignment::None);
                pad_cell(&cell(row, index), widths[index], alignment)
            })
            .collect();
        cells.join(" | ").trim_end().to_string()
    };

    let mut lines = vec![format_row(header)];
    let separator: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    lines.push(separator.join("-+-"));
    lines.extend(rows.iter().map(|row| format_row(row)));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an unstyled span, for brevity.
    fn plain(text: &str) -> Span {
        Span {
            text: text.to_string(),
            styles: Vec::new(),
        }
    }

    /// `trailing_newlines` counts newlines across span boundaries and stops at
    /// the first other character.
    #[test]
    fn trailing_newlines_spans_boundaries() {
        assert_eq!(trailing_newlines(&[]), 0);
        assert_eq!(trailing_newlines(&[plain("a")]), 0);
        assert_eq!(trailing_newlines(&[plain("a\n"), plain("\n")]), 2);
        assert_eq!(trailing_newlines(&[plain("\n"), plain("\n\n")]), 3);
        assert_eq!(trailing_newlines(&[plain("a\nb")]), 0);
    }

    /// `normalize` sorts and deduplicates so equal style sets compare equal.
    #[test]
    fn normalize_sorts_and_dedups() {
        let mut styles = vec![Style::Strong, Style::Emphasis, Style::Strong];
        normalize(&mut styles);
        assert_eq!(styles, vec![Style::Emphasis, Style::Strong]);
    }

    /// `pad_cell` honours each alignment, giving centred text's odd extra
    /// space to the right, and never truncates text wider than the column.
    #[test]
    fn pad_cell_alignments() {
        assert_eq!(pad_cell("ab", 5, Alignment::None), "ab   ");
        assert_eq!(pad_cell("ab", 5, Alignment::Left), "ab   ");
        assert_eq!(pad_cell("ab", 5, Alignment::Right), "   ab");
        assert_eq!(pad_cell("ab", 5, Alignment::Center), " ab  ");
        assert_eq!(pad_cell("abcdef", 3, Alignment::Left), "abcdef");
    }

    /// `pad_cell` measures width in characters, not bytes.
    #[test]
    fn pad_cell_counts_characters() {
        assert_eq!(pad_cell("äö", 3, Alignment::Left), "äö ");
    }

    /// `layout_table` pads short rows with empty cells and keeps empty
    /// columns at least one character wide.
    #[test]
    fn layout_table_short_rows_and_empty_columns() {
        let header = vec!["a".to_string(), String::new()];
        let rows = vec![vec!["long".to_string()]];
        let lines = layout_table(&[Alignment::None, Alignment::None], &header, &rows);
        assert_eq!(lines, vec!["a    |", "-----+--", "long |"]);
    }

    /// `layout_table` handles a table with no columns at all without panicking.
    #[test]
    fn layout_table_empty() {
        assert_eq!(layout_table(&[], &[], &[]), vec!["", ""]);
    }
}

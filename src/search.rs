//! Finding text in a document, independent of GTK.
//!
//! The find bar's matching logic lives here so it can be tested without a
//! display. A [`Matcher`] is built once for a query and a [`SearchMode`]
//! (compiling the regular expression once, rather than per match), then
//! `find` is called for each piece of text searched: the whole text buffer
//! and every indexed widget (table cells, code cards, diagram labels).
//!
//! Every match is reported as a byte range into the text searched, so the
//! GUI can map it to text-buffer tags, Pango attribute ranges or a whole
//! diagram card. [`document_units`] lists the text of a parsed document that
//! can be searched before its graphics are rendered, so the find bar covers
//! a document that is still being drawn.

use crate::{Block, Inline};
use std::ops::Range;

/// How a query is matched against text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// An all-lowercase query ignores case; any uppercase character makes
    /// the search case-sensitive.
    #[default]
    SmartCase,
    /// Ignore case for every query.
    NoCase,
    /// The query is a regular expression, matched exactly as written (use
    /// `(?i)` for case-insensitivity).
    Regex,
}

impl SearchMode {
    /// Every mode, in the order the mode popover offers them.
    pub const ALL: [SearchMode; 3] = [SearchMode::SmartCase, SearchMode::NoCase, SearchMode::Regex];

    /// The full name of the mode, for the popover.
    pub fn label(self) -> &'static str {
        match self {
            SearchMode::SmartCase => "Smart case",
            SearchMode::NoCase => "Ignore case",
            SearchMode::Regex => "Regex",
        }
    }

    /// The compact name shown on the mode button.
    pub fn short(self) -> &'static str {
        match self {
            SearchMode::SmartCase => "Aa",
            SearchMode::NoCase => "aa",
            SearchMode::Regex => ".*",
        }
    }
}

/// Why a query could not be matched (currently only an invalid regular
/// expression).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchError {
    /// A user-facing message describing the problem.
    pub message: String,
}

/// A query compiled against a mode, ready to find matches in many texts.
#[derive(Debug, Clone)]
pub struct Matcher {
    /// The query as typed.
    query: String,
    /// How the query is matched.
    mode: SearchMode,
    /// The compiled regular expression, for [`SearchMode::Regex`].
    regex: Option<regex::Regex>,
    /// Whether literal matching is case-sensitive.
    case_sensitive: bool,
}

impl Matcher {
    /// Compiles `query` with `mode`. Fails only for an invalid regular
    /// expression.
    pub fn new(query: &str, mode: SearchMode) -> Result<Self, SearchError> {
        let (regex, case_sensitive) = match mode {
            SearchMode::Regex => (
                Some(regex::Regex::new(query).map_err(|error| SearchError {
                    message: error.to_string(),
                })?),
                true,
            ),
            SearchMode::NoCase => (None, false),
            SearchMode::SmartCase => (None, has_uppercase(query)),
        };
        Ok(Matcher {
            query: query.to_string(),
            mode,
            regex,
            case_sensitive,
        })
    }

    /// The query as typed.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// How this matcher matches.
    pub fn mode(&self) -> SearchMode {
        self.mode
    }

    /// Every non-overlapping match of the query in `text`, as byte ranges,
    /// in order. An empty query matches nothing; zero-width regular
    /// expression matches are skipped so they never mark empty spots.
    pub fn find(&self, text: &str) -> Vec<Range<usize>> {
        if self.query.is_empty() {
            return Vec::new();
        }
        match self.mode {
            SearchMode::Regex => match &self.regex {
                Some(regex) => regex_ranges(regex, text),
                None => Vec::new(),
            },
            SearchMode::SmartCase | SearchMode::NoCase => {
                if self.case_sensitive {
                    literal_ranges(text, &self.query)
                } else {
                    fold_ranges(text, &fold(&self.query).0)
                }
            }
        }
    }
}

/// The character the GUI's text stands in for an embedded object (a table,
/// code card, diagram or picture) with.
pub const OBJECT_REPLACEMENT: char = '\u{fffc}';

/// The non-empty matches of `regex` in `text`, searched separately in each
/// piece between [`OBJECT_REPLACEMENT`] characters: a regex never matches
/// an embedded object's placeholder (`.`, `\W` or `\S` would) nor runs
/// across one, and `^`/`$` (with `(?m)`) see each piece's ends.
fn regex_ranges(regex: &regex::Regex, text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for piece in text.split(OBJECT_REPLACEMENT) {
        ranges.extend(
            regex
                .find_iter(piece)
                .map(|found| found.start() + start..found.end() + start)
                .filter(|range| !range.is_empty()),
        );
        start += piece.len() + OBJECT_REPLACEMENT.len_utf8();
    }
    ranges
}

/// A searchable piece of a parsed document that exists before any graphics
/// are rendered, so the find bar can search the whole document while it is
/// still being drawn. Rendering-dependent text (diagram labels, and the
/// sources of formulas or image placeholders that only appear once a graphic
/// is drawn) is not listed here; the GUI adds that as it is rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The index of the block the piece is in.
    pub block: usize,
    /// The text to search.
    pub text: String,
}

/// Every render-independent searchable piece of `blocks`, in document order
/// (block order, then the order the pieces appear within a block). A text
/// block's or table cell's span runs are split where a formula or image sits
/// (matching how the rendered text is separated by embedded objects), and
/// each image's alt text is a piece of its own. Diagrams are skipped: their
/// labels only exist once the diagram is drawn.
pub fn document_units(blocks: &[Block]) -> Vec<Unit> {
    let mut units = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Text(inlines) => push_inline_units(index, inlines, &mut units),
            Block::Table(table) => {
                for cell in table.header.iter().chain(table.rows.iter().flatten()) {
                    push_inline_units(index, cell, &mut units);
                }
            }
            Block::Code(code) => {
                if !code.text.is_empty() {
                    units.push(Unit {
                        block: index,
                        text: code.text.clone(),
                    });
                }
            }
            Block::Mermaid(_) => {}
        }
    }
    units
}

/// Adds the units of `inlines` to `units`: each run of consecutive text
/// spans (split where a formula or image sits) and each image's alt text,
/// in order.
fn push_inline_units(block: usize, inlines: &[Inline], units: &mut Vec<Unit>) {
    let mut run = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(span) => run.push_str(&span.text),
            Inline::Math(_) | Inline::Image(_) => {
                if !run.is_empty() {
                    units.push(Unit {
                        block,
                        text: std::mem::take(&mut run),
                    });
                }
                if let Inline::Image(image) = inline {
                    if !image.alt.is_empty() {
                        units.push(Unit {
                            block,
                            text: image.alt.clone(),
                        });
                    }
                }
            }
        }
    }
    if !run.is_empty() {
        units.push(Unit { block, text: run });
    }
}

/// True if `query` contains any uppercase character, which makes a smart
/// case search case-sensitive.
fn has_uppercase(query: &str) -> bool {
    query.chars().any(char::is_uppercase)
}

/// The case-sensitive matches of `query` in `text`, as byte ranges.
fn literal_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    text.match_indices(query)
        .map(|(start, found)| start..start + found.len())
        .collect()
}

/// Case-folds `text` with Unicode full case folding (see
/// [`crate::casefold`]) and, for each byte of the result, the byte offset in
/// `text` of the character it came from. A character that folds to several
/// characters (like `ß` -> `ss`) maps each of them to the same original
/// offset.
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::new();
    let mut map = Vec::new();
    for (offset, character) in text.char_indices() {
        match fold_char(character) {
            Some(folding) => folded.push_str(folding),
            None => folded.push(character),
        }
        map.resize(folded.len(), offset);
    }
    (folded, map)
}

/// What `character` case-folds to, or `None` when it folds to itself.
fn fold_char(character: char) -> Option<&'static str> {
    crate::casefold::FOLDS
        .binary_search_by_key(&character, |(from, _)| *from)
        .ok()
        .map(|index| crate::casefold::FOLDS[index].1)
}

/// Finds the case-insensitive matches of `folded_query` in `text`, folding
/// `text` and mapping the matches back to byte ranges of the original text.
fn fold_ranges(text: &str, folded_query: &str) -> Vec<Range<usize>> {
    let (folded, map) = fold(text);
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (start, found) in folded.match_indices(folded_query) {
        let end = start + found.len();
        let original_start = map[start];
        let last = map[end - 1];
        let original_end = last + text[last..].chars().next().map_or(0, char::len_utf8);
        let range = original_start..original_end;
        if ranges
            .last()
            .is_none_or(|last: &Range<usize>| range.start >= last.end)
        {
            ranges.push(range);
        }
    }
    ranges
}

/// Maps byte ranges into `text` (from [`Matcher::find`]) to the equivalent
/// character-index ranges, which is what a text buffer's offsets count.
/// The byte ranges must be in order and not overlap.
pub fn char_ranges(text: &str, byte_ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut result = Vec::with_capacity(byte_ranges.len());
    let mut byte = 0;
    let mut chars = 0;
    for range in byte_ranges {
        chars += text[byte..range.start].chars().count();
        let start = chars;
        chars += text[range.start..range.end].chars().count();
        result.push(start..chars);
        byte = range.end;
    }
    result
}

/// The index after `current` in a list of `count` matches, moving forward or
/// backward and wrapping around; `None` when there are no matches.
pub fn next_index(current: Option<usize>, count: usize, forward: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match current {
        Some(current) if current < count => Some(if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        }),
        _ => Some(0),
    }
}

/// The first match at or after `start` (matched by a position that grows
/// through the document, e.g. a buffer offset), or the first match if there
/// is none that far, or `None` when there are no matches. The positions
/// must be sorted.
pub fn first_at_or_after(positions: &[i32], start: i32) -> Option<usize> {
    if positions.is_empty() {
        return None;
    }
    let index = positions.partition_point(|position| *position < start);
    Some(if index < positions.len() { index } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `has_uppercase` sees an uppercase character anywhere in the query.
    #[test]
    fn has_uppercase_detects() {
        assert!(has_uppercase("Coffee"));
        assert!(has_uppercase("coffÉe"));
        assert!(has_uppercase("A"));
        assert!(!has_uppercase("coffee"));
        assert!(!has_uppercase(""));
        assert!(!has_uppercase("123 λ"));
    }

    /// `fold` case-folds every character and maps each byte of the result
    /// back to the byte of the original character it came from, including a
    /// character that folds to several characters.
    #[test]
    fn fold_maps_back_to_source() {
        let (folded, map) = fold("aλİ");
        assert_eq!(folded, "aλi\u{307}");
        assert_eq!(map, vec![0, 1, 1, 3, 3, 3]);
    }

    /// `literal_ranges` reports non-overlapping matches as byte ranges.
    #[test]
    fn literal_ranges_no_overlap() {
        assert_eq!(literal_ranges("aaaaa", "aa"), vec![0..2, 2..4]);
        assert_eq!(literal_ranges("abc", "z"), Vec::<Range<usize>>::new());
    }

    /// `fold_ranges` matches case-insensitively and never splits a
    /// multi-byte character in half.
    #[test]
    fn fold_ranges_whole_characters() {
        assert_eq!(fold_ranges("Straße", &fold("straße").0), vec![0..7]);
        assert_eq!(fold_ranges("İ", "i"), vec![0..2]);
        assert_eq!(fold_ranges("aaaaa", "aa"), vec![0..2, 2..4]);
    }

    /// The case-folding table is sorted by character with no repeats (so
    /// it can be binary-searched), never maps a character to itself, and
    /// folding is stable: what a character folds to folds to itself.
    #[test]
    fn casefold_table_is_well_formed() {
        let table = crate::casefold::FOLDS;
        assert!(table.len() > 1000);
        assert!(table.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for (from, to) in table {
            assert_ne!(to.chars().collect::<Vec<_>>(), [*from]);
            assert_eq!(fold(to).0, *to, "{from:?}");
        }
    }

    /// Full case folding matches what lowercasing alone would miss: `ß`
    /// and `ẞ` with `ss`, ligatures with their letters, final and medial
    /// sigma, the long s, the micro sign and Cherokee small letters with
    /// their capitals; every range covers whole original characters.
    #[test]
    fn fold_ranges_use_full_case_folding() {
        assert_eq!(fold_ranges("Straße", &fold("strasse").0), vec![0..7]);
        assert_eq!(fold_ranges("STRASSE", &fold("straße").0), vec![0..7]);
        assert_eq!(fold_ranges("GROẞ", &fold("gross").0), vec![0..6]);
        assert_eq!(fold_ranges("ﬁle", &fold("file").0), vec![0..5]);
        assert_eq!(fold_ranges("file", &fold("ﬁ").0), vec![0..2]);
        assert_eq!(fold_ranges("λόγος", &fold("ΛΌΓΟΣ").0), vec![0..10]);
        assert_eq!(fold_ranges("ſun", &fold("sun").0), vec![0..4]);
        assert_eq!(fold_ranges("5µm", &fold("μ").0), vec![1..3]);
        assert_eq!(fold_ranges("Ꭰ", &fold("ꭰ").0), vec![0..3]);
        assert_eq!(fold_ranges("ꭰ", &fold("Ꭰ").0), vec![0..3]);
        // Half of a folded `ß` is not a match on its own boundary.
        assert_eq!(fold_ranges("aßa", &fold("sa").0), vec![1..4]);
    }

    /// Ignore case and smart case find `ß` for a query of `ss`, through
    /// the public matcher.
    #[test]
    fn matcher_folds_case_fully() {
        let matcher = Matcher::new("strasse", SearchMode::SmartCase).unwrap();
        assert_eq!(matcher.find("Die Straße"), vec![4..11]);
        let matcher = Matcher::new("STRASSE", SearchMode::NoCase).unwrap();
        assert_eq!(matcher.find("straße"), vec![0..7]);
        // An uppercase query in smart case stays exact.
        let matcher = Matcher::new("STRASSE", SearchMode::SmartCase).unwrap();
        assert!(matcher.find("straße").is_empty());
    }

    /// In regex mode the object placeholder is never matched and no match
    /// runs across it; offsets still point into the whole text, and `^`/`$`
    /// anchor to each piece.
    #[test]
    fn regex_skips_object_placeholders() {
        let find =
            |query: &str, text: &str| Matcher::new(query, SearchMode::Regex).unwrap().find(text);
        assert_eq!(find(".", "a\u{fffc}b"), vec![0..1, 4..5]);
        assert_eq!(find("\\W", "a\u{fffc} b"), vec![4..5]);
        assert_eq!(find("\\S+", "ab\u{fffc}cd"), vec![0..2, 5..7]);
        assert!(find("a.b", "a\u{fffc}b").is_empty());
        assert_eq!(find("^b$", "a\u{fffc}b"), vec![4..5]);
        assert_eq!(find("é.", "xé\u{fffc}éz"), vec![6..9]);
        assert_eq!(find("x", "\u{fffc}\u{fffc}x"), vec![6..7]);
    }

    /// `document_units` lists the render-independent text of a parsed
    /// document in order: span runs split around a formula, an image's alt
    /// text and a code block's code, but not a diagram's labels.
    #[test]
    fn document_units_cover_parsed_text() {
        let blocks = crate::render(
            "See $x$ and ![cat](c.png).\n\n```sh\nls\n```\n\n```mermaid\ngraph TD; A-->B\n```",
        );
        let units = document_units(&blocks);
        assert!(units.windows(2).all(|pair| pair[0].block <= pair[1].block));
        let text: Vec<&str> = units.iter().map(|unit| unit.text.as_str()).collect();
        assert!(text.contains(&"See "), "{text:?}");
        assert!(text.contains(&" and "), "{text:?}");
        assert!(text.contains(&"cat"), "{text:?}");
        assert!(text.contains(&"ls"), "{text:?}");
        // No span run straddles the formula, and the diagram's labels are not
        // listed (they only exist once rendered).
        assert!(text.iter().all(|piece| !piece.contains('x')), "{text:?}");
        let joined = text.concat();
        assert!(!joined.contains("graph"), "{text:?}");
        assert!(!joined.contains("A-->B"), "{text:?}");
    }

    /// Table cells contribute their text too, split around a formula like a
    /// text block.
    #[test]
    fn document_units_include_table_cells() {
        let blocks = crate::render("| a |\n|---|\n| b $y$ c |");
        let text: Vec<String> = document_units(&blocks)
            .into_iter()
            .map(|unit| unit.text)
            .collect();
        assert_eq!(text, ["a", "b ", " c"]);
    }
}

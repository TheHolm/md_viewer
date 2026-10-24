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
//! diagram card.

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
                Some(regex) => regex
                    .find_iter(text)
                    .map(|found| found.range())
                    .filter(|range| !range.is_empty())
                    .collect(),
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

/// Lowercases `text` (with Unicode case folding) and, for each byte of the
/// result, the byte offset in `text` of the character it came from. A
/// character that lowercases to several characters (like `ß` -> `ss`) maps
/// each of them to the same original offset.
fn fold(text: &str) -> (String, Vec<usize>) {
    let mut folded = String::new();
    let mut map = Vec::new();
    for (offset, character) in text.char_indices() {
        for lower in character.to_lowercase() {
            let start = folded.len();
            folded.push(lower);
            for _ in start..folded.len() {
                map.push(offset);
            }
        }
    }
    (folded, map)
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

    /// `fold` lowercases every character and maps each byte of the result
    /// back to the byte of the original character it came from, including a
    /// character that lowercases to several characters.
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
        assert_eq!(fold_ranges("Straße", "straße"), vec![0..7]);
        assert_eq!(fold_ranges("İ", "i"), vec![0..2]);
        assert_eq!(fold_ranges("aaaaa", "aa"), vec![0..2, 2..4]);
    }
}

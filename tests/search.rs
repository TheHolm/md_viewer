//! Integration tests for the find bar's matching logic
//! (`md_viewer::search`): the modes, the matcher and the navigation helpers.

use md_viewer::search::{char_ranges, first_at_or_after, next_index, Matcher, SearchMode};

/// Every mode has a full name and a compact one, and smart case is the
/// default.
#[test]
fn search_mode_labels() {
    assert_eq!(SearchMode::default(), SearchMode::SmartCase);
    assert_eq!(SearchMode::SmartCase.label(), "Smart case");
    assert_eq!(SearchMode::NoCase.label(), "Ignore case");
    assert_eq!(SearchMode::Regex.label(), "Regex");
    assert_eq!(SearchMode::SmartCase.short(), "Aa");
    assert_eq!(SearchMode::NoCase.short(), "aa");
    assert_eq!(SearchMode::Regex.short(), ".*");
}

/// A matcher finds every case-insensitive match for an all-lowercase query,
/// in order and without overlaps; an uppercase query is case-sensitive.
#[test]
fn smart_case_matching() {
    let matcher = Matcher::new("coffee", SearchMode::SmartCase).unwrap();
    assert_eq!(
        matcher.find("Coffee COFFEE coffee"),
        vec![0..6, 7..13, 14..20]
    );
    let matcher = Matcher::new("Coffee", SearchMode::SmartCase).unwrap();
    assert_eq!(matcher.find("Coffee coffee COFFEE"), vec![0..6]);
    assert_eq!(matcher.query(), "Coffee");
    assert_eq!(matcher.mode(), SearchMode::SmartCase);
}

/// Ignore case matches regardless of the query's case; overlapping matches
/// are not reported twice.
#[test]
fn no_case_matching() {
    let matcher = Matcher::new("COFFEE", SearchMode::NoCase).unwrap();
    assert_eq!(
        matcher.find("Coffee coffee COFFEE"),
        vec![0..6, 7..13, 14..20]
    );
    let matcher = Matcher::new("aa", SearchMode::NoCase).unwrap();
    assert_eq!(matcher.find("aaaaa"), vec![0..2, 2..4]);
}

/// Unicode case folding matches across letter case and maps back to whole
/// characters.
#[test]
fn unicode_case_folding() {
    let matcher = Matcher::new("straße", SearchMode::NoCase).unwrap();
    assert_eq!(matcher.find("Straße"), vec![0..7]);
    // `İ` lowercases to `i` + a combining dot; matching the `i` still covers
    // the whole `İ` character (2 bytes), never half of it.
    let matcher = Matcher::new("i", SearchMode::NoCase).unwrap();
    assert_eq!(matcher.find("İ"), vec![0..2]);
}

/// A regular expression matches as written, honours `(?i)` and skips
/// zero-width matches; an invalid one is an error, not a panic.
#[test]
fn regex_matching() {
    let matcher = Matcher::new(r"\b\w+\d", SearchMode::Regex).unwrap();
    assert_eq!(matcher.find("ab x1 cd y22"), vec![3..5, 9..12]);
    let insensitive = Matcher::new("(?i)hello", SearchMode::Regex).unwrap();
    assert_eq!(insensitive.find("Hello HELLO"), vec![0..5, 6..11]);
    let zero_width = Matcher::new("x*", SearchMode::Regex).unwrap();
    assert!(zero_width.find("ab").is_empty());
    assert!(Matcher::new("([)", SearchMode::Regex).is_err());
}

/// An empty query matches nothing in every mode.
#[test]
fn empty_query_matches_nothing() {
    for mode in SearchMode::ALL {
        let matcher = Matcher::new("", mode).unwrap();
        assert!(matcher.find("text").is_empty());
    }
}

/// Byte ranges map to character ranges across multi-byte characters.
#[test]
fn char_ranges_across_multibyte() {
    let text = "aλbλc";
    let matcher = Matcher::new("λb", SearchMode::NoCase).unwrap();
    let bytes = matcher.find(text);
    assert_eq!(bytes, vec![1..4]);
    assert_eq!(char_ranges(text, &bytes), vec![1..3]);
}

/// `next_index` steps forward and backward with wrap-around, and gives
/// `None` with no matches.
#[test]
fn navigation_wraps() {
    assert_eq!(next_index(Some(0), 3, true), Some(1));
    assert_eq!(next_index(Some(2), 3, true), Some(0));
    assert_eq!(next_index(Some(0), 3, false), Some(2));
    assert_eq!(next_index(Some(1), 3, false), Some(0));
    assert_eq!(next_index(None, 3, true), Some(0));
    assert_eq!(next_index(Some(5), 3, true), Some(0));
    assert_eq!(next_index(None, 0, true), None);
}

/// `first_at_or_after` picks the first match at or after a position, the
/// first match overall past the end, and nothing when there are none.
#[test]
fn first_at_or_after_picks() {
    assert_eq!(first_at_or_after(&[10, 20, 30], 15), Some(1));
    assert_eq!(first_at_or_after(&[10, 20, 30], 20), Some(1));
    assert_eq!(first_at_or_after(&[10, 20, 30], 40), Some(0));
    assert_eq!(first_at_or_after(&[10, 20, 30], 0), Some(0));
    assert_eq!(first_at_or_after(&[], 0), None);
}

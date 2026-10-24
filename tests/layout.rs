//! Integration tests for the GUI's sizing and styling decisions
//! (`md_viewer::layout`).

use md_viewer::layout::{
    align_fraction, available_width, cell_classes, color_channel, diagram_size, font_size_px,
    heading_scale, math_rise, pango_channel, pango_color, percent_alpha, table_size,
    DIAGRAM_PADDING, DOCUMENT_MARGIN, MIN_DIAGRAM_WIDTH, MIN_TABLE_WIDTH,
};
use md_viewer::Align;

/// Pango's fixed-point scale (`pango::SCALE`), so these tests need no GTK.
const PANGO_SCALE: i32 = 1024;

/// The available width is the view minus both margins and some slack.
#[test]
fn available_width_subtracts_margins() {
    assert_eq!(available_width(800), 800 - 2 * DOCUMENT_MARGIN - 8);
}

/// A very narrow (or not yet allocated) view still leaves a usable width.
#[test]
fn available_width_has_a_floor() {
    assert_eq!(available_width(0), MIN_TABLE_WIDTH);
    assert_eq!(available_width(-1), MIN_TABLE_WIDTH);
    assert_eq!(available_width(50), MIN_TABLE_WIDTH);
}

/// A table that fits keeps its natural width and height.
#[test]
fn table_size_fits() {
    assert_eq!(table_size(300, 120, 800), (300, 120));
}

/// A table wider than the view is capped at the available width (it scrolls
/// horizontally) and keeps its height.
#[test]
fn table_size_capped() {
    assert_eq!(table_size(5000, 60, 800), (available_width(800), 60));
}

/// A diagram that fits is shown at its natural size, never scaled up.
#[test]
fn diagram_size_fits() {
    assert_eq!(diagram_size(200.0, 100.0, 800), (200, 100));
}

/// A diagram wider than the view is scaled down, keeping its aspect ratio,
/// so it fits next to its card padding.
#[test]
fn diagram_size_scaled_down() {
    let room = available_width(800) - 2 * DIAGRAM_PADDING;
    let (width, height) = diagram_size(2000.0, 500.0, 800);
    assert_eq!(width, room);
    assert_eq!(height, (500.0 * room as f32 / 2000.0).round() as i32);
}

/// However narrow the view, a diagram is not shrunk below a minimum width.
#[test]
fn diagram_size_has_a_floor() {
    let floor = (MIN_TABLE_WIDTH - 2 * DIAGRAM_PADDING).max(MIN_DIAGRAM_WIDTH);
    assert_eq!(diagram_size(1000.0, 1000.0, 0).0, floor);
}

/// A zero-width diagram does not divide by zero.
#[test]
fn diagram_size_zero_width() {
    assert_eq!(diagram_size(0.0, 10.0, 800), (0, 10));
}

/// Absolute font sizes are pixels; point sizes are converted at 96 dpi.
#[test]
fn font_size_conversion() {
    assert_eq!(
        font_size_px(15 * PANGO_SCALE, true, PANGO_SCALE),
        Some(15.0)
    );
    assert_eq!(
        font_size_px(12 * PANGO_SCALE, false, PANGO_SCALE),
        Some(16.0)
    );
}

/// A font without a size gives `None`, so the default is used.
#[test]
fn font_size_missing() {
    assert_eq!(font_size_px(0, false, PANGO_SCALE), None);
    assert_eq!(font_size_px(-5, true, PANGO_SCALE), None);
}

/// Colour channels map 0.0-1.0 to 0-255, rounding and clamping.
#[test]
fn color_channels() {
    assert_eq!(color_channel(0.0), 0);
    assert_eq!(color_channel(1.0), 255);
    assert_eq!(color_channel(0.5), 128);
    assert_eq!(color_channel(-1.0), 0);
    assert_eq!(color_channel(2.0), 255);
}

/// `pango_channel` widens an 8-bit channel to Pango's 16-bit range: 255
/// becomes full (65535), not near-black.
#[test]
fn pango_channel_widens() {
    assert_eq!(pango_channel(0), 0);
    assert_eq!(pango_channel(255), 65535);
    assert_eq!(pango_channel(128), 32896);
    assert_eq!(pango_channel(1), 257);
}

/// `pango_color` parses `#rrggbb` into Pango's 16-bit channels.
#[test]
fn pango_color_parses() {
    assert_eq!(pango_color("#808080"), Some((32896, 32896, 32896)));
    assert_eq!(pango_color("#3584e4"), Some((13621, 33924, 58596)));
    assert_eq!(pango_color("#000000"), Some((0, 0, 0)));
    assert_eq!(pango_color("#ffffff"), Some((65535, 65535, 65535)));
    assert_eq!(pango_color("808080"), None);
    assert_eq!(pango_color("#12345"), None);
    assert_eq!(pango_color("#zzzzzz"), None);
}

/// `percent_alpha` converts a percentage to Pango's 16-bit alpha, and the
/// inline code background alpha resolves to the same value as its markup.
#[test]
fn percent_alpha_converts() {
    assert_eq!(percent_alpha("0%"), 0);
    assert_eq!(percent_alpha("100%"), 65535);
    assert_eq!(percent_alpha("15%"), 9830);
    assert_eq!(percent_alpha("garbage"), 0);
    assert_eq!(percent_alpha(md_viewer::CODE_BACKGROUND_ALPHA), 9830);
}

/// Heading scales shrink with the level and clamp past level 6.
#[test]
fn heading_scales() {
    let scales: Vec<f64> = (1..=6).map(heading_scale).collect();
    assert!(
        scales.windows(2).all(|pair| pair[0] > pair[1]),
        "{scales:?}"
    );
    assert_eq!(heading_scale(0), heading_scale(1));
    assert_eq!(heading_scale(9), heading_scale(6));
}

/// A formula's depth below the baseline becomes a negative rise in Pango
/// units; a formula without depth is not moved.
#[test]
fn math_rise_lowers_formula() {
    assert_eq!(math_rise(2.0, PANGO_SCALE), -2048);
    assert_eq!(math_rise(0.0, PANGO_SCALE), 0);
}

/// Column alignments map to horizontal positions; unaligned is left.
#[test]
fn align_fractions() {
    assert_eq!(align_fraction(Align::None), 0.0);
    assert_eq!(align_fraction(Align::Left), 0.0);
    assert_eq!(align_fraction(Align::Center), 0.5);
    assert_eq!(align_fraction(Align::Right), 1.0);
}

/// Header, striped and plain body cells, and the last column and row, get
/// the right classes in a 2-column table with a header and three body rows.
#[test]
fn cell_class_assignment() {
    assert_eq!(cell_classes(0, 0, 2, 4), vec!["md-cell", "md-header"]);
    assert_eq!(
        cell_classes(0, 1, 2, 4),
        vec!["md-cell", "md-header", "md-last-column"]
    );
    assert_eq!(cell_classes(1, 0, 2, 4), vec!["md-cell"]);
    assert_eq!(cell_classes(2, 0, 2, 4), vec!["md-cell", "md-stripe"]);
    assert_eq!(
        cell_classes(3, 1, 2, 4),
        vec!["md-cell", "md-last-column", "md-last-row"]
    );
}

/// A header-only table's header row is also its last row.
#[test]
fn cell_classes_header_only() {
    assert_eq!(
        cell_classes(0, 0, 1, 1),
        vec!["md-cell", "md-header", "md-last-column", "md-last-row"]
    );
}

/// A code card fills the available width minus its indentation, with a
/// floor for deeply nested cards in narrow windows.
#[test]
fn code_card_widths() {
    use md_viewer::layout::{available_width, code_card_width, CODE_INDENT, MIN_TABLE_WIDTH};
    assert_eq!(code_card_width(800, 0), available_width(800));
    assert_eq!(
        code_card_width(800, 2),
        available_width(800) - 2 * CODE_INDENT
    );
    assert_eq!(code_card_width(150, 3), MIN_TABLE_WIDTH);
    assert_eq!(code_card_width(800, usize::MAX), MIN_TABLE_WIDTH);
}

/// Scrolling passed on from a table: pixels as they are, wheel clicks in
/// GTK's step of the page height to the power 2/3.
#[test]
fn scroll_pixels_steps() {
    use md_viewer::layout::scroll_pixels;
    assert_eq!(scroll_pixels(12.5, true, 1000.0), 12.5);
    assert!((scroll_pixels(1.0, false, 1000.0) - 100.0).abs() < 1e-9);
    assert!((scroll_pixels(-2.0, false, 1000.0) + 200.0).abs() < 1e-9);
    assert_eq!(scroll_pixels(1.0, false, -5.0), 0.0);
}

/// The spot at the top of the window is remembered as a fraction of its
/// line and restored for the line's new position and size.
#[test]
fn zoom_anchor() {
    use md_viewer::layout::{anchor_fraction, anchored_scroll, ANCHOR_HOLD_MS};
    assert_eq!(anchor_fraction(150.0, 100.0, 200.0), 0.25);
    assert_eq!(anchor_fraction(100.0, 100.0, 0.0), 0.0);
    assert_eq!(anchor_fraction(50.0, 100.0, 200.0), 0.0);
    assert_eq!(anchor_fraction(400.0, 100.0, 200.0), 1.0);
    assert_eq!(anchored_scroll(300.0, 400.0, 0.25), 400.0);
    assert_eq!(anchored_scroll(300.0, -1.0, 0.5), 300.0);
    const { assert!(ANCHOR_HOLD_MS > 0) };
}

/// The copy button's check mark shows for a noticeable but short time.
#[test]
fn copy_feedback_time() {
    let millis = md_viewer::layout::COPY_FEEDBACK_MS;
    assert!((500..=5000).contains(&millis));
}

/// A scrollbar mark sits at its fraction of the track and is clamped so it
/// never spills out of the track.
#[test]
fn search_mark_positions() {
    use md_viewer::layout::{search_mark_y, SEARCH_MARK_HEIGHT};
    let mark = SEARCH_MARK_HEIGHT;
    assert_eq!(search_mark_y(0.0, 100.0, mark), 0.0);
    assert_eq!(search_mark_y(1.0, 100.0, mark), 100.0 - mark);
    assert_eq!(search_mark_y(0.5, 100.0, mark), 50.0 - mark / 2.0);
    assert_eq!(search_mark_y(1.5, 100.0, mark), 100.0 - mark);
    assert_eq!(search_mark_y(-0.5, 100.0, mark), 0.0);
    // A track shorter than the mark just pins the mark to the top.
    assert_eq!(search_mark_y(1.0, 2.0, mark), 0.0);
    assert_eq!(search_mark_y(0.0, 0.0, mark), 0.0);
}

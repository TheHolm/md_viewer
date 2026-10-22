//! Integration tests for the GUI's sizing and styling decisions
//! (`md_viewer::layout`).

use md_viewer::layout::{
    align_fraction, available_width, cell_classes, color_channel, diagram_size, font_size_px,
    heading_scale, math_rise, table_size, DIAGRAM_PADDING, DOCUMENT_MARGIN, MIN_DIAGRAM_WIDTH,
    MIN_TABLE_WIDTH,
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

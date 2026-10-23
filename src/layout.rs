//! Sizing and styling decisions for the GUI, as plain functions.
//!
//! `main.rs` only applies these to GTK widgets; keeping the arithmetic here
//! means it is covered by tests that need no display.

use crate::Align;

/// The left and right margin of the document, in pixels.
pub const DOCUMENT_MARGIN: i32 = 16;

/// The padding around a diagram on its card (`.md-diagram` in the GUI's
/// CSS), plus the card's border, on each side, in pixels.
pub const DIAGRAM_PADDING: i32 = 13;

/// The narrowest a table may be made, in pixels, however narrow the view.
pub const MIN_TABLE_WIDTH: i32 = 100;

/// The narrowest a diagram may be scaled to, in pixels.
pub const MIN_DIAGRAM_WIDTH: i32 = 50;

/// How far a code card is indented per level of block quote or list it is
/// nested in, in pixels.
pub const CODE_INDENT: i32 = 28;

/// How long the copy button of a code card shows its check mark after
/// copying, in milliseconds.
pub const COPY_FEEDBACK_MS: u64 = 1500;

/// The body font size assumed when the view does not report one, in pixels.
pub const DEFAULT_FONT_PX: f32 = 15.0;

/// The width available to an anchored widget in a view `view_width` pixels
/// wide: the view minus both document margins and a little slack for the
/// text view's own borders, but never less than [`MIN_TABLE_WIDTH`].
pub fn available_width(view_width: i32) -> i32 {
    (view_width - 2 * DOCUMENT_MARGIN - 8).max(MIN_TABLE_WIDTH)
}

/// The size to request for a table scroller: the table's natural width
/// capped at the available width (a wider table scrolls horizontally), and
/// the table's height at its natural width.
pub fn table_size(natural_width: i32, natural_height: i32, view_width: i32) -> (i32, i32) {
    (
        natural_width.min(available_width(view_width)),
        natural_height,
    )
}

/// The width of a code card nested `indent` levels deep in a view
/// `view_width` pixels wide: the available width minus its indentation,
/// but never less than [`MIN_TABLE_WIDTH`]. Longer lines scroll sideways
/// inside the card.
pub fn code_card_width(view_width: i32, indent: usize) -> i32 {
    let indent = i32::try_from(indent).unwrap_or(i32::MAX / CODE_INDENT);
    (available_width(view_width) - indent * CODE_INDENT).max(MIN_TABLE_WIDTH)
}

/// How many pixels a document scrolls for a scroll of `dy` that a table
/// passes on to it: `dy` itself for smooth (touchpad, `pixels`) scrolling;
/// for a mouse wheel, `dy` clicks of GTK's own step, the page height to the
/// power 2/3.
pub fn scroll_pixels(dy: f64, pixels: bool, page_size: f64) -> f64 {
    if pixels {
        dy
    } else {
        dy * page_size.max(0.0).powf(2.0 / 3.0)
    }
}

/// Where the top of the window is within the line under it, as a fraction
/// of the line's height (0.0 = the line's top), so the same spot can be
/// brought back after a zoom changes the line's size. A line without height
/// gives 0.0.
pub fn anchor_fraction(top: f64, line_top: f64, line_height: f64) -> f64 {
    if line_height > 0.0 {
        ((top - line_top) / line_height).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The scroll position that puts the spot [`anchor_fraction`] described
/// back at the top of the window, for the line's new position and height.
pub fn anchored_scroll(line_top: f64, line_height: f64, fraction: f64) -> f64 {
    line_top + fraction * line_height.max(0.0)
}

/// How long the view keeps the anchored spot in place after the last zoom
/// step or re-rendered picture, in milliseconds.
pub const ANCHOR_HOLD_MS: u64 = 400;

/// The size to request for a diagram picture of logical size `width` x
/// `height`: unchanged if it fits next to its card padding, otherwise scaled
/// down (keeping the aspect ratio) to the available width. Diagrams are never
/// scaled up.
pub fn diagram_size(width: f32, height: f32, view_width: i32) -> (i32, i32) {
    let room = (available_width(view_width) - 2 * DIAGRAM_PADDING).max(MIN_DIAGRAM_WIDTH) as f32;
    let factor = if width > 0.0 {
        (room / width).min(1.0)
    } else {
        1.0
    };
    (
        (width * factor).round() as i32,
        (height * factor).round() as i32,
    )
}

/// Converts a Pango font size into logical pixels. `size` is in Pango units
/// (1/1024); an absolute size is already in pixels, otherwise it is in
/// points, which are relative to 96 dpi. Returns `None` for a missing (zero
/// or negative) size.
pub fn font_size_px(size: i32, absolute: bool, pango_scale: i32) -> Option<f32> {
    let size = size as f32 / pango_scale as f32;
    let px = if absolute { size } else { size * 96.0 / 72.0 };
    (px > 0.0).then_some(px)
}

/// Converts a colour channel from 0.0-1.0 to 0-255, clamping out-of-range
/// values.
pub fn color_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The font scale of a heading level (1 largest); levels past 6 use the
/// level 6 scale.
pub fn heading_scale(level: u8) -> f64 {
    match level {
        0 | 1 => 2.0,
        2 => 1.6,
        3 => 1.3,
        4 => 1.15,
        5 => 1.0,
        _ => 0.9,
    }
}

/// The text `rise` (in Pango units) that lowers an inline formula reaching
/// `depth` logical pixels below the baseline, so its baseline lines up with
/// the text's.
pub fn math_rise(depth: f32, pango_scale: i32) -> i32 {
    -(depth * pango_scale as f32).round() as i32
}

/// Where text sits horizontally for a column alignment: 0.0 left, 0.5
/// centred, 1.0 right. Unaligned columns are left-aligned.
pub fn align_fraction(alignment: Align) -> f32 {
    match alignment {
        Align::None | Align::Left => 0.0,
        Align::Center => 0.5,
        Align::Right => 1.0,
    }
}

/// The CSS classes of the table cell at `row` (0 = header) and `column` of
/// a table with `columns` columns and `rows` rows including the header: every
/// cell is `md-cell`; the header row is `md-header`; every second body row is
/// `md-stripe`; cells in the last column and last row drop their right and
/// bottom borders.
pub fn cell_classes(row: usize, column: usize, columns: usize, rows: usize) -> Vec<&'static str> {
    let mut classes = vec!["md-cell"];
    if row == 0 {
        classes.push("md-header");
    } else if row.is_multiple_of(2) {
        classes.push("md-stripe");
    }
    if column + 1 == columns {
        classes.push("md-last-column");
    }
    if row + 1 == rows {
        classes.push("md-last-row");
    }
    classes
}

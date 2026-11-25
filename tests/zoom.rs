//! Integration tests for the document zoom (`md_viewer::zoom`): stepping,
//! clamping, the document's ceiling, the indicator, scroll accumulation and
//! the CSS that applies a zoom.

use md_viewer::zoom::{css, wheel_clicks, Zoom, MAX, MIN, PIXELS_PER_CLICK, STEPS};

/// Zoom starts at 100% and steps through the step list both ways, stopping
/// at the ends.
#[test]
fn steps_in_and_out() {
    let mut zoom = Zoom::default();
    assert_eq!(zoom.level(), 1.0);
    assert_eq!(zoom.percent(), 100);
    let mut seen = vec![zoom.level()];
    while zoom.zoom_in() {
        seen.push(zoom.level());
    }
    assert_eq!(zoom.level(), MAX);
    assert_eq!(seen, STEPS[4..].to_vec());
    while zoom.zoom_out() {}
    assert_eq!(zoom.level(), MIN);
    assert!(!zoom.zoom_out());
    assert!(zoom.reset());
    assert!(!zoom.reset());
    assert_eq!(zoom.level(), 1.0);
}

/// The steps are sorted and span exactly MIN..=MAX, with 100% among them.
#[test]
fn step_list() {
    assert!(STEPS.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(STEPS[0], MIN);
    assert_eq!(STEPS[STEPS.len() - 1], MAX);
    assert!(STEPS.contains(&1.0));
}

/// From a level between steps (after a pinch), stepping goes to the
/// neighbouring step in that direction.
#[test]
fn steps_from_between() {
    let mut zoom = Zoom::default();
    assert!(zoom.set(1.3));
    assert!(zoom.zoom_in());
    assert_eq!(zoom.level(), 1.5);
    zoom.set(1.3);
    assert!(zoom.zoom_out());
    assert_eq!(zoom.level(), 1.25);
}

/// Free levels are clamped; unchanged or non-finite levels are handled.
#[test]
fn set_clamps() {
    let mut zoom = Zoom::default();
    assert!(zoom.set(10.0));
    assert_eq!(zoom.level(), MAX);
    assert!(!zoom.set(MAX + 1.0));
    assert!(zoom.set(0.01));
    assert_eq!(zoom.level(), MIN);
    assert!(zoom.set(f64::NAN));
    assert_eq!(zoom.level(), 1.0);
    assert_eq!(Zoom::default().percent(), 100);
    zoom.set(1.256);
    assert_eq!(zoom.percent(), 126);
}

/// Every whole wheel click is one step; away from the user (negative)
/// zooms in. Fractions accumulate, and a change of direction drops them.
#[test]
fn scroll_accumulates() {
    let mut zoom = Zoom::default();
    assert!(zoom.scroll(-1.0));
    assert_eq!(zoom.level(), 1.1);
    assert!(zoom.scroll(2.0));
    assert_eq!(zoom.level(), 0.9);
    assert!(!zoom.scroll(-0.6));
    assert!(zoom.scroll(-0.6));
    assert_eq!(zoom.level(), 1.0);
    assert!(!zoom.scroll(0.6));
    // The direction changed: the 0.6 towards zooming out is dropped.
    assert!(!zoom.scroll(-0.5));
    assert!(!zoom.scroll(f64::INFINITY));
    assert_eq!(zoom.level(), 1.0);
}

/// Scrolling past either end stops at once (a huge delta takes no longer
/// than the steps there are) and keeps nothing for the way back.
#[test]
fn scroll_stops_at_the_ends() {
    let mut zoom = Zoom::default();
    let started = std::time::Instant::now();
    assert!(zoom.scroll(-1e12));
    assert_eq!(zoom.level(), MAX);
    assert!(zoom.scroll(1e12));
    assert_eq!(zoom.level(), MIN);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    // Nothing accumulated past the end: one click back is one step.
    assert!(zoom.scroll(-1.0));
    assert_eq!(zoom.level(), STEPS[1]);
}

/// The document's ceiling caps every way of zooming in: stepping stops at
/// it (on it, even between steps), free levels are clamped to it, and
/// scrolling stops there.
#[test]
fn ceiling_caps_zooming_in() {
    let mut zoom = Zoom::default();
    assert!(!zoom.set_ceiling(1.6));
    assert_eq!(zoom.max(), 1.6);
    while zoom.zoom_in() {}
    assert_eq!(zoom.level(), 1.6);
    assert!(zoom.at_max());
    assert!(!zoom.set(2.5));
    assert_eq!(zoom.level(), 1.6);
    zoom.reset();
    assert!(zoom.scroll(-100.0));
    assert_eq!(zoom.level(), 1.6);
    // Zooming out is not affected.
    assert!(zoom.zoom_out());
    assert_eq!(zoom.level(), 1.5);
    assert!(!zoom.at_max());
}

/// A lower ceiling zooms out to it at once (and says so); a ceiling above
/// the level leaves it. Ceilings are kept between 100% and MAX, and a
/// missing (infinite or NaN) one is MAX.
#[test]
fn ceiling_lowers_the_level() {
    let mut zoom = Zoom::default();
    zoom.set(2.5);
    assert!(zoom.set_ceiling(1.8));
    assert_eq!(zoom.level(), 1.8);
    assert!(!zoom.set_ceiling(2.0));
    assert_eq!(zoom.level(), 1.8);
    assert!(zoom.set_ceiling(0.3));
    assert_eq!((zoom.max(), zoom.level()), (1.0, 1.0));
    assert!(!zoom.set_ceiling(f64::INFINITY));
    assert_eq!(zoom.max(), MAX);
    zoom.set_ceiling(f64::NAN);
    assert_eq!(zoom.max(), MAX);
    assert_eq!(Zoom::default().max(), MAX);
}

/// The indicator shows the percentage, marked "(max)" at the largest zoom
/// allowed, and nothing at 100%.
#[test]
fn indicator_text() {
    let mut zoom = Zoom::default();
    assert_eq!(zoom.indicator(), None);
    zoom.set(1.5);
    assert_eq!(zoom.indicator().as_deref(), Some("150%"));
    zoom.set(0.5);
    assert_eq!(zoom.indicator().as_deref(), Some("50%"));
    zoom.set(MAX);
    assert_eq!(zoom.indicator().as_deref(), Some("300% (max)"));
    zoom.set_ceiling(1.8);
    assert_eq!(zoom.indicator().as_deref(), Some("180% (max)"));
    // At a ceiling of 100% the zoom is 100%: still no indicator.
    zoom.set_ceiling(1.0);
    assert_eq!(zoom.indicator(), None);
}

/// Mouse wheels report clicks, touchpads pixels.
#[test]
fn wheel_click_units() {
    assert_eq!(wheel_clicks(-1.0, false), -1.0);
    assert_eq!(wheel_clicks(PIXELS_PER_CLICK * 2.0, true), 2.0);
}

/// The zoom CSS sets the class's font size to the zoomed text size.
#[test]
fn zoom_css() {
    assert_eq!(
        css("md-zoom-3", 15.0, 1.5),
        ".md-zoom-3 { font-size: 22.50px; }"
    );
    assert_eq!(css("x", 14.0, 1.0), ".x { font-size: 14.00px; }");
}

/// `set_ceiling_keeping_level` never zooms out: a ceiling below the level
/// stops at the level (zooming in is refused, zooming out works, and the
/// indicator says "(max)"); one above it is taken as is, and can rise again.
#[test]
fn ceiling_keeping_level() {
    let mut zoom = Zoom::default();
    zoom.set(2.0);
    zoom.set_ceiling_keeping_level(1.5);
    assert_eq!(zoom.level(), 2.0);
    assert_eq!(zoom.max(), 2.0);
    assert!(zoom.at_max());
    assert_eq!(zoom.indicator().as_deref(), Some("200% (max)"));
    assert!(!zoom.zoom_in());
    assert_eq!(zoom.level(), 2.0);
    assert!(zoom.zoom_out());
    assert_eq!(zoom.level(), 1.75);
    zoom.set_ceiling_keeping_level(2.5);
    assert_eq!(zoom.max(), 2.5);
    assert!(zoom.zoom_in());
    zoom.set_ceiling_keeping_level(f64::NAN);
    assert_eq!(zoom.max(), MAX);
    zoom.set_ceiling_keeping_level(0.2);
    assert_eq!(zoom.max(), zoom.level());
    zoom.set(1.0);
    zoom.set_ceiling_keeping_level(0.2);
    assert_eq!(zoom.max(), 1.0);
}

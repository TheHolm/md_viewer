//! Integration tests for the document zoom (`md_viewer::zoom`): stepping,
//! clamping, scroll accumulation and the CSS that applies a zoom.

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

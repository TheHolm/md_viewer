//! The document zoom: the steps Ctrl+wheel and Ctrl+plus/minus move
//! between, free values from a pinch gesture, and the CSS that applies a
//! zoom to a window's text.

/// The zoom levels Ctrl+wheel and Ctrl+plus/minus step through.
pub const STEPS: [f64; 12] = [
    0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

/// The smallest zoom.
pub const MIN: f64 = 0.5;

/// The largest zoom.
pub const MAX: f64 = 3.0;

/// How close two zoom levels must be to count as the same step.
const EPSILON: f64 = 1e-6;

/// A window's zoom level, plus the scroll distance not yet turned into a
/// step (touchpads scroll in small fractions of a wheel click), and the
/// ceiling the document shown sets on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zoom {
    /// The current zoom factor, 1.0 = 100%.
    level: f64,
    /// Accumulated vertical scroll, in wheel clicks.
    scrolled: f64,
    /// The largest zoom the document allows (its largest graphic would
    /// otherwise exceed the size limit), between 1.0 and [`MAX`].
    ceiling: f64,
}

impl Default for Zoom {
    /// 100%, with no ceiling below [`MAX`].
    fn default() -> Self {
        Zoom {
            level: 1.0,
            scrolled: 0.0,
            ceiling: MAX,
        }
    }
}

impl Zoom {
    /// The current zoom factor, 1.0 = 100%.
    pub fn level(&self) -> f64 {
        self.level
    }

    /// The largest zoom allowed: [`MAX`], or less if the document's ceiling
    /// is lower.
    pub fn max(&self) -> f64 {
        self.ceiling
    }

    /// Sets the ceiling the document imposes (`limit`, clamped to
    /// 1.0..=[`MAX`]; a non-finite limit means none), lowering the level to
    /// it if the level is above it. Returns true if the level changed.
    pub fn set_ceiling(&mut self, limit: f64) -> bool {
        self.ceiling = if limit.is_nan() {
            MAX
        } else {
            limit.clamp(1.0, MAX)
        };
        if self.level > self.ceiling + EPSILON {
            self.level = self.ceiling;
            true
        } else {
            false
        }
    }

    /// Sets the ceiling like [`Zoom::set_ceiling`], but never lowers the
    /// level: if the level is above `limit`, the ceiling stays at the level
    /// instead, so zooming in is refused there and zooming out works as
    /// usual. Used when the window moved to a monitor with another scale,
    /// where the text the user is reading should not shrink.
    pub fn set_ceiling_keeping_level(&mut self, limit: f64) {
        let limit = if limit.is_nan() {
            MAX
        } else {
            limit.clamp(1.0, MAX)
        };
        self.ceiling = limit.max(self.level).min(MAX);
    }

    /// True when the level is at the largest zoom allowed.
    pub fn at_max(&self) -> bool {
        self.level >= self.ceiling - EPSILON
    }

    /// The text of the zoom indicator: the percentage, with "(max)" at the
    /// largest zoom allowed; `None` at 100%, where no indicator is shown.
    pub fn indicator(&self) -> Option<String> {
        if self.percent() == 100 {
            return None;
        }
        Some(if self.at_max() {
            format!("{}% (max)", self.percent())
        } else {
            format!("{}%", self.percent())
        })
    }

    /// The zoom as a whole percentage, for display.
    pub fn percent(&self) -> u32 {
        (self.level * 100.0).round() as u32
    }

    /// Moves to the next larger step (from between steps: the next step
    /// above). Returns true if the level changed.
    pub fn zoom_in(&mut self) -> bool {
        let next = STEPS
            .iter()
            .copied()
            .find(|step| *step > self.level + EPSILON);
        self.set(next.unwrap_or(self.ceiling))
    }

    /// Moves to the next smaller step. Returns true if the level changed.
    pub fn zoom_out(&mut self) -> bool {
        let next = STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < self.level - EPSILON);
        self.set(next.unwrap_or(MIN))
    }

    /// Back to 100%. Returns true if the level changed.
    pub fn reset(&mut self) -> bool {
        self.set(1.0)
    }

    /// Sets any level, clamped to [`MIN`] and the largest zoom allowed
    /// ([`Zoom::max`]; a pinch gesture zooms smoothly rather than in steps).
    /// Returns true if the level changed.
    pub fn set(&mut self, level: f64) -> bool {
        let level = if level.is_finite() { level } else { 1.0 };
        let level = level.clamp(MIN, self.ceiling);
        let changed = (level - self.level).abs() > EPSILON;
        self.level = level;
        changed
    }

    /// Feeds a Ctrl+scroll of `dy` wheel clicks (negative: away from the
    /// user, which zooms in). Every whole click accumulated moves one step;
    /// a change of direction drops the remainder; scrolling on at either
    /// end of the range drops it too, so a huge `dy` takes no longer than
    /// the steps there are. Returns true if the level changed.
    pub fn scroll(&mut self, dy: f64) -> bool {
        if !dy.is_finite() {
            return false;
        }
        if self.scrolled * dy < 0.0 {
            self.scrolled = 0.0;
        }
        self.scrolled += dy;
        let mut changed = false;
        while self.scrolled <= -1.0 {
            self.scrolled += 1.0;
            if !self.zoom_in() {
                self.scrolled = 0.0;
                break;
            }
            changed = true;
        }
        while self.scrolled >= 1.0 {
            self.scrolled -= 1.0;
            if !self.zoom_out() {
                self.scrolled = 0.0;
                break;
            }
            changed = true;
        }
        changed
    }
}

/// How many pixels of smooth (touchpad) scrolling count as one wheel click.
pub const PIXELS_PER_CLICK: f64 = 50.0;

/// Converts a scroll of `dy` into wheel clicks: `dy` is already in clicks
/// for a mouse wheel, and in pixels (`pixels`) for smooth touchpad
/// scrolling.
pub fn wheel_clicks(dy: f64, pixels: bool) -> f64 {
    if pixels {
        dy / PIXELS_PER_CLICK
    } else {
        dy
    }
}

/// The CSS that sets the text size of the widgets with CSS class `class` to
/// `base_px` times `level`: applied to a document view, it zooms the text
/// and everything inside it (tables, code cards) that inherits the size.
pub fn css(class: &str, base_px: f32, level: f64) -> String {
    format!(".{class} {{ font-size: {:.2}px; }}", base_px as f64 * level)
}

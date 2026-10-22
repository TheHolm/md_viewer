//! Turning formulas (and, later, diagrams) into pixels, independent of GTK.
//!
//! Math is typeset to SVG by `mathra` (which also understands mhchem's
//! `\ce{}`/`\pu{}`), then every SVG is rasterized by `resvg` into
//! premultiplied RGBA pixels the GUI wraps in a texture. Nothing here needs a
//! display, so it all runs on a worker thread and is covered by tests.

use crate::{Block, Inline};
use resvg::{tiny_skia, usvg};
use std::sync::{Arc, OnceLock};

/// The result of rendering one graphic: the image, or a message saying why
/// it could not be rendered (the GUI then shows the source instead).
pub type Rendered = Result<Image, String>;

/// A rasterized graphic.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
    /// The device-pixel scale it was rendered at; the logical size is
    /// `width / scale` by `height / scale`.
    pub scale: f32,
    /// How far the image reaches below the text baseline, in logical pixels
    /// (non-zero for inline math with descenders, fractions and the like).
    pub depth: f32,
    /// Premultiplied RGBA pixels, row by row, `width * 4` bytes per row.
    pub pixels: Vec<u8>,
}

impl Image {
    /// The logical (scale-independent) width, in pixels.
    pub fn logical_width(&self) -> f32 {
        self.width as f32 / self.scale
    }

    /// The logical (scale-independent) height, in pixels.
    pub fn logical_height(&self) -> f32 {
        self.height as f32 / self.scale
    }
}

/// How graphics are rendered for a particular view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RasterSettings {
    /// The body text size in logical pixels; math is sized relative to it.
    pub font_px: f32,
    /// The device-pixel scale (2.0 on a HiDPI display).
    pub scale: f32,
    /// The text colour math is drawn in, as RGB.
    pub color: [u8; 3],
}

impl Default for RasterSettings {
    /// 15 px black text at scale 1, as used by the tests.
    fn default() -> Self {
        RasterSettings {
            font_px: 15.0,
            scale: 1.0,
            color: [0, 0, 0],
        }
    }
}

/// How much larger math is than the body text. 1.21 is what KaTeX uses, so
/// formulas look as big as in Joplin or a browser; math fonts have a small
/// x-height and look too small at exactly the text size.
pub const MATH_SCALE: f32 = 1.21;

/// The system font database, loaded once (it scans every installed font).
fn font_database() -> &'static Arc<usvg::fontdb::Database> {
    static DATABASE: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DATABASE.get_or_init(|| {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        if let Some(family) = preferred_sans_family(&database) {
            database.set_sans_serif_family(family);
        }
        Arc::new(database)
    })
}

/// Picks the family that SVG text asking for `sans-serif` (or for a font
/// that is not installed) is drawn with: the first of a few common desktop
/// sans-serif families that is installed, otherwise any installed family.
/// fontdb's own default (Arial) is rarely installed on Linux or FreeBSD.
fn preferred_sans_family(database: &usvg::fontdb::Database) -> Option<String> {
    let families: Vec<&str> = database
        .faces()
        .flat_map(|face| face.families.iter().map(|(name, _)| name.as_str()))
        .collect();
    pick_family(&families)
}

/// Returns the first of the preferred sans-serif families present in
/// `families`, or the first family at all, or `None` if there are none.
fn pick_family(families: &[&str]) -> Option<String> {
    const PREFERRED: [&str; 6] = [
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
        "Liberation Sans",
        "Adwaita Sans",
        "Inter",
    ];
    PREFERRED
        .iter()
        .find(|wanted| families.contains(wanted))
        .or(families.first())
        .map(|family| family.to_string())
}

/// SVG parsing options using the shared system font database, with `em`
/// units resolving to `font_px`.
fn svg_options(font_px: f32) -> usvg::Options<'static> {
    let database = font_database().clone();
    let font_family = preferred_sans_family(&database).unwrap_or_else(|| "sans-serif".into());
    usvg::Options {
        fontdb: database,
        font_family,
        font_size: font_px,
        ..usvg::Options::default()
    }
}

/// Rasterizes `svg` at `scale` device pixels per SVG pixel, with `em` units
/// resolving to `font_px`. Fails on invalid SVG or an empty image.
pub fn rasterize(svg: &str, font_px: f32, scale: f32) -> Rendered {
    let tree = usvg::Tree::from_str(svg, &svg_options(font_px)).map_err(|e| e.to_string())?;
    let size = tree.size();
    let width = (size.width() * scale).ceil() as u32;
    let height = (size.height() * scale).ceil() as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("image has no area ({width}x{height})"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Ok(Image {
        width,
        height,
        scale,
        depth: 0.0,
        pixels: pixmap.take(),
    })
}

/// Formats an RGB colour as `#rrggbb`.
pub fn hex_color(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

/// Typesets LaTeX math (including mhchem `\ce{}`/`\pu{}`) to SVG.
///
/// mathra recovers from most mistakes on its own (it draws what it can), so
/// this only fails on input it cannot read at all.
pub fn math_svg(tex: &str, display: bool) -> Result<String, String> {
    let options = mathra::Options {
        display,
        ..mathra::Options::default()
    };
    mathra::convert(tex, &options)
        .map(|output| output.svg)
        .map_err(|error| error.to_string())
}

/// Reads the `vertical-align: -<n>em` mathra puts on its root element: how
/// far the formula reaches below the baseline, in `em`. Zero if absent.
pub fn math_depth_em(svg: &str) -> f32 {
    let root_end = svg.find('>').unwrap_or(svg.len());
    let root = &svg[..root_end];
    root.split("vertical-align:")
        .nth(1)
        .and_then(|rest| rest.trim_start().split("em").next())
        .and_then(|value| value.trim().parse::<f32>().ok())
        .map(|value| (-value).max(0.0))
        .unwrap_or(0.0)
}

/// Renders a formula [`MATH_SCALE`] times the body text size, in the text
/// colour. Display math uses TeX's display style (larger operators, limits
/// above and below).
pub fn render_math(tex: &str, display: bool, settings: &RasterSettings) -> Rendered {
    if tex.trim().is_empty() {
        return Err("empty formula".to_string());
    }
    let svg = math_svg(tex, display)?;
    let font_px = settings.font_px * MATH_SCALE;
    let svg = svg.replace("currentColor", &hex_color(settings.color));
    let mut image = rasterize(&svg, font_px, settings.scale)?;
    image.depth = math_depth_em(&svg) * font_px;
    Ok(image)
}

/// Renders every graphic in `blocks` (math, in text and in table cells),
/// storing each result next to its source. Meant to run on a worker thread.
pub fn render_document(blocks: &mut [Block], settings: &RasterSettings) {
    for block in blocks.iter_mut() {
        match block {
            Block::Text(inlines) => render_inlines(inlines, settings),
            Block::Table(table) => {
                for cell in table
                    .header
                    .iter_mut()
                    .chain(table.rows.iter_mut().flatten())
                {
                    render_inlines(cell, settings);
                }
            }
        }
    }
}

/// Renders the math among `inlines`.
fn render_inlines(inlines: &mut [Inline], settings: &RasterSettings) {
    for inline in inlines.iter_mut() {
        if let Inline::Math(math) = inline {
            math.image = Some(render_math(&math.tex, math.display, settings));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The preferred families win in order; otherwise any family is used.
    #[test]
    fn pick_family_order() {
        assert_eq!(
            pick_family(&["Foo", "DejaVu Sans", "Noto Sans"]).as_deref(),
            Some("Noto Sans")
        );
        assert_eq!(pick_family(&["Foo", "Bar"]).as_deref(), Some("Foo"));
        assert_eq!(pick_family(&[]), None);
    }

    /// `svg_options` makes `em` units resolve to the requested size.
    #[test]
    fn svg_options_font_size() {
        assert_eq!(svg_options(20.0).font_size, 20.0);
    }
}

//! Integration tests for rendering graphics (`md_viewer::graphics`): math and
//! mhchem typesetting, rasterization, and filling a rendered document in.

use md_viewer::graphics::{
    hex_color, math_depth_em, math_svg, rasterize, render_document, render_math, RasterSettings,
    MATH_SCALE,
};
use md_viewer::{render, Block, Inline, Math};

/// A 20x10 SVG with a red rectangle filling its left half.
const HALF_RED: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
<rect x="0" y="0" width="10" height="10" fill="#ff0000"/></svg>"##;

/// Returns the premultiplied RGBA pixel at (`x`, `y`).
fn pixel(image: &md_viewer::graphics::Image, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * image.width + x) * 4) as usize;
    image.pixels[at..at + 4].try_into().unwrap()
}

/// True if any pixel of `image` is opaque enough to be visible.
fn has_ink(image: &md_viewer::graphics::Image) -> bool {
    image.pixels.chunks(4).any(|p| p[3] > 128)
}

/// Every formula in the rendered document, in order.
fn formulas(blocks: &[Block]) -> Vec<&Math> {
    let mut found = Vec::new();
    for block in blocks {
        let cells: Vec<&Vec<Inline>> = match block {
            Block::Text(inlines) => vec![inlines],
            Block::Table(table) => table
                .header
                .iter()
                .chain(table.rows.iter().flatten())
                .collect(),
        };
        for inline in cells.into_iter().flatten() {
            if let Inline::Math(math) = inline {
                found.push(math);
            }
        }
    }
    found
}

// --- Rasterization --------------------------------------------------------

/// An SVG is rasterized at its own size, with its colours, at scale 1.
#[test]
fn rasterize_size_and_colour() {
    let image = rasterize(HALF_RED, 15.0, 1.0).unwrap();
    assert_eq!((image.width, image.height), (20, 10));
    assert_eq!(image.pixels.len(), 20 * 10 * 4);
    assert_eq!(pixel(&image, 2, 5), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 15, 5), [0, 0, 0, 0]);
}

/// A device-pixel scale multiplies the pixel size but not the logical size.
#[test]
fn rasterize_scale() {
    let image = rasterize(HALF_RED, 15.0, 2.0).unwrap();
    assert_eq!((image.width, image.height), (40, 20));
    assert_eq!(image.logical_width(), 20.0);
    assert_eq!(image.logical_height(), 10.0);
}

/// `em` units resolve to the given font size.
#[test]
fn rasterize_em_units() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2em" height="1em"/>"#;
    let image = rasterize(svg, 10.0, 1.0).unwrap();
    assert_eq!((image.width, image.height), (20, 10));
}

/// Invalid SVG and an empty image are errors, not panics.
#[test]
fn rasterize_errors() {
    assert!(rasterize("not svg", 15.0, 1.0).is_err());
    let empty = r#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/>"#;
    assert!(rasterize(empty, 15.0, 1.0).is_err());
}

/// SVG text is drawn with an installed font, so labels are not lost even
/// when the requested family (here none at all) is missing.
#[test]
fn rasterize_draws_text() {
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="20">
<text x="2" y="15" font-size="14" font-family="No Such Font, sans-serif">Hello</text></svg>"#;
    assert!(has_ink(&rasterize(svg, 15.0, 1.0).unwrap()));
}

/// `hex_color` formats RGB as lowercase `#rrggbb`.
#[test]
fn hex_color_format() {
    assert_eq!(hex_color([255, 8, 171]), "#ff08ab");
}

// --- Math -----------------------------------------------------------------

/// A formula typesets to SVG.
#[test]
fn math_svg_is_svg() {
    let svg = math_svg(r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}", true).unwrap();
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains("currentColor"));
}

/// mhchem chemistry (`\ce`) and physical units (`\pu`) are supported.
#[test]
fn mhchem_renders() {
    let settings = RasterSettings::default();
    for tex in [
        r"\ce{2H2 + O2 -> 2H2O}",
        r"\ce{Hg^2+ ->[I-] HgI2}",
        r"\ce{CO2 + C <=> 2CO}",
        r"\pu{123 kJ/mol}",
    ] {
        let image = render_math(tex, false, &settings).unwrap_or_else(|e| panic!("{tex}: {e}"));
        assert!(has_ink(&image), "{tex} drew nothing");
    }
}

/// Inline math is sized relative to the body text: twice the font size
/// gives (about) twice the height.
#[test]
fn math_scales_with_font_size() {
    let small = RasterSettings {
        font_px: 10.0,
        ..RasterSettings::default()
    };
    let large = RasterSettings {
        font_px: 20.0,
        ..RasterSettings::default()
    };
    let a = render_math("x", false, &small).unwrap();
    let b = render_math("x", false, &large).unwrap();
    let ratio = b.height as f32 / a.height as f32;
    assert!((1.7..2.3).contains(&ratio), "ratio {ratio}");
}

/// Display math uses display style: a sum with limits is taller than the
/// same formula inline.
#[test]
fn display_math_is_larger() {
    let settings = RasterSettings::default();
    let inline = render_math(r"\sum_{i=1}^n x", false, &settings).unwrap();
    let display = render_math(r"\sum_{i=1}^n x", true, &settings).unwrap();
    assert!(display.height > inline.height);
}

/// Math is set larger than the body text, as KaTeX does: a plain `x` is
/// taller than half the font size even though math fonts have a small
/// x-height.
#[test]
fn math_is_scaled_up() {
    assert!(MATH_SCALE > 1.0);
    let settings = RasterSettings {
        font_px: 20.0,
        ..RasterSettings::default()
    };
    let image = render_math("M", false, &settings).unwrap();
    assert!(image.height as f32 >= 20.0 * 0.6, "height {}", image.height);
}

/// Math is drawn in the requested text colour.
#[test]
fn math_uses_text_colour() {
    let settings = RasterSettings {
        font_px: 40.0,
        color: [0, 200, 0],
        ..RasterSettings::default()
    };
    let image = render_math(r"\blacksquare", false, &settings).unwrap();
    let solid: Vec<&[u8]> = image.pixels.chunks(4).filter(|p| p[3] == 255).collect();
    assert!(!solid.is_empty());
    assert!(solid.iter().all(|p| p[0] == 0 && p[1] == 200 && p[2] == 0));
}

/// A formula reaching below the baseline reports its depth; one that does
/// not reports (about) zero.
#[test]
fn math_depth() {
    let settings = RasterSettings::default();
    let fraction = render_math(r"\frac{a}{b}", false, &settings).unwrap();
    assert!(fraction.depth > 1.0, "depth {}", fraction.depth);
    let plain = render_math("a", false, &settings).unwrap();
    assert!(plain.depth < fraction.depth);
}

/// `math_depth_em` reads the root element's negative vertical-align.
#[test]
fn math_depth_em_parsing() {
    assert_eq!(
        math_depth_em(r#"<svg style="vertical-align: -0.5em"><g/></svg>"#),
        0.5
    );
    assert_eq!(math_depth_em(r#"<svg width="1em"></svg>"#), 0.0);
    assert_eq!(
        math_depth_em(r#"<svg style="vertical-align: 0.2em"></svg>"#),
        0.0
    );
}

/// An empty formula is an error (shown as source), not an empty image.
#[test]
fn empty_formula_is_error() {
    assert!(render_math("  ", false, &RasterSettings::default()).is_err());
}

/// Broken LaTeX never panics: it is either recovered from (an image) or an
/// error message.
#[test]
fn broken_math_does_not_panic() {
    for tex in [r"\frac{", r"}{", r"\unknowncommand x", r"\ce{->->}", r"^^^"] {
        let _ = render_math(tex, true, &RasterSettings::default());
    }
}

// --- Whole documents ------------------------------------------------------

/// `render_document` renders every formula, in text and in table cells.
#[test]
fn render_document_fills_in_formulas() {
    let mut blocks = render("Inline $x$.\n\n$$\\ce{H2O}$$\n\n| m |\n|---|\n| $y$ |");
    assert!(formulas(&blocks).iter().all(|math| math.image.is_none()));
    render_document(&mut blocks, &RasterSettings::default());
    let found = formulas(&blocks);
    assert_eq!(found.len(), 3);
    for math in found {
        assert!(
            matches!(&math.image, Some(Ok(image)) if image.width > 0),
            "{} not rendered",
            math.tex
        );
    }
}

/// A document without math is left unchanged.
#[test]
fn render_document_without_math() {
    let original = render("# T\n\ntext\n\n| a |\n|---|");
    let mut blocks = original.clone();
    render_document(&mut blocks, &RasterSettings::default());
    assert_eq!(blocks, original);
}

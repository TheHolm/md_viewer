//! Integration tests for rendering graphics (`md_viewer::graphics`): math and
//! mhchem typesetting, rasterization, and filling a rendered document in.

use md_viewer::graphics::{
    decode_image, hex_color, load_image, math_depth_em, math_svg, max_px_for, mermaid_svg,
    rasterize, render_document, render_math, render_mermaid, zoom_limit, Cache, Graphic,
    RasterSettings, FALLBACK_MAX_PX, MATH_SCALE, SCREEN_MULTIPLE,
};
use md_viewer::{render, Block, ImageRef, Inline, Math};
use std::path::{Path, PathBuf};

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
            Block::Mermaid(_) | Block::Code(_) => Vec::new(),
        };
        for inline in cells.into_iter().flatten() {
            if let Inline::Math(math) = inline {
                found.push(math);
            }
        }
    }
    found
}

/// Every image in the rendered document, in order (in text and table cells).
fn images(blocks: &[Block]) -> Vec<&ImageRef> {
    let mut found = Vec::new();
    for block in blocks {
        let cells: Vec<&Vec<Inline>> = match block {
            Block::Text(inlines) => vec![inlines],
            Block::Table(table) => table
                .header
                .iter()
                .chain(table.rows.iter().flatten())
                .collect(),
            Block::Mermaid(_) | Block::Code(_) => Vec::new(),
        };
        for inline in cells.into_iter().flatten() {
            if let Inline::Image(image) = inline {
                found.push(image);
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
    let settings = RasterSettings {
        font_px: 20.0,
        ..RasterSettings::default()
    };
    let image = render_math("M", false, &settings).unwrap();
    assert!(
        image.height as f32 >= 20.0 * MATH_SCALE * 0.6,
        "height {}",
        image.height
    );
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

// --- Mermaid --------------------------------------------------------------

/// A flowchart lays out to SVG and rasterizes to a visible image whose
/// labels are drawn (text, not HTML, so resvg can draw it).
#[test]
fn mermaid_flowchart() {
    let source = "graph TD;\n A-->B;\n A-->C;\n B-->D;\n C-->D;";
    let svg = mermaid_svg(source).unwrap();
    assert!(svg.contains("<svg"));
    assert!(!svg.contains("<foreignObject"), "labels must be SVG text");
    let image = render_mermaid(source, &RasterSettings::default()).unwrap();
    assert!(image.width > 50 && image.height > 50);
    assert!(has_ink(&image));
}

/// Sequence diagrams and Gantt charts render too.
#[test]
fn mermaid_sequence_and_gantt() {
    let settings = RasterSettings::default();
    for source in [
        "sequenceDiagram\n Alice->>Bob: Hello Bob\n Bob-->>Alice: Hi Alice",
        "gantt\n title Plan\n dateFormat YYYY-MM-DD\n section Build\n \
         Tables :a1, 2026-11-01, 5d\n Math :after a1, 4d",
        "pie title Pets\n \"Dogs\" : 386\n \"Cats\" : 85",
        "classDiagram\n Animal <|-- Duck",
        "stateDiagram-v2\n [*] --> Still\n Still --> [*]",
    ] {
        let image = render_mermaid(source, &settings).unwrap_or_else(|e| panic!("{source}: {e}"));
        assert!(has_ink(&image), "{source} drew nothing");
    }
}

/// A diagram's text is actually drawn: the same flowchart with long labels
/// has more dark pixels than with one-letter labels (the node boxes are the
/// same size and filled white, so only the text differs).
#[test]
fn mermaid_labels_are_drawn() {
    let settings = RasterSettings::default();
    let ink = |source: &str| {
        let image = render_mermaid(source, &settings).unwrap();
        image
            .pixels
            .chunks(4)
            .filter(|p| p[3] > 200 && p[0] < 100 && p[1] < 100)
            .count()
    };
    let short = ink("graph LR; A[x] --> B[y]");
    let long = ink("graph LR; A[xxxxxxxxxxxx] --> B[yyyyyyyyyyyy]");
    assert!(long > short, "{long} <= {short}");
}

/// A device-pixel scale doubles the diagram's pixels, not its logical size.
#[test]
fn mermaid_scale() {
    let source = "graph LR; A-->B";
    let one = render_mermaid(source, &RasterSettings::default()).unwrap();
    let two = render_mermaid(
        source,
        &RasterSettings {
            scale: 2.0,
            ..RasterSettings::default()
        },
    )
    .unwrap();
    assert!((two.logical_width() - one.logical_width()).abs() <= 1.0);
    assert!(two.width >= one.width * 2 - 1);
}

/// Invalid, unknown and empty diagrams are errors with a message, never
/// panics.
#[test]
fn mermaid_errors() {
    let settings = RasterSettings::default();
    for source in ["graph TD; A-->", "notADiagramType\n x", "", "   "] {
        let error = render_mermaid(source, &settings).unwrap_err();
        assert!(!error.is_empty(), "{source:?}");
    }
}

/// A diagram's labels are extracted from the laid-out SVG (the find bar
/// searches them); math draws outlines, so it has none.
#[test]
fn diagram_labels_are_extracted() {
    let settings = RasterSettings::default();
    let image = render_mermaid("graph TD; A[Coffee] --> B[Water]", &settings).unwrap();
    assert!(
        image.labels.iter().any(|label| label == "Coffee"),
        "{:?}",
        image.labels
    );
    assert!(
        image.labels.iter().any(|label| label == "Water"),
        "{:?}",
        image.labels
    );
    assert!(render_math("x", false, &settings)
        .unwrap()
        .labels
        .is_empty());
}

// --- Images ---------------------------------------------------------------

/// Writes a `width` x `height` image of one straight-alpha colour, picking
/// the encoder from the file extension, and returns its path.
fn write_image(dir: &Path, name: &str, width: u32, height: u32, rgba: [u8; 4]) -> PathBuf {
    let path = dir.join(name);
    let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba(rgba));
    if name.ends_with(".jpg") || name.ends_with(".jpeg") {
        // JPEG has no alpha channel, so the encoder wants RGB.
        image::DynamicImage::ImageRgba8(buffer)
            .to_rgb8()
            .save(&path)
            .unwrap();
    } else {
        buffer.save(&path).unwrap();
    }
    path
}

/// A temporary directory for the image tests.
fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A PNG decodes to premultiplied RGBA at its natural size, fully opaque
/// pixels unchanged.
#[test]
fn image_decode_png() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "red.png", 3, 2, [255, 0, 0, 255]);
    let image = decode_image(&path, &RasterSettings::default()).unwrap();
    assert_eq!((image.width, image.height), (3, 2));
    assert_eq!((image.logical_width(), image.logical_height()), (3.0, 2.0));
    assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
    assert!(image.labels.is_empty());
}

/// Straight alpha is premultiplied: a half-transparent white becomes about
/// half grey in every channel, keeping the alpha.
#[test]
fn image_decode_premultiplies() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "half.png", 1, 1, [255, 255, 255, 128]);
    let image = decode_image(&path, &RasterSettings::default()).unwrap();
    let [red, green, blue, alpha] = pixel(&image, 0, 0);
    assert_eq!(alpha, 128);
    for channel in [red, green, blue] {
        assert!((127..=129).contains(&channel), "{channel}");
    }
}

/// A JPEG decodes too (the encoder is chosen by extension).
#[test]
fn image_decode_jpeg() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "photo.jpg", 4, 4, [0, 0, 255, 255]);
    let image = decode_image(&path, &RasterSettings::default()).unwrap();
    assert_eq!((image.width, image.height), (4, 4));
}

/// An SVG file is rasterized with the SVG path, keeping its natural size.
#[test]
fn image_decode_svg() {
    let dir = temp_dir();
    let path = dir.path().join("half.svg");
    std::fs::write(&path, HALF_RED).unwrap();
    let image = decode_image(&path, &RasterSettings::default()).unwrap();
    assert_eq!(
        (image.logical_width(), image.logical_height()),
        (20.0, 10.0)
    );
    assert!(has_ink(&image));
    assert_eq!(pixel(&image, 2, 5), [255, 0, 0, 255]);
}

/// A missing or broken file is an error, not a panic.
#[test]
fn image_decode_errors() {
    let error = decode_image(
        Path::new("/definitely/not/here.png"),
        &RasterSettings::default(),
    )
    .unwrap_err();
    assert!(error.contains("cannot read"), "{error}");

    let dir = temp_dir();
    let path = dir.path().join("bad.png");
    std::fs::write(&path, b"not an image").unwrap();
    assert!(decode_image(&path, &RasterSettings::default()).is_err());
}

/// A raster image is never enlarged for a HiDPI screen (that adds no
/// detail and multiplies the memory): at device scale 2 it keeps the file's
/// own pixels and its logical size is still its size in pixels; the screen
/// scales it when drawing.
#[test]
fn image_decode_at_device_scale_keeps_file_pixels() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "dot.png", 4, 2, [0, 255, 0, 255]);
    let settings = RasterSettings {
        scale: 2.0,
        ..RasterSettings::default()
    };
    let image = decode_image(&path, &settings).unwrap();
    assert_eq!((image.width, image.height), (4, 2));
    assert_eq!(image.scale, 1.0);
    assert_eq!((image.logical_width(), image.logical_height()), (4.0, 2.0));
    assert_eq!(image.pixels.len(), 4 * 2 * 4);
}

/// Writes a `width` x `height` JPEG carrying an Exif orientation tag (6 means
/// "rotate 90 degrees"), by splicing a minimal little-endian Exif APP1 segment
/// after the file's SOI marker, and returns its path.
fn write_jpeg_with_orientation(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
    let path = dir.join(name);
    let buffer = image::RgbImage::from_pixel(width, height, image::Rgb([10, 20, 30]));
    let mut jpeg = Vec::new();
    image::DynamicImage::ImageRgb8(buffer)
        .write_to(
            &mut std::io::Cursor::new(&mut jpeg),
            image::ImageFormat::Jpeg,
        )
        .unwrap();

    // TIFF header (little-endian), one IFD entry: orientation (0x0112), SHORT,
    // count 1, value 6.
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II\x2a\x00");
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&6u16.to_le_bytes());
    tiff.extend_from_slice(&0u16.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut app1 = Vec::new();
    app1.extend_from_slice(&[0xFF, 0xE1]);
    app1.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    app1.extend_from_slice(b"Exif\x00\x00");
    app1.extend_from_slice(&tiff);

    let mut out = Vec::new();
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&app1);
    out.extend_from_slice(&jpeg[2..]);
    std::fs::write(&path, out).unwrap();
    path
}

/// The Exif orientation is applied: a 6x2 JPEG tagged "rotate 90" decodes to
/// 2x6, so sideways photos come out upright.
#[test]
fn image_decode_applies_exif_orientation() {
    let dir = temp_dir();
    let path = write_jpeg_with_orientation(dir.path(), "rotated.jpg", 6, 2);
    let image = decode_image(&path, &RasterSettings::default()).unwrap();
    assert_eq!((image.width, image.height), (2, 6));
    assert_eq!((image.logical_width(), image.logical_height()), (2.0, 6.0));
}

/// Encodes `data` as standard base64 (with padding), for `data:` URLs.
fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let value = (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2]);
        out.push(ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(ALPHABET[(value >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(value >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[value as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// A raster image embedded in an SVG (here a `data:` PNG) is drawn, now that
/// resvg has raster image decoders.
#[test]
fn svg_embedded_raster_image() {
    let mut png = Vec::new();
    image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
             <image x="2" y="2" width="4" height="4" href="data:image/png;base64,{}"/>
           </svg>"#,
        base64(&png)
    );
    let image = rasterize(&svg, 15.0, 1.0).unwrap();
    assert_eq!((image.logical_width(), image.logical_height()), (8.0, 8.0));
    assert_eq!(pixel(&image, 4, 4), [255, 0, 0, 255]);
    assert_eq!(pixel(&image, 0, 0)[3], 0);
}

/// Loading at a zoom grows the logical size (the pixels stay the same), like
/// math and diagrams.
#[test]
fn image_zoom_scales_logical_size() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "dot.png", 4, 2, [0, 255, 0, 255]);
    let settings = RasterSettings {
        zoom: 2.0,
        ..RasterSettings::default()
    };
    let image = load_image(&path, &settings).unwrap();
    assert_eq!((image.logical_width(), image.logical_height()), (8.0, 4.0));
    assert_eq!((image.width, image.height), (4, 2));
}

/// An inline image is capped to a few lines tall (keeping its aspect ratio),
/// while the same image alone in a paragraph keeps its size.
#[test]
fn inline_image_is_capped() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "wide.png", 1000, 10, [0, 0, 255, 255]);
    let settings = RasterSettings::default();
    let cache = Cache::shared();
    let inline = cache
        .render(
            &Graphic::Image {
                path: path.clone(),
                inline: true,
            },
            &settings,
        )
        .unwrap();
    let standalone = cache
        .render(
            &Graphic::Image {
                path,
                inline: false,
            },
            &settings,
        )
        .unwrap();
    assert_eq!(standalone.logical_width(), 1000.0);
    assert!(
        inline.logical_width() <= 15.0 * 12.0 + 0.5,
        "{}",
        inline.logical_width()
    );
    let aspect = inline.logical_width() / inline.logical_height();
    assert!((aspect - 100.0).abs() < 0.01, "{aspect}");
}

// --- Size limit -----------------------------------------------------------

/// Settings with a graphics size limit of `max_px` (otherwise the defaults).
fn limited(max_px: [u32; 2]) -> RasterSettings {
    RasterSettings {
        max_px,
        ..RasterSettings::default()
    }
}

/// Asserts that `result` failed because the graphic is too large.
fn assert_too_large<T: std::fmt::Debug>(result: Result<T, String>) {
    match result {
        Err(reason) => assert!(reason.starts_with("too large"), "{reason}"),
        Ok(found) => panic!("expected a size error, got {found:?}"),
    }
}

/// The limit is ten screens in device pixels; an unknown or invalid screen
/// gives the fallback (ten 1920x1080 screens), and huge values saturate.
#[test]
fn max_px_is_ten_screens() {
    assert_eq!(SCREEN_MULTIPLE, 10);
    assert_eq!(max_px_for(1920, 1080, 1), [19_200, 10_800]);
    assert_eq!(max_px_for(1920, 1080, 2), [38_400, 21_600]);
    assert_eq!(FALLBACK_MAX_PX, [19_200, 10_800]);
    for (width, height, scale) in [(0, 1080, 1), (1920, 0, 1), (1920, 1080, 0), (-5, 10, 1)] {
        assert_eq!(max_px_for(width, height, scale), FALLBACK_MAX_PX);
    }
    assert_eq!(max_px_for(i32::MAX, 1, i32::MAX), [u32::MAX, u32::MAX]);
}

/// `zoom_limit` is how far a graphic can be zoomed before reaching the
/// limit on either side; one exactly at the limit fits (limit 1.0); one past
/// it, or with a size that is not a number, is too large; one with no area
/// is an error of its own. The limit is never below 1.0.
#[test]
fn zoom_limit_checks_both_sides() {
    let max = [1000, 500];
    assert_eq!(zoom_limit(100.0, 100.0, max), Ok(5.0));
    assert_eq!(zoom_limit(10.0, 250.0, max), Ok(2.0));
    assert_eq!(zoom_limit(1000.0, 500.0, max), Ok(1.0));
    assert_too_large(zoom_limit(1001.0, 10.0, max));
    assert_too_large(zoom_limit(10.0, 501.0, max));
    for bad in [f32::NAN, f32::INFINITY, 1e30] {
        assert_too_large(zoom_limit(bad, 10.0, max));
    }
    let message = zoom_limit(1e30, 10.0, max).unwrap_err();
    assert_eq!(message, "too large (huge x 10 px, limit 1000 x 500)");
    assert_eq!(
        zoom_limit(1001.0, 10.0, max).unwrap_err(),
        "too large (1001 x 10 px, limit 1000 x 500)"
    );
    let empty = zoom_limit(0.0, 10.0, max).unwrap_err();
    assert!(empty.contains("no area"), "{empty}");
}

/// An SVG larger than the limit is refused before anything is allocated,
/// with a size error (a huge size used to wrap round to a "no area" error);
/// one that fits draws normally and knows how far it can be zoomed.
#[test]
fn oversized_svg_is_refused() {
    let svg = |width: &str| {
        format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"10\"/>")
    };
    assert_too_large(rasterize(&svg("20000"), 15.0, 1.0));
    assert_too_large(rasterize(&svg("1e30"), 15.0, 1.0));
    // At device scale 2, 10,000 logical pixels are 20,000 device pixels.
    assert_too_large(rasterize(&svg("10000"), 15.0, 2.0));
    let fits = rasterize(&svg("1920"), 15.0, 1.0).unwrap();
    assert_eq!(fits.width, 1920);
    assert!((fits.max_zoom - 10.0).abs() < 1e-4, "{}", fits.max_zoom);
}

/// A formula too large at 100% shows its error instead of allocating
/// gigabytes (a `\rule` can be made any size).
#[test]
fn oversized_formula_is_refused() {
    let settings = RasterSettings::default();
    assert_too_large(render_math(r"\rule{100000em}{1em}", false, &settings));
    assert_too_large(render_math(r"\rule{1em}{100000em}", true, &settings));
    assert!(render_math(r"\rule{10em}{1em}", false, &settings).is_ok());
}

/// A formula zoomed past its limit is drawn at the limit, not larger: its
/// pixels stay within the limit and its size on screen is that of the
/// limit's zoom, so it never grows past ten screens; below the limit
/// nothing changes.
#[test]
fn zoomed_formula_stops_at_the_limit() {
    let tex = r"\rule{40em}{1em}";
    let natural = render_math(tex, false, &RasterSettings::default()).unwrap();
    let max = [natural.width * 2, 10_000];
    let at = |zoom: f32| {
        render_math(
            tex,
            false,
            &RasterSettings {
                zoom,
                ..limited(max)
            },
        )
        .unwrap()
    };
    let capped = at(3.0);
    assert!(capped.width <= max[0], "{} > {}", capped.width, max[0]);
    assert!((capped.max_zoom - 2.0).abs() < 0.02, "{}", capped.max_zoom);
    // Drawn as large as the limit allows (rounding aside), and no larger.
    let width = capped.logical_width();
    assert!(
        width <= max[0] as f32 && width >= max[0] as f32 - 3.0,
        "{width}"
    );
    let below = at(1.5);
    assert!((below.logical_width() - natural.logical_width() * 1.5).abs() <= 2.0);
}

/// A diagram zoomed past its limit is likewise drawn at its limit.
#[test]
fn zoomed_diagram_stops_at_the_limit() {
    let diagram = Graphic::Diagram {
        source: "graph LR; A-->B-->C".to_string(),
    };
    let cache = Cache::default();
    let natural = cache.render(&diagram, &RasterSettings::default()).unwrap();
    let max = [natural.width * 3 / 2, 10_000];
    let zoomed = RasterSettings {
        zoom: 3.0,
        ..limited(max)
    };
    let capped = cache.render(&diagram, &zoomed).unwrap();
    assert!(capped.width <= max[0], "{} > {}", capped.width, max[0]);
    assert!((capped.max_zoom - 1.5).abs() < 0.02, "{}", capped.max_zoom);
    assert!((capped.logical_width() - natural.logical_width() * 1.5).abs() <= 2.0);
    // Too large even at 100%: refused.
    assert_too_large(cache.render(&diagram, &limited([natural.width / 2, 10_000])));
}

/// An SVG file or raster image is never shown larger than its limit
/// either: zooming past it stops growing its logical size.
#[test]
fn zoomed_image_stops_at_the_limit() {
    let dir = temp_dir();
    let png = write_image(dir.path(), "wide.png", 100, 10, [0, 0, 255, 255]);
    let svg = dir.path().join("wide.svg");
    std::fs::write(
        &svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"10\"/>",
    )
    .unwrap();
    for path in [png, svg] {
        let settings = RasterSettings {
            zoom: 3.0,
            ..limited([200, 10_000])
        };
        let image = load_image(&path, &settings).unwrap();
        assert_eq!(image.width, 100, "{path:?}");
        assert!((image.max_zoom - 2.0).abs() < 1e-4, "{path:?}");
        assert!((image.logical_width() - 200.0).abs() < 1e-3, "{path:?}");
    }
}

/// A PNG file whose size is over the limit is refused, including when the
/// device scale makes it so.
#[test]
fn oversized_png_is_refused() {
    let dir = temp_dir();
    let wide = write_image(dir.path(), "wide.png", 20_000, 1, [0, 0, 0, 255]);
    assert_too_large(decode_image(&wide, &RasterSettings::default()));
    let half = write_image(dir.path(), "half.png", 10_000, 1, [0, 0, 0, 255]);
    assert!(decode_image(&half, &RasterSettings::default()).is_ok());
    let hidpi = RasterSettings {
        scale: 2.0,
        ..RasterSettings::default()
    };
    assert_too_large(decode_image(&half, &hidpi));
}

/// CRC-32 (as PNG chunks use) of `bytes`.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A decompression bomb is refused from its header alone: a tiny PNG whose
/// header claims 100,000 x 100,000 pixels (and has no pixel data at all)
/// gets the size error, so its pixels are never decoded or allocated.
#[test]
fn png_bomb_is_refused_from_its_header() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "bomb.png", 1, 1, [0, 0, 0, 255]);
    let mut png = std::fs::read(&path).unwrap();
    // The IHDR chunk follows the 8-byte signature: length, type, width,
    // height, five more bytes, CRC. Claim a huge size; the pixel data stays
    // that of one pixel.
    png[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    png[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    std::fs::write(&path, &png).unwrap();
    assert_too_large(decode_image(&path, &RasterSettings::default()));
}

/// The limit applies to the picture as shown: a sideways photo (Exif
/// "rotate 90") is checked upright.
#[test]
fn image_limit_uses_the_oriented_size() {
    let dir = temp_dir();
    let path = write_jpeg_with_orientation(dir.path(), "tall.jpg", 80, 40);
    // Upright it is 40 wide and 80 tall.
    assert!(decode_image(&path, &limited([50, 100])).is_ok());
    assert_too_large(decode_image(&path, &limited([100, 50])));
}

/// The cache keeps results per limit, so a picture refused under one limit
/// is not refused under a larger one.
#[test]
fn image_cache_is_per_limit() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "dot.png", 300, 10, [0, 255, 0, 255]);
    let graphic = Graphic::Image {
        path,
        inline: false,
    };
    let cache = Cache::default();
    assert_too_large(cache.render(&graphic, &limited([200, 200])));
    assert!(cache.render(&graphic, &limited([400, 400])).is_ok());
}

/// A large image used inline keeps only the pixels it is drawn with (its
/// capped size at the device scale), not the whole file, and its aspect
/// ratio; its zoom limit grows as it shrank.
#[test]
fn inline_image_keeps_only_drawn_pixels() {
    let dir = temp_dir();
    let path = write_image(dir.path(), "photo.png", 2000, 1000, [200, 100, 0, 255]);
    let cache = Cache::default();
    for scale in [1.0, 2.0] {
        let settings = RasterSettings {
            scale,
            ..RasterSettings::default()
        };
        let standalone = cache
            .render(
                &Graphic::Image {
                    path: path.clone(),
                    inline: false,
                },
                &settings,
            )
            .unwrap();
        let inline = cache
            .render(
                &Graphic::Image {
                    path: path.clone(),
                    inline: true,
                },
                &settings,
            )
            .unwrap();
        // 1.5 lines of 15 px: 22.5 px tall, 45 px wide.
        assert!(
            (inline.logical_height() - 22.5).abs() < 0.5,
            "{}",
            inline.logical_height()
        );
        assert!(
            (inline.logical_width() - 45.0).abs() < 0.5,
            "{}",
            inline.logical_width()
        );
        // At least 32 texels on the shorter side, else the drawn size.
        let expected_height = (22.5 * scale).max(32.0);
        assert!(
            (inline.height as f32 - expected_height).abs() <= 1.0,
            "scale {scale}: {}x{}",
            inline.width,
            inline.height
        );
        assert_eq!(
            inline.pixels.len(),
            (inline.width * inline.height * 4) as usize
        );
        assert!(inline.max_zoom > standalone.max_zoom * 40.0);
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

/// `render_document` renders local images, including one in a table cell.
#[test]
fn render_document_fills_in_cell_images() {
    let cat = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cat.svg");
    let source = format!("| image |\n|---|\n| ![cat]({}) |", cat.display());
    let mut blocks = render(&source);
    md_viewer::resolve_images(&mut blocks, std::path::Path::new("/nonexistent/doc.md"));
    assert!(images(&blocks).iter().all(|image| image.image.is_none()));
    render_document(&mut blocks, &RasterSettings::default());
    let found = images(&blocks);
    assert_eq!(found.len(), 1);
    assert!(
        matches!(&found[0].image, Some(Ok(image)) if image.width > 0),
        "the table-cell image did not render"
    );
}

/// `render_document` renders Mermaid diagrams, including failing ones.
#[test]
fn render_document_fills_in_diagrams() {
    let mut blocks = render("```mermaid\ngraph LR; A-->B\n```\n\n```mermaid\ngraph TD; A-->\n```");
    render_document(&mut blocks, &RasterSettings::default());
    let results: Vec<bool> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Mermaid(diagram) => diagram.image.as_ref().map(Result::is_ok),
            _ => None,
        })
        .collect();
    assert_eq!(results, vec![true, false]);
}

/// A document without graphics is left unchanged.
#[test]
fn render_document_without_graphics() {
    let original = render("# T\n\ntext\n\n| a |\n|---|");
    let mut blocks = original.clone();
    render_document(&mut blocks, &RasterSettings::default());
    assert_eq!(blocks, original);
}

/// The showcase document (`tests/fixtures/showcase.md`, used for manual
/// visual checks) renders end to end: every formula and diagram renders
/// except the one diagram that is deliberately broken.
#[test]
fn showcase_renders() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/showcase.md");
    let mut blocks = md_viewer::open_document(&path).unwrap();
    render_document(&mut blocks, &RasterSettings::default());

    let math_ok = formulas(&blocks)
        .iter()
        .filter(|math| matches!(math.image, Some(Ok(_))))
        .count();
    assert_eq!(math_ok, formulas(&blocks).len());
    assert!(math_ok >= 9, "only {math_ok} formulas");

    let diagrams: Vec<bool> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Mermaid(diagram) => diagram.image.as_ref().map(Result::is_ok),
            _ => None,
        })
        .collect();
    assert_eq!(diagrams, vec![true, true, true, true, false]);

    let tables = blocks
        .iter()
        .filter(|block| matches!(block, Block::Table(_)))
        .count();
    assert_eq!(tables, 3);

    let rendered = images(&blocks);
    assert_eq!(rendered.len(), 3);
    assert!(
        rendered
            .iter()
            .all(|image| matches!(image.image, Some(Ok(_)))),
        "an image did not render"
    );
}

/// The human-facing showcase at the repository root (`showcase.md`, the demo
/// for people trying the viewer) renders completely: every formula and every
/// diagram renders, with no fallbacks, and it has at least the tables,
/// formulas and diagrams it is meant to show off. A failure names the
/// offending source so the document can be fixed.
#[test]
fn root_showcase_renders() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("showcase.md");
    let mut blocks = md_viewer::open_document(&path).unwrap();
    render_document(&mut blocks, &RasterSettings::default());

    let math = formulas(&blocks);
    let failed: Vec<String> = math
        .iter()
        .filter_map(|math| match &math.image {
            Some(Ok(_)) => None,
            Some(Err(e)) => Some(format!("{}: {e}", math.tex)),
            None => Some(format!("{}: not rendered", math.tex)),
        })
        .collect();
    assert!(failed.is_empty(), "formulas failed: {failed:#?}");
    assert!(math.len() >= 25, "only {} formulas", math.len());
    assert!(math.iter().filter(|math| math.display).count() >= 10);

    let diagrams: Vec<&md_viewer::Diagram> = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Mermaid(diagram) => Some(diagram),
            _ => None,
        })
        .collect();
    let failed: Vec<String> = diagrams
        .iter()
        .filter_map(|diagram| match &diagram.image {
            Some(Ok(_)) => None,
            Some(Err(e)) => Some(format!("{}: {e}", diagram.source)),
            None => Some(format!("{}: not rendered", diagram.source)),
        })
        .collect();
    assert!(failed.is_empty(), "diagrams failed: {failed:#?}");
    assert_eq!(diagrams.len(), 6);

    let tables = blocks
        .iter()
        .filter(|block| matches!(block, Block::Table(_)))
        .count();
    assert_eq!(tables, 4);
}

/// The fallbacks fixture (`tests/fixtures/fallbacks.md`, opened by the GUI
/// test in `tests/cli.rs`) really contains what it is meant to exercise: an
/// inline and a display formula that fail, a working and a failing formula
/// in table cells, and two diagrams that fail.
#[test]
fn fallbacks_fixture_fails_where_intended() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fallbacks.md");
    let mut blocks = md_viewer::open_document(&path).unwrap();
    render_document(&mut blocks, &RasterSettings::default());
    let results: Vec<(bool, bool)> = formulas(&blocks)
        .iter()
        .map(|math| (math.display, matches!(math.image, Some(Ok(_)))))
        .collect();
    assert_eq!(
        results,
        vec![
            (false, false),
            (true, false),
            (false, true),
            (false, true),
            (false, true),
            (false, false),
        ]
    );
    let diagrams = blocks
        .iter()
        .filter(|block| matches!(block, Block::Mermaid(d) if matches!(d.image, Some(Err(_)))))
        .count();
    assert_eq!(diagrams, 2);
}

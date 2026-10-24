//! Integration tests for rendering graphics (`md_viewer::graphics`): math and
//! mhchem typesetting, rasterization, and filling a rendered document in.

use md_viewer::graphics::{
    hex_color, math_depth_em, math_svg, mermaid_svg, rasterize, render_document, render_math,
    render_mermaid, RasterSettings, MATH_SCALE,
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
    assert_eq!(tables, 2);
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
    assert_eq!(tables, 3);
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

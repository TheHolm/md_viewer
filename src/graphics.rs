//! Turning formulas and diagrams into pixels, independent of GTK.
//!
//! Math is typeset to SVG by `mathra` (which also understands mhchem's
//! `\ce{}`/`\pu{}`), Mermaid diagrams are laid out to SVG by `merman`
//! (using its resvg-compatible output), then every SVG is rasterized by `resvg` into
//! premultiplied RGBA pixels the GUI wraps in a texture. Nothing here needs a
//! display, so it all runs on a worker thread and is covered by tests.

use crate::Block;
use image::ImageDecoder as _;
use resvg::{tiny_skia, usvg};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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
    /// Shared, so the cache, the document and the GUI's texture hold one
    /// buffer between them rather than a copy each (an `Arc<Vec>` rather
    /// than `Arc<[u8]>`, which would copy the buffer once more on creation).
    pub pixels: Arc<Vec<u8>>,
    /// The text drawn in the SVG, in reading order. Diagrams use this for
    /// the find bar (their labels are otherwise only pixels); formulas draw
    /// outlines, so their list is empty.
    pub labels: Vec<String>,
    /// The largest zoom this graphic can be shown at without exceeding
    /// [`RasterSettings::max_px`] on screen (see [`zoom_limit`]); always at
    /// least 1.0, since a graphic too large at 100% is refused instead. A
    /// graphic asked for at a larger zoom is drawn at this one.
    pub max_zoom: f32,
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
    /// The device-pixel scale: 2.0 on a HiDPI display, fractional (1.25,
    /// 1.5, ...) with fractional scaling.
    pub scale: f32,
    /// The text colour math is drawn in, as RGB.
    pub color: [u8; 3],
    /// The document's zoom factor (1.0 = 100%). Math is typeset at
    /// `font_px * zoom`, and diagrams are drawn `zoom` times their natural
    /// size; in both cases the image's logical size grows with the zoom.
    pub zoom: f32,
    /// The largest a graphic may be on screen, in device pixels (width,
    /// height): [`SCREEN_MULTIPLE`] times the screen (see [`max_px_for`]).
    /// A graphic larger than this at 100% zoom is refused with an error;
    /// one that fits is never drawn larger, whatever the zoom.
    pub max_px: [u32; 2],
}

/// The most bytes of pixels one document's graphics may hold at once (see
/// `progressive::pick_drops`); a single graphic needing more at 100% zoom is
/// refused (see [`zoom_limit`]).
pub const DOCUMENT_BUDGET: usize = 1024 * 1024 * 1024;

/// How many screens wide and high a graphic may be at most.
pub const SCREEN_MULTIPLE: u32 = 10;

/// [`RasterSettings::max_px`] when the screen is not known (yet): ten times
/// a 1920x1080 screen.
pub const FALLBACK_MAX_PX: [u32; 2] = [1920 * SCREEN_MULTIPLE, 1080 * SCREEN_MULTIPLE];

/// The largest a graphic may be on a screen of `width` x `height` logical
/// pixels at device `scale` (fractional with fractional scaling, e.g.
/// 1.25): [`SCREEN_MULTIPLE`] times its size in device pixels (rounded to
/// whole pixels). An empty screen size or an invalid scale (not finite or
/// not positive) gives [`FALLBACK_MAX_PX`].
pub fn max_px_for(width: i32, height: i32, scale: f64) -> [u32; 2] {
    if width <= 0 || height <= 0 || !scale.is_finite() || scale <= 0.0 {
        return FALLBACK_MAX_PX;
    }
    // Float to integer casts saturate, so huge values stay at u32::MAX.
    let side =
        |pixels: i32| ((f64::from(pixels) * scale).round() as u32).saturating_mul(SCREEN_MULTIPLE);
    [side(width), side(height)]
}

/// The largest zoom at which a graphic `width` x `height` device pixels at
/// 100% zoom stays within `max_px`, never less than 1.0. Fails, with the
/// message shown in the graphic's place, when it does not fit even at 100%
/// (or its size is not a finite number), when it has no area, or when its
/// pixels alone would take more than the [`DOCUMENT_BUDGET`].
pub fn zoom_limit(width: f32, height: f32, max_px: [u32; 2]) -> Result<f32, String> {
    if !(width.is_finite() && height.is_finite())
        || width > max_px[0] as f32
        || height > max_px[1] as f32
    {
        return Err(format!(
            "too large ({} x {} px, limit {} x {})",
            pixels_for_message(width),
            pixels_for_message(height),
            max_px[0],
            max_px[1]
        ));
    }
    if width < 0.5 || height < 0.5 {
        let round = |side: f32| side.max(0.0).round() as u32;
        return Err(format!(
            "image has no area ({}x{})",
            round(width),
            round(height)
        ));
    }
    let bytes = f64::from(width.ceil()) * f64::from(height.ceil()) * 4.0;
    if bytes > DOCUMENT_BUDGET as f64 {
        const MIB: f64 = 1024.0 * 1024.0;
        return Err(format!(
            "can't render: object memory limit reached ({} MiB, limit {} MiB)",
            (bytes / MIB).ceil(),
            DOCUMENT_BUDGET / (1024 * 1024)
        ));
    }
    let limit = (max_px[0] as f32 / width).min(max_px[1] as f32 / height);
    Ok(limit.max(1.0))
}

/// A pixel size as shown in an error message: rounded, or "huge" for a size
/// too large to print meaningfully.
fn pixels_for_message(side: f32) -> String {
    if side.is_finite() && side < 1e12 {
        format!("{}", side.round() as u64)
    } else {
        "huge".to_string()
    }
}

impl Default for RasterSettings {
    /// 15 px black text at scale 1 and 100% zoom, as used by the tests.
    fn default() -> Self {
        RasterSettings {
            font_px: 15.0,
            scale: 1.0,
            color: [0, 0, 0],
            zoom: 1.0,
            max_px: FALLBACK_MAX_PX,
        }
    }
}

impl RasterSettings {
    /// An exact, hashable form of the settings, for cache keys.
    fn key(&self) -> SettingsKey {
        (
            self.font_px.to_bits(),
            self.scale.to_bits(),
            self.zoom.to_bits(),
            self.color,
            self.max_px,
        )
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
/// sans-serif families that is installed (GNOME's own UI fonts first, so
/// diagrams match the text around them), otherwise any installed family.
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
        "Adwaita Sans",
        "Cantarell",
        "Noto Sans",
        "DejaVu Sans",
        "Liberation Sans",
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
#[cfg(test)]
fn svg_options(font_px: f32) -> usvg::Options<'static> {
    svg_options_for(font_px, None, FALLBACK_MAX_PX)
}

/// The usvg options for an SVG whose `<image>` references to files resolve
/// against `resources` (an SVG file's folder), with embedded images no
/// larger than `max_px`.
///
/// usvg's own resolver reads any path relative to the working directory, with
/// no limit; this one reads files only for an SVG file (`resources` is
/// `None` for formulas and diagrams, which never refer to files), through
/// [`crate::read_limited`], and checks a raster image's size from its header
/// before resvg decodes it (for `data:` URLs too). Images that fail any of
/// this are left out, as usvg does with broken ones.
fn svg_options_for(
    font_px: f32,
    resources: Option<&Path>,
    max_px: [u32; 2],
) -> usvg::Options<'static> {
    let database = font_database().clone();
    let font_family = preferred_sans_family(&database).unwrap_or_else(|| "sans-serif".into());
    let resources = resources.map(Path::to_path_buf);
    let default_data = usvg::ImageHrefResolver::default_data_resolver();
    let resolver = usvg::ImageHrefResolver {
        resolve_data: Box::new(move |mime, data, options| {
            if embedded_raster_fits(&data, max_px) {
                default_data(mime, data, options)
            } else {
                None
            }
        }),
        resolve_string: Box::new(move |href, options| {
            let dir = resources.as_ref()?;
            let bytes = read_svg_reference(dir, href)?;
            embedded_image(bytes, max_px, options)
        }),
    };
    usvg::Options {
        fontdb: database,
        font_family,
        font_size: font_px,
        resources_dir: None,
        image_href_resolver: resolver,
        ..usvg::Options::default()
    }
}

/// Reads the file an SVG's `<image href>` names, relative to the SVG's
/// folder `dir` (or absolute, or a `file:` URL); `None` for other URLs and
/// for files [`crate::read_limited`] refuses.
fn read_svg_reference(dir: &Path, href: &str) -> Option<Vec<u8>> {
    let path = if href.starts_with("file:") {
        url::Url::parse(href).ok()?.to_file_path().ok()?
    } else if href.contains("://") {
        return None;
    } else {
        let decoded = percent_encoding::percent_decode_str(href)
            .decode_utf8()
            .ok()?;
        dir.join(decoded.as_ref())
    };
    crate::read_limited(&path, crate::MAX_FILE_BYTES).ok()
}

/// Turns the bytes of a file an SVG refers to into an image usvg can draw:
/// a PNG, JPEG, GIF or WebP no larger than `max_px` (checked from its
/// header), or a nested SVG (which, per the SVG specification, may not refer
/// to further files).
fn embedded_image(
    bytes: Vec<u8>,
    max_px: [u32; 2],
    options: &usvg::Options,
) -> Option<usvg::ImageKind> {
    let format = image::guess_format(&bytes).ok();
    if format.is_some() && !embedded_raster_fits(&bytes, max_px) {
        return None;
    }
    let data = Arc::new(bytes);
    match format {
        Some(image::ImageFormat::Png) => Some(usvg::ImageKind::PNG(data)),
        Some(image::ImageFormat::Jpeg) => Some(usvg::ImageKind::JPEG(data)),
        Some(image::ImageFormat::Gif) => Some(usvg::ImageKind::GIF(data)),
        Some(image::ImageFormat::WebP) => Some(usvg::ImageKind::WEBP(data)),
        Some(_) => None,
        None => usvg::Tree::from_data_nested(&data, options)
            .ok()
            .map(usvg::ImageKind::SVG),
    }
}

/// False if `bytes` are a raster image whose header gives a size over
/// `max_px` (or no readable size at all); true for anything else (an SVG,
/// which has no pixels of its own to decode).
fn embedded_raster_fits(bytes: &[u8], max_px: [u32; 2]) -> bool {
    if image::guess_format(bytes).is_err() {
        return true;
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format();
    match reader.ok().and_then(|reader| reader.into_dimensions().ok()) {
        Some((width, height)) => {
            width <= max_px[0].max(max_px[1]) && height <= max_px[0].max(max_px[1])
        }
        None => false,
    }
}

/// Rasterizes `svg` at `scale` device pixels per SVG pixel, with `em` units
/// resolving to `font_px`, within [`FALLBACK_MAX_PX`]. Fails on invalid SVG,
/// an empty image or one larger than the limit.
pub fn rasterize(svg: &str, font_px: f32, scale: f32) -> Rendered {
    rasterize_within(svg, font_px, scale, 1.0, FALLBACK_MAX_PX, None)
}

/// Rasterizes `svg` at `scale` device pixels per SVG pixel (`em` units
/// resolving to `font_px`), where `zoom` is the part of that size due to the
/// document zoom, so the size at 100% is the drawn size divided by `zoom`.
///
/// The size is checked against `max_px` before anything is allocated: a
/// graphic larger than `max_px` at 100% is refused (see [`zoom_limit`]), and
/// one that would only exceed it at `zoom` is drawn at its
/// [`Image::max_zoom`] instead - smaller than asked, never larger than the
/// limit. The returned image's `scale` is the device-pixel scale actually
/// drawn at. `<image>` references to files resolve against `resources`
/// (see [`svg_options_for`]); without it, they are left out.
fn rasterize_within(
    svg: &str,
    font_px: f32,
    scale: f32,
    zoom: f32,
    max_px: [u32; 2],
    resources: Option<&Path>,
) -> Rendered {
    let options = svg_options_for(font_px, resources, max_px);
    let tree = usvg::Tree::from_str(svg, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let zoom = if zoom.is_finite() && zoom > 0.0 {
        zoom
    } else {
        1.0
    };
    let max_zoom = zoom_limit(
        size.width() * scale / zoom,
        size.height() * scale / zoom,
        max_px,
    )?;
    let scale = scale * (max_zoom / zoom).min(1.0);
    let width = ((size.width() * scale).ceil() as u32).clamp(1, max_px[0]);
    let height = ((size.height() * scale).ceil() as u32).clamp(1, max_px[1]);
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("cannot allocate a {width}x{height} image"))?;
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
        pixels: pixmap.take().into(),
        labels: text_labels(&tree),
        max_zoom,
    })
}

/// How much smaller than asked `image` was drawn because of its zoom limit:
/// 1.0 when drawn at `zoom`, less when `zoom` exceeds [`Image::max_zoom`].
fn cap_factor(image: &Image, zoom: f32) -> f32 {
    if zoom > image.max_zoom {
        image.max_zoom / zoom
    } else {
        1.0
    }
}

/// Every text node drawn in `tree`, as its decoded text, in document order
/// (a multi-line label is one node per line). `use` elements are expanded by
/// usvg, so labels appear at their drawn position rather than in the
/// `<defs>` they are stored in.
fn text_labels(tree: &usvg::Tree) -> Vec<String> {
    /// Appends the text of every text node in `group` and its subgroups to
    /// `labels`, in document order.
    fn walk(group: &usvg::Group, labels: &mut Vec<String>) {
        for node in group.children() {
            match node {
                usvg::Node::Group(group) => walk(group, labels),
                usvg::Node::Text(text) => {
                    let text: String = text
                        .chunks()
                        .iter()
                        .map(usvg::TextChunk::text)
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        labels.push(text);
                    }
                }
                _ => {}
            }
        }
    }
    let mut labels = Vec::new();
    walk(tree.root(), &mut labels);
    labels
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

/// Renders a formula [`MATH_SCALE`] times the (zoomed) body text size, in
/// the text colour. Display math uses TeX's display style (larger operators, limits
/// above and below).
pub fn render_math(tex: &str, display: bool, settings: &RasterSettings) -> Rendered {
    if tex.trim().is_empty() {
        return Err("empty formula".to_string());
    }
    let svg = math_svg(tex, display)?;
    let font_px = settings.font_px * settings.zoom * MATH_SCALE;
    let svg = svg.replace("currentColor", &hex_color(settings.color));
    let mut image = rasterize_within(
        &svg,
        font_px,
        settings.scale,
        settings.zoom,
        settings.max_px,
        None,
    )?;
    // Drawn smaller than asked when capped; the logical size and depth stay
    // those of the formula as drawn.
    let factor = cap_factor(&image, settings.zoom);
    image.scale = settings.scale;
    image.depth = math_depth_em(&svg) * font_px * factor;
    // Formulas are drawn as outlines; any SVG text mathra emits is not the
    // TeX source, so it is never offered to the find bar.
    image.labels.clear();
    Ok(image)
}

/// The font size Mermaid diagrams are rasterized with; diagrams set their
/// own text sizes, so this only resolves stray `em` units.
const DIAGRAM_FONT_PX: f32 = 16.0;

/// The font files of `family` in `database` (every face: regular, bold,
/// italic, …), in database order and without duplicates. Faces loaded from
/// memory rather than from a file are skipped.
fn font_files(database: &usvg::fontdb::Database, family: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for face in database.faces() {
        if !face.families.iter().any(|(name, _)| name == family) {
            continue;
        }
        let path = match &face.source {
            usvg::fontdb::Source::File(path) => path,
            usvg::fontdb::Source::SharedFile(path, _) => path,
            usvg::fontdb::Source::Binary(_) => continue,
        };
        if !files.contains(path) {
            files.push(path.clone());
        }
    }
    files
}

/// merman's text measurement using real fonts: the files of the family
/// diagram text is drawn with (see [`pick_family`]), loaded once. `None` if
/// no such files exist or they cannot be loaded; diagrams then fall back to
/// merman's built-in estimates, which can be narrower than the real text.
fn native_fonts() -> Option<&'static merman::svg::export::NativeFontContext> {
    static CONTEXT: OnceLock<Option<merman::svg::export::NativeFontContext>> = OnceLock::new();
    CONTEXT
        .get_or_init(|| {
            let database = font_database();
            let family = preferred_sans_family(database)?;
            load_native_fonts(&font_files(database, &family))
        })
        .as_ref()
}

/// Loads `files` into a merman font context for measuring text. `None` if
/// there are no files or merman cannot use them.
fn load_native_fonts(files: &[PathBuf]) -> Option<merman::svg::export::NativeFontContext> {
    if files.is_empty() {
        return None;
    }
    let options = files.iter().fold(
        merman::svg::export::NativeFontOptions::default(),
        |options, file| options.with_font_file(file),
    );
    options.load(&merman::OperationControl::new()).ok()
}

/// The SVG environment diagrams are laid out in: text measured with
/// `fonts` when given, merman's deterministic estimates otherwise.
fn svg_environment(
    fonts: Option<&merman::svg::export::NativeFontContext>,
) -> merman::svg::SvgEnvironment {
    use merman::svg::SvgEnvironment;
    fonts
        .and_then(|fonts| {
            SvgEnvironment::deterministic()
                .with_native_font_context(fonts.clone())
                .ok()
        })
        .unwrap_or_else(SvgEnvironment::deterministic)
}

/// Lays out a Mermaid diagram (flowchart, sequence, Gantt, class, state,
/// pie, …) as SVG that resvg can draw: labels are plain SVG text rather than
/// HTML, and label boxes are sized by measuring the text with the font it
/// will be drawn in. Fails with the parser's message on invalid source.
pub fn mermaid_svg(source: &str) -> Result<String, String> {
    mermaid_svg_with(source, native_fonts())
}

/// [`mermaid_svg`] with text measured by `fonts` (`None`: merman's
/// built-in estimates).
fn mermaid_svg_with(
    source: &str,
    fonts: Option<&merman::svg::export::NativeFontContext>,
) -> Result<String, String> {
    use merman::svg::SvgPipeline;
    use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest};
    let request = SvgRequest {
        environment: svg_environment(fonts),
        pipeline: Some(SvgPipeline::resvg_safe()),
        ..SvgRequest::default()
    };
    match Renderer::new().render(RenderRequest::svg(source, OperationControl::new(), request)) {
        Ok(RenderOutput::Svg(Some(svg))) => Ok(svg.svg().to_string()),
        Ok(_) => Err("no Mermaid diagram found".to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Rasterizes a laid-out diagram `svg` `settings.zoom` times its natural
/// size (on a transparent background; the GUI puts it on a light card, as
/// Mermaid's colours assume).
fn rasterize_diagram(svg: &str, settings: &RasterSettings) -> Rendered {
    let mut image = rasterize_within(
        svg,
        DIAGRAM_FONT_PX,
        settings.scale * settings.zoom,
        settings.zoom,
        settings.max_px,
        None,
    )?;
    image.scale = settings.scale;
    Ok(image)
}

/// Renders a Mermaid diagram at its natural size times the zoom.
pub fn render_mermaid(source: &str, settings: &RasterSettings) -> Rendered {
    if source.trim().is_empty() {
        return Err("empty diagram".to_string());
    }
    rasterize_diagram(&mermaid_svg(source)?, settings)
}

/// One graphic to render: a formula, a diagram or an image.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Graphic {
    /// LaTeX math; `display` for `$$…$$`.
    Math {
        /// The LaTeX source, without the dollar signs.
        tex: String,
        /// True for display math.
        display: bool,
    },
    /// A Mermaid diagram.
    Diagram {
        /// The Mermaid source.
        source: String,
    },
    /// A local image file.
    Image {
        /// The file to load.
        path: PathBuf,
        /// True when the image sits in the text (scaled to the line height)
        /// rather than alone in its paragraph (scaled to the window width).
        inline: bool,
    },
}

/// The tallest an inline image is drawn, as a multiple of the body line
/// height.
const INLINE_IMAGE_MAX_LINES: f32 = 1.5;

/// The widest an inline image is drawn, as a multiple of the body font size.
const INLINE_IMAGE_MAX_EM: f32 = 12.0;

/// The fewest pixels a shrunk inline image keeps on its shorter side.
const INLINE_IMAGE_MIN_TEXELS: f32 = 32.0;

/// Decodes the image file at `path` into premultiplied RGBA at 100% zoom,
/// honouring the file's Exif orientation. SVG files are rasterized at the
/// device scale with [`rasterize`]'s limits (which also draws raster images
/// embedded in the SVG); other files go through the `image` crate (PNG,
/// JPEG, GIF - first frame only - and WebP) and keep the file's own pixels:
/// a raster image is never enlarged for a HiDPI screen (that adds no
/// detail), its logical size is simply its size in pixels.
///
/// Either way the picture's size on screen at 100% is checked against
/// `settings.max_px` (see [`zoom_limit`]); a raster file over it is refused
/// from its header, before its pixels are decoded.
pub fn decode_image(path: &Path, settings: &RasterSettings) -> Rendered {
    let bytes = crate::read_limited(path, crate::MAX_FILE_BYTES)
        .map_err(|e| format!("cannot read file: {e}"))?;
    let device_scale = settings.scale.max(1.0);
    if is_svg_path(path) {
        let svg = String::from_utf8(bytes).map_err(|e| format!("invalid SVG text: {e}"))?;
        return rasterize_within(
            &svg,
            settings.font_px,
            device_scale,
            1.0,
            settings.max_px,
            path.parent(),
        );
    }
    let mut decoder = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_decoder()
        .map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let (file_width, file_height) = oriented(decoder.dimensions(), orientation);
    // One file pixel is one logical pixel, `device_scale` device pixels.
    let max_zoom = zoom_limit(
        file_width as f32 * device_scale,
        file_height as f32 * device_scale,
        settings.max_px,
    )?;
    // A backstop in case the header lied about the size.
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(file_width.max(file_height));
    limits.max_image_height = Some(file_width.max(file_height));
    decoder.set_limits(limits).map_err(|e| e.to_string())?;
    let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    decoded.apply_orientation(orientation);
    let rgba = decoded.into_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err("empty image".to_string());
    }
    Ok(Image {
        width,
        height,
        scale: 1.0,
        depth: 0.0,
        pixels: premultiply(rgba.into_raw()).into(),
        labels: Vec::new(),
        max_zoom,
    })
}

/// The size of an image of `size` (as stored in the file) once `orientation`
/// is applied: a quarter turn swaps width and height.
fn oriented(size: (u32, u32), orientation: image::metadata::Orientation) -> (u32, u32) {
    use image::metadata::Orientation::{Rotate270, Rotate270FlipH, Rotate90, Rotate90FlipH};
    match orientation {
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (size.1, size.0),
        _ => size,
    }
}

/// Loads the image file at `path` for display at the zoom in `settings`: its
/// logical size grows with the zoom, like math and diagrams, so a re-render
/// replaces the stretched picture with the right size.
pub fn load_image(path: &Path, settings: &RasterSettings) -> Rendered {
    decode_image(path, settings).map(|image| zoom_image(image, settings.zoom))
}

/// Multiplies every RGB channel by its alpha, for GDK's premultiplied format
/// (the `image` crate returns straight alpha). Rounding matches resvg's own
/// premultiplied output closely enough for the eye.
fn premultiply(rgba: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &pixel[..3] {
            out.push(((u32::from(*channel) * alpha + 127) / 255) as u8);
        }
        out.push(pixel[3]);
    }
    out
}

/// True if `path` names an SVG file by extension (case-insensitive).
fn is_svg_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
}

/// Scales an image's logical size by `zoom`, but never past its
/// [`Image::max_zoom`] (its pixels stay the same, so a raster image is
/// upscaled when zoomed in).
fn zoom_image(mut image: Image, zoom: f32) -> Image {
    let zoom = zoom.min(image.max_zoom);
    if zoom > 0.0 && (zoom - 1.0).abs() > f32::EPSILON {
        image.scale /= zoom;
    }
    image
}

/// Shrinks `image` (keeping its aspect ratio) so an inline image does not
/// dwarf the line it sits on: at most [`INLINE_IMAGE_MAX_LINES`] lines tall
/// and [`INLINE_IMAGE_MAX_EM`] body-font widths wide. Smaller images are left
/// alone. A shrunk image also keeps only the pixels it is drawn with (its
/// capped size at the device scale), so a large file used as an inline icon
/// does not hold its full resolution in the texture; its zoom limit grows
/// accordingly.
fn cap_inline(mut image: Image, settings: &RasterSettings) -> Image {
    let line = settings.font_px * settings.zoom;
    let max_height = line * INLINE_IMAGE_MAX_LINES;
    let max_width = line * INLINE_IMAGE_MAX_EM;
    let (width, height) = (image.logical_width(), image.logical_height());
    let mut factor = 1.0_f32;
    if height > max_height && height > 0.0 {
        factor = factor.min(max_height / height);
    }
    if width > max_width && width > 0.0 {
        factor = factor.min(max_width / width);
    }
    if factor >= 1.0 {
        return image;
    }
    let logical_width = width * factor;
    // Keep the pixels drawn at the device scale, but at least
    // INLINE_IMAGE_MIN_TEXELS on the shorter side, so rounding the texture
    // size to whole pixels barely changes the aspect ratio.
    let shorter = image.width.min(image.height) as f32;
    let resample = (logical_width * settings.scale.max(1.0) / image.width as f32)
        .max(INLINE_IMAGE_MIN_TEXELS / shorter);
    if resample < 1.0 {
        let texels_width = (image.width as f32 * resample).round().max(1.0) as u32;
        let texels_height = (image.height as f32 * resample).round().max(1.0) as u32;
        // Premultiplied pixels resample correctly as they are.
        let buffer = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
            image.width,
            image.height,
            &image.pixels,
        );
        if let Some(buffer) = buffer {
            let resized = image::imageops::resize(
                &buffer,
                texels_width,
                texels_height,
                image::imageops::FilterType::Triangle,
            );
            image.width = texels_width;
            image.height = texels_height;
            image.pixels = resized.into_raw().into();
        }
    }
    image.scale = image.width as f32 / logical_width;
    image.max_zoom /= factor;
    image
}

/// The exact, hashable form of [`RasterSettings`].
type SettingsKey = (u32, u32, u32, [u8; 3], [u32; 2]);

/// What a cached formula is looked up by: source, display flag, settings.
type MathKey = (String, bool, SettingsKey);

/// What a decoded image is looked up by: path, the file's stamp, device
/// scale, body font size, size limit.
type ImageKey = (PathBuf, Option<FileStamp>, u32, u32, [u32; 2]);

/// What identifies one version of a file for the image cache: its size,
/// modification time (seconds and nanoseconds since the epoch) and inode.
/// A file rewritten, replaced or created since it was cached has another
/// stamp, so it is decoded again.
pub type FileStamp = (u64, i64, i64, u64);

/// The stamp of the file at `path` as it is now, or `None` when it cannot
/// be read (missing, for instance; a missing file that appears later then
/// has a stamp and misses the cache).
pub fn file_stamp(path: &Path) -> Option<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ino(),
    ))
}

/// How many rendered formulas [`Cache`] keeps before it starts over.
const MATH_CACHE_LIMIT: usize = 4096;

/// How many laid-out diagrams [`Cache`] keeps before it starts over.
const DIAGRAM_CACHE_LIMIT: usize = 256;

/// How many decoded images [`Cache`] keeps before it starts over.
const IMAGE_CACHE_LIMIT: usize = 128;

/// Remembers rendered formulas (by source and settings) and laid-out
/// diagrams (by source), so a formula that appears many times is typeset
/// once, and zooming only re-rasterizes diagrams instead of laying them out
/// again (the slow part). Safe to share between worker threads; work is
/// done outside the lock, so two threads may occasionally both compute the
/// same entry. Each map is simply emptied when it reaches its entry or byte
/// limit ([`CACHE_BYTES`]); pixels are shared with the windows showing them.
#[derive(Debug, Default)]
pub struct Cache {
    /// Rendered formulas by (source, display, settings).
    math: Mutex<HashMap<MathKey, Rendered>>,
    /// Laid-out diagram SVG (or the layout error) by source.
    diagrams: Mutex<HashMap<String, Result<Arc<String>, String>>>,
    /// Decoded images by (path, file stamp, device scale, body font size,
    /// size limit), at 100% zoom.
    images: Mutex<HashMap<ImageKey, Rendered>>,
    /// Inline images shrunk to the text (see `cap_inline`), by image key
    /// and zoom, so an image used inline many times is resampled once and
    /// every use shares its pixels.
    inline_images: Mutex<HashMap<(ImageKey, u32), Image>>,
    /// How many times a formula was typeset or a diagram laid out.
    misses: AtomicUsize,
}

impl Cache {
    /// The cache shared by every window of the application.
    pub fn shared() -> &'static Cache {
        static CACHE: OnceLock<Cache> = OnceLock::new();
        CACHE.get_or_init(Cache::default)
    }

    /// How many formulas were typeset and diagrams laid out so far, i.e.
    /// lookups that were not answered from the cache.
    pub fn misses(&self) -> usize {
        self.misses.load(Ordering::Relaxed)
    }

    /// Renders `graphic` with `settings`, reusing earlier work.
    pub fn render(&self, graphic: &Graphic, settings: &RasterSettings) -> Rendered {
        match graphic {
            Graphic::Math { tex, display } => {
                let key = (tex.clone(), *display, settings.key());
                if let Some(found) = lock(&self.math).get(&key) {
                    return found.clone();
                }
                self.misses.fetch_add(1, Ordering::Relaxed);
                let rendered = render_math(tex, *display, settings);
                insert_bounded(&self.math, key, rendered, MATH_CACHE_LIMIT, rendered_bytes)
            }
            Graphic::Diagram { source } => {
                if source.trim().is_empty() {
                    return Err("empty diagram".to_string());
                }
                let cached = lock(&self.diagrams).get(source).cloned();
                let svg = match cached {
                    Some(svg) => svg,
                    None => {
                        self.misses.fetch_add(1, Ordering::Relaxed);
                        let svg = mermaid_svg(source).map(Arc::new);
                        insert_bounded(
                            &self.diagrams,
                            source.clone(),
                            svg,
                            DIAGRAM_CACHE_LIMIT,
                            svg_bytes,
                        )
                    }
                };
                rasterize_diagram(&svg?, settings)
            }
            Graphic::Image { path, inline } => {
                let stamp = file_stamp(path);
                let key = (
                    path.clone(),
                    stamp,
                    settings.scale.to_bits(),
                    settings.font_px.to_bits(),
                    settings.max_px,
                );
                let inline_key = (key.clone(), settings.zoom.to_bits());
                if *inline {
                    if let Some(found) = lock(&self.inline_images).get(&inline_key).cloned() {
                        return Ok(found);
                    }
                }
                let cached = lock(&self.images).get(&key).cloned();
                let decoded = match cached {
                    Some(found) => found,
                    None => {
                        self.misses.fetch_add(1, Ordering::Relaxed);
                        let decoded = decode_image(path, settings);
                        // Results for an older version of the file are
                        // stale: drop them rather than hold their pixels.
                        lock(&self.images).retain(|(cached, cached_stamp, ..), _| {
                            cached != path || *cached_stamp == stamp
                        });
                        lock(&self.inline_images).retain(|((cached, cached_stamp, ..), _), _| {
                            cached != path || *cached_stamp == stamp
                        });
                        insert_bounded(
                            &self.images,
                            key,
                            decoded,
                            IMAGE_CACHE_LIMIT,
                            rendered_bytes,
                        )
                    }
                };
                let image = zoom_image(decoded?, settings.zoom);
                if !*inline {
                    return Ok(image);
                }
                let original = image.pixels.clone();
                let capped = cap_inline(image, settings);
                if Arc::ptr_eq(&original, &capped.pixels) {
                    // Not shrunk: it already shares the decoded pixels.
                    return Ok(capped);
                }
                Ok(insert_bounded(
                    &self.inline_images,
                    inline_key,
                    capped,
                    IMAGE_CACHE_LIMIT,
                    |image| image.pixels.len(),
                ))
            }
        }
    }
}

/// Locks `mutex`, carrying on with the data if another thread panicked
/// while holding it (a cache is still usable then).
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The most bytes of pixels (or SVG text) each cache map holds; the three
/// together stay within about 256 MiB however many windows are open.
const CACHE_BYTES: usize = 96 * 1024 * 1024;

/// Inserts into a cache map, emptying it first if it holds `limit` entries
/// or the new value would take it past [`CACHE_BYTES`] (`weigh` gives a
/// value's size). A value larger than that on its own is not cached.
///
/// Returns the value to use: if another thread cached the same key while
/// this one was computing it, that value (so every user of an entry shares
/// one pixel buffer), otherwise `value`.
fn insert_bounded<K: std::hash::Hash + Eq, V: Clone>(
    map: &Mutex<HashMap<K, V>>,
    key: K,
    value: V,
    limit: usize,
    weigh: fn(&V) -> usize,
) -> V {
    let mut map = lock(map);
    if let Some(found) = map.get(&key) {
        return found.clone();
    }
    let size = weigh(&value);
    if size > CACHE_BYTES {
        return value;
    }
    let held: usize = map.values().map(weigh).sum();
    if map.len() >= limit || held + size > CACHE_BYTES {
        map.clear();
    }
    map.insert(key, value.clone());
    value
}

/// The bytes of pixels a rendered result holds.
fn rendered_bytes(rendered: &Rendered) -> usize {
    rendered.as_ref().map_or(0, |image| image.pixels.len())
}

/// The bytes of SVG text a laid-out diagram holds.
fn svg_bytes(svg: &Result<Arc<String>, String>) -> usize {
    svg.as_ref().map_or(0, |svg| svg.len())
}

/// Renders every graphic in `blocks` (math, in text and in table cells, and
/// Mermaid diagrams), storing each result next to its source, one after the
/// other. The GUI renders in parallel instead (see `crate::progressive`).
pub fn render_document(blocks: &mut [Block], settings: &RasterSettings) {
    let cache = Cache::default();
    crate::progressive::for_each_graphic(blocks, |_, graphic, image| {
        *image = Some(cache.render(&graphic, settings));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only raster data with a readable header within the limit fits;
    /// anything else that is not raster (an SVG) passes, and broken raster
    /// data does not.
    #[test]
    fn embedded_raster_fits_checks_the_header() {
        let mut png = Vec::new();
        image::RgbaImage::new(30, 10)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(embedded_raster_fits(&png, [30, 30]));
        assert!(!embedded_raster_fits(&png, [20, 20]));
        assert!(!embedded_raster_fits(&png[..12], [30, 30]));
        assert!(embedded_raster_fits(b"<svg/>", [1, 1]));
    }

    /// A file an SVG refers to becomes the usvg image kind of its format
    /// (PNG, JPEG, GIF, WebP, or a nested SVG); a raster format without a
    /// decoder (BMP), one over the size limit, and bytes that are neither
    /// raster nor SVG are left out.
    #[test]
    fn embedded_image_kinds() {
        let encode = |format: image::ImageFormat| {
            let mut bytes = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::new(4, 4))
                .write_to(&mut std::io::Cursor::new(&mut bytes), format)
                .unwrap();
            bytes
        };
        let options = svg_options(15.0);
        let kind = |bytes: Vec<u8>, max_px: [u32; 2]| embedded_image(bytes, max_px, &options);
        let roomy = [100, 100];
        assert!(matches!(
            kind(encode(image::ImageFormat::Png), roomy),
            Some(usvg::ImageKind::PNG(_))
        ));
        assert!(matches!(
            kind(encode(image::ImageFormat::Jpeg), roomy),
            Some(usvg::ImageKind::JPEG(_))
        ));
        assert!(matches!(
            kind(encode(image::ImageFormat::Gif), roomy),
            Some(usvg::ImageKind::GIF(_))
        ));
        assert!(matches!(
            kind(encode(image::ImageFormat::WebP), roomy),
            Some(usvg::ImageKind::WEBP(_))
        ));
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>"#;
        assert!(matches!(
            kind(svg.to_vec(), roomy),
            Some(usvg::ImageKind::SVG(_))
        ));
        // A BMP header: recognised, but there is no decoder to size it.
        let mut bmp = b"BM".to_vec();
        bmp.resize(64, 0);
        assert!(kind(bmp, roomy).is_none());
        assert!(kind(encode(image::ImageFormat::Png), [2, 2]).is_none());
        assert!(kind(b"not an image".to_vec(), roomy).is_none());
    }

    /// References from an SVG are read relative to its folder, as absolute
    /// paths or `file:` URLs, but never from the network or with invalid
    /// percent-encoding.
    #[test]
    fn read_svg_reference_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a b.txt"), "x").unwrap();
        let read = |href: &str| read_svg_reference(dir.path(), href);
        assert_eq!(read("a b.txt").as_deref(), Some(&b"x"[..]));
        assert_eq!(read("a%20b.txt").as_deref(), Some(&b"x"[..]));
        let absolute = dir.path().join("a b.txt");
        assert!(read(absolute.to_str().unwrap()).is_some());
        let url = url::Url::from_file_path(&absolute).unwrap();
        assert!(read(url.as_str()).is_some());
        assert_eq!(read("http://example.com/a.png"), None);
        assert_eq!(read("file://host/a.png"), None);
        assert_eq!(read("%FF.txt"), None);
        assert_eq!(read("missing"), None);
    }

    /// A quarter turn swaps width and height; other orientations (flips,
    /// half turn) keep them.
    #[test]
    fn oriented_swaps_on_quarter_turns() {
        use image::metadata::Orientation::*;
        for turn in [Rotate90, Rotate270, Rotate90FlipH, Rotate270FlipH] {
            assert_eq!(oriented((3, 7), turn), (7, 3), "{turn:?}");
        }
        for same in [NoTransforms, Rotate180, FlipHorizontal, FlipVertical] {
            assert_eq!(oriented((3, 7), same), (3, 7), "{same:?}");
        }
    }

    /// How much smaller a capped graphic was drawn: the ratio of its limit
    /// to the zoom asked for, or 1.0 when within the limit.
    #[test]
    fn cap_factor_is_limit_over_zoom() {
        let image = Image {
            width: 1,
            height: 1,
            scale: 1.0,
            depth: 0.0,
            pixels: vec![0; 4].into(),
            labels: Vec::new(),
            max_zoom: 1.5,
        };
        assert_eq!(cap_factor(&image, 3.0), 0.5);
        assert_eq!(cap_factor(&image, 1.5), 1.0);
        assert_eq!(cap_factor(&image, 1.0), 1.0);
    }

    /// Sizes in error messages are whole pixels, or "huge" when not
    /// printable meaningfully.
    #[test]
    fn pixels_for_message_rounds_or_says_huge() {
        assert_eq!(pixels_for_message(10.4), "10");
        assert_eq!(pixels_for_message(19_201.0), "19201");
        assert_eq!(pixels_for_message(1e30), "huge");
        assert_eq!(pixels_for_message(f32::NAN), "huge");
        assert_eq!(pixels_for_message(f32::INFINITY), "huge");
    }

    /// A shrunk inline image whose pixel buffer does not match its size (a
    /// hand-made `Image`) is left with its pixels rather than panicking; its
    /// logical size is still capped.
    #[test]
    fn cap_inline_tolerates_mismatched_pixels() {
        let image = Image {
            width: 1000,
            height: 1000,
            scale: 1.0,
            depth: 0.0,
            pixels: vec![0; 4].into(),
            labels: Vec::new(),
            max_zoom: 1.0,
        };
        let capped = cap_inline(image, &RasterSettings::default());
        assert_eq!((capped.width, capped.pixels.len()), (1000, 4));
        assert!((capped.logical_height() - 22.5).abs() < 0.01);
    }
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

    /// GNOME's UI font wins when it is installed, so diagrams match the
    /// text around them.
    #[test]
    fn pick_family_prefers_adwaita_sans() {
        assert_eq!(
            pick_family(&["DejaVu Sans", "Cantarell", "Adwaita Sans"]).as_deref(),
            Some("Adwaita Sans")
        );
    }

    /// `font_files` lists each file of a family once and skips faces that
    /// were not loaded from a file; an unknown family has none.
    #[test]
    fn font_files_of_family() {
        let database = font_database();
        assert!(font_files(database, "No Such Family 1234").is_empty());
        let Some(family) = preferred_sans_family(database) else {
            return; // no fonts installed at all
        };
        let files = font_files(database, &family);
        assert!(!files.is_empty(), "{family}");
        let mut unique = files.clone();
        unique.dedup();
        assert_eq!(unique.len(), files.len());
        let mut memory = usvg::fontdb::Database::new();
        memory.load_font_data(std::fs::read(&files[0]).unwrap());
        let name = memory.faces().next().unwrap().families[0].0.clone();
        assert!(font_files(&memory, &name).is_empty());
    }

    /// Without font files there is no native measurement; diagrams are then
    /// laid out with merman's own estimates and still render.
    #[test]
    fn fallback_without_native_fonts() {
        assert!(load_native_fonts(&[]).is_none());
        assert!(load_native_fonts(&[PathBuf::from("/nonexistent/font.ttf")]).is_none());
        let svg = mermaid_svg_with("graph TD; A-->B", None).unwrap();
        assert!(svg.contains("<svg"));
    }

    /// Every diagram label's box (a nested `<svg>` that clips its content)
    /// is at least as wide as the label text as resvg actually draws it, so
    /// no label is cut off. Labels are laid out with the system fonts, so
    /// this checks the real-font measurement end to end.
    #[test]
    fn labels_fit_their_boxes() {
        if native_fonts().is_none() {
            return; // no usable fonts installed
        }
        let source = "classDiagram\n    AppliancePlusAVeryLongName <|-- CoffeeMachine\n    \
                      CoffeeMachine : +brew(pressureInBar)\n";
        let svg = mermaid_svg(source).unwrap();
        let labels = label_boxes(&svg);
        assert!(labels.len() >= 2, "{labels:?}");
        for (width, text, style) in labels {
            let drawn = drawn_width(&text, &style);
            assert!(width + 0.5 >= drawn, "{text:?}: box {width}, text {drawn}");
        }
    }

    /// The (box width, text, text style) of each label in merman's SVG.
    fn label_boxes(svg: &str) -> Vec<(f32, String, String)> {
        let mut labels = Vec::new();
        for part in svg.split("id=\"merman-foreignobject-label-").skip(1) {
            let attribute = |name: &str| -> Option<String> {
                let start = part.find(&format!(" {name}=\""))? + name.len() + 3;
                Some(part[start..].split('"').next()?.to_string())
            };
            let Some(width) = attribute("width").and_then(|w| w.parse().ok()) else {
                continue;
            };
            let Some(text_start) = part.find("<text") else {
                continue;
            };
            let text_part = &part[text_start..];
            let style = text_part
                .split("style=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_default()
                .to_string();
            let Some(text) = text_part
                .split_once('>')
                .and_then(|(_, rest)| rest.split("</text>").next())
            else {
                continue;
            };
            labels.push((width, text.to_string(), style));
        }
        labels
    }

    /// The width of the ink of `text` drawn with CSS `style` by resvg.
    fn drawn_width(text: &str, style: &str) -> f32 {
        let svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2000\" height=\"100\">\
             <text x=\"0\" y=\"50\" style=\"{style}\">{text}</text></svg>"
        );
        let tree = usvg::Tree::from_str(&svg, &svg_options(16.0)).unwrap();
        tree.root().abs_bounding_box().width()
    }

    /// The zoom enlarges formulas like a larger text size does, and
    /// diagrams by the zoom factor, at the same device scale.
    #[test]
    fn zoom_enlarges_graphics() {
        let plain = RasterSettings::default();
        let zoomed = RasterSettings { zoom: 2.0, ..plain };
        let small = render_math("x^2", false, &plain).unwrap();
        let large = render_math("x^2", false, &zoomed).unwrap();
        assert!(
            large.width > small.width * 3 / 2,
            "{} {}",
            small.width,
            large.width
        );
        assert_eq!(large.scale, 1.0);
        let svg = mermaid_svg("graph TD; A-->B").unwrap();
        let natural = rasterize_diagram(&svg, &plain).unwrap();
        let double = rasterize_diagram(&svg, &zoomed).unwrap();
        assert!(double.width.abs_diff(natural.width * 2) <= 2);
        assert_eq!(double.scale, 1.0);
        assert!((double.logical_width() - 2.0 * natural.logical_width()).abs() <= 2.0);
    }

    /// The cache answers repeated formulas and diagram layouts itself: a
    /// formula is typeset once per settings, a diagram laid out once
    /// whatever the zoom; results are the same as rendering directly.
    #[test]
    fn cache_reuses_work() {
        let cache = Cache::default();
        let settings = RasterSettings::default();
        let math = Graphic::Math {
            tex: "a+b".to_string(),
            display: false,
        };
        let first = cache.render(&math, &settings);
        assert_eq!(first, render_math("a+b", false, &settings));
        assert_eq!(cache.render(&math, &settings), first);
        assert_eq!(cache.misses(), 1);
        let zoomed = RasterSettings {
            zoom: 1.5,
            ..settings
        };
        assert_ne!(cache.render(&math, &zoomed), first);
        assert_eq!(cache.misses(), 2);
        let diagram = Graphic::Diagram {
            source: "graph TD; A-->B".to_string(),
        };
        let natural = cache.render(&diagram, &settings).unwrap();
        let large = cache.render(&diagram, &zoomed).unwrap();
        assert_eq!(cache.misses(), 3);
        assert!(large.width > natural.width);
        let broken = Graphic::Diagram {
            source: "graph TD; A-->".to_string(),
        };
        assert!(cache.render(&broken, &settings).is_err());
        assert!(cache.render(&broken, &settings).is_err());
        assert_eq!(cache.misses(), 4);
        let empty = Graphic::Diagram {
            source: " ".to_string(),
        };
        assert_eq!(
            cache.render(&empty, &settings),
            Err("empty diagram".to_string())
        );
        assert_eq!(cache.misses(), 4);
    }

    /// A full cache map starts over instead of growing without bound.
    #[test]
    fn cache_is_bounded() {
        let map = Mutex::new(HashMap::new());
        for key in 0..5 {
            insert_bounded(&map, key, (), 3, |_| 0);
        }
        let map = lock(&map);
        assert!(map.len() <= 3);
        assert!(map.contains_key(&4));
    }

    /// A cache map also starts over when its bytes would exceed
    /// `CACHE_BYTES`, and a value larger than that alone is not kept.
    #[test]
    fn cache_is_bounded_in_bytes() {
        let map = Mutex::new(HashMap::new());
        let third = CACHE_BYTES / 3;
        for key in 0..3 {
            insert_bounded(&map, key, third, 100, |size| *size);
        }
        assert_eq!(lock(&map).len(), 3);
        insert_bounded(&map, 3, third, 100, |size| *size);
        assert_eq!(lock(&map).keys().copied().collect::<Vec<_>>(), [3]);
        insert_bounded(&map, 4, CACHE_BYTES + 1, 100, |size| *size);
        assert!(!lock(&map).contains_key(&4));
        assert_eq!(lock(&map).len(), 1);
    }

    /// Two threads computing the same entry end up sharing one value: the
    /// later insert returns the one already cached.
    #[test]
    fn cache_insert_returns_the_cached_value() {
        let map = Mutex::new(HashMap::new());
        let first = insert_bounded(&map, 1, Arc::new(10), 100, |_| 0);
        let second = insert_bounded(&map, 1, Arc::new(20), 100, |_| 0);
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(*second, 10);
    }

    /// The weights: an image's pixel bytes (none for an error), a diagram's
    /// SVG length.
    #[test]
    fn cache_weights() {
        let image = Image {
            width: 2,
            height: 1,
            scale: 1.0,
            depth: 0.0,
            pixels: vec![0; 8].into(),
            labels: Vec::new(),
            max_zoom: 1.0,
        };
        assert_eq!(rendered_bytes(&Ok(image)), 8);
        assert_eq!(rendered_bytes(&Err("x".to_string())), 0);
        assert_eq!(svg_bytes(&Ok(Arc::new("<svg/>".to_string()))), 6);
        assert_eq!(svg_bytes(&Err("bad".to_string())), 0);
    }

    /// A poisoned cache lock (a worker panicked) is still usable.
    #[test]
    fn lock_survives_poisoning() {
        let mutex = Arc::new(Mutex::new(1));
        let poisoner = mutex.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(mutex.is_poisoned());
        assert_eq!(*lock(&mutex), 1);
    }

    /// An image file changed on disk (or created after it was found
    /// missing) is decoded again rather than answered from the cache, and
    /// the entry for its old version is dropped.
    #[test]
    fn cache_notices_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("picture.png");
        let cache = Cache::default();
        let settings = RasterSettings::default();
        let graphic = Graphic::Image {
            path: path.clone(),
            inline: false,
        };
        assert!(cache.render(&graphic, &settings).is_err());
        image::RgbaImage::new(4, 3).save(&path).unwrap();
        let first = cache.render(&graphic, &settings).unwrap();
        assert_eq!((first.width, first.height), (4, 3));
        assert_eq!(cache.render(&graphic, &settings).unwrap(), first);
        let misses = cache.misses();
        // A different size, so the stamp differs even within one second.
        image::RgbaImage::new(7, 5).save(&path).unwrap();
        let second = cache.render(&graphic, &settings).unwrap();
        assert_eq!((second.width, second.height), (7, 5));
        assert_eq!(cache.misses(), misses + 1);
        let images = lock(&cache.images);
        assert_eq!(images.len(), 1);
        assert!(images.keys().all(|key| key.1 == file_stamp(&path)));
    }

    /// An inline image shrunk to the text is resampled once per zoom: its
    /// uses share one pixel buffer, another zoom gets its own, and a new
    /// version of the file replaces the shrunk entries of the old one. An
    /// inline image too small to shrink is not stored a second time.
    #[test]
    fn inline_images_are_shrunk_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        image::RgbaImage::new(400, 300).save(&path).unwrap();
        let cache = Cache::default();
        let settings = RasterSettings::default();
        let graphic = Graphic::Image {
            path: path.clone(),
            inline: true,
        };
        let first = cache.render(&graphic, &settings).unwrap();
        assert!(first.width < 400);
        let second = cache.render(&graphic, &settings).unwrap();
        assert!(Arc::ptr_eq(&first.pixels, &second.pixels));
        let zoomed = RasterSettings {
            zoom: 2.0,
            ..settings
        };
        let large = cache.render(&graphic, &zoomed).unwrap();
        assert!(!Arc::ptr_eq(&first.pixels, &large.pixels));
        assert_eq!(lock(&cache.inline_images).len(), 2);
        image::RgbaImage::new(500, 300).save(&path).unwrap();
        let replaced = cache.render(&graphic, &settings).unwrap();
        assert!(!Arc::ptr_eq(&first.pixels, &replaced.pixels));
        let inline = lock(&cache.inline_images);
        assert_eq!(inline.len(), 1);
        assert!(inline.keys().all(|(key, _)| key.1 == file_stamp(&path)));
        drop(inline);
        let small = dir.path().join("small.png");
        image::RgbaImage::new(8, 8).save(&small).unwrap();
        let icon = Graphic::Image {
            path: small,
            inline: true,
        };
        assert_eq!(cache.render(&icon, &settings).unwrap().width, 8);
        assert_eq!(lock(&cache.inline_images).len(), 1);
    }

    /// `file_stamp` is `None` for a missing file and changes with the
    /// file's contents.
    #[test]
    fn file_stamp_follows_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        assert_eq!(file_stamp(&path), None);
        std::fs::write(&path, "a").unwrap();
        let first = file_stamp(&path);
        assert!(first.is_some());
        assert_eq!(file_stamp(&path), first);
        std::fs::write(&path, "ab").unwrap();
        assert_ne!(file_stamp(&path), first);
    }

    /// The shared cache is one instance.
    #[test]
    fn shared_cache_is_single() {
        assert!(std::ptr::eq(Cache::shared(), Cache::shared()));
    }

    /// `svg_options` makes `em` units resolve to the requested size.
    #[test]
    fn svg_options_font_size() {
        assert_eq!(svg_options(20.0).font_size, 20.0);
    }
}

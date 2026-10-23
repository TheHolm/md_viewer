//! Turning formulas and diagrams into pixels, independent of GTK.
//!
//! Math is typeset to SVG by `mathra` (which also understands mhchem's
//! `\ce{}`/`\pu{}`), Mermaid diagrams are laid out to SVG by `merman`
//! (using its resvg-compatible output), then every SVG is rasterized by `resvg` into
//! premultiplied RGBA pixels the GUI wraps in a texture. Nothing here needs a
//! display, so it all runs on a worker thread and is covered by tests.

use crate::Block;
use resvg::{tiny_skia, usvg};
use std::collections::HashMap;
use std::path::PathBuf;
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
    /// The document's zoom factor (1.0 = 100%). Math is typeset at
    /// `font_px * zoom`, and diagrams are drawn `zoom` times their natural
    /// size; in both cases the image's logical size grows with the zoom.
    pub zoom: f32,
}

impl Default for RasterSettings {
    /// 15 px black text at scale 1 and 100% zoom, as used by the tests.
    fn default() -> Self {
        RasterSettings {
            font_px: 15.0,
            scale: 1.0,
            color: [0, 0, 0],
            zoom: 1.0,
        }
    }
}

impl RasterSettings {
    /// An exact, hashable form of the settings, for cache keys.
    fn key(&self) -> (u32, u32, u32, [u8; 3]) {
        (
            self.font_px.to_bits(),
            self.scale.to_bits(),
            self.zoom.to_bits(),
            self.color,
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
    let mut image = rasterize(&svg, font_px, settings.scale)?;
    image.depth = math_depth_em(&svg) * font_px;
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
    let mut image = rasterize(svg, DIAGRAM_FONT_PX, settings.scale * settings.zoom)?;
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

/// One graphic to render: a formula or a diagram, by its source.
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
}

/// What a cached formula is looked up by: source, display flag, settings.
type MathKey = (String, bool, (u32, u32, u32, [u8; 3]));

/// How many rendered formulas [`Cache`] keeps before it starts over.
const MATH_CACHE_LIMIT: usize = 4096;

/// How many laid-out diagrams [`Cache`] keeps before it starts over.
const DIAGRAM_CACHE_LIMIT: usize = 256;

/// Remembers rendered formulas (by source and settings) and laid-out
/// diagrams (by source), so a formula that appears many times is typeset
/// once, and zooming only re-rasterizes diagrams instead of laying them out
/// again (the slow part). Safe to share between worker threads; work is
/// done outside the lock, so two threads may occasionally both compute the
/// same entry. Each map is simply emptied when it reaches its limit.
#[derive(Debug, Default)]
pub struct Cache {
    /// Rendered formulas by (source, display, settings).
    math: Mutex<HashMap<MathKey, Rendered>>,
    /// Laid-out diagram SVG (or the layout error) by source.
    diagrams: Mutex<HashMap<String, Result<Arc<String>, String>>>,
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
                insert_bounded(&self.math, key, rendered.clone(), MATH_CACHE_LIMIT);
                rendered
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
                            svg.clone(),
                            DIAGRAM_CACHE_LIMIT,
                        );
                        svg
                    }
                };
                rasterize_diagram(&svg?, settings)
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

/// Inserts into a cache map, emptying it first if it holds `limit` entries.
fn insert_bounded<K: std::hash::Hash + Eq, V>(
    map: &Mutex<HashMap<K, V>>,
    key: K,
    value: V,
    limit: usize,
) {
    let mut map = lock(map);
    if map.len() >= limit {
        map.clear();
    }
    map.insert(key, value);
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
            insert_bounded(&map, key, (), 3);
        }
        let map = lock(&map);
        assert!(map.len() <= 3);
        assert!(map.contains_key(&4));
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

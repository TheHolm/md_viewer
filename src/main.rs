//! The GTK4/libadwaita front-end of `md_viewer`.
//!
//! Deliberately thin: it parses the command line, builds a window around a
//! read-only text view, and maps the library's blocks onto it: text blocks
//! become tagged text, tables become grid widgets anchored in the text. All
//! Markdown handling lives in the library (`src/lib.rs`).

use adw::prelude::*;
use clap::Parser;
use gtk4::glib::translate::IntoGlib;
use gtk4::{gdk, gio, glib, pango};
use md_viewer::graphics::{Image, RasterSettings};
use md_viewer::{Align, Block, Inline, Math, Style, Table};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// The application ID registered with GTK.
const APP_ID: &str = "io.github.TheHolm.md_viewer";

/// The left and right margin of the document, in pixels.
const MARGIN: i32 = 16;

/// The name of the text tag that centres display math on its line.
const DISPLAY_MATH_TAG: &str = "math-display";

/// Roughly how many characters wide a table cell grows before its text wraps.
const CELL_WRAP_CHARS: i32 = 40;

/// Styling for the widgets the document is built from (tables). Colours use
/// the theme's named colours so light and dark themes both work.
const CSS: &str = r#"
.md-table {
  border: 1px solid alpha(currentColor, 0.25);
  border-radius: 6px;
}
.md-cell {
  padding: 6px 10px;
  border-right: 1px solid alpha(currentColor, 0.15);
  border-bottom: 1px solid alpha(currentColor, 0.15);
}
.md-cell.md-last-column {
  border-right: none;
}
.md-cell.md-last-row {
  border-bottom: none;
}
.md-cell.md-header {
  background-color: alpha(currentColor, 0.08);
}
.md-cell.md-stripe {
  background-color: alpha(currentColor, 0.03);
}
"#;

/// Command line arguments. `--version` prints the contents of the `VERSION`
/// file (via `md_viewer::version`), not the copy in `Cargo.toml`.
#[derive(Parser, Debug)]
#[command(version = md_viewer::version(), about = "View a Markdown document")]
struct Args {
    /// The Markdown file to view.
    file: PathBuf,
}

/// Parses the command line and runs the application.
///
/// The arguments are parsed with clap before GTK starts, and GTK is then
/// given only the program name, so GApplication never tries to interpret
/// them itself.
fn main() -> glib::ExitCode {
    let args = Args::parse();
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(move |app| build_window(app, args.file.clone()));
    app.run_with_args(&[env!("CARGO_PKG_NAME")])
}

/// Installs the document widget styling for the whole application.
fn load_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// The widgets whose size follows the width of the document view.
///
/// Widgets anchored in a text view are only given their minimum size, so
/// each one's size request is recomputed from its content and the view
/// width on every frame; GTK caches measurements, so this is cheap, and a
/// request is only changed when the computed size differs. Doing it every
/// frame (rather than once) also picks up the final sizes after styling and
/// font changes.
#[derive(Default)]
struct Resizable {
    /// The horizontal scrollers around tables, sized to the table's natural
    /// width but never wider than the view.
    tables: RefCell<Vec<gtk4::ScrolledWindow>>,
}

impl Resizable {
    /// Re-sizes every registered widget for a view `width` pixels wide.
    fn update(&self, width: i32) {
        let available = (width - 2 * MARGIN - 8).max(100);
        for scroller in self.tables.borrow().iter() {
            let Some(grid) = scroller.child() else {
                continue;
            };
            let natural = grid.measure(gtk4::Orientation::Horizontal, -1).1;
            // The grid always gets its natural width inside the scroller;
            // the scroller is capped at the space available, and a wider
            // table scrolls horizontally (libadwaita's scrollbars overlay
            // the content, so no extra height is needed for them).
            let height = grid.measure(gtk4::Orientation::Vertical, natural).1;
            let size = (natural.min(available), height);
            if scroller.size_request() != size {
                scroller.set_size_request(size.0, size.1);
            }
        }
    }
}

/// Builds and shows the main window, then loads `path` into it.
fn build_window(app: &adw::Application, path: PathBuf) {
    let view = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(MARGIN)
        .right_margin(MARGIN)
        .build();
    create_tags(&view.buffer().tag_table());
    let scrolled = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&view)
        .build();
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&adw::HeaderBar::new());
    content.append(&scrolled);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(md_viewer::window_title(&path))
        .default_width(800)
        .default_height(900)
        .content(&content)
        .build();
    window.present();

    let resizable = Rc::new(Resizable::default());
    {
        let resizable = resizable.clone();
        view.add_tick_callback(move |view, _| {
            resizable.update(view.width());
            glib::ControlFlow::Continue
        });
    }

    // Read, render and rasterize on the I/O thread pool, then fill the view
    // back on the main thread.
    let settings = raster_settings(&view);
    glib::spawn_future_local(async move {
        let worker_path = path.clone();
        let result = gio::spawn_blocking(move || {
            md_viewer::open_document(&worker_path).map(|mut blocks| {
                md_viewer::graphics::render_document(&mut blocks, &settings);
                blocks
            })
        })
        .await;
        match result {
            Ok(Ok(blocks)) => fill_view(&view, &blocks, &resizable),
            Ok(Err(error)) => {
                eprintln!("{}", md_viewer::error_message(&path, &error));
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("Rendering {} failed unexpectedly", path.display());
                std::process::exit(1);
            }
        }
    });
}

/// The settings graphics are rendered with for `view`: its font size, the
/// display's scale factor and the theme's text colour.
fn raster_settings(view: &gtk4::TextView) -> RasterSettings {
    let font_px = view
        .pango_context()
        .font_description()
        .map(|font| {
            let size = font.size() as f32 / pango::SCALE as f32;
            // Point sizes are relative to 96 dpi logical pixels.
            if font.is_size_absolute() {
                size
            } else {
                size * 96.0 / 72.0
            }
        })
        .filter(|size| *size > 0.0)
        .unwrap_or(15.0);
    // `style_context().color()` is the GTK 4.8 way to read the text colour.
    #[allow(deprecated)]
    let color = view.style_context().color();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    RasterSettings {
        font_px,
        scale: view.scale_factor().max(1) as f32,
        color: [
            channel(color.red()),
            channel(color.green()),
            channel(color.blue()),
        ],
    }
}

/// Appends every block to `view`'s buffer: text as tagged text, tables as
/// anchored grid widgets.
fn fill_view(view: &gtk4::TextView, blocks: &[Block], resizable: &Rc<Resizable>) {
    let buffer = view.buffer();
    for block in blocks {
        match block {
            Block::Text(inlines) => insert_inlines(&buffer, inlines),
            Block::Table(table) => {
                let widget = table_widget(table);
                let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
                view.add_child_at_anchor(&widget, &anchor);
                resizable.tables.borrow_mut().push(widget);
            }
        }
    }
    // Size the new widgets for the current width straight away.
    resizable.update(view.width());
}

/// Inserts `inlines` at the end of `buffer`, tagging each with its styles.
fn insert_inlines(buffer: &gtk4::TextBuffer, inlines: &[Inline]) {
    for inline in inlines {
        match inline {
            Inline::Text(span) => {
                let styles: Vec<Style> = span.styles.clone();
                insert_styled(buffer, &span.text, &styles);
            }
            Inline::Math(math) => insert_math(buffer, math),
        }
    }
}

/// Inserts `text` at the end of `buffer` with the tags for `styles`.
fn insert_styled(buffer: &gtk4::TextBuffer, text: &str, styles: &[Style]) {
    let names: Vec<&str> = styles.iter().map(|style| style.tag_name()).collect();
    buffer.insert_with_tags_by_name(&mut buffer.end_iter(), text, &names);
}

/// Inserts a rendered formula at the end of `buffer`: inline math is lowered
/// so its baseline lines up with the text, display math is centred. A
/// formula that could not be rendered is shown as its source in code style,
/// display math with a note saying why.
fn insert_math(buffer: &gtk4::TextBuffer, math: &Math) {
    match &math.image {
        Some(Ok(image)) => {
            let start = buffer.end_iter().offset();
            buffer.insert_paintable(&mut buffer.end_iter(), &ScaledTexture::new(image));
            let range = (buffer.iter_at_offset(start), buffer.end_iter());
            let rise = gtk4::TextTag::builder()
                .rise(-(image.depth * pango::SCALE as f32).round() as i32)
                .build();
            buffer.tag_table().add(&rise);
            buffer.apply_tag(&rise, &range.0, &range.1);
            if math.display {
                buffer.apply_tag_by_name(DISPLAY_MATH_TAG, &range.0, &range.1);
            }
        }
        failed => {
            let reason = match failed {
                Some(Err(reason)) => reason.as_str(),
                _ => "not rendered",
            };
            if math.display {
                insert_styled(buffer, &math.source(), &[Style::CodeBlock]);
                insert_styled(
                    buffer,
                    &format!("\n⚠ formula could not be rendered: {reason}"),
                    &[Style::Image],
                );
            } else {
                insert_styled(buffer, &math.source(), &[Style::Code]);
            }
        }
    }
}

/// Builds the widget for `table`: a bordered grid of labels (one per cell,
/// keeping the cells' inline styles), inside a horizontal
/// scroller that only scrolls when the table is wider than the document.
fn table_widget(table: &Table) -> gtk4::ScrolledWindow {
    let grid = gtk4::Grid::builder()
        .css_classes(["md-table"])
        .overflow(gtk4::Overflow::Hidden)
        .build();
    let rows = std::iter::once(&table.header).chain(table.rows.iter());
    let row_count = table.rows.len() + 1;
    for (row_index, row) in rows.enumerate() {
        for (column, cell) in row.iter().enumerate() {
            let mut classes = vec!["md-cell"];
            if row_index == 0 {
                classes.push("md-header");
            } else if row_index % 2 == 0 {
                classes.push("md-stripe");
            }
            if column + 1 == table.columns() {
                classes.push("md-last-column");
            }
            if row_index + 1 == row_count {
                classes.push("md-last-row");
            }
            let alignment = table.alignments[column];
            let cell_widget = cell_widget(cell, alignment, &classes);
            grid.attach(&cell_widget, column as i32, row_index as i32, 1, 1);
        }
    }
    gtk4::ScrolledWindow::builder()
        .child(&grid)
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        // External: no vertical scrollbar, and the child's height (measured
        // without a width, so far too tall for wrapping cells) is not forced
        // on the scroller; `Resizable` sets the real height instead.
        .vscrollbar_policy(gtk4::PolicyType::External)
        .build()
}

/// Builds one table cell: a wrapping label showing the cell's styled text
/// as Pango markup, aligned according to the column's alignment. Long cells
/// wrap at about [`CELL_WRAP_CHARS`] characters instead of widening the table.
fn cell_widget(cell: &[Inline], alignment: Align, classes: &[&str]) -> gtk4::Widget {
    if cell.iter().any(|inline| matches!(inline, Inline::Math(_))) {
        return math_cell_widget(cell, alignment, classes);
    }
    let (halign, xalign, justify) = match alignment {
        Align::None | Align::Left => (gtk4::Align::Start, 0.0, gtk4::Justification::Left),
        Align::Center => (gtk4::Align::Center, 0.5, gtk4::Justification::Center),
        Align::Right => (gtk4::Align::End, 1.0, gtk4::Justification::Right),
    };
    let label = gtk4::Label::builder()
        .use_markup(true)
        .label(md_viewer::pango_markup(cell))
        .wrap(true)
        // Word wrapping keeps a cell at least as wide as its longest word, so
        // short cells are never broken mid-word.
        .wrap_mode(pango::WrapMode::Word)
        .max_width_chars(CELL_WRAP_CHARS)
        .selectable(true)
        .xalign(xalign)
        .justify(justify)
        .halign(halign)
        .build();
    let frame = gtk4::Box::builder().css_classes(classes.to_vec()).build();
    label.set_hexpand(true);
    frame.append(&label);
    frame.upcast()
}

/// Builds a table cell containing formulas: a single (non-wrapping) row of
/// labels for the text and pictures for the formulas; formulas that could
/// not be rendered show their source.
fn math_cell_widget(cell: &[Inline], alignment: Align, classes: &[&str]) -> gtk4::Widget {
    let halign = match alignment {
        Align::None | Align::Left => gtk4::Align::Start,
        Align::Center => gtk4::Align::Center,
        Align::Right => gtk4::Align::End,
    };
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .halign(halign)
        .hexpand(true)
        .build();
    for inline in cell {
        match inline {
            Inline::Math(Math {
                image: Some(Ok(image)),
                ..
            }) => {
                let picture = gtk4::Picture::for_paintable(&ScaledTexture::new(image));
                picture.set_can_shrink(false);
                picture.set_valign(gtk4::Align::Center);
                row.append(&picture);
            }
            other => {
                let label = gtk4::Label::builder()
                    .use_markup(true)
                    .label(md_viewer::pango_markup(std::slice::from_ref(other)))
                    .selectable(true)
                    .valign(gtk4::Align::Center)
                    .build();
                row.append(&label);
            }
        }
    }
    let frame = gtk4::Box::builder().css_classes(classes.to_vec()).build();
    frame.append(&row);
    frame.upcast()
}

/// Registers one text tag per [`Style`] in `table`, plus the display math
/// centring tag.
fn create_tags(table: &gtk4::TextTagTable) {
    for style in Style::ALL {
        table.add(&tag_for(style));
    }
    table.add(
        &gtk4::TextTag::builder()
            .name(DISPLAY_MATH_TAG)
            .justification(gtk4::Justification::Center)
            .pixels_above_lines(4)
            .pixels_below_lines(4)
            .build(),
    );
}

/// Builds the text tag implementing `style`.
fn tag_for(style: Style) -> gtk4::TextTag {
    let muted = gdk::RGBA::parse(md_viewer::MUTED_COLOR).expect("valid colour");
    let mut code_background = gdk::RGBA::parse(md_viewer::CODE_BACKGROUND).expect("valid colour");
    code_background.set_alpha(0.15);
    let link = gdk::RGBA::parse(md_viewer::LINK_COLOR).expect("valid colour");
    let builder = gtk4::TextTag::builder().name(style.tag_name());
    match style {
        Style::Heading(level) => {
            let scale = match level {
                1 => 2.0,
                2 => 1.6,
                3 => 1.3,
                4 => 1.15,
                5 => 1.0,
                _ => 0.9,
            };
            builder
                .weight(pango::Weight::Bold.into_glib())
                .scale(scale)
                .pixels_above_lines(6)
        }
        Style::Emphasis => builder.style(pango::Style::Italic),
        Style::Strong => builder.weight(pango::Weight::Bold.into_glib()),
        Style::Strikethrough => builder.strikethrough(true),
        Style::Code => builder
            .family("monospace")
            .background_rgba(&code_background),
        Style::CodeBlock => builder
            .family("monospace")
            .paragraph_background_rgba(&code_background)
            .wrap_mode(gtk4::WrapMode::None)
            .left_margin(28),
        Style::Quote => builder
            .style(pango::Style::Italic)
            .foreground_rgba(&muted)
            .left_margin(36),
        Style::Link => builder
            .underline(pango::Underline::Single)
            .foreground_rgba(&link),
        Style::Image => builder.style(pango::Style::Italic).foreground_rgba(&muted),
        Style::Rule => builder.foreground_rgba(&muted),
    }
    .build()
}

/// A texture drawn at its logical size: a [`Image`] rendered at a device
/// scale of 2 has twice the pixels but the same on-screen size, so formulas
/// and diagrams stay sharp on HiDPI displays.
mod scaled_texture {
    use gtk4::gdk::subclass::prelude::*;
    use gtk4::prelude::*;
    use gtk4::{gdk, glib, graphene};
    use std::cell::{Cell, RefCell};

    /// The instance state: the texture and its logical size.
    #[derive(Default)]
    pub struct Imp {
        /// The device-pixel texture.
        pub texture: RefCell<Option<gdk::Texture>>,
        /// The logical width, in pixels.
        pub width: Cell<f64>,
        /// The logical height, in pixels.
        pub height: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Imp {
        const NAME: &'static str = "MdViewerScaledTexture";
        type Type = super::ScaledTexture;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for Imp {}

    impl PaintableImpl for Imp {
        /// The logical width, rounded up.
        fn intrinsic_width(&self) -> i32 {
            self.width.get().ceil() as i32
        }

        /// The logical height, rounded up.
        fn intrinsic_height(&self) -> i32 {
            self.height.get().ceil() as i32
        }

        /// Draws the texture scaled into the requested area.
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let (Some(texture), Some(snapshot)) = (
                self.texture.borrow().clone(),
                snapshot.downcast_ref::<gtk4::Snapshot>(),
            ) else {
                return;
            };
            let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
            snapshot.append_texture(&texture, &bounds);
        }
    }
}

glib::wrapper! {
    /// A paintable showing an [`Image`] at its logical size.
    pub struct ScaledTexture(ObjectSubclass<scaled_texture::Imp>)
        @implements gdk::Paintable;
}

impl ScaledTexture {
    /// Wraps `image` (premultiplied RGBA) in a paintable of its logical size.
    fn new(image: &Image) -> Self {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        let paintable: Self = glib::Object::new();
        let texture = gdk::MemoryTexture::new(
            image.width as i32,
            image.height as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &glib::Bytes::from(&image.pixels),
            image.width as usize * 4,
        );
        let imp = paintable.imp();
        imp.texture.replace(Some(texture.upcast()));
        imp.width.set(image.logical_width() as f64);
        imp.height.set(image.logical_height() as f64);
        paintable
    }
}

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
use md_viewer::layout;
use md_viewer::{Align, Block, Diagram, Inline, Math, Origin, Style, Table, Target, APP_ID};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The name of the text tag that centres display math on its line.
const DISPLAY_MATH_TAG: &str = "math-display";

/// Roughly how many characters wide a table cell grows before its text wraps.
const CELL_WRAP_CHARS: i32 = 40;

/// Styling for the widgets the document is built from (tables, diagrams).
/// Table colours derive from the theme's text colour, so light and dark
/// themes both work; diagrams sit on a white card in either theme, because
/// Mermaid's colours assume a light background.
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
.md-diagram {
  background-color: #ffffff;
  border: 1px solid alpha(currentColor, 0.2);
  border-radius: 6px;
  padding: 12px;
}
"#;

/// Command line arguments. `--version` prints the contents of the `VERSION`
/// file (via `md_viewer::version`), not the copy in `Cargo.toml`.
#[derive(Parser, Debug)]
#[command(version = md_viewer::version(), about = "View a Markdown document")]
struct Args {
    /// The Markdown file to view. Without it the window starts empty, with
    /// a button to open a document.
    file: Option<PathBuf>,
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
    app.connect_startup(|app| {
        load_css();
        quit_on_signals(app);
        gtk4::Window::set_default_icon_name(APP_ID);
        app.set_accels_for_action("win.open", &["<Control>o"]);
        app.set_accels_for_action("window.close", &["<Control>w"]);
    });
    app.connect_activate(move |app| {
        let window = DocWindow::new(app);
        if let Some(file) = &args.file {
            open_file(&window, file.clone(), Origin::CommandLine);
        }
    });
    app.run_with_args(&[env!("CARGO_PKG_NAME")])
}

/// SIGINT and SIGTERM (the same numbers on Linux and FreeBSD).
const QUIT_SIGNALS: [i32; 2] = [2, 15];

/// Makes SIGINT (Ctrl+C in a terminal) and SIGTERM quit the application
/// cleanly through the main loop instead of killing the process outright.
fn quit_on_signals(app: &adw::Application) {
    for signal in QUIT_SIGNALS {
        let app = app.downgrade();
        glib_unix::unix_signal_add_local(signal, move || {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
            glib::ControlFlow::Continue
        });
    }
}

/// Installs the document widget styling for the whole application.
fn load_css() {
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(CSS);
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
    /// Diagram pictures with their natural logical size; scaled down
    /// (keeping the aspect ratio) when the view is narrower.
    diagrams: RefCell<Vec<(gtk4::Picture, f32, f32)>>,
}

impl Resizable {
    /// Re-sizes every registered widget for a view `width` pixels wide.
    fn update(&self, width: i32) {
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
            let size = layout::table_size(natural, height, width);
            if scroller.size_request() != size {
                scroller.set_size_request(size.0, size.1);
            }
        }
        for (picture, natural_width, natural_height) in self.diagrams.borrow().iter() {
            let size = layout::diagram_size(*natural_width, *natural_height, width);
            if picture.size_request() != size {
                picture.set_size_request(size.0, size.1);
            }
        }
    }
}

/// One viewer window: a header bar with an "Open" button over either the
/// empty start page or a document.
struct DocWindow {
    /// The application this window belongs to, for opening new windows.
    app: adw::Application,
    /// The top-level window.
    window: adw::ApplicationWindow,
    /// Holds the header bar; its content is the start page or the document.
    toolbar: adw::ToolbarView,
    /// The document shown, or `None` while the window shows the start page.
    document: RefCell<Option<PathBuf>>,
}

impl DocWindow {
    /// Builds and shows a window with the empty start page, and installs
    /// its `win.open` action (the header bar button, the start page button
    /// and Ctrl+O).
    fn new(app: &adw::Application) -> Rc<Self> {
        let open_button = gtk4::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open a Document (Ctrl+O)")
            .action_name("win.open")
            .build();
        let header = adw::HeaderBar::new();
        header.pack_start(&open_button);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&start_page()));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(md_viewer::APP_NAME)
            .default_width(800)
            .default_height(900)
            .content(&toolbar)
            .build();
        let this = Rc::new(DocWindow {
            app: app.clone(),
            window,
            toolbar,
            document: RefCell::new(None),
        });

        // The action holds the only strong reference to the window state, so
        // the state lives exactly as long as the window: removing the action
        // when the window closes breaks the window -> action -> state cycle.
        let open = gio::SimpleAction::new("open", None);
        let state = this.clone();
        open.connect_activate(move |_, _| choose_file(&state));
        this.window.add_action(&open);
        this.window.connect_close_request(|window| {
            window.remove_action("open");
            glib::Propagation::Proceed
        });
        this.window.present();
        this
    }

    /// Replaces the window's content with a fresh document view titled
    /// after `path`, and returns the view to fill.
    fn show_document(&self, path: &Path) -> gtk4::TextView {
        let view = gtk4::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(layout::DOCUMENT_MARGIN)
            .right_margin(layout::DOCUMENT_MARGIN)
            .build();
        create_tags(&view.buffer().tag_table());
        let scrolled = gtk4::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(&view)
            .build();
        self.toolbar.set_content(Some(&scrolled));
        self.window.set_title(Some(&md_viewer::window_title(path)));
        self.document.replace(Some(path.to_path_buf()));
        view
    }
}

/// The page an empty window shows: an invitation to open a document.
fn start_page() -> adw::StatusPage {
    let button = gtk4::Button::builder()
        .label("Open…")
        .action_name("win.open")
        .halign(gtk4::Align::Center)
        .css_classes(["pill", "suggested-action"])
        .build();
    adw::StatusPage::builder()
        .icon_name("text-x-generic-symbolic")
        .title("Open a Markdown Document")
        .description("Or pass a file on the command line: md_viewer FILE")
        .child(&button)
        .build()
}

/// Shows the file chooser over `window` and opens the picked document,
/// in `window` itself if it is still empty and in a new window otherwise.
/// Cancelling the chooser does nothing.
fn choose_file(window: &Rc<DocWindow>) {
    let filters = gio::ListStore::new::<gtk4::FileFilter>();
    let markdown = gtk4::FileFilter::new();
    markdown.set_name(Some("Markdown"));
    for mime_type in md_viewer::MARKDOWN_MIME_TYPES {
        markdown.add_mime_type(mime_type);
    }
    for pattern in md_viewer::MARKDOWN_PATTERNS {
        markdown.add_pattern(pattern);
    }
    let all = gtk4::FileFilter::new();
    all.set_name(Some("All Files"));
    all.add_pattern("*");
    filters.append(&markdown);
    filters.append(&all);

    let dialog = gtk4::FileDialog::builder()
        .title("Open a Document")
        .modal(true)
        .filters(&filters)
        .default_filter(&markdown)
        .build();
    let folder = md_viewer::chooser_folder(window.document.borrow().as_deref());
    if let Some(folder) = folder {
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
    }
    let window = window.clone();
    glib::spawn_future_local(async move {
        let file = match dialog.open_future(Some(&window.window)).await {
            Ok(file) => file,
            Err(error) if error.matches(gtk4::DialogError::Dismissed) => return,
            Err(error) => return show_error(&window.window, &error.to_string()),
        };
        match file.path() {
            Some(path) => open_file(&window, path, Origin::Chooser),
            None => show_error(
                &window.window,
                &format!("Cannot open {}: not a local file", file.uri()),
            ),
        }
    });
}

/// Shows `message` in an alert dialog over `window`.
fn show_error(window: &adw::ApplicationWindow, message: &str) {
    let dialog = adw::AlertDialog::new(Some("Cannot Open Document"), Some(message));
    dialog.add_response("close", "Close");
    dialog.present(Some(window));
}

/// Reports a document that could not be opened: on stderr with exit status
/// 1 for the command line argument, in a dialog over `window` otherwise.
fn report_open_error(window: &DocWindow, origin: Origin, message: &str) {
    if origin.exits_on_error() {
        eprintln!("{message}");
        std::process::exit(1);
    }
    show_error(&window.window, message);
}

/// Opens the document at `path` from `window`.
///
/// The file is read on the I/O thread pool first, so a failure is reported
/// (see `report_open_error`) before any window changes. The document is
/// then shown in `window` or a new window (`md_viewer::open_target`), and
/// rendered and rasterized on the pool, with settings taken from the new
/// view, before the view is filled back on the main thread.
fn open_file(window: &Rc<DocWindow>, path: PathBuf, origin: Origin) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let worker_path = path.clone();
        let text = match gio::spawn_blocking(move || md_viewer::read_document(&worker_path)).await {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => {
                let message = md_viewer::error_message(&path, &error);
                return report_open_error(&window, origin, &message);
            }
            Err(_) => {
                let message = format!("Reading {} failed unexpectedly", path.display());
                return report_open_error(&window, origin, &message);
            }
        };
        let has_document = window.document.borrow().is_some();
        let target = match md_viewer::open_target(has_document) {
            Target::SameWindow => window,
            Target::NewWindow => DocWindow::new(&window.app),
        };
        let view = target.show_document(&path);
        let resizable = Rc::new(Resizable::default());
        {
            let resizable = resizable.clone();
            view.add_tick_callback(move |view, _| {
                resizable.update(view.width());
                glib::ControlFlow::Continue
            });
        }
        let settings = raster_settings(&view);
        let rendered = gio::spawn_blocking(move || {
            let mut blocks = md_viewer::render(&text);
            md_viewer::graphics::render_document(&mut blocks, &settings);
            blocks
        })
        .await;
        match rendered {
            Ok(blocks) => fill_view(&view, &blocks, &resizable),
            Err(_) => {
                let message = format!("Rendering {} failed unexpectedly", path.display());
                report_open_error(&target, origin, &message);
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
        .and_then(|font| layout::font_size_px(font.size(), font.is_size_absolute(), pango::SCALE))
        .unwrap_or(layout::DEFAULT_FONT_PX);
    let color = view.color();
    let channel = layout::color_channel;
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

/// Appends every block to `view`'s buffer: text as tagged text, tables and
/// diagrams as anchored widgets.
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
            Block::Mermaid(diagram) => insert_diagram(view, diagram, resizable),
        }
    }
    // Size the new widgets for the current width straight away.
    resizable.update(view.width());
}

/// Inserts a Mermaid diagram at the end of `view`'s buffer: the picture on a
/// white card, scaled down to fit the view. A diagram that could not be
/// rendered is shown as its source in a code block, with the reason.
fn insert_diagram(view: &gtk4::TextView, diagram: &Diagram, resizable: &Rc<Resizable>) {
    let buffer = view.buffer();
    match &diagram.image {
        Some(Ok(image)) => {
            let picture = gtk4::Picture::for_paintable(&ScaledTexture::new(image));
            picture.set_can_shrink(true);
            picture.set_content_fit(gtk4::ContentFit::Contain);
            let card = gtk4::Box::builder()
                .css_classes(["md-diagram"])
                .halign(gtk4::Align::Start)
                .build();
            card.append(&picture);
            let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
            view.add_child_at_anchor(&card, &anchor);
            resizable.diagrams.borrow_mut().push((
                picture,
                image.logical_width(),
                image.logical_height(),
            ));
        }
        failed => {
            let reason = match failed {
                Some(Err(reason)) => reason.as_str(),
                _ => "not rendered",
            };
            insert_styled(&buffer, &diagram.source, &[Style::CodeBlock]);
            insert_styled(
                &buffer,
                &format!("\n⚠ diagram could not be rendered: {reason}"),
                &[Style::Image],
            );
        }
    }
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
                .rise(layout::math_rise(image.depth, pango::SCALE))
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
            let classes = layout::cell_classes(row_index, column, table.columns(), row_count);
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
    let (halign, justify) = match alignment {
        Align::None | Align::Left => (gtk4::Align::Start, gtk4::Justification::Left),
        Align::Center => (gtk4::Align::Center, gtk4::Justification::Center),
        Align::Right => (gtk4::Align::End, gtk4::Justification::Right),
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
        .xalign(layout::align_fraction(alignment))
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
        Style::Heading(level) => builder
            .weight(pango::Weight::Bold.into_glib())
            .scale(layout::heading_scale(level))
            .pixels_above_lines(6),
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

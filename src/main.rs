//! The GTK4/libadwaita front-end of `md_viewer`.
//!
//! Deliberately thin: it parses the command line, builds a window around a
//! read-only text view, and maps the library's blocks onto it: text blocks
//! become tagged text, tables, code blocks and diagrams become widgets
//! anchored in the text. Graphics are rendered by the library's worker pool
//! (`md_viewer::progressive`) and blocks are appended as they complete. All
//! Markdown handling lives in the library (`src/lib.rs`).

use adw::prelude::*;
use clap::Parser;
use gtk4::glib::translate::IntoGlib;
use gtk4::{gdk, gio, glib, pango};
use md_viewer::graphics::{Image, RasterSettings, Rendered};
use md_viewer::progressive::{self, Job, Phase, Pool, Progress};
use md_viewer::zoom::{self, Zoom};
use md_viewer::{layout, CodeBlock};
use md_viewer::{Align, Block, Diagram, Inline, Math, Origin, Style, Table, Target, APP_ID};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

/// The name of the text tag that centres display math on its line.
const DISPLAY_MATH_TAG: &str = "math-display";

/// Roughly how many characters wide a table cell grows before its text wraps.
const CELL_WRAP_CHARS: i32 = 40;

/// How often rendered graphics are collected from the workers while a
/// document loads, in milliseconds (about once a frame).
const POLL_MS: u64 = 16;

/// How long after the last zoom step graphics are re-rendered, in
/// milliseconds; until then they are shown stretched.
const REZOOM_DELAY_MS: u64 = 150;

/// Styling for the widgets the document is built from (tables, code cards,
/// diagrams, the loading cover). Colours derive from the theme's text
/// colour, so light and dark themes both work; diagrams sit on a white card
/// in either theme, because Mermaid's colours assume a light background.
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
.md-code {
  background-color: alpha(currentColor, 0.06);
  border: 1px solid alpha(currentColor, 0.15);
  border-radius: 6px;
}
.md-code-header {
  padding: 2px 2px 0px 10px;
}
.md-code-text {
  padding: 2px 10px 10px 10px;
}
.md-loading-cover {
  background-color: @view_bg_color;
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
        app.set_accels_for_action(
            "win.zoom-in",
            &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
        );
        app.set_accels_for_action("win.zoom-out", &["<Control>minus", "<Control>KP_Subtract"]);
        app.set_accels_for_action("win.zoom-reset", &["<Control>0", "<Control>KP_0"]);
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

/// Installs `css` for the whole application through `provider`.
fn add_css(provider: &gtk4::CssProvider, css: &str) {
    provider.load_from_string(css);
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Installs the document widget styling for the whole application.
fn load_css() {
    add_css(&gtk4::CssProvider::new(), CSS);
}

/// The widgets whose size follows the width of the document view.
///
/// Widgets anchored in a text view are only given their minimum size, so
/// each one's size request is recomputed from its content and the view
/// width on every frame; GTK caches measurements, so this is cheap, and a
/// request is only changed when the computed size differs. Doing it every
/// frame (rather than once) also picks up the final sizes after styling,
/// font and zoom changes.
#[derive(Default)]
struct Resizable {
    /// The horizontal scrollers around tables, sized to the table's natural
    /// width but never wider than the view.
    tables: RefCell<Vec<gtk4::ScrolledWindow>>,
    /// Diagram pictures with their natural logical size (at the current
    /// zoom); scaled down (keeping the aspect ratio) when the view is
    /// narrower.
    diagrams: RefCell<Vec<(gtk4::Picture, f32, f32)>>,
    /// Code cards with their nesting depth, as wide as the view allows.
    codes: RefCell<Vec<(gtk4::Box, usize)>>,
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
        for (card, indent) in self.codes.borrow().iter() {
            let card_width = layout::code_card_width(width, *indent);
            if card.size_request() != (card_width, -1) {
                card.set_size_request(card_width, -1);
            }
        }
    }
}

/// Where a rendered graphic is shown, so a re-render (after a zoom) can
/// replace its picture in place.
struct Slot {
    /// The paintable showing the graphic.
    texture: ScaledTexture,
    /// The tag lowering an inline formula onto the baseline, if any.
    rise: Option<gtk4::TextTag>,
    /// The diagram's entry in [`Resizable::diagrams`], if it is a diagram.
    diagram: Option<usize>,
    /// The buffer offset the graphic (or its table) is at.
    offset: i32,
    /// How far the graphic reaches below the baseline, in pixels.
    depth: Cell<f32>,
}

impl Slot {
    /// Shows `image` in place of the current picture.
    fn apply(&self, image: &Image, resizable: &Resizable) {
        self.texture.set_image(image);
        self.depth.set(image.depth);
        self.update_extras(image.logical_width(), image.logical_height(), resizable);
    }

    /// Stretches the current picture by `ratio` until a sharp one arrives.
    fn stretch(&self, ratio: f64, resizable: &Resizable) {
        let (width, height) = self.texture.scale_size(ratio);
        self.depth.set(self.depth.get() * ratio as f32);
        self.update_extras(width, height, resizable);
    }

    /// Updates the baseline offset and the diagram's natural size after the
    /// picture changed size.
    fn update_extras(&self, width: f32, height: f32, resizable: &Resizable) {
        if let Some(rise) = &self.rise {
            rise.set_rise(layout::math_rise(self.depth.get(), pango::SCALE));
        }
        if let Some(index) = self.diagram {
            if let Some(entry) = resizable.diagrams.borrow_mut().get_mut(index) {
                entry.1 = width;
                entry.2 = height;
            }
        }
    }
}

/// One rendered result from a worker: generation, graphic id and result.
type Delivery = (u64, usize, Rendered);

/// A document being shown in a window: its view, the blocks not on the page
/// yet, and the rendering of its graphics (see `md_viewer::progressive`).
struct Session {
    /// The document view.
    view: gtk4::TextView,
    /// The scroller around the view.
    scrolled: gtk4::ScrolledWindow,
    /// Hides the page behind a spinner until the first screen is full.
    cover: gtk4::Revealer,
    /// The spinner on the cover, shown after a short delay.
    cover_spinner: adw::Spinner,
    /// The window's header bar spinner, shown while more is loading.
    header_spinner: adw::Spinner,
    /// The widgets that follow the view's width.
    resizable: Rc<Resizable>,
    /// The document's blocks; the graphics of those not on the page yet
    /// are filled in as they arrive.
    blocks: RefCell<Vec<Block>>,
    /// Every graphic of the document, by id.
    jobs: Vec<Job>,
    /// Which graphics are rendered and which blocks are on the page.
    progress: RefCell<Progress>,
    /// Where each graphic on the page is shown, by id (`None` for graphics
    /// shown as their source, or not on the page yet).
    slots: RefCell<Vec<Option<Slot>>>,
    /// The id of the next graphic to be put on the page.
    next_id: Cell<usize>,
    /// What the window shows (cover, header spinner, nothing).
    phase: Cell<Phase>,
    /// The workers rendering graphics, if any are running.
    pool: RefCell<Option<Pool>>,
    /// Identifies the current rendering run; results of older runs (before
    /// a zoom) are dropped.
    generation: Cell<u64>,
    /// How many graphics of the current run have not arrived yet.
    outstanding: Cell<usize>,
    /// The settings of the current run.
    settings: Cell<RasterSettings>,
    /// The zoom the pictures on the page are currently sized for.
    shown_zoom: Cell<f32>,
    /// A re-render waiting for zooming to settle, if any.
    rezoom: RefCell<Option<glib::SourceId>>,
    /// The spot kept at the top of the window while zooming: a mark at the
    /// start of its line, how far into the line it is, and until when.
    anchor: RefCell<Option<(gtk4::TextMark, f64, std::time::Instant)>>,
    /// Results from the workers.
    receiver: mpsc::Receiver<Delivery>,
    /// The sending end the workers get a copy of.
    sender: Mutex<mpsc::Sender<Delivery>>,
    /// True while results are being collected regularly.
    polling: Cell<bool>,
}

impl Session {
    /// Starts showing `blocks` with graphics rendered with `settings`: the
    /// workers start, and blocks are appended as they complete.
    fn start(self: &Rc<Self>, settings: RasterSettings) {
        self.settings.set(settings);
        self.shown_zoom.set(settings.zoom);
        let jobs = self.jobs.clone();
        self.run(jobs, settings);
        self.append_ready();
        self.update_status();
    }

    /// Starts a new rendering run over `jobs` with `settings`, replacing the
    /// current one.
    fn run(self: &Rc<Self>, jobs: Vec<Job>, settings: RasterSettings) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.outstanding.set(jobs.len());
        let sender = Mutex::new(self.sender.lock().expect("sender").clone());
        let threads = progressive::worker_count(
            std::thread::available_parallelism().map_or(1, usize::from),
            jobs.len(),
        );
        let pool = (!jobs.is_empty()).then(|| {
            Pool::start(jobs, settings, threads, move |id, rendered| {
                if let Ok(sender) = sender.lock() {
                    let _ = sender.send((generation, id, rendered));
                }
            })
        });
        self.pool.replace(pool);
        self.poll_regularly();
    }

    /// Collects results every [`POLL_MS`] until nothing is outstanding.
    fn poll_regularly(self: &Rc<Self>) {
        if self.polling.replace(true) {
            return;
        }
        let session = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(POLL_MS), move || {
            let Some(session) = session.upgrade() else {
                return glib::ControlFlow::Break;
            };
            session.poll();
            if session.busy() {
                glib::ControlFlow::Continue
            } else {
                session.polling.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    /// True while graphics are being rendered, a re-render is waiting, or
    /// blocks are still to be shown.
    fn busy(&self) -> bool {
        self.outstanding.get() > 0
            || self.rezoom.borrow().is_some()
            || !self.progress.borrow().all_shown()
            || self.phase.get() != Phase::Done
    }

    /// Takes every result that has arrived: graphics on the page get their
    /// new picture, the others are stored with their block; then appends
    /// the blocks that became complete.
    fn poll(&self) {
        while let Ok((generation, id, rendered)) = self.receiver.try_recv() {
            if generation != self.generation.get() {
                continue;
            }
            self.outstanding
                .set(self.outstanding.get().saturating_sub(1));
            if self.progress.borrow().is_shown(id) {
                if let (Ok(image), Some(Some(slot))) = (&rendered, self.slots.borrow().get(id)) {
                    slot.apply(image, &self.resizable);
                    if self.anchor.borrow().is_some() {
                        self.hold_anchor();
                    }
                }
            } else {
                progressive::place(&mut self.blocks.borrow_mut(), id, rendered);
                self.progress.borrow_mut().complete(id);
            }
        }
        self.append_ready();
        self.update_status();
    }

    /// Appends every block that is complete and follows the ones shown.
    fn append_ready(&self) {
        let ready = self.progress.borrow_mut().take_ready();
        if ready.is_empty() {
            return;
        }
        for index in ready {
            self.insert_block(index);
        }
        self.resizable.update(self.view.width());
    }

    /// Moves to the next loading phase, revealing the page once the first
    /// screen is full, and shows the header spinner while more is coming.
    fn update_status(&self) {
        // The adjustment's upper bound is never below the window height, so
        // the page's real height is measured at the end of the buffer.
        let window_height = self.scrolled.vadjustment().page_size();
        let (end_top, end_height) = self.view.line_yrange(&self.view.buffer().end_iter());
        let page_height = f64::from(end_top + end_height);
        let all_shown = self.progress.borrow().all_shown();
        let phase = self.phase.get().next(all_shown, page_height, window_height);
        if phase != Phase::FirstScreen && self.cover.reveals_child() {
            self.cover.set_reveal_child(false);
            self.cover.set_can_target(false);
            self.cover_spinner.set_visible(false);
        }
        self.phase.set(phase);
        let loading = phase == Phase::Background
            || (phase == Phase::Done
                && (self.outstanding.get() > 0 || self.rezoom.borrow().is_some()));
        self.header_spinner.set_visible(loading);
    }

    /// Remembers the spot at the top of the window (the line there and how
    /// far into it), unless a spot is already being held, and keeps it in
    /// place for [`layout::ANCHOR_HOLD_MS`] from now (see `keep_anchor`).
    fn hold_anchor(&self) {
        let until = std::time::Instant::now() + Duration::from_millis(layout::ANCHOR_HOLD_MS);
        if let Some(anchor) = self.anchor.borrow_mut().as_mut() {
            anchor.2 = until;
            return;
        }
        let top = self.scrolled.vadjustment().value();
        let (iter, _) = self.view.line_at_y(top as i32);
        let (line_top, line_height) = self.view.line_yrange(&iter);
        let fraction = layout::anchor_fraction(top, line_top.into(), line_height.into());
        let mark = self.view.buffer().create_mark(None, &iter, true);
        self.anchor.replace(Some((mark, fraction, until)));
    }

    /// Scrolls so the held spot (see `hold_anchor`) is at the top of the
    /// window again, as zooming and new pictures change the size of the
    /// lines above it; lets go of it once its time is up.
    fn keep_anchor(&self) {
        let mut anchor = self.anchor.borrow_mut();
        let Some((mark, fraction, until)) = anchor.as_ref() else {
            return;
        };
        let buffer = self.view.buffer();
        if std::time::Instant::now() > *until {
            buffer.delete_mark(mark);
            *anchor = None;
            return;
        }
        let iter = buffer.iter_at_mark(mark);
        let (line_top, line_height) = self.view.line_yrange(&iter);
        let target = layout::anchored_scroll(line_top.into(), line_height.into(), *fraction);
        let adjustment = self.scrolled.vadjustment();
        if (adjustment.value() - target).abs() > 0.5 {
            adjustment.set_value(target);
        }
    }

    /// Lets go of the held spot at once (the user scrolled).
    fn release_anchor(&self) {
        if let Some((mark, _, _)) = self.anchor.take() {
            self.view.buffer().delete_mark(&mark);
        }
    }

    /// Records where the next graphic is shown (`None` if it is shown as
    /// its source).
    fn add_slot(&self, slot: Option<Slot>) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        if let Some(entry) = self.slots.borrow_mut().get_mut(id) {
            *entry = slot;
        }
    }

    /// Appends block `index` to the page.
    fn insert_block(&self, index: usize) {
        let blocks = self.blocks.borrow();
        let buffer = self.view.buffer();
        match &blocks[index] {
            Block::Text(inlines) => self.insert_inlines(&buffer, inlines),
            Block::Table(table) => {
                let offset = buffer.end_iter().offset();
                let widget = self.table_widget(table, offset);
                let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
                self.view.add_child_at_anchor(&widget, &anchor);
                self.resizable.tables.borrow_mut().push(widget);
            }
            Block::Mermaid(diagram) => self.insert_diagram(diagram),
            Block::Code(code) => self.insert_code(code),
        }
    }

    /// Inserts a Mermaid diagram at the end of the buffer: the picture on a
    /// white card, scaled down to fit the view. A diagram that could not be
    /// rendered is shown as its source in a code block, with the reason.
    fn insert_diagram(&self, diagram: &Diagram) {
        let buffer = self.view.buffer();
        match &diagram.image {
            Some(Ok(image)) => {
                let texture = ScaledTexture::new(image);
                let picture = gtk4::Picture::for_paintable(&texture);
                picture.set_can_shrink(true);
                picture.set_content_fit(gtk4::ContentFit::Contain);
                let card = gtk4::Box::builder()
                    .css_classes(["md-diagram"])
                    .halign(gtk4::Align::Start)
                    .build();
                card.append(&picture);
                let offset = buffer.end_iter().offset();
                let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
                self.view.add_child_at_anchor(&card, &anchor);
                let mut diagrams = self.resizable.diagrams.borrow_mut();
                self.add_slot(Some(Slot {
                    texture,
                    rise: None,
                    diagram: Some(diagrams.len()),
                    offset,
                    depth: Cell::new(0.0),
                }));
                diagrams.push((picture, image.logical_width(), image.logical_height()));
            }
            failed => {
                self.add_slot(None);
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

    /// Inserts a code card at the end of the buffer: a header with the
    /// language and a copy button over the code, which scrolls sideways
    /// when its lines are longer than the card is wide.
    fn insert_code(&self, code: &CodeBlock) {
        let card = code_card(code);
        let buffer = self.view.buffer();
        let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
        self.view.add_child_at_anchor(&card, &anchor);
        self.resizable.codes.borrow_mut().push((card, code.indent));
    }

    /// Inserts `inlines` at the end of `buffer`, tagging each with its styles.
    fn insert_inlines(&self, buffer: &gtk4::TextBuffer, inlines: &[Inline]) {
        for inline in inlines {
            match inline {
                Inline::Text(span) => insert_styled(buffer, &span.text, &span.styles),
                Inline::Math(math) => self.insert_math(buffer, math),
            }
        }
    }

    /// Inserts a rendered formula at the end of `buffer`: inline math is
    /// lowered so its baseline lines up with the text, display math is
    /// centred. A formula that could not be rendered is shown as its source
    /// in code style, display math with a note saying why.
    fn insert_math(&self, buffer: &gtk4::TextBuffer, math: &Math) {
        match &math.image {
            Some(Ok(image)) => {
                let start = buffer.end_iter().offset();
                let texture = ScaledTexture::new(image);
                buffer.insert_paintable(&mut buffer.end_iter(), &texture);
                let range = (buffer.iter_at_offset(start), buffer.end_iter());
                let rise = gtk4::TextTag::builder()
                    .rise(layout::math_rise(image.depth, pango::SCALE))
                    .build();
                buffer.tag_table().add(&rise);
                buffer.apply_tag(&rise, &range.0, &range.1);
                if math.display {
                    buffer.apply_tag_by_name(DISPLAY_MATH_TAG, &range.0, &range.1);
                }
                self.add_slot(Some(Slot {
                    texture,
                    rise: Some(rise),
                    diagram: None,
                    offset: start,
                    depth: Cell::new(image.depth),
                }));
            }
            failed => {
                self.add_slot(None);
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

    /// Builds the widget for `table` (at buffer `offset`): a bordered grid
    /// of labels (one per cell, keeping the cells' inline styles), inside a
    /// horizontal scroller that only scrolls when the table is wider than
    /// the document.
    fn table_widget(&self, table: &Table, offset: i32) -> gtk4::ScrolledWindow {
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
                let cell_widget = self.cell_widget(cell, alignment, &classes, offset);
                grid.attach(&cell_widget, column as i32, row_index as i32, 1, 1);
            }
        }
        let scroller = gtk4::ScrolledWindow::builder()
            .child(&grid)
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            // External: no vertical scrollbar, and the child's height
            // (measured without a width, so far too tall for wrapping cells)
            // is not forced on the scroller; `Resizable` sets the real height
            // instead.
            .vscrollbar_policy(gtk4::PolicyType::External)
            .build();
        pass_vertical_scrolls(&scroller, &self.scrolled);
        scroller
    }

    /// Builds one table cell: a wrapping label showing the cell's styled
    /// text as Pango markup, aligned according to the column's alignment.
    /// Long cells wrap at about [`CELL_WRAP_CHARS`] characters instead of
    /// widening the table.
    fn cell_widget(
        &self,
        cell: &[Inline],
        alignment: Align,
        classes: &[&str],
        offset: i32,
    ) -> gtk4::Widget {
        if cell.iter().any(|inline| matches!(inline, Inline::Math(_))) {
            return self.math_cell_widget(cell, alignment, classes, offset);
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
            // Word wrapping keeps a cell at least as wide as its longest
            // word, so short cells are never broken mid-word.
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

    /// Builds a table cell containing formulas: a single (non-wrapping) row
    /// of labels for the text and pictures for the formulas; formulas that
    /// could not be rendered show their source.
    fn math_cell_widget(
        &self,
        cell: &[Inline],
        alignment: Align,
        classes: &[&str],
        offset: i32,
    ) -> gtk4::Widget {
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
                    let texture = ScaledTexture::new(image);
                    let picture = gtk4::Picture::for_paintable(&texture);
                    picture.set_can_shrink(false);
                    picture.set_valign(gtk4::Align::Center);
                    row.append(&picture);
                    self.add_slot(Some(Slot {
                        texture,
                        rise: None,
                        diagram: None,
                        offset,
                        depth: Cell::new(image.depth),
                    }));
                }
                other => {
                    if matches!(other, Inline::Math(_)) {
                        self.add_slot(None);
                    }
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

    /// Shows the document at zoom `level`: pictures are stretched at once,
    /// the rendering run stops, and a new one starts once zooming has
    /// settled for [`REZOOM_DELAY_MS`].
    fn zoom_to(self: &Rc<Self>, level: f32) {
        let ratio = f64::from(level) / f64::from(self.shown_zoom.get());
        self.shown_zoom.set(level);
        for slot in self.slots.borrow().iter().flatten() {
            slot.stretch(ratio, &self.resizable);
        }
        // Stop the current run: its results are for the old zoom.
        self.generation.set(self.generation.get() + 1);
        self.pool.replace(None);
        self.outstanding.set(0);
        if let Some(pending) = self.rezoom.take() {
            pending.remove();
        }
        let session = Rc::downgrade(self);
        let pending = glib::timeout_add_local_once(Duration::from_millis(REZOOM_DELAY_MS), {
            move || {
                if let Some(session) = session.upgrade() {
                    session.rezoom.replace(None);
                    session.rerender();
                }
            }
        });
        self.rezoom.replace(Some(pending));
        self.update_status();
        self.poll_regularly();
    }

    /// Re-renders every graphic at the current zoom: the visible ones
    /// first, then those not on the page yet, then those above the window.
    fn rerender(self: &Rc<Self>) {
        let mut settings = self.settings.get();
        settings.zoom = self.shown_zoom.get();
        self.settings.set(settings);
        let unshown = self.progress.borrow_mut().reset_unshown();
        let shown: Vec<(usize, i32)> = self
            .slots
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| slot.as_ref().map(|slot| (id, slot.offset)))
            .collect();
        let order = progressive::zoom_order(&shown, self.first_visible_offset(), &unshown);
        let jobs = order.into_iter().map(|id| self.jobs[id].clone()).collect();
        self.run(jobs, settings);
        self.update_status();
    }

    /// The buffer offset at the top of the visible part of the view.
    fn first_visible_offset(&self) -> i32 {
        let top = self.view.visible_rect().y();
        self.view.line_at_y(top).0.offset()
    }
}

/// Makes mostly vertical scrolling over `inner` (a table's scroller, which
/// would otherwise take it: an External scrollbar policy still counts as
/// scrollable) scroll the document in `outer` instead. Horizontal
/// scrolling still scrolls the table.
fn pass_vertical_scrolls(inner: &gtk4::ScrolledWindow, outer: &gtk4::ScrolledWindow) {
    let controller = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::BOTH_AXES);
    controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    let outer = outer.downgrade();
    controller.connect_scroll(move |controller, dx, dy| {
        let Some(outer) = outer.upgrade().filter(|_| dy.abs() > dx.abs()) else {
            return glib::Propagation::Proceed;
        };
        let adjustment = outer.vadjustment();
        let pixels = controller.unit() == gdk::ScrollUnit::Surface;
        let delta = layout::scroll_pixels(dy, pixels, adjustment.page_size());
        adjustment.set_value(adjustment.value() + delta);
        glib::Propagation::Stop
    });
    inner.add_controller(controller);
}

/// Builds a code card: a header row with the language (if any) and a copy
/// button, over the code in a horizontal scroller.
fn code_card(code: &CodeBlock) -> gtk4::Box {
    let header = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .css_classes(["md-code-header"])
        .build();
    let language = gtk4::Label::builder()
        .label(code.language.as_deref().unwrap_or(""))
        .css_classes(["dim-label", "caption", "monospace"])
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    header.append(&language);
    header.append(&copy_button(code.clipboard_text()));

    let text = gtk4::Label::builder()
        .label(&code.text)
        .css_classes(["monospace", "md-code-text"])
        .selectable(true)
        .xalign(0.0)
        .halign(gtk4::Align::Start)
        .build();
    let scroller = gtk4::ScrolledWindow::builder()
        .child(&text)
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        // Never: the scroller is exactly as tall as the code, and vertical
        // scrolling passes through to the document.
        .vscrollbar_policy(gtk4::PolicyType::Never)
        .build();
    let card = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .css_classes(["md-code"])
        .halign(gtk4::Align::Start)
        .margin_start(layout::CODE_INDENT * code.indent.min(16) as i32)
        .overflow(gtk4::Overflow::Hidden)
        .build();
    card.append(&header);
    card.append(&scroller);
    card
}

/// A flat button that copies `text` to the clipboard, briefly showing a
/// check mark afterwards.
fn copy_button(text: &str) -> gtk4::Button {
    let button = gtk4::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy")
        .css_classes(["flat"])
        .valign(gtk4::Align::Center)
        .build();
    let text = text.to_string();
    button.connect_clicked(move |button| {
        button.clipboard().set_text(&text);
        button.set_icon_name("object-select-symbolic");
        let button = button.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(layout::COPY_FEEDBACK_MS), move || {
            if let Some(button) = button.upgrade() {
                button.set_icon_name("edit-copy-symbolic");
            }
        });
    });
    button
}

/// Numbers windows, for each window's zoom CSS class.
static WINDOW_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One viewer window: a header bar with an "Open" button over either the
/// empty start page or a document.
struct DocWindow {
    /// The application this window belongs to, for opening new windows.
    app: adw::Application,
    /// The top-level window.
    window: adw::ApplicationWindow,
    /// Holds the header bar; its content is the start page or the document.
    toolbar: adw::ToolbarView,
    /// The header bar spinner, shown while a document is still loading.
    spinner: adw::Spinner,
    /// The document shown, or `None` while the window shows the start page.
    document: RefCell<Option<PathBuf>>,
    /// The document being shown, if any.
    session: RefCell<Option<Rc<Session>>>,
    /// The window's zoom; it carries over to the next document opened in
    /// the window.
    zoom: Cell<Zoom>,
    /// The zoom level at the start of a pinch gesture.
    pinch_start: Cell<f64>,
    /// The CSS class that applies this window's zoom to its document view.
    zoom_class: String,
    /// Holds the CSS for `zoom_class`.
    zoom_css: gtk4::CssProvider,
    /// The text size at 100% zoom, in pixels.
    base_px: f32,
}

impl DocWindow {
    /// Builds and shows a window with the empty start page, and installs
    /// its actions: `win.open` (the header bar button, the start page button
    /// and Ctrl+O) and the zoom actions, plus Ctrl+scroll and pinch zooming.
    fn new(app: &adw::Application) -> Rc<Self> {
        let open_button = gtk4::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open a Document (Ctrl+O)")
            .action_name("win.open")
            .build();
        let spinner = adw::Spinner::builder()
            .tooltip_text("Loading…")
            .visible(false)
            .width_request(16)
            .height_request(16)
            .build();
        let header = adw::HeaderBar::new();
        header.pack_start(&open_button);
        header.pack_end(&spinner);
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
        let base_px = window
            .pango_context()
            .font_description()
            .and_then(|font| {
                layout::font_size_px(font.size(), font.is_size_absolute(), pango::SCALE)
            })
            .unwrap_or(layout::DEFAULT_FONT_PX);
        let number = WINDOW_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let this = Rc::new(DocWindow {
            app: app.clone(),
            window,
            toolbar,
            spinner,
            document: RefCell::new(None),
            session: RefCell::new(None),
            zoom: Cell::new(Zoom::default()),
            pinch_start: Cell::new(1.0),
            zoom_class: format!("md-zoom-{number}"),
            zoom_css: gtk4::CssProvider::new(),
            base_px,
        });
        add_css(
            &this.zoom_css,
            &zoom::css(&this.zoom_class, base_px, this.zoom.get().level()),
        );

        // The open action holds the only strong reference to the window
        // state, so the state lives exactly as long as the window: removing
        // the action when the window closes breaks the window -> action ->
        // state cycle. Everything else holds weak references.
        let open = gio::SimpleAction::new("open", None);
        let state = this.clone();
        open.connect_activate(move |_, _| choose_file(&state));
        this.window.add_action(&open);
        this.install_zoom();
        this.window.connect_close_request(|window| {
            window.remove_action("open");
            glib::Propagation::Proceed
        });
        this.window.present();
        this
    }

    /// Installs the zoom actions, Ctrl+scroll and the pinch gesture.
    fn install_zoom(self: &Rc<Self>) {
        type Change = fn(&mut Zoom) -> bool;
        let actions: [(&str, Change); 3] = [
            ("zoom-in", Zoom::zoom_in),
            ("zoom-out", Zoom::zoom_out),
            ("zoom-reset", Zoom::reset),
        ];
        for (name, change) in actions {
            let action = gio::SimpleAction::new(name, None);
            let state = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(state) = state.upgrade() {
                    state.change_zoom(change);
                }
            });
            self.window.add_action(&action);
        }

        let scroll = gtk4::EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let state = Rc::downgrade(self);
        scroll.connect_scroll(move |controller, _, dy| {
            let control = controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            let Some(state) = state.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if !control {
                if let Some(session) = state.session.borrow().as_ref() {
                    session.release_anchor();
                }
                return glib::Propagation::Proceed;
            }
            let pixels = controller.unit() == gdk::ScrollUnit::Surface;
            let clicks = zoom::wheel_clicks(dy, pixels);
            state.change_zoom(|zoom| zoom.scroll(clicks));
            glib::Propagation::Stop
        });
        self.toolbar.add_controller(scroll);

        let pinch = gtk4::GestureZoom::new();
        let state = Rc::downgrade(self);
        pinch.connect_begin(move |_, _| {
            if let Some(state) = state.upgrade() {
                state.pinch_start.set(state.zoom.get().level());
            }
        });
        let state = Rc::downgrade(self);
        pinch.connect_scale_changed(move |_, scale| {
            if let Some(state) = state.upgrade() {
                let level = state.pinch_start.get() * scale;
                state.change_zoom(|zoom| zoom.set(level));
            }
        });
        self.toolbar.add_controller(pinch);
    }

    /// Applies `change` to the zoom; if the level changed, resizes the text
    /// and re-renders the document's graphics, keeping the line at the top
    /// of the window in place.
    fn change_zoom(&self, change: impl FnOnce(&mut Zoom) -> bool) {
        let mut zoom = self.zoom.get();
        if !change(&mut zoom) {
            self.zoom.set(zoom);
            return;
        }
        self.zoom.set(zoom);
        let session = self.session.borrow().clone();
        if let Some(session) = &session {
            session.hold_anchor();
        }
        self.zoom_css
            .load_from_string(&zoom::css(&self.zoom_class, self.base_px, zoom.level()));
        if let Some(session) = session {
            session.zoom_to(zoom.level() as f32);
        }
    }

    /// Replaces the window's content with a fresh document view titled
    /// after `path`, covered by a loading spinner, and returns the session
    /// for `blocks`.
    fn show_document(&self, path: &Path, mut blocks: Vec<Block>) -> Rc<Session> {
        if let Some(old) = self.session.take() {
            old.pool.replace(None);
        }
        let view = gtk4::TextView::builder()
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk4::WrapMode::WordChar)
            .top_margin(12)
            .bottom_margin(12)
            .left_margin(layout::DOCUMENT_MARGIN)
            .right_margin(layout::DOCUMENT_MARGIN)
            .css_classes([self.zoom_class.as_str()])
            .build();
        create_tags(&view.buffer().tag_table());
        let scrolled = gtk4::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .child(&view)
            .build();
        let cover_spinner = adw::Spinner::builder()
            .width_request(48)
            .height_request(48)
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .visible(false)
            .build();
        let cover_box = gtk4::Box::builder()
            .css_classes(["md-loading-cover"])
            .build();
        cover_box.append(&cover_spinner);
        let cover = gtk4::Revealer::builder()
            .transition_type(gtk4::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .child(&cover_box)
            .build();
        let overlay = gtk4::Overlay::builder().child(&scrolled).build();
        overlay.add_overlay(&cover);
        self.toolbar.set_content(Some(&overlay));
        self.window.set_title(Some(&md_viewer::window_title(path)));
        self.document.replace(Some(path.to_path_buf()));

        let jobs = progressive::jobs(&mut blocks);
        let (sender, receiver) = mpsc::channel();
        let session = Rc::new(Session {
            view,
            scrolled,
            cover,
            cover_spinner,
            header_spinner: self.spinner.clone(),
            resizable: Rc::new(Resizable::default()),
            progress: RefCell::new(Progress::new(blocks.len(), &jobs)),
            slots: RefCell::new((0..jobs.len()).map(|_| None).collect()),
            blocks: RefCell::new(blocks),
            jobs,
            next_id: Cell::new(0),
            phase: Cell::new(Phase::FirstScreen),
            pool: RefCell::new(None),
            generation: Cell::new(0),
            outstanding: Cell::new(0),
            settings: Cell::new(RasterSettings::default()),
            shown_zoom: Cell::new(1.0),
            rezoom: RefCell::new(None),
            anchor: RefCell::new(None),
            receiver,
            sender: Mutex::new(sender),
            polling: Cell::new(false),
        });
        {
            let weak = Rc::downgrade(&session);
            session.view.add_tick_callback(move |view, _| {
                if let Some(session) = weak.upgrade() {
                    session.resizable.update(view.width());
                    session.keep_anchor();
                }
                glib::ControlFlow::Continue
            });
        }
        let weak: Weak<Session> = Rc::downgrade(&session);
        glib::timeout_add_local_once(
            Duration::from_millis(progressive::SPINNER_DELAY_MS),
            move || {
                if let Some(session) = weak.upgrade() {
                    if session.phase.get() == Phase::FirstScreen {
                        session.cover_spinner.set_visible(true);
                    }
                }
            },
        );
        self.session.replace(Some(session.clone()));
        session
    }

    /// The settings graphics are rendered with for `view`: the window's
    /// text size, the display's scale factor, the theme's text colour and
    /// the window's zoom.
    fn raster_settings(&self, view: &gtk4::TextView) -> RasterSettings {
        let color = view.color();
        let channel = layout::color_channel;
        RasterSettings {
            font_px: self.base_px,
            scale: view.scale_factor().max(1) as f32,
            color: [
                channel(color.red()),
                channel(color.green()),
                channel(color.blue()),
            ],
            zoom: self.zoom.get().level() as f32,
        }
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
/// The file is read and parsed on the I/O thread pool first, so a failure
/// is reported (see `report_open_error`) before any window changes. The
/// document is then shown in `window` or a new window
/// (`md_viewer::open_target`), and its graphics are rendered by the worker
/// pool while its blocks are appended in order (see [`Session`]).
fn open_file(window: &Rc<DocWindow>, path: PathBuf, origin: Origin) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let worker_path = path.clone();
        let parsed = gio::spawn_blocking(move || {
            md_viewer::read_document(&worker_path).map(|text| md_viewer::render(&text))
        })
        .await;
        let blocks = match parsed {
            Ok(Ok(blocks)) => blocks,
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
        let session = target.show_document(&path, blocks);
        session.start(target.raster_settings(&session.view));
    });
}

/// Inserts `text` at the end of `buffer` with the tags for `styles`.
fn insert_styled(buffer: &gtk4::TextBuffer, text: &str, styles: &[Style]) {
    let names: Vec<&str> = styles.iter().map(|style| style.tag_name()).collect();
    buffer.insert_with_tags_by_name(&mut buffer.end_iter(), text, &names);
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
/// and diagrams stay sharp on HiDPI displays. Its picture and size can be
/// replaced (zooming), and the widgets showing it follow.
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
        let paintable: Self = glib::Object::new();
        paintable.store(image);
        paintable
    }

    /// Stores `image` as the texture and its logical size.
    fn store(&self, image: &Image) {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        let texture = gdk::MemoryTexture::new(
            image.width as i32,
            image.height as i32,
            gdk::MemoryFormat::R8g8b8a8Premultiplied,
            &glib::Bytes::from(&image.pixels),
            image.width as usize * 4,
        );
        let imp = self.imp();
        imp.texture.replace(Some(texture.upcast()));
        imp.width.set(image.logical_width() as f64);
        imp.height.set(image.logical_height() as f64);
    }

    /// Shows `image` instead, at its logical size.
    fn set_image(&self, image: &Image) {
        self.store(image);
        self.invalidate_size();
        self.invalidate_contents();
    }

    /// Scales the logical size by `ratio`, stretching the current texture,
    /// and returns the new size.
    fn scale_size(&self, ratio: f64) -> (f32, f32) {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        let imp = self.imp();
        imp.width.set(imp.width.get() * ratio);
        imp.height.set(imp.height.get() * ratio);
        self.invalidate_size();
        self.invalidate_contents();
        (imp.width.get() as f32, imp.height.get() as f32)
    }
}

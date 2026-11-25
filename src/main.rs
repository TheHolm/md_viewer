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
use md_viewer::graphics::{self, Image, RasterSettings, Rendered};
use md_viewer::links::{self, Anchors, LinkAction, LinkRanges};
use md_viewer::progressive::{self, Job, Phase, Pool, Progress};
use md_viewer::search::{self, Matcher, SearchMode, Unit};
use md_viewer::zoom::{self, Zoom};
use md_viewer::{
    label_text, Align, Block, Diagram, ImageRef, ImageState, Inline, LabelLink, LabelText, Math,
    Origin, Style, StyledRun, Table, Target, APP_ID,
};
use md_viewer::{layout, CodeBlock};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

/// The name of the text tag that centres display math on its line.
const DISPLAY_MATH_TAG: &str = "math-display";

/// The text tag marking a matched search string.
const SEARCH_MATCH_TAG: &str = "search-match";

/// The text tag marking the current search match.
const SEARCH_CURRENT_TAG: &str = "search-current";

/// The colour a matched search string is drawn in, as 8-bit RGB
/// (`#rrggbb`); Pango attributes need `layout::pango_channel` to widen them
/// to 16 bits.
const SEARCH_MATCH_RGB: (u8, u8, u8) = (255, 213, 0);
const SEARCH_CURRENT_RGB: (u8, u8, u8) = (255, 145, 0);

/// The text colour drawn on a highlighted label (black on the light
/// background, readable in both themes).
const SEARCH_TEXT_RGB: (u8, u8, u8) = (0, 0, 0);

/// Roughly how many characters wide a table cell grows before its text wraps.
const CELL_WRAP_CHARS: i32 = 40;

/// How often rendered graphics are collected from the workers while a
/// document loads, in milliseconds (about once a frame).
const POLL_MS: u64 = 16;

/// How long after the last zoom step graphics are re-rendered, in
/// milliseconds; until then they are shown stretched.
const REZOOM_DELAY_MS: u64 = 150;

/// What the placeholder for an image with a network destination says: the
/// viewer never fetches anything, and that is not planned.
const NETWORK_UNSUPPORTED: &str = "network loading is not supported";

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
.md-diagram.md-search-hit {
  box-shadow: 0 0 0 2px alpha(#f5c211, 0.55);
}
.md-diagram.md-search-current {
  box-shadow: 0 0 0 3px alpha(#f08c00, 0.65);
}
.md-image.md-search-hit {
  box-shadow: 0 0 0 2px alpha(#f5c211, 0.55);
}
.md-image.md-search-current {
  box-shadow: 0 0 0 3px alpha(#f08c00, 0.65);
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
        app.set_accels_for_action("win.find", &["<Control>f"]);
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
    /// Pictures (diagrams and standalone images) with their natural logical
    /// size (at the current zoom); scaled down (keeping the aspect ratio) when
    /// the view is narrower.
    pictures: RefCell<Vec<(gtk4::Picture, f32, f32)>>,
    /// Code cards with their nesting depth, as wide as the view allows.
    codes: RefCell<Vec<(gtk4::Box, usize)>>,
}

impl Resizable {
    /// Re-sizes every registered widget for a view `width` pixels wide;
    /// returns true if any size changed.
    fn update(&self, width: i32) -> bool {
        let mut changed = false;
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
                changed = true;
            }
        }
        for (picture, natural_width, natural_height) in self.pictures.borrow().iter() {
            let size = layout::diagram_size(*natural_width, *natural_height, width);
            if picture.size_request() != size {
                picture.set_size_request(size.0, size.1);
                changed = true;
            }
        }
        for (card, indent) in self.codes.borrow().iter() {
            let card_width = layout::code_card_width(width, *indent);
            if card.size_request() != (card_width, -1) {
                card.set_size_request(card_width, -1);
                changed = true;
            }
        }
        changed
    }
}

/// Where a rendered graphic is shown, so a re-render (after a zoom) can
/// replace its picture in place.
struct Slot {
    /// The paintable showing the graphic.
    texture: ScaledTexture,
    /// The tag lowering an inline formula onto the baseline, if any.
    rise: Option<gtk4::TextTag>,
    /// The picture's entry in [`Resizable::pictures`], for a diagram or a
    /// standalone image (which are sized to the view width).
    picture: Option<usize>,
    /// The buffer offset the graphic (or its table) is at.
    offset: i32,
    /// How far the graphic reaches below the baseline, in pixels.
    depth: Cell<f32>,
    /// The bytes of pixels the graphic holds (0 once dropped).
    bytes: Cell<usize>,
    /// The bytes of its pixels when last drawn (kept when dropped).
    size: Cell<usize>,
    /// Identifies its pixel buffer (shared between graphics showing the
    /// same rendering, which the budget counts once).
    key: Cell<usize>,
    /// When it was last drawn (see [`Session::arrivals`]).
    arrived: Cell<u64>,
    /// Whether it holds its pixels, or was dropped to stay within the
    /// document's memory budget.
    held: Cell<Held>,
}

/// Whether a graphic on the page holds its pixels (see
/// [`Session::enforce_budget`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// Drawn with its pixels.
    Drawn,
    /// Dropped: a spinner of the same size stands in until it is near the
    /// view again.
    Dropped,
    /// Dropped, and asked to be rendered again in the run with this
    /// generation.
    Requested(u64),
    /// Not drawn because the graphics near the view already fill the
    /// document's budget; says so in its place.
    Limited,
}

/// What a graphic over the document's memory budget shows in its place.
const DOCUMENT_LIMIT_MESSAGE: &str = "can't render: document memory limit reached";

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

    /// Updates the baseline offset and the picture's natural size after the
    /// picture changed size.
    fn update_extras(&self, width: f32, height: f32, resizable: &Resizable) {
        if let Some(rise) = &self.rise {
            rise.set_rise(layout::math_rise(self.depth.get(), pango::SCALE));
        }
        if let Some(index) = self.picture {
            if let Some(entry) = resizable.pictures.borrow_mut().get_mut(index) {
                entry.1 = width;
                entry.2 = height;
            }
        }
    }
}

/// One rendered result from a worker: generation, graphic id and result.
type Delivery = (u64, usize, Rendered);

/// What the window shows for a search, to update the find bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SearchSummary {
    /// The number of matches found.
    total: usize,
    /// The 1-based number of the current match (0 when there is none).
    current: usize,
    /// True when the query could not be matched (an invalid regex).
    error: bool,
    /// True while content that could still add matches (diagram labels, or
    /// blocks not yet rendered) is still being drawn.
    pending: bool,
}

/// Where a match can be highlighted.
enum SearchTarget {
    /// A label (a table cell or a code card) whose substrings can be marked.
    /// `runs` are the base style runs of the label's text, used to rebuild
    /// the label's attribute list.
    Label {
        label: gtk4::Label,
        runs: Vec<StyledRun>,
    },
    /// A whole diagram card, marked by a CSS class.
    Diagram(gtk4::Box),
    /// A whole image (a standalone paragraph picture or a table-cell picture)
    /// in its box, marked by the same CSS class as a diagram.
    Image(gtk4::Box),
    /// Text with nothing on the page to highlight (an inline paragraph image,
    /// which is a paintable, not a widget): a match still counts and gets a
    /// scrollbar mark, but shows no highlight.
    Unhighlighted,
}

/// One searchable piece of text, in document order.
struct SearchEntry {
    /// The text searched.
    text: String,
    /// The buffer offset the piece is at (its anchor), for ordering and
    /// scrolling.
    offset: i32,
    /// The order within the same offset, so ties keep document order.
    order: usize,
    /// What to highlight.
    target: SearchTarget,
    /// True while the label carries search highlights, so they can be
    /// cleared back to the base styling when the query changes.
    highlighted: Cell<bool>,
}

/// A found match and where it is.
struct SearchMatch {
    /// The buffer offset, for ordering, scrolling and scrollbar marks.
    offset: i32,
    /// The order within the same offset.
    order: usize,
    /// The part of the document the match is in.
    target: MatchTarget,
}

/// The part of the document a match is in.
enum MatchTarget {
    /// A range of the text buffer, in character offsets.
    Buffer { chars: std::ops::Range<i32> },
    /// A byte range in entry `entry`'s text.
    Entry {
        entry: usize,
        range: std::ops::Range<usize>,
    },
    /// Text in a block not on the page yet (searched from the parsed
    /// document, so it counts before it is rendered).
    Block { block: usize },
}

/// The state of the find bar for one session.
#[derive(Default)]
struct Search {
    /// The query as typed.
    query: String,
    /// How the query is matched; kept when the query is cleared.
    mode: SearchMode,
    /// The compiled query, `None` when there is no query or it is invalid.
    matcher: Option<Matcher>,
    /// True when the query could not be matched (an invalid regex).
    error: bool,
    /// Every searchable widget on the page, in document order.
    entries: Vec<SearchEntry>,
    /// The render-independent text of the whole parsed document, in document
    /// order, so matches in blocks not yet rendered are found too.
    units: Vec<Unit>,
    /// Every match, sorted by document position.
    matches: Vec<SearchMatch>,
    /// The index in `matches` of the current match, if any.
    current: Option<usize>,
    /// The next `order` to hand out, for ordering within one offset.
    next_order: usize,
}

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
    /// How many graphics (a prefix of [`Session::jobs`], in document order)
    /// have been submitted for the current run. Graphics beyond it are left
    /// unrendered until the user scrolls near them.
    frontier: Cell<usize>,
    /// How many worker threads the pool is started with (one per CPU, capped
    /// at the number of graphics); kept for re-creating the pool.
    threads: usize,
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
    /// The state of the find bar for this document.
    search: RefCell<Search>,
    /// The strip over the scrollbar that marks matches.
    marks: gtk4::DrawingArea,
    /// Reports the outcome of a search to the window.
    search_changed: Box<dyn Fn(SearchSummary)>,
    /// The path of the document, for resolving relative link destinations.
    document: PathBuf,
    /// The clickable ranges of the text buffer, in character offsets.
    links: RefCell<LinkRanges>,
    /// Each heading's anchor and the buffer offset of its first character.
    anchors: RefCell<Anchors>,
    /// An anchor requested before its heading was appended; scrolled to as
    /// soon as the heading appears.
    pending_anchor: RefCell<Option<String>>,
    /// When the search was last refreshed (see [`Session::request_refresh`]).
    last_refresh: Cell<Option<std::time::Instant>>,
    /// True while a delayed search refresh is waiting.
    refresh_waiting: Cell<bool>,
    /// True while the frame callback runs (see [`Session::wake_ticks`]).
    ticking: Cell<bool>,
    /// How many frames in a row the frame callback found nothing to do.
    idle_frames: Cell<u32>,
    /// Every heading of the document by anchor, with its block index (see
    /// `links::heading_blocks`), so a link can render up to a heading not
    /// on the page yet.
    heading_blocks: Anchors,
    /// The largest zoom every graphic shown so far can be drawn at (see
    /// `Image::max_zoom`); infinite until one is shown.
    zoom_limit: Cell<f64>,
    /// True when `zoom_limit` was lowered and the window has not been told.
    limit_dirty: Cell<bool>,
    /// The bytes of pixels the graphics on the page hold (see
    /// [`Session::enforce_budget`]).
    held_bytes: Cell<usize>,
    /// The most bytes of pixels the page may hold: always
    /// [`graphics::DOCUMENT_BUDGET`], except in tests.
    budget: Cell<usize>,
    /// True while a [`Session::schedule_budget`] run is waiting.
    budget_pending: Cell<bool>,
    /// How many times a graphic was drawn, numbering them for the budget
    /// (newest first when limiting).
    arrivals: Cell<u64>,
    /// Tells the window the document's new zoom limit.
    limit_changed: Box<dyn Fn(f64)>,
    /// A block (searched before it was rendered) to scroll to once it is on
    /// the page.
    pending_scroll: RefCell<Option<usize>>,
    /// Opens a resolved link that is not a same-document anchor (the window
    /// launches external links and opens linked Markdown documents).
    open_link: Box<dyn Fn(LinkAction)>,
    /// A weak handle to this session, set once it is wrapped in `Rc`, so
    /// callbacks attached to child widgets (table-cell labels) can reach it.
    weak_self: RefCell<Option<Weak<Session>>>,
}

impl Session {
    /// Starts showing `blocks` with graphics rendered with `settings`: the
    /// first batch of graphics is submitted, and blocks are appended as they
    /// complete (more graphics are submitted as the user scrolls near the
    /// end of what is rendered, see [`Session::advance_frontier`]).
    fn start(self: &Rc<Self>, settings: RasterSettings) {
        self.settings.set(settings);
        let text_limit = layout::text_zoom_limit(settings.font_px, settings.scale, settings.max_px);
        if text_limit < self.zoom_limit.get() {
            self.zoom_limit.set(text_limit);
        }
        self.limit_dirty.set(true);
        self.shown_zoom.set(settings.zoom);
        self.frontier.set(0);
        self.generation.set(self.generation.get() + 1);
        self.outstanding.set(0);
        self.pool.replace(None);
        self.append_ready();
        self.update_status();
        self.advance_frontier();
        self.report_limit();
    }

    /// Submits `jobs` to the worker pool (creating the pool if this run has
    /// none yet), counting them as outstanding and collecting their results.
    fn submit_batch(self: &Rc<Self>, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        self.outstanding.set(self.outstanding.get() + jobs.len());
        if self.pool.borrow().is_none() {
            let generation = self.generation.get();
            let settings = self.settings.get();
            let threads = self.threads;
            let sender = Mutex::new(self.sender.lock().expect("sender").clone());
            let pool = Pool::start(jobs, settings, threads, move |id, rendered| {
                if let Ok(sender) = sender.lock() {
                    let _ = sender.send((generation, id, rendered));
                }
            });
            self.pool.replace(Some(pool));
        } else if let Some(pool) = self.pool.borrow().as_ref() {
            pool.submit(jobs);
        }
        self.poll_regularly();
    }

    /// Submits the next batch of graphics when the rendered content is too
    /// short for the view: either nothing has been submitted yet, or the
    /// bottom of the rendered content is closer than
    /// [`layout::FRONTIER_LOOKAHEAD`] window heights below the viewport.
    fn advance_frontier(self: &Rc<Self>) {
        if self.rezoom.borrow().is_some() || self.frontier.get() >= self.jobs.len() {
            return;
        }
        if !self.need_more() {
            return;
        }
        self.submit_next_batch();
    }

    /// Submits the next [`layout::FRONTIER_BATCH`] graphics, whatever the
    /// view, advancing the frontier.
    fn submit_next_batch(self: &Rc<Self>) {
        if self.frontier.get() >= self.jobs.len() {
            return;
        }
        let start = self.frontier.get();
        let end = (start + layout::FRONTIER_BATCH).min(self.jobs.len());
        let jobs = self.jobs[start..end].to_vec();
        self.frontier.set(end);
        self.submit_batch(jobs);
    }

    /// Renders up to and including `block`, so a search match there can be
    /// shown (see [`Session::scroll_current`]).
    fn render_to(self: &Rc<Self>, block: usize) {
        if self.rezoom.borrow().is_some() {
            return;
        }
        while self.frontier.get() < self.jobs.len() {
            if self.frontier.get() > 0 && self.jobs[self.frontier.get() - 1].block >= block {
                break;
            }
            self.submit_next_batch();
        }
    }

    /// True when more graphics should be rendered: the first batch has not
    /// been submitted yet, or the rendered content is not yet
    /// [`layout::FRONTIER_LOOKAHEAD`] window heights past the bottom of the
    /// viewport.
    fn need_more(&self) -> bool {
        if self.frontier.get() == 0 {
            return true;
        }
        let adjustment = self.scrolled.vadjustment();
        let page = adjustment.page_size();
        let viewport_bottom = adjustment.value() + page;
        let (top, height) = self.view.line_yrange(&self.view.buffer().end_iter());
        let content_bottom = f64::from(top + height);
        layout::needs_more(
            content_bottom,
            viewport_bottom,
            page,
            layout::FRONTIER_LOOKAHEAD,
        )
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

    /// True while graphics are being rendered or a re-render is waiting.
    fn busy(&self) -> bool {
        self.outstanding.get() > 0 || self.rezoom.borrow().is_some()
    }

    /// Takes every result that has arrived: graphics on the page get their
    /// new picture, the others are stored with their block; then appends
    /// the blocks that became complete and, if the page is still too short,
    /// submits more graphics.
    fn poll(self: &Rc<Self>) {
        while let Ok((generation, id, rendered)) = self.receiver.try_recv() {
            if generation != self.generation.get() {
                continue;
            }
            self.outstanding
                .set(self.outstanding.get().saturating_sub(1));
            if self.progress.borrow().is_shown(id) {
                if let (Ok(image), Some(Some(slot))) = (&rendered, self.slots.borrow().get(id)) {
                    self.redraw_slot(slot, image);
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
        // Once the new lines are laid out, so distances from the view are
        // real.
        self.schedule_budget();
        self.update_status();
        self.advance_frontier();
        self.report_limit();
    }

    /// Appends every block that is complete and follows the ones shown.
    fn append_ready(&self) {
        let ready = self.progress.borrow_mut().take_ready();
        if ready.is_empty() {
            return;
        }
        self.wake_ticks();
        for index in ready {
            let offset = self.view.buffer().end_iter().offset();
            self.insert_block(index);
            progressive::clear_graphics(&mut self.blocks.borrow_mut()[index]);
            if self.pending_scroll.borrow().as_ref() == Some(&index) {
                self.pending_scroll.replace(None);
                let mut iter = self.view.buffer().iter_at_offset(offset);
                self.view.scroll_to_iter(&mut iter, 0.15, true, 0.0, 0.3);
            }
        }
        self.resizable.update(self.view.width());
        self.request_refresh();
    }

    /// Refreshes an active search after blocks were appended, at most once
    /// every [`layout::SEARCH_REFRESH_MS`] while the document loads (each
    /// refresh redoes the whole search) and at once when the last block is
    /// on the page; requests in between are merged into one later refresh.
    fn request_refresh(&self) {
        if self.search.borrow().query.is_empty() {
            return;
        }
        let done = self.progress.borrow().all_shown();
        let since = self.last_refresh.get().map(|at| at.elapsed());
        match layout::refresh_delay(since, done) {
            None => self.refresh_now(),
            Some(_) if self.refresh_waiting.get() => {}
            Some(delay) => {
                self.refresh_waiting.set(true);
                let weak = self.weak_self.borrow().clone();
                glib::timeout_add_local_once(delay, move || {
                    if let Some(session) = weak.as_ref().and_then(Weak::upgrade) {
                        session.refresh_waiting.set(false);
                        session.refresh_now();
                    }
                });
            }
        }
    }

    /// Refreshes the search now, noting when.
    fn refresh_now(&self) {
        self.last_refresh.set(Some(std::time::Instant::now()));
        self.refresh_search();
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
        let loading = phase != Phase::FirstScreen
            && (self.outstanding.get() > 0 || self.rezoom.borrow().is_some());
        self.header_spinner.set_visible(loading);
    }

    /// Remembers the spot at the top of the window (the line there and how
    /// far into it), unless a spot is already being held, and keeps it in
    /// place for [`layout::ANCHOR_HOLD_MS`] from now (see `keep_anchor`).
    fn hold_anchor(&self) {
        self.wake_ticks();
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

    /// Runs the per-frame work (sizing tables, pictures and code cards to
    /// the view, keeping the held spot in place) from the next frame on,
    /// until [`layout::IDLE_FRAMES`] frames in a row change nothing and no
    /// spot is held; then the frame callback stops, so an idle window costs
    /// nothing. Called whenever something it acts on may have changed: the
    /// view's size or content, a new or redrawn graphic, a zoom.
    fn wake_ticks(&self) {
        self.idle_frames.set(0);
        if self.ticking.replace(true) {
            return;
        }
        let weak = self.weak_self.borrow().clone();
        self.view.add_tick_callback(move |view, _| {
            let Some(session) = weak.as_ref().and_then(Weak::upgrade) else {
                return glib::ControlFlow::Break;
            };
            let resized = session.resizable.update(view.width());
            session.keep_anchor();
            let busy = resized || session.anchor.borrow().is_some();
            let idle = if busy {
                0
            } else {
                session.idle_frames.get() + 1
            };
            session.idle_frames.set(idle);
            if idle >= layout::IDLE_FRAMES {
                session.ticking.set(false);
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    /// Lets go of the held spot at once (the user scrolled).
    fn release_anchor(&self) {
        if let Some((mark, _, _)) = self.anchor.take() {
            self.view.buffer().delete_mark(&mark);
        }
    }

    /// Records where the next graphic, rendered as `image`, is shown, and
    /// lowers the document's zoom limit to the image's.
    fn add_graphic(&self, image: &Image, slot: Slot) {
        self.note_limit(image);
        self.hold_pixels(&slot, image);
        self.add_slot(Some(slot));
    }

    /// Notes that `slot` now shows `image`'s pixels (see
    /// [`Session::enforce_budget`]).
    fn hold_pixels(&self, slot: &Slot, image: &Image) {
        slot.bytes.set(image.pixels.len());
        slot.size.set(image.pixels.len());
        slot.key.set(std::sync::Arc::as_ptr(&image.pixels) as usize);
        let arrived = self.arrivals.get() + 1;
        self.arrivals.set(arrived);
        slot.arrived.set(arrived);
        slot.held.set(Held::Drawn);
    }

    /// Shows `image` (a re-render) in `slot`.
    fn redraw_slot(&self, slot: &Slot, image: &Image) {
        self.wake_ticks();
        self.note_limit(image);
        slot.apply(image, &self.resizable);
        self.hold_pixels(slot, image);
    }

    /// The graphics on the page holding their pixels, as the budget sees
    /// them (`progressive::Held`).
    fn held_graphics(&self) -> Vec<progressive::Held> {
        self.slots
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| {
                let slot = slot.as_ref()?;
                (slot.held.get() == Held::Drawn).then(|| progressive::Held {
                    id,
                    key: slot.key.get(),
                    bytes: slot.bytes.get(),
                    distance: self.distance_from_view(slot.offset),
                    arrived: slot.arrived.get(),
                })
            })
            .collect()
    }

    /// Keeps the page's pixels within the document's budget
    /// ([`graphics::DOCUMENT_BUDGET`]), each shared buffer counted once
    /// (`progressive::plan_budget`): the graphics farthest from the view
    /// are dropped, keeping their size and showing a spinner until they are
    /// near the view again ([`Session::redraw_near`]); graphics on or
    /// within [`layout::KEEP_SCREENS`] screens of the view are never
    /// dropped. If those alone exceed the budget, the newest of them show
    /// "can't render: document memory limit reached" instead, and are tried
    /// again once there is room.
    fn enforce_budget(&self) {
        let held = self.held_graphics();
        self.held_bytes.set(progressive::held_bytes(&held));
        if self.held_bytes.get() <= self.budget.get() {
            return;
        }
        let (drops, limited) = progressive::plan_budget(&held, self.budget.get());
        {
            let slots = self.slots.borrow();
            let release = |id: usize, state: Held| {
                if let Some(Some(slot)) = slots.get(id) {
                    match state {
                        Held::Limited => slot
                            .texture
                            .show_message(self.view.upcast_ref(), DOCUMENT_LIMIT_MESSAGE),
                        _ => slot.texture.show_spinner(self.view.upcast_ref()),
                    }
                    slot.bytes.set(0);
                    slot.held.set(state);
                }
            };
            for id in drops {
                release(id, Held::Dropped);
            }
            for id in limited {
                release(id, Held::Limited);
            }
        }
        self.held_bytes
            .set(progressive::held_bytes(&self.held_graphics()));
    }

    /// Runs [`Session::enforce_budget`] and [`Session::redraw_near`] once
    /// the main loop is idle: the page's layout (and so where each graphic
    /// is) settles after scrolling and as lines are measured, and the
    /// adjustment signals that report it may come in the middle of a
    /// layout pass. Several requests before then run once.
    fn schedule_budget(self: &Rc<Self>) {
        if self.budget_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(session) = weak.upgrade() {
                session.budget_pending.set(false);
                session.enforce_budget();
                session.redraw_near();
            }
        });
    }

    /// Asks again for the dropped graphics that are now on or near the
    /// view (within [`layout::KEEP_SCREENS`] screens), so they are drawn
    /// before they are seen, and for graphics over the document limit once
    /// they fit. A request from an earlier run (since cancelled by a zoom)
    /// is made again.
    fn redraw_near(self: &Rc<Self>) {
        if self.rezoom.borrow().is_some() {
            return;
        }
        let generation = self.generation.get();
        let mut room = self.budget.get().saturating_sub(self.held_bytes.get());
        let ids: Vec<usize> = self
            .slots
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| {
                let slot = slot.as_ref()?;
                let wanted = match slot.held.get() {
                    Held::Drawn => false,
                    Held::Dropped => true,
                    Held::Requested(asked) => asked != generation,
                    // Only once it would fit, so it is not refused again.
                    Held::Limited => room >= slot.size.get(),
                };
                if !(wanted && self.distance_from_view(slot.offset) == 0.0) {
                    return None;
                }
                if slot.held.get() == Held::Limited {
                    room -= slot.size.get();
                }
                slot.held.set(Held::Requested(generation));
                Some(id)
            })
            .collect();
        let jobs = ids.into_iter().map(|id| self.jobs[id].clone()).collect();
        self.submit_batch(jobs);
    }

    /// How far (in pixels) the line at buffer `offset` is from the view
    /// widened by [`layout::KEEP_SCREENS`] window heights each way; 0 when
    /// it is within that.
    fn distance_from_view(&self, offset: i32) -> f64 {
        let adjustment = self.scrolled.vadjustment();
        let page = adjustment.page_size();
        let (top, height) = self
            .view
            .line_yrange(&self.view.buffer().iter_at_offset(offset));
        layout::distance_from_view(
            f64::from(top),
            f64::from(top + height),
            adjustment.value(),
            adjustment.value() + page,
            page * layout::KEEP_SCREENS,
        )
    }

    /// Lowers the document's zoom limit to `image`'s, if that is lower; the
    /// window hears of it from [`Session::report_limit`].
    fn note_limit(&self, image: &Image) {
        let limit = f64::from(image.max_zoom);
        if limit < self.zoom_limit.get() {
            self.zoom_limit.set(limit);
            self.limit_dirty.set(true);
        }
    }

    /// Tells the window the document's zoom limit if it changed since the
    /// last report. Called once nothing on the page is borrowed (the window
    /// may zoom out at once, which re-renders and restretches the graphics).
    fn report_limit(&self) {
        if self.limit_dirty.replace(false) {
            (self.limit_changed)(self.zoom_limit.get());
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
            Block::Text(inlines) => {
                self.insert_inlines(&buffer, inlines);
            }
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
                let labels = image
                    .labels
                    .iter()
                    .filter(|label| !label.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                self.add_diagram_entry(&card, labels, offset);
                let mut pictures = self.resizable.pictures.borrow_mut();
                self.add_graphic(
                    image,
                    Slot {
                        texture,
                        rise: None,
                        picture: Some(pictures.len()),
                        offset,
                        depth: Cell::new(0.0),
                        bytes: Cell::new(0),
                        size: Cell::new(0),
                        key: Cell::new(0),
                        arrived: Cell::new(0),
                        held: Cell::new(Held::Drawn),
                    },
                );
                pictures.push((picture, image.logical_width(), image.logical_height()));
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
        let (card, text) = code_card(code);
        let buffer = self.view.buffer();
        let offset = buffer.end_iter().offset();
        let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
        self.view.add_child_at_anchor(&card, &anchor);
        self.add_label_entry(
            &text,
            LabelText {
                text: code.text.clone(),
                runs: Vec::new(),
                links: Vec::new(),
            },
            offset,
        );
        self.resizable.codes.borrow_mut().push((card, code.indent));
    }

    /// Inserts `inlines` at the end of `buffer`, tagging each with its styles.
    /// An image alone in the block is shown as a picture scaled to the view
    /// width; one among text is shown inline.
    fn insert_inlines(&self, buffer: &gtk4::TextBuffer, inlines: &[Inline]) {
        let standalone: Vec<bool> = (0..inlines.len())
            .map(|at| {
                matches!(inlines[at], Inline::Image(_))
                    && md_viewer::image_is_standalone(inlines, at)
            })
            .collect();
        let mut headings = links::heading_starts(inlines).into_iter().peekable();
        for (at, inline) in inlines.iter().enumerate() {
            if let Some((_, text)) = headings.next_if(|(start, _)| *start == at) {
                self.record_heading(&text, buffer.end_iter().offset());
            }
            match inline {
                Inline::Text(span) => {
                    let start = buffer.end_iter().offset();
                    insert_styled(buffer, &span.text, &span.styles);
                    if let Some(dest) = &span.link {
                        let end = buffer.end_iter().offset();
                        self.links
                            .borrow_mut()
                            .push(start as usize, end as usize, dest.clone());
                    }
                }
                Inline::Math(math) => self.insert_math(buffer, math),
                Inline::Image(image) => self.insert_image(buffer, image, standalone[at]),
            }
        }
    }

    /// Inserts a rendered formula at the end of `buffer`: inline math is
    /// lowered so its baseline lines up with the text, display math is
    /// centred. A formula that could not be rendered is shown as its source
    /// in code style, followed by a note saying why (`Math::failure_note`).
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
                self.add_graphic(
                    image,
                    Slot {
                        texture,
                        rise: Some(rise),
                        picture: None,
                        offset: start,
                        depth: Cell::new(image.depth),
                        bytes: Cell::new(0),
                        size: Cell::new(0),
                        key: Cell::new(0),
                        arrived: Cell::new(0),
                        held: Cell::new(Held::Drawn),
                    },
                );
            }
            failed => {
                self.add_slot(None);
                let reason = match failed {
                    Some(Err(reason)) => reason.as_str(),
                    _ => "not rendered",
                };
                let style = if math.display {
                    Style::CodeBlock
                } else {
                    Style::Code
                };
                insert_styled(buffer, &math.source(), &[style]);
                insert_styled(buffer, &math.failure_note(reason), &[Style::Image]);
            }
        }
    }

    /// Inserts an image at the end of `buffer`. A local image that loaded is
    /// shown as a picture (standalone, scaled to the view width, in a box so
    /// it can be highlighted) or as an inline paintable; anything else - a
    /// network destination, an unsupported scheme, a missing or broken file -
    /// is shown as a placeholder saying why. Images that are not local files
    /// are not rendered by the worker pool, so they add no slot. A picture
    /// whose image is part of a link is clickable, and every picture's alt
    /// text is searchable.
    fn insert_image(&self, buffer: &gtk4::TextBuffer, image: &ImageRef, standalone: bool) {
        match &image.state {
            ImageState::Remote => {
                self.insert_image_placeholder(buffer, image, Some(NETWORK_UNSUPPORTED));
                return;
            }
            ImageState::Unsupported(reason) => {
                self.insert_image_placeholder(buffer, image, Some(reason));
                return;
            }
            ImageState::Unresolved => {
                self.insert_image_placeholder(buffer, image, Some("not resolved"));
                return;
            }
            ImageState::Local(_) => {}
        }
        match &image.image {
            Some(Ok(rendered)) => {
                let texture = ScaledTexture::new(rendered);
                if standalone {
                    let picture = gtk4::Picture::for_paintable(&texture);
                    picture.set_can_shrink(true);
                    picture.set_content_fit(gtk4::ContentFit::Contain);
                    let card = gtk4::Box::builder()
                        .css_classes(["md-image"])
                        .halign(gtk4::Align::Start)
                        .build();
                    card.append(&picture);
                    let offset = buffer.end_iter().offset();
                    let anchor = buffer.create_child_anchor(&mut buffer.end_iter());
                    self.view.add_child_at_anchor(&card, &anchor);
                    self.add_image_entry(&image.alt, offset, SearchTarget::Image(card.clone()));
                    if let Some(dest) = &image.link {
                        self.make_picture_clickable(&card, dest);
                    }
                    let mut pictures = self.resizable.pictures.borrow_mut();
                    let index = pictures.len();
                    self.add_graphic(
                        rendered,
                        Slot {
                            texture,
                            rise: None,
                            picture: Some(index),
                            offset,
                            depth: Cell::new(0.0),
                            bytes: Cell::new(0),
                            size: Cell::new(0),
                            key: Cell::new(0),
                            arrived: Cell::new(0),
                            held: Cell::new(Held::Drawn),
                        },
                    );
                    pictures.push((picture, rendered.logical_width(), rendered.logical_height()));
                } else {
                    let start = buffer.end_iter().offset();
                    buffer.insert_paintable(&mut buffer.end_iter(), &texture);
                    // The picture is a single U+FFFC character in the buffer;
                    // a link over it makes the text view open it on click.
                    if let Some(dest) = &image.link {
                        self.links.borrow_mut().push(
                            start as usize,
                            start as usize + 1,
                            dest.clone(),
                        );
                    }
                    self.add_image_entry(&image.alt, start, SearchTarget::Unhighlighted);
                    self.add_graphic(
                        rendered,
                        Slot {
                            texture,
                            rise: None,
                            picture: None,
                            offset: start,
                            depth: Cell::new(0.0),
                            bytes: Cell::new(0),
                            size: Cell::new(0),
                            key: Cell::new(0),
                            arrived: Cell::new(0),
                            held: Cell::new(Held::Drawn),
                        },
                    );
                }
            }
            failed => {
                self.add_slot(None);
                let reason = match failed {
                    Some(Err(reason)) => reason.as_str(),
                    _ => "not rendered",
                };
                self.insert_image_placeholder(buffer, image, Some(reason));
            }
        }
    }

    /// Inserts the `[image: <alt> — <reason>]` placeholder for `image` at the
    /// end of `buffer`, clickable when the image is part of a link.
    fn insert_image_placeholder(
        &self,
        buffer: &gtk4::TextBuffer,
        image: &ImageRef,
        reason: Option<&str>,
    ) {
        let start = buffer.end_iter().offset();
        insert_styled(buffer, &image.placeholder(reason), &[Style::Image]);
        if let Some(dest) = &image.link {
            let end = buffer.end_iter().offset();
            self.links
                .borrow_mut()
                .push(start as usize, end as usize, dest.clone());
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

    /// Builds one table cell. A cell with only text (or only a network or
    /// unsupported image) is a single wrapping label showing it as Pango
    /// markup, aligned according to the column. A cell with formulas or local
    /// images is a single non-wrapping row of labels and pictures (see
    /// [`rich_cell_widget`]).
    fn cell_widget(
        &self,
        cell: &[Inline],
        alignment: Align,
        classes: &[&str],
        offset: i32,
    ) -> gtk4::Widget {
        if cell.iter().any(|inline| match inline {
            Inline::Math(_) => true,
            // Only local images are graphics (they need slots for the worker
            // results); a network or unsupported image is plain placeholder
            // text and keeps the wrapping label path.
            Inline::Image(image) => matches!(image.state, ImageState::Local(_)),
            Inline::Text(_) => false,
        }) {
            return self.rich_cell_widget(cell, alignment, classes, offset);
        }
        let (halign, justify) = match alignment {
            Align::None | Align::Left => (gtk4::Align::Start, gtk4::Justification::Left),
            Align::Center => (gtk4::Align::Center, gtk4::Justification::Center),
            Align::Right => (gtk4::Align::End, gtk4::Justification::Right),
        };
        let content = label_text(cell);
        let label = gtk4::Label::builder()
            .label(&content.text)
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
        set_base_attributes(&label, &content.runs);
        self.make_label_clickable(&label, &content);
        self.add_label_entry(&label, content, offset);
        let frame = gtk4::Box::builder().css_classes(classes.to_vec()).build();
        label.set_hexpand(true);
        frame.append(&label);
        frame.upcast()
    }

    /// Builds a table cell that holds formulas or images: a single
    /// (non-wrapping) row of labels for the text and pictures for the
    /// formulas and images. Formulas and images that could not be rendered
    /// show their source or a placeholder instead.
    fn rich_cell_widget(
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
                    self.add_graphic(
                        image,
                        Slot {
                            texture,
                            rise: None,
                            picture: None,
                            offset,
                            depth: Cell::new(image.depth),
                            bytes: Cell::new(0),
                            size: Cell::new(0),
                            key: Cell::new(0),
                            arrived: Cell::new(0),
                            held: Cell::new(Held::Drawn),
                        },
                    );
                }
                // A formula that could not be rendered is a graphic (it has a
                // job), so it needs a slot even though it shows its source.
                Inline::Math(_) => {
                    self.add_slot(None);
                    row.append(&self.cell_label(inline, offset));
                }
                // A local image is a graphic: a picture if it loaded, a
                // placeholder (with a slot) otherwise. A network or
                // unsupported image is not rendered by the workers, so it gets
                // no slot.
                Inline::Image(image) => match (&image.state, &image.image) {
                    (ImageState::Local(_), Some(Ok(rendered))) => {
                        let texture = ScaledTexture::new(rendered);
                        let picture = gtk4::Picture::for_paintable(&texture);
                        picture.set_can_shrink(false);
                        picture.set_valign(gtk4::Align::Center);
                        let card = gtk4::Box::builder()
                            .css_classes(["md-image"])
                            .valign(gtk4::Align::Center)
                            .build();
                        card.append(&picture);
                        row.append(&card);
                        self.add_image_entry(&image.alt, offset, SearchTarget::Image(card.clone()));
                        if let Some(dest) = &image.link {
                            self.make_picture_clickable(&card, dest);
                        }
                        self.add_graphic(
                            rendered,
                            Slot {
                                texture,
                                rise: None,
                                picture: None,
                                offset,
                                depth: Cell::new(0.0),
                                bytes: Cell::new(0),
                                size: Cell::new(0),
                                key: Cell::new(0),
                                arrived: Cell::new(0),
                                held: Cell::new(Held::Drawn),
                            },
                        );
                    }
                    (ImageState::Local(_), _) => {
                        self.add_slot(None);
                        row.append(&self.cell_label(inline, offset));
                    }
                    _ => {
                        row.append(&self.cell_label(inline, offset));
                    }
                },
                _ => {
                    row.append(&self.cell_label(inline, offset));
                }
            }
        }
        let frame = gtk4::Box::builder().css_classes(classes.to_vec()).build();
        frame.append(&row);
        frame.upcast()
    }

    /// Builds a non-wrapping table label for one inline element (text, or the
    /// source/placeholder of a formula or image), clickable and searchable
    /// like any cell text.
    fn cell_label(&self, inline: &Inline, offset: i32) -> gtk4::Label {
        let content = label_text(std::slice::from_ref(inline));
        let label = gtk4::Label::builder()
            .label(&content.text)
            .selectable(true)
            .valign(gtk4::Align::Center)
            .build();
        set_base_attributes(&label, &content.runs);
        self.make_label_clickable(&label, &content);
        self.add_label_entry(&label, content, offset);
        label
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

    /// Re-renders the graphics of the current run at the new zoom: the ones
    /// already submitted (those on the page plus those waiting below it), in
    /// the order visible first, then below, then above the window. Graphics
    /// beyond the frontier are left for later, rendered lazily when scrolled
    /// near.
    fn rerender(self: &Rc<Self>) {
        let mut settings = self.settings.get();
        settings.zoom = self.shown_zoom.get();
        // The window may have moved to another monitor since.
        settings.max_px = screen_max_px(&self.view);
        self.settings.set(settings);
        let frontier = self.frontier.get();
        let unshown = self.progress.borrow_mut().reset_unshown(frontier);
        let shown: Vec<(usize, i32)> = self
            .slots
            .borrow()
            .iter()
            .enumerate()
            .filter_map(|(id, slot)| {
                let slot = slot.as_ref()?;
                match slot.held.get() {
                    Held::Drawn => {}
                    // Asked for again when near the view (and, if limited,
                    // once there is room).
                    Held::Requested(_) => {
                        slot.held.set(Held::Dropped);
                        return None;
                    }
                    Held::Dropped | Held::Limited => return None,
                }
                Some((id, slot.offset))
            })
            .collect();
        let order = progressive::zoom_order(&shown, self.first_visible_offset(), &unshown);
        let jobs = order.into_iter().map(|id| self.jobs[id].clone()).collect();
        self.generation.set(self.generation.get() + 1);
        self.outstanding.set(0);
        self.pool.replace(None);
        self.submit_batch(jobs);
        self.redraw_near();
        self.update_status();
    }

    /// The buffer offset at the top of the visible part of the view.
    fn first_visible_offset(&self) -> i32 {
        let top = self.view.visible_rect().y();
        self.view.line_at_y(top).0.offset()
    }

    /// Registers `content`, shown in `label` at buffer `offset`, as searchable.
    fn add_label_entry(&self, label: &gtk4::Label, content: LabelText, offset: i32) {
        let mut search = self.search.borrow_mut();
        let order = search.next_order;
        search.next_order += 1;
        search.entries.push(SearchEntry {
            text: content.text,
            offset,
            order,
            target: SearchTarget::Label {
                label: label.clone(),
                runs: content.runs,
            },
            highlighted: Cell::new(false),
        });
    }

    /// Registers a whole diagram `card` as searchable for its label `text`.
    fn add_diagram_entry(&self, card: &gtk4::Box, text: String, offset: i32) {
        if text.is_empty() {
            return;
        }
        let mut search = self.search.borrow_mut();
        let order = search.next_order;
        search.next_order += 1;
        search.entries.push(SearchEntry {
            text,
            offset,
            order,
            target: SearchTarget::Diagram(card.clone()),
            highlighted: Cell::new(false),
        });
    }

    /// Registers an image's `alt` text as searchable at buffer `offset`, with
    /// `target` saying what to highlight (the picture's box, or nothing for an
    /// inline picture). Empty alt text adds nothing.
    fn add_image_entry(&self, alt: &str, offset: i32, target: SearchTarget) {
        if alt.is_empty() {
            return;
        }
        let mut search = self.search.borrow_mut();
        let order = search.next_order;
        search.next_order += 1;
        search.entries.push(SearchEntry {
            text: alt.to_string(),
            offset,
            order,
            target,
            highlighted: Cell::new(false),
        });
    }

    /// Registers a heading with `text`, starting at buffer `offset`, under
    /// its anchor; scrolls to it if a pending anchor names it.
    fn record_heading(&self, text: &str, offset: i32) {
        let Some(slug) = self.anchors.borrow_mut().register(text, offset) else {
            return;
        };
        if self.pending_anchor.borrow().as_deref() == Some(slug.as_str()) {
            self.pending_anchor.borrow_mut().take();
            let mut iter = self.view.buffer().iter_at_offset(offset);
            self.view.scroll_to_iter(&mut iter, 0.15, true, 0.0, 0.3);
        }
    }

    /// Opens the link under buffer position `x, y` (widget coordinates), if
    /// any, for a primary click with no selection.
    fn click_at(&self, x: f64, y: f64) {
        if self.view.buffer().has_selection() {
            return;
        }
        let Some(dest) = self.dest_at(x, y) else {
            return;
        };
        self.click_link(&dest);
    }

    /// Shows a pointer cursor over the link under `x, y`, if any.
    fn point_at(&self, x: f64, y: f64) {
        let over_link = self.dest_at(x, y).is_some();
        self.view
            .set_cursor_from_name(if over_link { Some("pointer") } else { None });
    }

    /// The link destination at buffer position `x, y` (widget coordinates).
    fn dest_at(&self, x: f64, y: f64) -> Option<String> {
        let (bx, by) = self.view.window_to_buffer_coords(
            gtk4::TextWindowType::Widget,
            x.round() as i32,
            y.round() as i32,
        );
        let iter = self.view.iter_at_location(bx, by)?;
        let offset = iter.offset() as usize;
        self.links.borrow().dest_at(offset).map(str::to_string)
    }

    /// Acts on a link `dest`: a same-document anchor scrolls to its heading,
    /// everything else is handed to the window.
    fn click_link(&self, dest: &str) {
        match self.link_action(dest) {
            LinkAction::Anchor(fragment) => {
                let this = self.weak_self.borrow().as_ref().and_then(Weak::upgrade);
                if let Some(this) = this {
                    this.scroll_to_anchor(&fragment);
                }
            }
            other => (self.open_link.as_ref())(other),
        }
    }

    /// What clicking the link `dest` does, looking at the file it names
    /// (see [`file_facts`]).
    fn link_action(&self, dest: &str) -> LinkAction {
        links::link_action(links::resolve_link(&self.document, dest), file_facts)
    }

    /// The tooltip of the link `dest`: what a click does, including why a
    /// link is refused or asks first.
    fn link_tooltip(&self, dest: &str) -> String {
        links::link_tooltip(dest, &self.link_action(dest))
    }

    /// Scrolls to the heading named `fragment`; an empty fragment (`#`)
    /// scrolls to the top. A heading not on the page yet is rendered up to
    /// and scrolled to once appended; a fragment naming no heading at all
    /// does nothing (and leaves nothing pending).
    fn scroll_to_anchor(self: &Rc<Self>, fragment: &str) {
        self.pending_anchor.replace(None);
        if fragment.is_empty() {
            self.scrolled.vadjustment().set_value(0.0);
            return;
        }
        let offset = self.anchors.borrow().lookup(fragment);
        if let Some(offset) = offset {
            let mut iter = self.view.buffer().iter_at_offset(offset);
            self.view.scroll_to_iter(&mut iter, 0.15, true, 0.0, 0.3);
        } else if let Some(block) = self.heading_blocks.lookup(fragment) {
            *self.pending_anchor.borrow_mut() = Some(fragment.to_string());
            self.render_to(usize::try_from(block).unwrap_or(0));
        }
    }

    /// Makes the links inside a label's `content` clickable: a click is
    /// mapped to a byte offset with the label's layout and resolved against
    /// the document; a pointer cursor and a tooltip with the raw destination
    /// show while over a link.
    fn make_label_clickable(&self, label: &gtk4::Label, content: &LabelText) {
        if content.links.is_empty() {
            return;
        }
        let weak = self
            .weak_self
            .borrow()
            .clone()
            .expect("session weak handle set after construction");
        let label_for_click = label.clone();
        let links = content.links.clone();
        let click = gtk4::GestureClick::new();
        {
            let weak = weak.clone();
            let label = label_for_click.clone();
            let links = links.clone();
            click.connect_released(move |gesture, _, x, y| {
                if gesture.current_button() != gdk::BUTTON_PRIMARY {
                    return;
                }
                let Some(session) = weak.upgrade() else {
                    return;
                };
                if let Some(dest) = label_link_at(&label, &links, x, y) {
                    session.click_link(&dest);
                }
            });
        }
        label.add_controller(click);
        let motion = gtk4::EventControllerMotion::new();
        {
            let label = label_for_click.clone();
            let links = links.clone();
            motion.connect_motion(move |_, x, y| {
                let over_link = label_link_at(&label, &links, x, y).is_some();
                label.set_cursor_from_name(if over_link { Some("pointer") } else { None });
            });
        }
        label.add_controller(motion);
        label.set_has_tooltip(true);
        label.connect_query_tooltip(move |label, x, y, _, tooltip| {
            let Some(session) = weak.upgrade() else {
                return false;
            };
            match label_link_at(label, &links, f64::from(x), f64::from(y)) {
                Some(dest) if !dest.is_empty() => {
                    tooltip.set_text(Some(&session.link_tooltip(&dest)));
                    true
                }
                _ => false,
            }
        });
    }

    /// Makes a whole image `card` open `dest` when clicked, with a pointer
    /// cursor and the raw destination as a tooltip, like a link in text. The
    /// image is the link's content, so the whole picture is the target.
    fn make_picture_clickable(&self, card: &gtk4::Box, dest: &str) {
        let weak = self
            .weak_self
            .borrow()
            .clone()
            .expect("session weak handle set after construction");
        let dest = dest.to_string();
        let click = gtk4::GestureClick::new();
        {
            let weak = weak.clone();
            let dest = dest.clone();
            click.connect_released(move |gesture, _, _, _| {
                if gesture.current_button() != gdk::BUTTON_PRIMARY {
                    return;
                }
                if let Some(session) = weak.upgrade() {
                    session.click_link(&dest);
                }
            });
        }
        card.add_controller(click);
        card.set_cursor_from_name(Some("pointer"));
        card.set_tooltip_text(Some(&self.link_tooltip(&dest)));
    }

    /// Runs a search for `query` in `mode`, replacing the current one, and
    /// reports the outcome to the window.
    fn search(self: &Rc<Self>, query: &str, mode: SearchMode) -> SearchSummary {
        {
            let mut search = self.search.borrow_mut();
            search.query = query.to_string();
            search.mode = mode;
            search.error = false;
            search.matcher = None;
            search.matches.clear();
            search.current = None;
        }
        if !query.is_empty() {
            match Matcher::new(query, mode) {
                Ok(matcher) => {
                    self.search.borrow_mut().matcher = Some(matcher);
                    self.collect_matches();
                }
                Err(_) => {
                    self.search.borrow_mut().error = true;
                }
            }
        }
        self.choose_current_near_view();
        self.apply_highlights();
        self.scroll_current();
        self.marks.queue_draw();
        let summary = self.summary();
        (self.search_changed)(summary);
        summary
    }

    /// Steps the current match forward or backward, wrapping around.
    fn navigate(self: &Rc<Self>, forward: bool) -> SearchSummary {
        // Step through up-to-date matches, not ones from before blocks
        // that were appended since.
        if self.refresh_waiting.get() {
            self.refresh_now();
        }
        {
            let mut search = self.search.borrow_mut();
            search.current = search::next_index(search.current, search.matches.len(), forward);
        }
        self.apply_highlights();
        self.scroll_current();
        self.marks.queue_draw();
        let summary = self.summary();
        (self.search_changed)(summary);
        summary
    }

    /// Clears the query and its highlights, keeping the mode.
    fn clear_search(&self) {
        {
            let mut search = self.search.borrow_mut();
            search.query.clear();
            search.matcher = None;
            search.error = false;
            search.matches.clear();
            search.current = None;
        }
        self.apply_highlights();
        self.marks.queue_draw();
        let summary = self.summary();
        (self.search_changed)(summary);
    }

    /// Re-runs the search after more blocks were appended, keeping the
    /// current match where it still exists.
    fn refresh_search(&self) {
        if self.search.borrow().query.is_empty() || self.search.borrow().error {
            return;
        }
        let previous = {
            let search = self.search.borrow();
            search
                .current
                .and_then(|index| search.matches.get(index).map(|m| (m.offset, m.order)))
        };
        self.collect_matches();
        {
            let search = self.search.borrow();
            let new_current = previous.and_then(|(offset, order)| {
                search
                    .matches
                    .iter()
                    .position(|m| m.offset == offset && m.order == order)
            });
            drop(search);
            self.search.borrow_mut().current = new_current;
        }
        self.apply_highlights();
        self.marks.queue_draw();
        let summary = self.summary();
        (self.search_changed)(summary);
    }

    /// Fills `search.matches` from the whole text buffer, every indexed
    /// entry, and the parsed text of blocks not on the page yet (so a search
    /// covers the whole document while it is still being rendered).
    fn collect_matches(&self) {
        let buffer = self.view.buffer();
        let text = searchable_text(&buffer);
        let matcher = self.search.borrow().matcher.clone();
        let Some(matcher) = matcher else {
            return;
        };
        let mut search = self.search.borrow_mut();
        search.matches.clear();
        search.current = None;
        let byte_ranges = matcher.find(&text);
        for chars in search::char_ranges(&text, &byte_ranges) {
            search.matches.push(SearchMatch {
                offset: chars.start as i32,
                order: 0,
                target: MatchTarget::Buffer {
                    chars: chars.start as i32..chars.end as i32,
                },
            });
        }
        let mut entry_matches = Vec::new();
        for (index, entry) in search.entries.iter().enumerate() {
            for range in matcher.find(&entry.text) {
                entry_matches.push(SearchMatch {
                    offset: entry.offset,
                    order: entry.order,
                    target: MatchTarget::Entry {
                        entry: index,
                        range,
                    },
                });
            }
        }
        search.matches.extend(entry_matches);
        // The blocks not on the page yet: their text is searched from the
        // parsed document, and every such match sorts after the rendered
        // ones (a sentinel offset), ordered by block.
        let shown = self.progress.borrow().shown();
        let mut block_matches = Vec::new();
        for unit in search.units.iter().filter(|unit| unit.block >= shown) {
            for _ in matcher.find(&unit.text) {
                block_matches.push(SearchMatch {
                    offset: i32::MAX,
                    order: unit.block,
                    target: MatchTarget::Block { block: unit.block },
                });
            }
        }
        search.matches.extend(block_matches);
        search.matches.sort_by_key(|m| (m.offset, m.order));
    }

    /// Picks the current match: the first at or after the top of the
    /// window, or the first match overall.
    fn choose_current_near_view(&self) {
        let mut search = self.search.borrow_mut();
        if search.matches.is_empty() {
            search.current = None;
            return;
        }
        let positions: Vec<i32> = search.matches.iter().map(|m| m.offset).collect();
        let start = self.first_visible_offset();
        search.current = Some(search::first_at_or_after(&positions, start).unwrap_or(0));
    }

    /// Applies (or clears) the match and current-match highlighting.
    fn apply_highlights(&self) {
        let buffer = self.view.buffer();
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        buffer.remove_tag_by_name(SEARCH_MATCH_TAG, &start, &end);
        buffer.remove_tag_by_name(SEARCH_CURRENT_TAG, &start, &end);
        let search = self.search.borrow();
        let current = search.current;
        let mut grouped: Vec<Vec<(std::ops::Range<usize>, bool)>> =
            vec![Vec::new(); search.entries.len()];
        for (index, m) in search.matches.iter().enumerate() {
            match &m.target {
                MatchTarget::Buffer { chars } => {
                    let tag = if Some(index) == current {
                        SEARCH_CURRENT_TAG
                    } else {
                        SEARCH_MATCH_TAG
                    };
                    let start = buffer.iter_at_offset(chars.start);
                    let end = buffer.iter_at_offset(chars.end);
                    buffer.apply_tag_by_name(tag, &start, &end);
                }
                MatchTarget::Entry { entry, range } => {
                    grouped[*entry].push((range.clone(), Some(index) == current));
                }
                // A match in a block not on the page yet has nothing to
                // highlight until the block is rendered.
                MatchTarget::Block { .. } => {}
            }
        }
        for (entry, ranges) in search.entries.iter().zip(grouped) {
            match &entry.target {
                SearchTarget::Label { label, runs } => {
                    if ranges.is_empty() {
                        if entry.highlighted.get() {
                            set_base_attributes(label, runs);
                            entry.highlighted.set(false);
                        }
                    } else {
                        label.set_attributes(Some(&label_attributes(runs, &ranges)));
                        entry.highlighted.set(true);
                    }
                }
                SearchTarget::Diagram(card) | SearchTarget::Image(card) => {
                    card.remove_css_class("md-search-hit");
                    card.remove_css_class("md-search-current");
                    if !ranges.is_empty() {
                        let current = ranges.iter().any(|(_, is_current)| *is_current);
                        card.add_css_class(if current {
                            "md-search-current"
                        } else {
                            "md-search-hit"
                        });
                    }
                }
                SearchTarget::Unhighlighted => {}
            }
        }
    }

    /// Scrolls the current match into view, a little below the top. A match
    /// in a block not on the page yet first renders up to that block and is
    /// scrolled to once it is appended.
    fn scroll_current(self: &Rc<Self>) {
        let search = self.search.borrow();
        let Some(current) = search.current else {
            return;
        };
        let Some(m) = search.matches.get(current) else {
            return;
        };
        if let MatchTarget::Block { block } = &m.target {
            let block = *block;
            drop(search);
            self.render_to(block);
            *self.pending_scroll.borrow_mut() = Some(block);
            return;
        }
        let offset = m.offset;
        drop(search);
        let mut iter = self.view.buffer().iter_at_offset(offset);
        self.view.scroll_to_iter(&mut iter, 0.15, true, 0.0, 0.3);
    }

    /// The current outcome of the search.
    fn summary(&self) -> SearchSummary {
        let search = self.search.borrow();
        SearchSummary {
            total: search.matches.len(),
            current: search.current.map_or(0, |current| current + 1),
            error: search.error,
            pending: !self.progress.borrow().all_shown(),
        }
    }

    /// Draws the scrollbar marks for the matches.
    fn draw_marks(&self, cr: &gtk4::cairo::Context, width: f64, height: f64) {
        let search = self.search.borrow();
        if search.matches.is_empty() {
            return;
        }
        let buffer = self.view.buffer();
        let (end_top, end_height) = self.view.line_yrange(&buffer.end_iter());
        let total = f64::from(end_top + end_height);
        if total <= 0.0 || height <= 0.0 {
            return;
        }
        let current = search.current;
        let mark_height = layout::SEARCH_MARK_HEIGHT;
        for (index, m) in search.matches.iter().enumerate() {
            // A match in a block not rendered yet has no position on the
            // page, so it gets no mark (the "…" in the find bar says more
            // results are still coming).
            if matches!(m.target, MatchTarget::Block { .. }) {
                continue;
            }
            let iter = buffer.iter_at_offset(m.offset);
            let (top, line_height) = self.view.line_yrange(&iter);
            let centre = f64::from(top) + f64::from(line_height) / 2.0;
            let fraction = (centre / total).clamp(0.0, 1.0);
            let y = layout::search_mark_y(fraction, height, mark_height);
            if Some(index) == current {
                cr.set_source_rgba(1.0, 0.57, 0.0, 0.9);
                cr.rectangle(0.0, y, width, mark_height);
            } else {
                cr.set_source_rgba(0.95, 0.78, 0.1, 0.65);
                cr.rectangle(width / 2.0 - 1.5, y, 3.0, mark_height);
            }
            let _ = cr.fill();
        }
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
/// button, over the code in a horizontal scroller. Returns the card and its
/// code label (which the find bar searches).
fn code_card(code: &CodeBlock) -> (gtk4::Box, gtk4::Label) {
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
    (card, text)
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
    /// The header bar's zoom percentage (hidden at 100%); clicking it resets
    /// the zoom.
    zoom_indicator: gtk4::Button,
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
    /// The find bar and its widgets.
    search_bar: gtk4::SearchBar,
    search_entry: gtk4::SearchEntry,
    search_status: gtk4::Label,
    search_prev: gtk4::Button,
    search_next: gtk4::Button,
    search_mode_button: gtk4::MenuButton,
    /// The find bar's matching mode, kept for the window (and its future
    /// documents); a new window starts with smart case.
    mode: Cell<SearchMode>,
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
        let zoom_indicator = gtk4::Button::builder()
            .tooltip_text("Reset Zoom (Ctrl+0)")
            .action_name("win.zoom-reset")
            .css_classes(["flat"])
            .visible(false)
            .build();
        let header = adw::HeaderBar::new();
        header.pack_start(&open_button);
        header.pack_end(&spinner);
        header.pack_end(&zoom_indicator);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&start_page()));

        let search_entry = gtk4::SearchEntry::builder()
            .placeholder_text("Search document")
            .width_chars(24)
            .hexpand(true)
            .build();
        let search_status = gtk4::Label::builder().css_classes(["dim-label"]).build();
        let search_prev = gtk4::Button::builder()
            .icon_name("go-up-symbolic")
            .tooltip_text("Previous match")
            .css_classes(["flat"])
            .sensitive(false)
            .build();
        let search_next = gtk4::Button::builder()
            .icon_name("go-down-symbolic")
            .tooltip_text("Next match (Enter)")
            .css_classes(["flat"])
            .sensitive(false)
            .build();
        let search_mode_button = gtk4::MenuButton::builder()
            .label(SearchMode::SmartCase.short())
            .tooltip_text("Search mode")
            .css_classes(["flat"])
            .build();
        let search_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        search_box.append(&search_entry);
        search_box.append(&search_status);
        search_box.append(&search_prev);
        search_box.append(&search_next);
        search_box.append(&search_mode_button);
        let search_bar = gtk4::SearchBar::builder()
            .child(&search_box)
            .show_close_button(true)
            .build();
        search_bar.connect_entry(&search_entry);

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
            zoom_indicator,
            document: RefCell::new(None),
            session: RefCell::new(None),
            zoom: Cell::new(Zoom::default()),
            pinch_start: Cell::new(1.0),
            zoom_class: format!("md-zoom-{number}"),
            zoom_css: gtk4::CssProvider::new(),
            base_px,
            search_bar,
            search_entry,
            search_status,
            search_prev,
            search_next,
            search_mode_button,
            mode: Cell::new(SearchMode::SmartCase),
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
        this.install_search();
        this.window.connect_close_request(|window| {
            window.remove_action("open");
            glib::Propagation::Proceed
        });
        this.window.present();
        this
    }

    /// Installs the find action (Ctrl+F), the mode popover and the key
    /// bindings for `/`, `n`, `N` and Escape, and the find bar's signals.
    fn install_search(self: &Rc<Self>) {
        let find = gio::SimpleAction::new("find", None);
        let state = Rc::downgrade(self);
        find.connect_activate(move |_, _| {
            if let Some(state) = state.upgrade() {
                state.open_search();
            }
        });
        self.window.add_action(&find);

        let keys = gtk4::EventControllerKey::new();
        keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        let state = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, keyval, _, modifiers| {
            let Some(state) = state.upgrade() else {
                return glib::Propagation::Proceed;
            };
            state.handle_key(keyval, modifiers)
        });
        self.window.add_controller(keys);

        let weak = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |entry| {
            let Some(state) = weak.upgrade() else {
                return;
            };
            let query = entry.text().to_string();
            let mode = state.mode.get();
            let session = state.session.borrow().clone();
            if let Some(session) = session {
                session.search(&query, mode);
            }
        });

        let weak = Rc::downgrade(self);
        self.search_entry.connect_activate(move |_| {
            if let Some(state) = weak.upgrade() {
                state.navigate_from_key(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_next_match(move |_| {
            if let Some(state) = weak.upgrade() {
                state.navigate_from_key(true);
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_previous_match(move |_| {
            if let Some(state) = weak.upgrade() {
                state.navigate_from_key(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.search_entry.connect_stop_search(move |_| {
            if let Some(state) = weak.upgrade() {
                state.search_bar.set_search_mode(false);
            }
        });

        let weak = Rc::downgrade(self);
        self.search_prev.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.navigate_from_key(false);
            }
        });
        let weak = Rc::downgrade(self);
        self.search_next.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.navigate_from_key(true);
            }
        });

        let weak = Rc::downgrade(self);
        self.search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if !bar.is_search_mode() {
                    if let Some(state) = weak.upgrade() {
                        state.focus_view();
                    }
                }
            });

        let popover = gtk4::Popover::new();
        let choices = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        let mut group: Option<gtk4::CheckButton> = None;
        for (index, mode) in SearchMode::ALL.iter().enumerate() {
            let button = gtk4::CheckButton::with_label(mode.label());
            if index == 0 {
                button.set_active(true);
            } else {
                button.set_group(group.as_ref());
            }
            if group.is_none() {
                group = Some(button.clone());
            }
            let weak = Rc::downgrade(self);
            let mode = *mode;
            button.connect_toggled(move |button| {
                if button.is_active() {
                    if let Some(state) = weak.upgrade() {
                        state.set_search_mode(mode);
                    }
                }
            });
            choices.append(&button);
        }
        popover.set_child(Some(&choices));
        self.search_mode_button.set_popover(Some(&popover));
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
        let changed = change(&mut zoom);
        self.zoom.set(zoom);
        self.update_zoom_indicator();
        if !changed {
            return;
        }
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

    /// Lowers the zoom ceiling to `limit`, the largest zoom the document's
    /// graphics allow; if the window is zoomed in further, it zooms out to
    /// the ceiling (keeping the line at the top of the window in place).
    fn limit_zoom(&self, limit: f64) {
        self.change_zoom(|zoom| zoom.set_ceiling(limit));
    }

    /// Shows the zoom percentage in the header bar, or hides it at 100%.
    fn update_zoom_indicator(&self) {
        match self.zoom.get().indicator() {
            Some(text) => {
                self.zoom_indicator.set_label(&text);
                self.zoom_indicator.set_visible(true);
            }
            None => self.zoom_indicator.set_visible(false),
        }
    }

    /// Opens the find bar over the current document and focuses its entry.
    fn open_search(&self) {
        if self.session.borrow().is_none() {
            return;
        }
        self.search_bar.set_search_mode(true);
        self.search_entry.grab_focus();
        self.search_entry.select_region(0, -1);
    }

    /// Handles a key press on the window (ignored while the search entry is
    /// focused, so typing works normally): `/` opens the find bar, `n`/`N`
    /// step between matches, and Escape hides the bar or clears the search.
    fn handle_key(&self, keyval: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let in_entry = gtk4::prelude::RootExt::focus(&self.window).is_some_and(|focus| {
            focus == *self.search_entry.upcast_ref::<gtk4::Widget>()
                || focus.is_ancestor(&self.search_entry)
        });
        if in_entry
            || modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK)
        {
            return glib::Propagation::Proceed;
        }
        match keyval {
            gdk::Key::slash => {
                self.open_search();
                glib::Propagation::Stop
            }
            gdk::Key::n => {
                self.navigate_from_key(true);
                glib::Propagation::Stop
            }
            gdk::Key::N => {
                self.navigate_from_key(false);
                glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                self.escape_search();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    /// Steps to the next (`forward`) or previous match, if there is a query.
    fn navigate_from_key(&self, forward: bool) {
        if let Some(session) = self.session.borrow().as_ref() {
            session.navigate(forward);
        }
    }

    /// Hides the find bar (keeping the query and highlights, so `n`/`N`
    /// still work) or, when it is already hidden, clears the search.
    fn escape_search(&self) {
        if self.search_bar.is_search_mode() {
            self.search_bar.set_search_mode(false);
        } else if let Some(session) = self.session.borrow().as_ref() {
            session.clear_search();
        }
    }

    /// Puts the keyboard focus back on the document.
    fn focus_view(&self) {
        if let Some(session) = self.session.borrow().as_ref() {
            session.view.grab_focus();
        }
    }

    /// Switches the find bar to `mode`, re-running the current query.
    fn set_search_mode(self: &Rc<Self>, mode: SearchMode) {
        if self.mode.get() == mode {
            return;
        }
        self.mode.set(mode);
        self.search_mode_button.set_label(mode.short());
        self.search_mode_button.set_tooltip_text(Some(mode.label()));
        if let Some(session) = self.session.borrow().as_ref() {
            let query = self.search_entry.text().to_string();
            session.search(&query, mode);
        }
    }

    /// Updates the find bar's count and buttons from a search outcome. While
    /// more content is still being rendered (the `pending` flag), the count
    /// is followed by "…" so the user knows more results may appear.
    fn show_search_summary(&self, summary: SearchSummary) {
        if summary.error {
            self.search_entry.add_css_class("error");
            self.search_status.set_label("Invalid pattern");
            self.search_prev.set_sensitive(false);
            self.search_next.set_sensitive(false);
            return;
        }
        self.search_entry.remove_css_class("error");
        if summary.total == 0 {
            let text = if self.search_entry.text().is_empty() {
                ""
            } else if summary.pending {
                "Searching…"
            } else {
                "No results"
            };
            self.search_status.set_label(text);
            self.search_prev.set_sensitive(false);
            self.search_next.set_sensitive(false);
            return;
        }
        let text = if summary.pending {
            format!("{} of {} …", summary.current, summary.total)
        } else {
            format!("{} of {}", summary.current, summary.total)
        };
        self.search_status.set_label(&text);
        self.search_prev.set_sensitive(true);
        self.search_next.set_sensitive(true);
    }

    /// Replaces the window's content with a fresh document view titled
    /// after `path`, covered by a loading spinner, and returns the session
    /// for `blocks`. The find bar is hidden and cleared for the new
    /// document.
    fn show_document(self: &Rc<Self>, path: &Path, mut blocks: Vec<Block>) -> Rc<Session> {
        if let Some(old) = self.session.take() {
            old.pool.replace(None);
        }
        // The new document sets its own ceiling as its graphics arrive.
        let mut zoom = self.zoom.get();
        zoom.set_ceiling(f64::INFINITY);
        self.zoom.set(zoom);
        self.update_zoom_indicator();
        self.search_bar.set_search_mode(false);
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
        // The strip over the scrollbar's right edge that marks matches; it
        // must not catch clicks, and it sits under the cover so it is hidden
        // while the first screen loads.
        let marks = gtk4::DrawingArea::builder()
            .width_request(6)
            .halign(gtk4::Align::End)
            .valign(gtk4::Align::Fill)
            .can_target(false)
            .build();
        let overlay = gtk4::Overlay::builder().child(&scrolled).build();
        overlay.add_overlay(&marks);
        overlay.add_overlay(&cover);
        overlay.set_vexpand(true);
        overlay.set_hexpand(true);
        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        content.append(&self.search_bar);
        content.append(&overlay);
        self.toolbar.set_content(Some(&content));
        self.window.set_title(Some(&md_viewer::window_title(path)));
        self.document.replace(Some(path.to_path_buf()));

        let jobs = progressive::jobs(&mut blocks);
        let threads = progressive::worker_count(
            std::thread::available_parallelism().map_or(1, usize::from),
            jobs.len(),
        );
        let units = search::document_units(&blocks);
        let (sender, receiver) = mpsc::channel();
        let search_changed = {
            let window = Rc::downgrade(self);
            Box::new(move |summary| {
                if let Some(window) = window.upgrade() {
                    window.show_search_summary(summary);
                }
            })
        };
        let open_link = {
            let window = Rc::downgrade(self);
            Box::new(move |resolved| {
                if let Some(window) = window.upgrade() {
                    window.open_link(resolved);
                }
            })
        };
        let limit_changed = {
            let window = Rc::downgrade(self);
            Box::new(move |limit| {
                if let Some(window) = window.upgrade() {
                    window.limit_zoom(limit);
                }
            })
        };
        let heading_blocks = links::heading_blocks(&blocks);
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
            frontier: Cell::new(0),
            threads,
            settings: Cell::new(RasterSettings::default()),
            shown_zoom: Cell::new(1.0),
            rezoom: RefCell::new(None),
            anchor: RefCell::new(None),
            receiver,
            sender: Mutex::new(sender),
            polling: Cell::new(false),
            search: RefCell::new(Search {
                units,
                ..Search::default()
            }),
            marks,
            search_changed,
            document: path.to_path_buf(),
            links: RefCell::new(LinkRanges::default()),
            anchors: RefCell::new(Anchors::default()),
            pending_anchor: RefCell::new(None),
            last_refresh: Cell::new(None),
            refresh_waiting: Cell::new(false),
            ticking: Cell::new(false),
            idle_frames: Cell::new(0),
            heading_blocks,
            zoom_limit: Cell::new(f64::INFINITY),
            limit_dirty: Cell::new(false),
            held_bytes: Cell::new(0),
            budget: Cell::new(graphics::DOCUMENT_BUDGET),
            budget_pending: Cell::new(false),
            arrivals: Cell::new(0),
            limit_changed,
            pending_scroll: RefCell::new(None),
            open_link,
            weak_self: RefCell::new(None),
        });
        session.weak_self.replace(Some(Rc::downgrade(&session)));
        {
            session.wake_ticks();
            // The view's width shows in the horizontal adjustment.
            let weak = Rc::downgrade(&session);
            session.scrolled.hadjustment().connect_changed(move |_| {
                if let Some(session) = weak.upgrade() {
                    session.wake_ticks();
                }
            });
        }
        {
            let weak = Rc::downgrade(&session);
            session.marks.set_draw_func(move |_, cr, width, height| {
                if let Some(session) = weak.upgrade() {
                    session.draw_marks(cr, f64::from(width), f64::from(height));
                }
            });
        }
        {
            let weak = Rc::downgrade(&session);
            let click = gtk4::GestureClick::new();
            click.connect_released(move |gesture, _, x, y| {
                if gesture.current_button() != gdk::BUTTON_PRIMARY {
                    return;
                }
                if let Some(session) = weak.upgrade() {
                    session.click_at(x, y);
                }
            });
            session.view.add_controller(click);
        }
        {
            let weak = Rc::downgrade(&session);
            let motion = gtk4::EventControllerMotion::new();
            motion.connect_motion(move |_, x, y| {
                if let Some(session) = weak.upgrade() {
                    session.point_at(x, y);
                }
            });
            session.view.add_controller(motion);
        }
        {
            let weak = Rc::downgrade(&session);
            session.view.set_has_tooltip(true);
            session
                .view
                .connect_query_tooltip(move |_, x, y, _, tooltip| {
                    let Some(session) = weak.upgrade() else {
                        return false;
                    };
                    match session.dest_at(f64::from(x), f64::from(y)) {
                        Some(dest) if !dest.is_empty() => {
                            tooltip.set_text(Some(&session.link_tooltip(&dest)));
                            true
                        }
                        _ => false,
                    }
                });
        }
        {
            let weak = Rc::downgrade(&session);
            session.scrolled.vadjustment().connect_changed(move |_| {
                if let Some(session) = weak.upgrade() {
                    session.marks.queue_draw();
                    session.wake_ticks();
                    session.advance_frontier();
                    session.schedule_budget();
                }
            });
            // Plain scrolling only changes the value: render more, and draw
            // dropped graphics coming near the view again.
            let weak = Rc::downgrade(&session);
            session
                .scrolled
                .vadjustment()
                .connect_value_changed(move |_| {
                    if let Some(session) = weak.upgrade() {
                        session.advance_frontier();
                        session.schedule_budget();
                    }
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
        // Clearing the entry triggers a search on the new, empty session.
        self.search_entry.set_text("");
        self.search_status.set_label("");
        self.search_prev.set_sensitive(false);
        self.search_next.set_sensitive(false);
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
            max_px: screen_max_px(view),
        }
    }

    /// Opens a resolved link: web, mail and file destinations are handed to
    /// the desktop's default handler, a Markdown document opens in the
    /// viewer (a new window, scrolled to its fragment if any).
    fn open_link(self: &Rc<Self>, action: LinkAction) {
        match action {
            LinkAction::Browse(uri) | LinkAction::Mail(uri) => launch_uri(&self.window, &uri),
            LinkAction::OpenFile(path) => launch_file(&self.window, &path),
            LinkAction::AskFile(path, reason) => self.confirm_open(path, &reason),
            LinkAction::Markdown(path, fragment) => {
                open_file_to(self, path, Origin::Chooser, fragment);
            }
            LinkAction::Anchor(_)
            | LinkAction::Missing(_)
            | LinkAction::Refused(_)
            | LinkAction::Nothing => {}
        }
    }

    /// Asks before opening the potentially dangerous local file `path`,
    /// saying why it is considered dangerous (`reason`); opens it only if
    /// the user chooses "Open Anyway" (Cancel is the default).
    fn confirm_open(&self, path: PathBuf, reason: &str) {
        let dialog = confirm_dialog(&path, reason);
        let window = self.window.clone();
        dialog.connect_response(None, move |_, response| {
            if response == OPEN_ANYWAY {
                launch_file(&window, &path);
            }
        });
        dialog.present(Some(&self.window));
    }
}

/// The response id of the confirmation dialog's "Open Anyway" button.
const OPEN_ANYWAY: &str = "open";

/// The dialog asking before a potentially dangerous local file is opened:
/// the reason, the path, Cancel as the default and "Open Anyway" styled as
/// destructive.
fn confirm_dialog(path: &Path, reason: &str) -> adw::AlertDialog {
    let dialog = adw::AlertDialog::new(
        Some("Open Dangerous Content?"),
        Some(&confirm_body(path, reason)),
    );
    dialog.add_responses(&[("cancel", "_Cancel"), (OPEN_ANYWAY, "_Open Anyway")]);
    dialog.set_response_appearance(OPEN_ANYWAY, adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog
}

/// The text of the confirmation dialog: why the file is considered
/// dangerous (`reason`, as `links::local_file_risk` words it), what opening
/// it means, and its path.
fn confirm_body(path: &Path, reason: &str) -> String {
    format!(
        "This link opens potentially dangerous content: {reason}.\n\n\
         It will be opened by another application, outside MD Viewer. Only \
         continue if you trust this document and where it came from.\n\n{}",
        path.display()
    )
}

/// Opens the local file at `path` in its default application.
fn launch_file(window: &adw::ApplicationWindow, path: &Path) {
    if let Some(uri) = links::file_uri(path) {
        launch_uri(window, &uri);
    }
}

/// What is known about the local file at `path` for deciding whether it is
/// safe to open (see `links::local_file_risk`); `None` if it does not
/// exist. The content types come from GIO, from the name and from the
/// first bytes (read without blocking, regular files only).
fn file_facts(path: &Path) -> Option<links::FileFacts> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path).ok()?;
    let regular = metadata.is_file();
    let (name_type, _) = gio::content_type_guess(Some(path), None);
    let head = if regular {
        md_viewer::read_head(path, 4096).unwrap_or_default()
    } else {
        Vec::new()
    };
    let data_type = if head.is_empty() {
        None
    } else {
        let (sniffed, uncertain) = gio::content_type_guess(None::<&Path>, Some(head.as_slice()));
        (!uncertain && sniffed != "application/octet-stream").then(|| sniffed.to_string())
    };
    // GIO counts every text type as possibly executable (a script); plain
    // text itself is fine, its subtypes (scripts, source) are not.
    let executable = |content_type: &str| {
        content_type != "text/plain" && gio::content_type_can_be_executable(content_type)
    };
    let can_be_executable = executable(&name_type) || data_type.as_deref().is_some_and(executable);
    Some(links::FileFacts {
        regular,
        executable_bit: metadata.permissions().mode() & 0o111 != 0,
        name_type: name_type.to_string(),
        data_type,
        can_be_executable,
    })
}

/// Hands `uri` to the desktop's default handler (a browser, mail client or
/// file viewer).
fn launch_uri(window: &adw::ApplicationWindow, uri: &str) {
    let launcher = gtk4::UriLauncher::new(uri);
    let window = window.clone();
    let uri = uri.to_string();
    launcher.launch(Some(&window), None::<&gio::Cancellable>, move |result| {
        if let Err(error) = result {
            glib::g_warning!("md_viewer", "Could not open {uri}: {error}");
        }
    });
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
    open_file_to(window, path, origin, None);
}

/// Opens the document at `path` from `window`, scrolling to `fragment` (a
/// heading anchor) once it is shown, if given.
fn open_file_to(window: &Rc<DocWindow>, path: PathBuf, origin: Origin, fragment: Option<String>) {
    let window = window.clone();
    glib::spawn_future_local(async move {
        let worker_path = path.clone();
        let parsed = gio::spawn_blocking(move || {
            md_viewer::read_document(&worker_path).map(|text| {
                let mut blocks = md_viewer::render(&text);
                md_viewer::resolve_images(&mut blocks, &worker_path);
                blocks
            })
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
        if let Some(fragment) = fragment {
            session.scroll_to_anchor(&fragment);
        }
    });
}

/// The raw destination of the link in `label` at label position `x, y`, if
/// the pointer is over one. Used for clicking, the pointer cursor and the
/// hover tooltip, so all three agree on the hit.
fn label_link_at(label: &gtk4::Label, links: &[LabelLink], x: f64, y: f64) -> Option<String> {
    let layout = label.layout();
    let scale = f64::from(pango::SCALE);
    let (hit, index, _trailing) =
        layout.xy_to_index((x * scale).round() as i32, (y * scale).round() as i32);
    if !hit {
        return None;
    }
    links
        .iter()
        .find(|link| link.range.contains(&(index as usize)))
        .map(|link| link.dest.clone())
}

/// The size limit for graphics shown in `widget`: [`graphics::SCREEN_MULTIPLE`]
/// times the monitor its window is on, in device pixels, or
/// [`graphics::FALLBACK_MAX_PX`] before the window is on a monitor.
fn screen_max_px(widget: &impl IsA<gtk4::Widget>) -> [u32; 2] {
    let monitor = widget
        .native()
        .and_then(|native| native.surface())
        .and_then(|surface| {
            let display = surface.display();
            display.monitor_at_surface(&surface)
        });
    match monitor {
        Some(monitor) => {
            let geometry = monitor.geometry();
            graphics::max_px_for(geometry.width(), geometry.height(), widget.scale_factor())
        }
        None => graphics::FALLBACK_MAX_PX,
    }
}

/// Inserts `text` at the end of `buffer` with the tags for `styles`.
fn insert_styled(buffer: &gtk4::TextBuffer, text: &str, styles: &[Style]) {
    let names: Vec<&str> = styles.iter().map(|style| style.tag_name()).collect();
    buffer.insert_with_tags_by_name(&mut buffer.end_iter(), text, &names);
}

/// The whole buffer's text with embedded objects (child anchors and
/// paintables) represented by U+FFFC, so that character indexes into the
/// string match buffer offsets. `TextBuffer::text` omits those characters,
/// which would drift every match after a table, code card, diagram or
/// formula; `slice` keeps them.
fn searchable_text(buffer: &gtk4::TextBuffer) -> String {
    buffer
        .slice(&buffer.start_iter(), &buffer.end_iter(), true)
        .to_string()
}

/// Builds the Pango attribute list for a label: the base style attributes
/// from `runs`, plus a light background for every highlighted range (a
/// stronger one for the current match). Labels are styled this way rather
/// than with markup, because mixing `use_markup` with `set_attributes`
/// makes GTK fold the attributes into the label's markup list, where they
/// cannot be cleared again.
fn label_attributes(
    runs: &[StyledRun],
    highlights: &[(std::ops::Range<usize>, bool)],
) -> pango::AttrList {
    let list = pango::AttrList::new();
    for run in runs {
        for style in &run.styles {
            insert_style_attributes(&list, *style, &run.range);
        }
    }
    for (range, current) in highlights {
        let background = if *current {
            SEARCH_CURRENT_RGB
        } else {
            SEARCH_MATCH_RGB
        };
        insert_attribute(
            &list,
            pango::AttrColor::new_background(
                layout::pango_channel(background.0),
                layout::pango_channel(background.1),
                layout::pango_channel(background.2),
            ),
            range,
        );
        insert_attribute(
            &list,
            pango::AttrColor::new_foreground(
                layout::pango_channel(SEARCH_TEXT_RGB.0),
                layout::pango_channel(SEARCH_TEXT_RGB.1),
                layout::pango_channel(SEARCH_TEXT_RGB.2),
            ),
            range,
        );
    }
    list
}

/// Applies the base styling of a label (its `runs`, with no highlights),
/// clearing any search highlights.
fn set_base_attributes(label: &gtk4::Label, runs: &[StyledRun]) {
    if runs.is_empty() {
        label.set_attributes(None);
    } else {
        label.set_attributes(Some(&label_attributes(runs, &[])));
    }
}

/// Inserts `attribute` into `list`, covering `range` (in bytes).
fn insert_attribute(
    list: &pango::AttrList,
    attribute: impl Into<pango::Attribute>,
    range: &std::ops::Range<usize>,
) {
    let mut attribute = attribute.into();
    attribute.set_start_index(range.start as u32);
    attribute.set_end_index(range.end as u32);
    list.insert(attribute);
}

/// Inserts the Pango attributes implementing `style` over `range`; the
/// attribute equivalent of `md_viewer::markup_attributes`.
fn insert_style_attributes(list: &pango::AttrList, style: Style, range: &std::ops::Range<usize>) {
    let muted = layout::pango_color(md_viewer::MUTED_COLOR).expect("valid colour");
    match style {
        Style::Heading(_) | Style::Strong => {
            insert_attribute(list, pango::AttrInt::new_weight(pango::Weight::Bold), range)
        }
        Style::Emphasis => {
            insert_attribute(list, pango::AttrInt::new_style(pango::Style::Italic), range)
        }
        Style::Strikethrough => {
            insert_attribute(list, pango::AttrInt::new_strikethrough(true), range)
        }
        Style::Code | Style::CodeBlock => {
            insert_attribute(list, pango::AttrString::new_family("monospace"), range);
            let background = layout::pango_color(md_viewer::CODE_BACKGROUND).expect("valid colour");
            insert_attribute(
                list,
                pango::AttrColor::new_background(background.0, background.1, background.2),
                range,
            );
            insert_attribute(
                list,
                pango::AttrInt::new_background_alpha(layout::percent_alpha(
                    md_viewer::CODE_BACKGROUND_ALPHA,
                )),
                range,
            );
        }
        Style::Quote | Style::Image => {
            insert_attribute(list, pango::AttrInt::new_style(pango::Style::Italic), range);
            insert_attribute(
                list,
                pango::AttrColor::new_foreground(muted.0, muted.1, muted.2),
                range,
            );
        }
        Style::Link => {
            insert_attribute(
                list,
                pango::AttrInt::new_underline(pango::Underline::Single),
                range,
            );
            let link = layout::pango_color(md_viewer::LINK_COLOR).expect("valid colour");
            insert_attribute(
                list,
                pango::AttrColor::new_foreground(link.0, link.1, link.2),
                range,
            );
        }
        Style::Rule => insert_attribute(
            list,
            pango::AttrColor::new_foreground(muted.0, muted.1, muted.2),
            range,
        ),
    }
}

/// Registers one text tag per [`Style`] in `table`, plus the display math
/// centring tag and the two search tags.
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
    let mut match_background = gdk::RGBA::parse("#ffd500").expect("valid colour");
    match_background.set_alpha(0.45);
    table.add(
        &gtk4::TextTag::builder()
            .name(SEARCH_MATCH_TAG)
            .background_rgba(&match_background)
            .build(),
    );
    let mut current_background = gdk::RGBA::parse("#ff9100").expect("valid colour");
    current_background.set_alpha(0.6);
    table.add(
        &gtk4::TextTag::builder()
            .name(SEARCH_CURRENT_TAG)
            .background_rgba(&current_background)
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
        /// The device-pixel texture (`None` while a spinner stands in).
        pub texture: RefCell<Option<gdk::Texture>>,
        /// The spinner drawn instead of a dropped texture, with the handler
        /// forwarding its animation frames.
        pub spinner: RefCell<Option<(gdk::Paintable, glib::SignalHandlerId)>>,
        /// The message drawn instead of a texture the document's memory
        /// budget had no room for, in the widget's text colour (dimmed).
        pub message: RefCell<Option<(gtk4::pango::Layout, gdk::RGBA)>>,
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

        /// Draws the texture scaled into the requested area, or the spinner
        /// centred in it (at most 32 pixels, and not at all in a tiny area).
        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let Some(gtk_snapshot) = snapshot.downcast_ref::<gtk4::Snapshot>() else {
                return;
            };
            if let Some(texture) = self.texture.borrow().clone() {
                let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
                gtk_snapshot.append_texture(&texture, &bounds);
            } else if let Some((layout, color)) = &*self.message.borrow() {
                let bounds = graphene::Rect::new(0.0, 0.0, width as f32, height as f32);
                gtk_snapshot.push_clip(&bounds);
                layout.set_width((width * f64::from(gtk4::pango::SCALE)) as i32);
                let (_, text_height) = layout.pixel_size();
                gtk_snapshot.save();
                gtk_snapshot.translate(&graphene::Point::new(
                    0.0,
                    ((height - f64::from(text_height)) / 2.0).max(0.0) as f32,
                ));
                gtk_snapshot.append_layout(layout, color);
                gtk_snapshot.restore();
                gtk_snapshot.pop();
            } else if let Some((spinner, _)) = &*self.spinner.borrow() {
                let size = super::layout::spinner_size(width, height);
                if size > 0.0 {
                    gtk_snapshot.save();
                    gtk_snapshot.translate(&graphene::Point::new(
                        ((width - size) / 2.0) as f32,
                        ((height - size) / 2.0) as f32,
                    ));
                    spinner.snapshot(snapshot, size, size);
                    gtk_snapshot.restore();
                }
            }
        }
    }
}

/// An image's shared pixel buffer, handed to GDK without copying it.
struct SharedPixels(std::sync::Arc<Vec<u8>>);

impl AsRef<[u8]> for SharedPixels {
    /// The pixels.
    fn as_ref(&self) -> &[u8] {
        &self.0
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
            &glib::Bytes::from_owned(SharedPixels(image.pixels.clone())),
            image.width as usize * 4,
        );
        let imp = self.imp();
        if let Some((spinner, handler)) = imp.spinner.take() {
            spinner.disconnect(handler);
        }
        imp.message.replace(None);
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

    /// Frees the texture, keeping the logical size, and draws a spinner
    /// (animated while `widget` is mapped) in its place until
    /// [`ScaledTexture::set_image`] gives it pixels again.
    fn show_spinner(&self, widget: &gtk4::Widget) {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        let spinner = adw::SpinnerPaintable::new(Some(widget));
        let weak = self.downgrade();
        let handler = spinner.connect_invalidate_contents(move |_| {
            if let Some(texture) = weak.upgrade() {
                texture.invalidate_contents();
            }
        });
        let imp = self.imp();
        imp.texture.replace(None);
        imp.message.replace(None);
        if let Some((old, handler)) = imp.spinner.replace(Some((spinner.upcast(), handler))) {
            old.disconnect(handler);
        }
        self.invalidate_contents();
    }

    /// Frees the texture, keeping the logical size, and draws `text` (laid
    /// out with `widget`'s font, centred, wrapping, dimmed) in its place
    /// until [`ScaledTexture::set_image`] gives it pixels again.
    fn show_message(&self, widget: &gtk4::Widget, text: &str) {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        let layout = widget.create_pango_layout(Some(text));
        layout.set_alignment(gtk4::pango::Alignment::Center);
        layout.set_wrap(gtk4::pango::WrapMode::WordChar);
        let color = widget.color();
        let dimmed = gdk::RGBA::new(
            color.red(),
            color.green(),
            color.blue(),
            color.alpha() * 0.55,
        );
        let imp = self.imp();
        imp.texture.replace(None);
        if let Some((spinner, handler)) = imp.spinner.take() {
            spinner.disconnect(handler);
        }
        imp.message.replace(Some((layout, dimmed)));
        self.invalidate_contents();
    }

    /// The message shown in place of the texture, if any.
    #[cfg(test)]
    fn message(&self) -> Option<String> {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        self.imp()
            .message
            .borrow()
            .as_ref()
            .map(|(layout, _)| layout.text().to_string())
    }

    /// True while a spinner stands in for a dropped texture.
    #[cfg(test)]
    fn shows_spinner(&self) -> bool {
        use gtk4::subclass::prelude::ObjectSubclassIsExt;
        self.imp().spinner.borrow().is_some()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// Initializes GTK once for the tests that need it, reporting whether a
    /// display was available (tests that need one are skipped otherwise).
    fn gtk_ready() -> bool {
        static READY: OnceLock<bool> = OnceLock::new();
        *READY.get_or_init(|| gtk4::init().is_ok())
    }

    /// Match highlights use Pango's 16-bit colour range: the intended
    /// yellow background is full red + green, not the near-black of an
    /// 8-bit value passed straight through.
    #[test]
    fn label_attributes_highlight_16_bit_colours() {
        let list = label_attributes(&[], &[(0..1, false)]);
        let mut colours: Vec<String> = list
            .iterator()
            .attrs()
            .into_iter()
            .filter_map(|attr| attr.downcast::<pango::AttrColor>().ok())
            .map(|color| color.color().to_str().to_string())
            .collect();
        colours.sort();
        assert_eq!(
            colours,
            vec!["#000000000000".to_string(), "#ffffd5d50000".to_string()]
        );
    }

    /// The base style runs of a label become the matching Pango attributes:
    /// bold, italic, strikethrough, monospace (with background and alpha),
    /// link underline and quote foreground, each over its own byte range.
    #[test]
    fn label_attributes_include_styles() {
        let runs = vec![
            StyledRun {
                range: 0..3,
                styles: vec![Style::Strong, Style::Emphasis],
            },
            StyledRun {
                range: 3..6,
                styles: vec![Style::Code],
            },
            StyledRun {
                range: 6..8,
                styles: vec![Style::Link],
            },
        ];
        let list = label_attributes(&runs, &[]);
        let mut ints: Vec<(pango::AttrType, u32, u32)> = Vec::new();
        let mut strings: Vec<(pango::AttrType, u32, u32)> = Vec::new();
        for attrs in list.iterator() {
            for attr in attrs {
                if let Some(a) = attr.downcast_ref::<pango::AttrInt>() {
                    ints.push((a.type_(), a.start_index(), a.end_index()));
                }
                if let Some(a) = attr.downcast_ref::<pango::AttrString>() {
                    strings.push((a.type_(), a.start_index(), a.end_index()));
                }
            }
        }
        assert!(ints.contains(&(pango::AttrType::Weight, 0, 3)));
        assert!(ints.contains(&(pango::AttrType::Style, 0, 3)));
        assert!(ints.contains(&(pango::AttrType::BackgroundAlpha, 3, 6)));
        assert!(ints.contains(&(pango::AttrType::Underline, 6, 8)));
        assert!(strings.contains(&(pango::AttrType::Family, 3, 6)));
    }

    /// Every visual style produces Pango attributes over its range, so each
    /// arm of `insert_style_attributes` is exercised (headings and strong
    /// share the bold arm, code and code blocks the monospace arm, quote and
    /// image the muted italic arm, and rules the muted foreground arm).
    #[test]
    fn label_attributes_cover_every_style() {
        for style in Style::ALL {
            let runs = vec![StyledRun {
                range: 0..2,
                styles: vec![style],
            }];
            let list = label_attributes(&runs, &[]);
            let mut count = 0;
            for attrs in list.iterator() {
                count += attrs.into_iter().count();
            }
            assert!(count > 0, "{style:?} produced no attributes");
        }
    }

    /// Checks that build GTK widgets or need a display run together in this
    /// one test (GTK is not thread-safe), skipped when GTK cannot
    /// initialize: the text searched for the buffer keeps character indexes
    /// aligned with buffer offsets (an embedded child anchor becomes U+FFFC
    /// via `slice`), replacing a plain-text label's attributes drops its
    /// previous highlights, every style gets a text tag, code cards show
    /// their code and language, a scaled texture stretches its logical size,
    /// and a label's link hit-testing finds the destination under a point.
    #[test]
    fn gtk_helpers() {
        if !gtk_ready() {
            return;
        }

        let buffer = gtk4::TextBuffer::new(None);
        buffer.insert(&mut buffer.end_iter(), "ab");
        buffer.create_child_anchor(&mut buffer.end_iter());
        buffer.insert(&mut buffer.end_iter(), "cd");
        let text = searchable_text(&buffer);
        assert_eq!(text.chars().count(), buffer.end_iter().offset() as usize);
        assert_eq!(text.chars().filter(|&c| c == '\u{fffc}').count(), 1);
        assert_eq!(
            buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .chars()
                .count(),
            4
        );

        let label = gtk4::Label::new(Some("Alice coffee"));
        let background_ranges = |label: &gtk4::Label| {
            let layout = label.layout();
            let Some(attrs) = layout.attributes() else {
                return Vec::new();
            };
            let mut ranges = Vec::new();
            for attrs in attrs.iterator() {
                for attr in attrs {
                    if let Ok(color) = attr.downcast::<pango::AttrColor>() {
                        if color.type_() == pango::AttrType::Background {
                            ranges.push((color.start_index(), color.end_index()));
                        }
                    }
                }
            }
            ranges
        };
        label.set_attributes(Some(&label_attributes(&[], &[(0..1, false)])));
        assert!(background_ranges(&label).contains(&(0, 1)));
        label.set_attributes(Some(&label_attributes(&[], &[(6..12, false)])));
        let ranges = background_ranges(&label);
        assert!(!ranges.contains(&(0, 1)), "{ranges:?}");
        assert!(ranges.contains(&(6, 12)), "{ranges:?}");

        // Every style, plus display math and the two search tags, is
        // registered; a heading's tag carries its name.
        let table = gtk4::TextTagTable::new();
        create_tags(&table);
        for style in Style::ALL {
            assert!(table.lookup(style.tag_name()).is_some(), "{style:?}");
        }
        assert!(table.lookup(DISPLAY_MATH_TAG).is_some());
        assert!(table.lookup(SEARCH_MATCH_TAG).is_some());
        assert!(table.lookup(SEARCH_CURRENT_TAG).is_some());
        assert_eq!(tag_for(Style::Heading(1)).name().as_deref(), Some("h1"));
        assert_eq!(tag_for(Style::Code).name().as_deref(), Some("code"));

        // A code card shows the code verbatim in its text label and names the
        // language in its header.
        let code = CodeBlock {
            language: Some("rust".to_string()),
            text: "fn main() {}".to_string(),
            indent: 0,
        };
        let (card, code_label) = code_card(&code);
        assert_eq!(code_label.text().as_str(), "fn main() {}");
        let header = card.first_child().expect("card starts with its header");
        let language = header
            .first_child()
            .expect("header starts with the language");
        assert_eq!(
            language
                .downcast::<gtk4::Label>()
                .expect("language label")
                .text()
                .as_str(),
            "rust"
        );

        // Stretching a scaled texture scales its logical size.
        let image = Image {
            width: 40,
            height: 20,
            scale: 2.0,
            depth: 0.0,
            pixels: vec![0; 40 * 20 * 4].into(),
            labels: Vec::new(),
            max_zoom: 1.0,
        };
        let texture = ScaledTexture::new(&image);
        assert_eq!(texture.scale_size(2.0), (40.0, 20.0));

        // A click inside the first characters finds a link covering them; a
        // point in a gap or with no links finds nothing.
        let link_label = gtk4::Label::new(Some("Alice coffee"));
        let (_, height) = link_label.layout().pixel_size();
        let y = f64::from(height) / 2.0;
        let links = vec![LabelLink {
            range: 0..5,
            dest: "u".to_string(),
        }];
        assert_eq!(
            label_link_at(&link_label, &links, 0.5, y),
            Some("u".to_string())
        );
        let gap = vec![LabelLink {
            range: 10..20,
            dest: "v".to_string(),
        }];
        assert_eq!(label_link_at(&link_label, &gap, 0.5, y), None);
        assert_eq!(label_link_at(&link_label, &[], 0.5, y), None);

        session_checks();
    }

    /// Checks on a real window and document session (part of
    /// [`gtk_helpers`], which owns the GTK thread): every heading of a text
    /// block is registered at the buffer offset of its own first character,
    /// with repeats suffixed, so `#fragment` links find each of them; an
    /// anchor requested before its heading is appended is remembered.
    fn session_checks() {
        adw::init().expect("libadwaita initializes once GTK does");
        let app = adw::Application::builder()
            .application_id("io.github.TheHolm.md_viewer.Tests")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        app.register(None::<&gio::Cancellable>)
            .expect("the test application registers");
        let window = DocWindow::new(&app);
        let markdown = "# Intro\n\nSome text.\n\n## Usage\n\n- item\n\n## Usage\n\n## Über uns\n";
        let session =
            window.show_document(Path::new("/doc/readme.md"), md_viewer::render(markdown));
        session.start(RasterSettings::default());
        let buffer = session.view.buffer();
        let text_at = |fragment: &str| {
            let offset = session.anchors.borrow().lookup(fragment)?;
            let start = buffer.iter_at_offset(offset);
            let mut end = start;
            end.forward_to_line_end();
            Some(buffer.text(&start, &end, false).to_string())
        };
        assert_eq!(text_at("intro").as_deref(), Some("Intro"));
        assert_eq!(text_at("usage").as_deref(), Some("Usage"));
        assert_eq!(text_at("usage-1").as_deref(), Some("Usage"));
        assert_eq!(text_at("über-uns").as_deref(), Some("Über uns"));
        let first = session.anchors.borrow().lookup("usage");
        let second = session.anchors.borrow().lookup("usage-1");
        assert!(first < second, "{first:?} {second:?}");
        // A clicked fragment is resolved, decoded and found.
        session.click_link("#%C3%BCber-uns");
        assert!(session.pending_anchor.borrow().is_none());
        // A fragment naming no heading leaves nothing pending; `#` alone
        // scrolls to the top.
        session.click_link("#later");
        assert!(session.pending_anchor.borrow().is_none());
        session.click_link("#");
        assert_eq!(session.scrolled.vadjustment().value(), 0.0);
        // Once the page has settled, the frame callback stops; a change
        // (here a held spot, as a zoom makes) starts it again.
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while session.ticking.get() {
            assert!(
                std::time::Instant::now() < deadline,
                "the frame callback stops"
            );
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        session.hold_anchor();
        assert!(session.ticking.get());
        session.release_anchor();
        window.window.close();
        resize_after_idle_checks(&app);
        far_heading_checks(&app);
        size_limit_checks(&app);
        link_safety_checks(&app);
    }

    /// Checks of link safety with GIO's real type detection (part of
    /// [`gtk_helpers`]): images, PDFs and plain text are safe; scripts,
    /// executables, launchers, a text file that is really a script and a
    /// file whose name lies are potentially dangerous; clicking one shows
    /// the confirmation dialog (and opens nothing), whose text gives the
    /// reason; tooltips say what a click does.
    fn link_safety_checks(app: &adw::Application) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temporary directory");
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).expect("write file");
            path
        };
        let mut png = Vec::new();
        image::RgbaImage::new(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode PNG");
        let risk = |path: &Path| {
            let facts = file_facts(path).expect("file exists");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            links::local_file_risk(&name, &facts)
        };
        assert_eq!(risk(&write("pic.png", &png)), None);
        assert_eq!(risk(&write("notes.txt", b"just some notes\n")), None);
        assert_eq!(
            risk(&write("doc.pdf", b"%PDF-1.4\n%\xe2\xe3\n1 0 obj\n")),
            None
        );
        let script = write("run.sh", b"#!/bin/sh\necho hi\n");
        assert!(risk(&script).is_some());
        let tool = write(
            "tool",
            b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        );
        assert!(risk(&tool).is_some());
        let marked = write("marked.png", &png);
        std::fs::set_permissions(&marked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(risk(&marked).unwrap().contains("executable"));
        let launcher = write("app.desktop", b"[Desktop Entry]\nExec=rm -rf ~\n");
        assert!(risk(&launcher).unwrap().contains("launcher"));
        let fake = write("fake.txt", b"#!/bin/sh\nrm -rf ~\n");
        assert!(risk(&fake).is_some());
        let disguised = write(
            "cat.png",
            b"\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        );
        assert!(risk(&disguised).is_some());
        assert_eq!(file_facts(&dir.path().join("missing")), None);
        let fifo = dir.path().join("pipe.png");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        assert!(risk(&fifo).unwrap().contains("not a regular file"));

        let body = confirm_body(Path::new("/d/run.sh"), "it is a program or script");
        assert!(body.starts_with(
            "This link opens potentially dangerous content: it is a program or script."
        ));
        assert!(body.ends_with("/d/run.sh"));

        // Clicking a dangerous file asks; a missing one or a refused link
        // does nothing; tooltips tell them apart.
        let document = write("doc.md", b"[run](run.sh)\n");
        let window = DocWindow::new(app);
        let session = window.show_document(&document, md_viewer::render("[run](run.sh)\n"));
        session.start(RasterSettings::default());
        assert!(window.window.visible_dialog().is_none());
        session.click_link("missing.zip");
        session.click_link("ftp://example.com/x");
        assert!(window.window.visible_dialog().is_none());
        session.click_link("run.sh");
        let dialog = window
            .window
            .visible_dialog()
            .expect("the confirmation dialog");
        let dialog = dialog
            .downcast::<adw::AlertDialog>()
            .expect("an alert dialog");
        assert_eq!(dialog.heading().as_deref(), Some("Open Dangerous Content?"));
        assert!(dialog.body().contains("could run code"));
        assert_eq!(dialog.default_response().as_deref(), Some("cancel"));
        assert_eq!(dialog.close_response(), "cancel");
        dialog.close();
        assert!(session
            .link_tooltip("run.sh")
            .starts_with("⚠ Potentially dangerous: "));
        assert!(session
            .link_tooltip("pic.png")
            .starts_with("Open with the default application: "));
        assert!(session
            .link_tooltip("missing.zip")
            .starts_with("File not found: "));
        assert!(session
            .link_tooltip("ftp://example.com/x")
            .starts_with("Not opened: "));
        window.window.close();
    }

    /// A change of the view's width wakes the stopped frame callback
    /// (part of [`gtk_helpers`]): GTK reports a new width through the
    /// horizontal adjustment's `changed` signal, and the callback then
    /// re-sizes tables, pictures and code cards. (Headless X without a
    /// window manager does not resize windows, so the signal is emitted
    /// directly.)
    fn resize_after_idle_checks(app: &adw::Application) {
        let window = DocWindow::new(app);
        let session = window.show_document(
            Path::new("/doc/table.md"),
            md_viewer::render("| a | b |\n|---|---|\n| 1 | 2 |\n"),
        );
        session.start(RasterSettings::default());
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !(session.progress.borrow().all_shown() && !session.ticking.get()) {
            assert!(std::time::Instant::now() < deadline, "the page goes idle");
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        session
            .scrolled
            .hadjustment()
            .emit_by_name::<()>("changed", &[]);
        assert!(session.ticking.get());
        window.window.close();
    }

    /// A link to a heading far beyond what lazy rendering has put on the
    /// page renders up to it and scrolls there (part of [`gtk_helpers`]).
    fn far_heading_checks(app: &adw::Application) {
        let mut markdown = String::from("# Top\n\n[far](#far-away)\n\n");
        for i in 0..400 {
            markdown += &format!("Formula $x_{{{i}}}$ here.\n\n");
        }
        markdown += "## Far away\n\nThe end.\n";
        let window = DocWindow::new(app);
        let session = window.show_document(Path::new("/doc/far.md"), md_viewer::render(&markdown));
        session.start(RasterSettings::default());
        let context = glib::MainContext::default();
        for _ in 0..20 {
            context.iteration(false);
        }
        assert!(session.anchors.borrow().lookup("far-away").is_none());
        let summary = session.search("Formula", SearchMode::SmartCase);
        assert_eq!(summary.total, 400);
        session.click_link("#far-away");
        assert_eq!(session.pending_anchor.borrow().as_deref(), Some("far-away"));
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while session.pending_anchor.borrow().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "the heading is reached"
            );
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(session.anchors.borrow().lookup("far-away").is_some());
        // The search, refreshed in throttled steps while the blocks came in,
        // still finds every match, now all on the page.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while session.refresh_waiting.get() {
            assert!(std::time::Instant::now() < deadline, "the refresh runs");
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        let summary = session.summary();
        assert_eq!(summary.total, 400);
        assert!(!summary.pending);
        window.window.close();
    }

    /// Checks of the graphics size limit on a real window (part of
    /// [`gtk_helpers`]): an image and an inline formula too large at 100%
    /// show why in their place; a window zoomed in further than a wide
    /// picture allows zooms out to the picture's limit once it arrives, and
    /// the header bar shows the zoom with "(max)".
    fn size_limit_checks(app: &adw::Application) {
        let dir = tempfile::tempdir().expect("temporary directory");
        let svg = |name: &str, width: u32| {
            let path = dir.path().join(name);
            let svg = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"10\"/>"
            );
            std::fs::write(&path, svg).expect("write SVG");
            path
        };
        // Ten 1920 px screens are 19,200 px: this one allows 150%.
        svg("wide.svg", 12_800);
        svg("huge.svg", 30_000);
        let document = dir.path().join("doc.md");
        let markdown = "![wide](wide.svg)\n\n![huge](huge.svg)\n\nA $\\rule{100000em}{1em}$ b\n";
        let window = DocWindow::new(app);
        window.change_zoom(|zoom| zoom.set(2.0));
        let mut blocks = md_viewer::render(markdown);
        md_viewer::resolve_images(&mut blocks, &document);
        let session = window.show_document(&document, blocks);
        session.start(RasterSettings {
            zoom: 2.0,
            ..RasterSettings::default()
        });
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !(session.progress.borrow().all_shown() && session.outstanding.get() == 0) {
            assert!(std::time::Instant::now() < deadline, "the document loads");
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(5));
        }
        let buffer = session.view.buffer();
        let text = searchable_text(&buffer);
        assert!(text.contains("[image: huge — too large"), "{text}");
        assert!(
            text.contains("$\\rule{100000em}{1em}$ ⚠ too large"),
            "{text}"
        );
        assert!((window.zoom.get().level() - 1.5).abs() < 1e-3);
        assert!((window.zoom.get().max() - 1.5).abs() < 1e-3);
        assert!(window.zoom_indicator.is_visible());
        assert_eq!(window.zoom_indicator.label().as_deref(), Some("150% (max)"));
        // Zooming in further is refused; back at 100% the indicator hides.
        window.change_zoom(Zoom::zoom_in);
        assert!((window.zoom.get().level() - 1.5).abs() < 1e-3);
        window.change_zoom(Zoom::reset);
        assert!(!window.zoom_indicator.is_visible());
        window.window.close();
        budget_checks(app);
    }

    /// Runs the main loop until `session` has every block on the page and
    /// no render outstanding.
    fn settle(session: &Session) {
        let context = glib::MainContext::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            while context.iteration(false) {}
            if session.progress.borrow().all_shown()
                && session.outstanding.get() == 0
                && !session.budget_pending.get()
                && session.rezoom.borrow().is_none()
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the document settles");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Checks of the memory budget on a real window (part of
    /// [`gtk_helpers`]), with the budget lowered to fit four pictures: a
    /// long document with many pictures loads completely while holding no
    /// more than the budget; pictures far from the view are dropped, keeping
    /// their size and showing a spinner; scrolling to the end draws the
    /// pictures there again, and the ones at the top are dropped instead.
    fn budget_checks(app: &adw::Application) {
        let dir = tempfile::tempdir().expect("temporary directory");
        let mut markdown = String::new();
        for i in 0..12 {
            let path = dir.path().join(format!("p{i}.png"));
            image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 200, 255]))
                .save(&path)
                .expect("write picture");
            markdown += &format!("![p{i}](p{i}.png)\n\n");
            for line in 0..60 {
                markdown += &format!("Filler {i}.{line}\n\n");
            }
        }
        let document = dir.path().join("doc.md");
        let mut blocks = md_viewer::render(&markdown);
        md_viewer::resolve_images(&mut blocks, &document);
        let window = DocWindow::new(app);
        let session = window.show_document(&document, blocks);
        let picture_bytes = 200 * 100 * 4;
        session.budget.set(4 * picture_bytes);
        session.start(RasterSettings::default());
        settle(&session);
        // Render everything (as a search would) and settle again.
        session.render_to(usize::MAX);
        settle(&session);
        let count = |held: fn(Held) -> bool| {
            session
                .slots
                .borrow()
                .iter()
                .flatten()
                .filter(|slot| held(slot.held.get()))
                .count()
        };
        assert_eq!(count(|_| true), 12);
        assert!(session.held_bytes.get() <= session.budget.get());
        assert!(count(|held| held != Held::Drawn) >= 8);
        {
            let slots = session.slots.borrow();
            let first = slots[0].as_ref().expect("first picture");
            assert_eq!(first.held.get(), Held::Drawn);
            let last = slots[11].as_ref().expect("last picture");
            assert_ne!(last.held.get(), Held::Drawn);
            assert!(last.texture.shows_spinner());
            assert_eq!(last.bytes.get(), 0);
            assert_eq!(last.texture.intrinsic_width(), 200);
            assert_eq!(last.texture.intrinsic_height(), 100);
        }
        // Scroll to the last picture: it is drawn again, the first one
        // dropped, and the budget still holds.
        let offset = session.slots.borrow()[11].as_ref().expect("last").offset;
        let (top, _) = session
            .view
            .line_yrange(&session.view.buffer().iter_at_offset(offset));
        session.scrolled.vadjustment().set_value(f64::from(top));
        settle(&session);
        {
            let slots = session.slots.borrow();
            let last = slots[11].as_ref().expect("last picture");
            assert_eq!(last.held.get(), Held::Drawn);
            assert!(!last.texture.shows_spinner());
            assert_eq!(last.bytes.get(), picture_bytes);
            assert_ne!(slots[0].as_ref().expect("first").held.get(), Held::Drawn);
        }
        assert!(session.held_bytes.get() <= session.budget.get());
        // A zoom re-renders only what is drawn; dropped pictures keep their
        // spinner and are stretched with the page.
        session.zoom_to(1.5);
        settle(&session);
        {
            let slots = session.slots.borrow();
            let last = slots[11].as_ref().expect("last picture");
            assert_eq!(last.held.get(), Held::Drawn);
            assert_eq!(last.texture.intrinsic_width(), 300);
            let first = slots[0].as_ref().expect("first picture");
            assert!(first.texture.shows_spinner());
            assert_eq!(first.texture.intrinsic_width(), 300);
        }
        assert!(session.held_bytes.get() <= session.budget.get());
        window.window.close();
        shared_and_limited_checks(app);
    }

    /// Budget checks on a real window (part of [`gtk_helpers`]): one
    /// formula shown many times shares its pixels and counts (about) once;
    /// pictures near the view that alone exceed the budget show the
    /// document limit message in their place (keeping their size), and are
    /// drawn once there is room.
    fn shared_and_limited_checks(app: &adw::Application) {
        let markdown = "Same $x^2+y^2$ formula. ".repeat(50);
        let window = DocWindow::new(app);
        let session = window.show_document(Path::new("/doc/same.md"), md_viewer::render(&markdown));
        session.start(RasterSettings::default());
        settle(&session);
        let one = {
            let slots = session.slots.borrow();
            assert_eq!(slots.iter().flatten().count(), 50);
            slots
                .iter()
                .flatten()
                .map(|slot| slot.size.get())
                .max()
                .unwrap()
        };
        session.enforce_budget();
        assert!(session.held_bytes.get() > 0);
        assert_eq!(session.held_bytes.get(), one);
        window.window.close();

        let dir = tempfile::tempdir().expect("temporary directory");
        let mut markdown = String::new();
        for i in 0..3 {
            let path = dir.path().join(format!("p{i}.png"));
            image::RgbaImage::from_pixel(100, 50, image::Rgba([0, 120, 0, 255]))
                .save(&path)
                .expect("write picture");
            markdown += &format!("![p{i}](p{i}.png)\n\n");
        }
        let document = dir.path().join("doc.md");
        let mut blocks = md_viewer::render(&markdown);
        md_viewer::resolve_images(&mut blocks, &document);
        let window = DocWindow::new(app);
        let session = window.show_document(&document, blocks);
        let picture_bytes = 100 * 50 * 4;
        session.budget.set(picture_bytes);
        session.start(RasterSettings::default());
        settle(&session);
        let states = || {
            session
                .slots
                .borrow()
                .iter()
                .flatten()
                .map(|slot| slot.held.get())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            states().iter().filter(|held| **held == Held::Drawn).count(),
            1
        );
        assert_eq!(
            states()
                .iter()
                .filter(|held| **held == Held::Limited)
                .count(),
            2
        );
        for slot in session.slots.borrow().iter().flatten() {
            if slot.held.get() == Held::Limited {
                assert_eq!(
                    slot.texture.message().as_deref(),
                    Some(DOCUMENT_LIMIT_MESSAGE)
                );
                assert_eq!(slot.texture.intrinsic_width(), 100);
            }
        }
        assert!(session.held_bytes.get() <= picture_bytes);
        // With room for all three, the limited ones are drawn.
        session.budget.set(3 * picture_bytes);
        session.schedule_budget();
        settle(&session);
        assert!(
            states().iter().all(|held| *held == Held::Drawn),
            "{:?}",
            states()
        );
        assert!(session
            .slots
            .borrow()
            .iter()
            .flatten()
            .all(|slot| slot.texture.message().is_none()));
        window.window.close();
    }
}

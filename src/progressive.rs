//! Progressive, parallel rendering of a document's graphics.
//!
//! Opening a document no longer waits for every formula and diagram: the
//! graphics are numbered in document order ([`jobs`]), rendered by a pool of
//! worker threads ([`Pool`]) that works through them in that order, and the
//! GUI appends each block to the page as soon as it and every block before
//! it are complete ([`Progress`]). Content is only ever added at the end, so
//! nothing already on the page moves when a graphic arrives. The page is
//! revealed once the first screen is full ([`Phase`]).
//!
//! Zooming re-renders the graphics through the same pool, visible ones
//! first ([`zoom_order`]); results of an older run are told apart by a
//! generation number and dropped.

use crate::graphics::{Cache, Graphic, RasterSettings, Rendered};
use crate::{Block, ImageState, Inline};
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Calls `visit` for every graphic in `blocks`, in document order (formulas
/// and local images in text, then formulas and images in table cells row by
/// row, then diagrams), with the index of the block it is in, what to render,
/// and where its result goes. Images that are not local files are skipped.
pub fn for_each_graphic(
    blocks: &mut [Block],
    mut visit: impl FnMut(usize, Graphic, &mut Option<Rendered>),
) {
    for (index, block) in blocks.iter_mut().enumerate() {
        match block {
            Block::Text(inlines) => visit_inlines(index, inlines, true, true, &mut visit),
            Block::Table(table) => {
                for cell in table
                    .header
                    .iter_mut()
                    .chain(table.rows.iter_mut().flatten())
                {
                    // A table cell is a single line, so its images are always
                    // inline, never standalone.
                    visit_inlines(index, cell, true, false, &mut visit);
                }
            }
            Block::Mermaid(diagram) => {
                let graphic = Graphic::Diagram {
                    source: diagram.source.clone(),
                };
                visit(index, graphic, &mut diagram.image);
            }
            Block::Code(_) => {}
        }
    }
}

/// Visits the formulas among `inlines`, and - when `images` is true - the
/// local images too (see [`for_each_graphic`]). An image is marked standalone
/// only when `allow_standalone` is true (text blocks) and it is alone on its
/// line; images in table cells are always inline.
fn visit_inlines(
    index: usize,
    inlines: &mut [Inline],
    images: bool,
    allow_standalone: bool,
    visit: &mut impl FnMut(usize, Graphic, &mut Option<Rendered>),
) {
    let standalone: Vec<bool> = (0..inlines.len())
        .map(|at| {
            allow_standalone
                && images
                && matches!(inlines[at], Inline::Image(_))
                && crate::image_is_standalone(inlines, at)
        })
        .collect();
    for (at, inline) in inlines.iter_mut().enumerate() {
        match inline {
            Inline::Math(math) => {
                let graphic = Graphic::Math {
                    tex: math.tex.clone(),
                    display: math.display,
                };
                visit(index, graphic, &mut math.image);
            }
            Inline::Image(image) if images => {
                if let ImageState::Local(path) = &image.state {
                    let graphic = Graphic::Image {
                        path: path.clone(),
                        inline: !standalone[at],
                    };
                    visit(index, graphic, &mut image.image);
                }
            }
            _ => {}
        }
    }
}

/// One graphic to render, identified by its position in document order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// The graphic's number in document order, starting at 0.
    pub id: usize,
    /// The index of the block the graphic is in.
    pub block: usize,
    /// What to render.
    pub graphic: Graphic,
}

/// Every graphic in `blocks` as a job, in document order.
pub fn jobs(blocks: &mut [Block]) -> Vec<Job> {
    let mut jobs = Vec::new();
    for_each_graphic(blocks, |block, graphic, _| {
        jobs.push(Job {
            id: jobs.len(),
            block,
            graphic,
        });
    });
    jobs
}

/// Stores `rendered` as the result of graphic `id` in `blocks`. Returns
/// false (and stores nothing) if there is no such graphic.
pub fn place(blocks: &mut [Block], id: usize, rendered: Rendered) -> bool {
    let mut rendered = Some(rendered);
    let mut next = 0;
    for_each_graphic(blocks, |_, _, image| {
        if next == id {
            *image = rendered.take();
        }
        next += 1;
    });
    rendered.is_none()
}

/// Tracks which graphics have been rendered and which blocks are on the
/// page. Blocks go on the page strictly in order, and only once all of their
/// graphics are rendered, so a slow graphic holds back the blocks after it
/// but nothing on the page ever moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    /// The block each graphic (by id) belongs to.
    block_of: Vec<usize>,
    /// Whether each graphic (by id) has its result.
    rendered: Vec<bool>,
    /// How many of each block's graphics are still missing.
    missing: Vec<usize>,
    /// How many blocks, from the start, are on the page.
    shown: usize,
}

impl Progress {
    /// Starts tracking a document of `blocks` blocks whose graphics are
    /// `jobs` (as returned by [`jobs`]); nothing is shown or rendered yet.
    pub fn new(blocks: usize, jobs: &[Job]) -> Self {
        let mut missing = vec![0; blocks];
        for job in jobs {
            missing[job.block] += 1;
        }
        Progress {
            block_of: jobs.iter().map(|job| job.block).collect(),
            rendered: vec![false; jobs.len()],
            missing,
            shown: 0,
        }
    }

    /// Records that graphic `id` has its result. Recording the same graphic
    /// twice, or an unknown one, changes nothing.
    pub fn complete(&mut self, id: usize) {
        let Some(rendered) = self.rendered.get_mut(id) else {
            return;
        };
        if !*rendered {
            *rendered = true;
            self.missing[self.block_of[id]] -= 1;
        }
    }

    /// True if graphic `id` is in a block already on the page.
    pub fn is_shown(&self, id: usize) -> bool {
        self.block_of
            .get(id)
            .is_some_and(|block| *block < self.shown)
    }

    /// The blocks that can go on the page now: the complete blocks directly
    /// after the ones already shown. They count as shown from now on.
    pub fn take_ready(&mut self) -> Range<usize> {
        let start = self.shown;
        while self.shown < self.missing.len() && self.missing[self.shown] == 0 {
            self.shown += 1;
        }
        start..self.shown
    }

    /// True once every block is on the page.
    pub fn all_shown(&self) -> bool {
        self.shown == self.missing.len()
    }

    /// How many blocks, from the start, are on the page.
    pub fn shown(&self) -> usize {
        self.shown
    }

    /// Forgets the results of every graphic not on the page yet, but only up
    /// to `frontier` (the number of graphics submitted for this run; graphics
    /// beyond it were never rendered). Their earlier results no longer count
    /// and they are returned, in order, to be rendered again. Graphics beyond
    /// the frontier are left untouched, to be rendered lazily when scrolled
    /// near.
    pub fn reset_unshown(&mut self, frontier: usize) -> Vec<usize> {
        let mut reset = Vec::new();
        for id in 0..frontier.min(self.rendered.len()) {
            if !self.is_shown(id) {
                if self.rendered[id] {
                    self.rendered[id] = false;
                    self.missing[self.block_of[id]] += 1;
                }
                reset.push(id);
            }
        }
        reset
    }
}

/// The order graphics are re-rendered in after a zoom: the ones on the page
/// from the first visible one on, then those not on the page yet (needed
/// before more of the page can appear), then those above the visible part.
///
/// `shown` lists the graphics on the page with their position (any value
/// that grows through the document, e.g. a text buffer offset), in document
/// order; `first_visible` is the position at the top of the window.
pub fn zoom_order(shown: &[(usize, i32)], first_visible: i32, unshown: &[usize]) -> Vec<usize> {
    let split = shown.partition_point(|(_, position)| *position < first_visible);
    shown[split..]
        .iter()
        .map(|(id, _)| *id)
        .chain(unshown.iter().copied())
        .chain(shown[..split].iter().map(|(id, _)| *id))
        .collect()
}

/// How far loading a document has got, which decides what the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The first screen is not full yet: the page is hidden behind a
    /// spinner in the middle of the window.
    FirstScreen,
    /// The page is visible and more is being added below it: a small
    /// spinner shows in the header bar.
    Background,
    /// Everything is on the page.
    Done,
}

impl Phase {
    /// The phase after an update: `Done` once every block is shown; out of
    /// `FirstScreen` once the page is at least as tall as the window (a
    /// window not laid out yet, 0 high, never counts as full). Loading never
    /// goes back to an earlier phase.
    pub fn next(self, all_shown: bool, page_height: f64, window_height: f64) -> Phase {
        if all_shown {
            Phase::Done
        } else if self == Phase::FirstScreen && window_height > 0.0 && page_height >= window_height
        {
            Phase::Background
        } else {
            self
        }
    }
}

/// How long opening a document may take before the centre spinner appears,
/// in milliseconds, so it does not flash for documents that load at once.
pub const SPINNER_DELAY_MS: u64 = 150;

/// The number of worker threads to render `jobs` graphics with: one per
/// CPU (`cpus`), but no more than there are graphics, and at least one.
pub fn worker_count(cpus: usize, jobs: usize) -> usize {
    cpus.min(jobs).max(1)
}

/// Worker threads rendering a list of graphics in order.
///
/// The threads share one queue and take the next job from its front, so
/// graphics finish roughly in document order. Each result is passed to the
/// `deliver` callback (from a worker thread). The workers wait (rather than
/// finish) when the queue empties, so more jobs can be added with
/// [`Pool::submit`] - this is how lazy rendering hands out more work as the
/// user scrolls. Cancelling (or dropping the pool) stops the workers and
/// wakes any that are waiting; a result finished at that very moment may
/// still be delivered, so callers tag results with a generation and drop
/// stale ones.
pub struct Pool {
    /// The queue, wake-up channel and cancellation flag the workers share.
    shared: Arc<Shared>,
}

/// The state the workers of a [`Pool`] share: the job queue, a condition
/// variable to wait on when it is empty, and the cancellation flag.
struct Shared {
    /// The jobs still to render, taken from the front.
    queue: Mutex<VecDeque<Job>>,
    /// Signalled when jobs are added or the pool is cancelled.
    condvar: Condvar,
    /// Set to stop the workers.
    cancelled: AtomicBool,
}

impl Shared {
    /// Blocks until a job is available or the pool is cancelled, then returns
    /// it (`None` when cancelled).
    fn next_job(&self) -> Option<Job> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return None;
            }
            if let Some(job) = queue.pop_front() {
                return Some(job);
            }
            queue = self
                .condvar
                .wait(queue)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Pushes `jobs` onto the back of the queue and wakes the workers.
    fn submit(&self, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(jobs);
        self.condvar.notify_all();
    }

    /// Stops the workers and wakes any that are waiting for a job.
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.condvar.notify_all();
    }
}

impl Pool {
    /// Starts `threads` workers rendering `jobs` with `settings`, through
    /// the shared [`Cache`].
    pub fn start(
        jobs: Vec<Job>,
        settings: RasterSettings,
        threads: usize,
        deliver: impl Fn(usize, Rendered) + Send + Sync + 'static,
    ) -> Pool {
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::from(jobs)),
            condvar: Condvar::new(),
            cancelled: AtomicBool::new(false),
        });
        let deliver = Arc::new(deliver);
        for _ in 0..threads.max(1) {
            let (shared, deliver) = (shared.clone(), deliver.clone());
            std::thread::spawn(move || loop {
                let Some(job) = shared.next_job() else {
                    return;
                };
                let rendered = Cache::shared().render(&job.graphic, &settings);
                if shared.cancelled.load(Ordering::Relaxed) {
                    return;
                }
                deliver(job.id, rendered);
            });
        }
        Pool { shared }
    }

    /// Adds `jobs` to the back of the queue, waking the workers (see
    /// [`Pool`] for why they wait instead of finishing).
    pub fn submit(&self, jobs: Vec<Job>) {
        self.shared.submit(jobs);
    }

    /// Stops the workers (see [`Pool`] for results already on their way).
    pub fn cancel(&self) {
        self.shared.cancel();
    }
}

impl Drop for Pool {
    /// Dropping the pool cancels it.
    fn drop(&mut self) {
        self.cancel();
    }
}

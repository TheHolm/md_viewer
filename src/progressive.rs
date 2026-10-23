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
use crate::{Block, Inline};
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Calls `visit` for every graphic in `blocks`, in document order (formulas
/// in text, then in table cells row by row, and diagrams), with the index of
/// the block it is in, what to render, and where its result goes.
pub fn for_each_graphic(
    blocks: &mut [Block],
    mut visit: impl FnMut(usize, Graphic, &mut Option<Rendered>),
) {
    for (index, block) in blocks.iter_mut().enumerate() {
        match block {
            Block::Text(inlines) => visit_inlines(index, inlines, &mut visit),
            Block::Table(table) => {
                for cell in table
                    .header
                    .iter_mut()
                    .chain(table.rows.iter_mut().flatten())
                {
                    visit_inlines(index, cell, &mut visit);
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

/// Visits the formulas among `inlines` (see [`for_each_graphic`]).
fn visit_inlines(
    index: usize,
    inlines: &mut [Inline],
    visit: &mut impl FnMut(usize, Graphic, &mut Option<Rendered>),
) {
    for inline in inlines.iter_mut() {
        if let Inline::Math(math) = inline {
            let graphic = Graphic::Math {
                tex: math.tex.clone(),
                display: math.display,
            };
            visit(index, graphic, &mut math.image);
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

    /// Forgets the results of every graphic not on the page yet (they were
    /// rendered with settings that no longer apply, e.g. before a zoom) and
    /// returns their ids, in order, to be rendered again.
    pub fn reset_unshown(&mut self) -> Vec<usize> {
        let mut reset = Vec::new();
        for id in 0..self.rendered.len() {
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
/// `deliver` callback (from a worker thread). Cancelling (or dropping the
/// pool) stops the workers after the graphic each is working on; a result
/// finished at that very moment may still be delivered, so callers tag
/// results with a generation and drop stale ones.
pub struct Pool {
    /// Set to stop the workers.
    cancelled: Arc<AtomicBool>,
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
        let cancelled = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Mutex::new(VecDeque::from(jobs)));
        let deliver = Arc::new(deliver);
        for _ in 0..threads.max(1) {
            let (cancelled, queue, deliver) = (cancelled.clone(), queue.clone(), deliver.clone());
            std::thread::spawn(move || loop {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let job = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pop_front();
                let Some(job) = job else {
                    return;
                };
                let rendered = Cache::shared().render(&job.graphic, &settings);
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                deliver(job.id, rendered);
            });
        }
        Pool { cancelled }
    }

    /// Stops the workers (see [`Pool`] for results already on their way).
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for Pool {
    /// Dropping the pool cancels it.
    fn drop(&mut self) {
        self.cancel();
    }
}

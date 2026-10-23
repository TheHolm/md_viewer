//! Integration tests for progressive, parallel rendering
//! (`md_viewer::progressive`): numbering graphics, storing results, deciding
//! which blocks may go on the page, the loading phases, the order graphics
//! are re-rendered in after a zoom, and the worker pool.

use md_viewer::graphics::{Graphic, RasterSettings};
use md_viewer::progressive::{
    for_each_graphic, jobs, place, worker_count, zoom_order, Job, Phase, Pool, Progress,
    SPINNER_DELAY_MS,
};
use md_viewer::{render, Block, Inline};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

/// A document with graphics in text, in a table cell and in a diagram,
/// with a code block and plain text in between.
const DOCUMENT: &str = "Intro $a$ and $b$.\n\n\
                        | x |\n|---|\n| $c$ |\n\n\
                        ```mermaid\npie\n  \"A\" : 1\n```\n\n\
                        ```sh\nls\n```\n\n\
                        $$d$$\n\nThe end.";

/// The graphics `jobs` finds, as (block, short description) pairs.
fn summary(jobs: &[Job]) -> Vec<(usize, String)> {
    jobs.iter()
        .map(|job| {
            let what = match &job.graphic {
                Graphic::Math { tex, display } => {
                    format!("{tex}{}", if *display { "!" } else { "" })
                }
                Graphic::Diagram { .. } => "diagram".to_string(),
            };
            (job.block, what)
        })
        .collect()
}

/// Graphics are numbered in document order, across text, table cells and
/// diagrams; code blocks have none.
#[test]
fn jobs_in_document_order() {
    let mut blocks = render(DOCUMENT);
    let jobs = jobs(&mut blocks);
    assert!(jobs.iter().enumerate().all(|(index, job)| job.id == index));
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|block| match block {
            Block::Text(_) => "text",
            Block::Table(_) => "table",
            Block::Mermaid(_) => "diagram",
            Block::Code(_) => "code",
        })
        .collect();
    // Non-text blocks are separated by short text blocks (blank lines).
    assert_eq!(
        kinds,
        ["text", "table", "text", "diagram", "text", "code", "text"]
    );
    assert_eq!(
        summary(&jobs),
        [
            (0, "a".to_string()),
            (0, "b".to_string()),
            (1, "c".to_string()),
            (3, "diagram".to_string()),
            (6, "d!".to_string()),
        ]
    );
}

/// `for_each_graphic` hands out each graphic's result slot, in the same
/// order as `jobs`.
#[test]
fn for_each_graphic_visits_slots() {
    let mut blocks = render(DOCUMENT);
    let mut count = 0;
    for_each_graphic(&mut blocks, |_, _, image| {
        assert!(image.is_none());
        *image = Some(Err(format!("#{count}")));
        count += 1;
    });
    assert_eq!(count, 5);
    let mut seen = Vec::new();
    for_each_graphic(&mut blocks, |_, _, image| {
        seen.push(image.clone().unwrap().unwrap_err());
    });
    assert_eq!(seen, ["#0", "#1", "#2", "#3", "#4"]);
}

/// `place` stores a result with the right graphic and rejects unknown ids.
#[test]
fn place_stores_results() {
    let mut blocks = render(DOCUMENT);
    assert!(place(&mut blocks, 2, Err("cell".to_string())));
    assert!(!place(&mut blocks, 5, Err("none".to_string())));
    let Block::Table(table) = &blocks[1] else {
        panic!("expected the table");
    };
    let Inline::Math(math) = &table.rows[0][0][0] else {
        panic!("expected a formula in the cell");
    };
    assert_eq!(math.image, Some(Err("cell".to_string())));
    let mut stored = 0;
    for_each_graphic(&mut blocks, |_, _, image| {
        stored += image.is_some() as usize
    });
    assert_eq!(stored, 1);
}

/// Blocks go on the page in order, each once all its graphics are in: the
/// code block and the blocks before the first graphic come at once, a slow
/// graphic holds back the blocks after it, and completing a graphic twice
/// (or an unknown one) changes nothing.
#[test]
fn progress_shows_blocks_in_order() {
    let mut blocks = render(DOCUMENT);
    let jobs = jobs(&mut blocks);
    let mut progress = Progress::new(blocks.len(), &jobs);
    assert_eq!(progress.take_ready(), 0..0);
    progress.complete(2); // the table's formula, before the text's
    assert_eq!(progress.take_ready(), 0..0);
    progress.complete(0);
    progress.complete(0);
    progress.complete(99);
    assert_eq!(progress.take_ready(), 0..0);
    progress.complete(1);
    // The text, the table and the blank line after it; the diagram waits.
    assert_eq!(progress.take_ready(), 0..3);
    assert!(progress.is_shown(0) && progress.is_shown(2));
    assert!(!progress.is_shown(3) && !progress.is_shown(99));
    progress.complete(3);
    assert_eq!(progress.take_ready(), 3..6);
    assert!(!progress.all_shown());
    progress.complete(4);
    assert_eq!(progress.take_ready(), 6..7);
    assert!(progress.all_shown());
    assert_eq!(progress.take_ready(), 7..7);
}

/// A document without graphics can be shown all at once; an empty one is
/// shown from the start.
#[test]
fn progress_without_graphics() {
    let mut blocks = render("a\n\n```\nb\n```\n\nc");
    let jobs = jobs(&mut blocks);
    assert!(jobs.is_empty());
    let mut progress = Progress::new(blocks.len(), &jobs);
    assert_eq!(progress.take_ready(), 0..3);
    assert!(progress.all_shown());
    assert!(Progress::new(0, &[]).all_shown());
}

/// After a zoom, the graphics not on the page yet are rendered again: their
/// earlier results no longer count, the ones on the page are untouched.
#[test]
fn progress_reset_unshown() {
    let mut blocks = render(DOCUMENT);
    let jobs = jobs(&mut blocks);
    let mut progress = Progress::new(blocks.len(), &jobs);
    for id in [0, 1, 3] {
        progress.complete(id);
    }
    assert_eq!(progress.take_ready(), 0..1);
    assert_eq!(progress.reset_unshown(), vec![2, 3, 4]);
    progress.complete(2);
    // The diagram's earlier result was forgotten, so its block waits.
    assert_eq!(progress.take_ready(), 1..3);
    progress.complete(3);
    progress.complete(4);
    assert_eq!(progress.take_ready(), 3..7);
    assert_eq!(progress.reset_unshown(), Vec::<usize>::new());
}

/// Re-rendering after a zoom starts at the first visible graphic, then does
/// the graphics not on the page yet, then those above the window.
#[test]
fn zoom_order_visible_first() {
    let shown = [(0, 10), (1, 20), (2, 30), (3, 40)];
    assert_eq!(zoom_order(&shown, 25, &[4, 5]), vec![2, 3, 4, 5, 0, 1]);
    assert_eq!(zoom_order(&shown, 0, &[]), vec![0, 1, 2, 3]);
    assert_eq!(zoom_order(&shown, 30, &[]), vec![2, 3, 0, 1]);
    assert_eq!(zoom_order(&shown, 99, &[4]), vec![4, 0, 1, 2, 3]);
    assert_eq!(zoom_order(&[], 0, &[7]), vec![7]);
}

/// The page is revealed once it fills the window (a window not laid out
/// yet never counts as full), loading is done once everything is shown,
/// and it never goes back.
#[test]
fn phases() {
    let first = Phase::FirstScreen;
    assert_eq!(first.next(false, 100.0, 500.0), Phase::FirstScreen);
    assert_eq!(first.next(false, 100.0, 0.0), Phase::FirstScreen);
    assert_eq!(first.next(false, 500.0, 500.0), Phase::Background);
    assert_eq!(first.next(true, 0.0, 500.0), Phase::Done);
    let background = Phase::Background;
    assert_eq!(background.next(false, 10.0, 500.0), Phase::Background);
    assert_eq!(background.next(true, 10.0, 500.0), Phase::Done);
    assert_eq!(Phase::Done.next(false, 0.0, 500.0), Phase::Done);
    const { assert!(SPINNER_DELAY_MS > 0 && SPINNER_DELAY_MS < 1000) };
}

/// One worker per CPU, but no more than there are graphics, and at least one.
#[test]
fn worker_counts() {
    assert_eq!(worker_count(8, 100), 8);
    assert_eq!(worker_count(8, 3), 3);
    assert_eq!(worker_count(8, 0), 1);
    assert_eq!(worker_count(0, 5), 1);
}

/// The pool renders every job exactly once and delivers each result with
/// its id; results match rendering the graphic directly.
#[test]
fn pool_delivers_every_result() {
    let mut blocks = render(DOCUMENT);
    let jobs = jobs(&mut blocks);
    let expected = jobs.len();
    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    let settings = RasterSettings::default();
    let _pool = Pool::start(jobs.clone(), settings, 3, move |id, rendered| {
        sender.lock().unwrap().send((id, rendered)).unwrap();
    });
    let mut results: Vec<_> = (0..expected)
        .map(|_| receiver.recv_timeout(Duration::from_secs(60)).unwrap())
        .collect();
    results.sort_by_key(|(id, _)| *id);
    assert_eq!(
        results.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        (0..expected).collect::<Vec<_>>()
    );
    assert!(results.iter().all(|(_, rendered)| rendered.is_ok()));
    let direct = md_viewer::graphics::render_math("a", false, &settings).unwrap();
    assert_eq!(results[0].1.as_ref().unwrap(), &direct);
}

/// A cancelled (or dropped) pool stops: of many jobs, far from all are
/// delivered, and the workers end (the channel closes).
#[test]
fn pool_cancel_stops_workers() {
    for cancel_by_drop in [false, true] {
        // Different formulas each time, so none come from the shared cache.
        let source: String = (0..400)
            .map(|i| format!("$x_{{{i}}}+{i}+{cancel_by_drop}$ "))
            .collect();
        let mut blocks = render(&source);
        let jobs = jobs(&mut blocks);
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        let pool = Pool::start(jobs.clone(), RasterSettings::default(), 2, move |id, _| {
            let _ = sender.lock().unwrap().send(id);
        });
        receiver.recv_timeout(Duration::from_secs(60)).unwrap();
        if cancel_by_drop {
            drop(pool);
        } else {
            pool.cancel();
        }
        let mut delivered = 1;
        while receiver.recv_timeout(Duration::from_secs(60)).is_ok() {
            delivered += 1;
        }
        assert!(delivered < jobs.len(), "{delivered} of {}", jobs.len());
    }
}

/// A pool with nothing to do (or asked for no threads) starts and ends.
#[test]
fn pool_without_jobs() {
    let (sender, receiver) = mpsc::channel::<usize>();
    let sender = Mutex::new(sender);
    let _pool = Pool::start(Vec::new(), RasterSettings::default(), 0, move |id, _| {
        let _ = sender.lock().unwrap().send(id);
    });
    assert!(receiver.recv_timeout(Duration::from_secs(10)).is_err());
}

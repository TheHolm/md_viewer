//! Integration tests for progressive, parallel rendering
//! (`md_viewer::progressive`): numbering graphics, storing results, deciding
//! which blocks may go on the page, the loading phases, the order graphics
//! are re-rendered in after a zoom, and the worker pool.

use md_viewer::graphics::{Graphic, RasterSettings};
use md_viewer::progressive::{
    clear_graphics, for_each_graphic, held_bytes, jobs, place, plan_budget, worker_count,
    zoom_order, Held, Job, Location, Phase, Pool, Progress, SPINNER_DELAY_MS,
};
use md_viewer::{render, Block, Inline};
use std::path::Path;
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
                Graphic::Image { .. } => "image".to_string(),
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

/// `place` stores a result with the graphic at a job's location, and
/// refuses locations that hold no graphic (a missing block, cell or inline,
/// plain text, a code block, a diagram location in a text block).
#[test]
fn place_stores_results() {
    let mut blocks = render(DOCUMENT);
    let found = jobs(&mut blocks);
    let cell = &found[2];
    assert_eq!(cell.block, 1);
    assert_eq!(
        cell.location,
        Location {
            cell: Some((1, 0)),
            inline: Some(0)
        }
    );
    assert!(place(
        &mut blocks,
        cell.block,
        cell.location,
        Err("cell".to_string())
    ));
    let text = Location {
        cell: None,
        inline: Some(0),
    };
    let whole = Location {
        cell: None,
        inline: None,
    };
    let err = || Err("none".to_string());
    assert!(!place(&mut blocks, 9, text, err()));
    assert!(!place(&mut blocks, 0, text, err()), "plain text");
    assert!(!place(&mut blocks, 0, whole, err()));
    assert!(!place(&mut blocks, 5, whole, err()), "code block");
    assert!(!place(&mut blocks, 3, text, err()), "inline in a diagram");
    let missing_cell = Location {
        cell: Some((5, 0)),
        inline: Some(0),
    };
    assert!(!place(&mut blocks, 1, missing_cell, err()));
    let missing_inline = Location {
        cell: Some((1, 0)),
        inline: Some(7),
    };
    assert!(!place(&mut blocks, 1, missing_inline, err()));
    let header = Location {
        cell: Some((0, 0)),
        inline: Some(0),
    };
    assert!(!place(&mut blocks, 1, header, err()), "header text");
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

/// Clearing a block forgets every result stored in it (formulas, images,
/// diagrams) and leaves the other blocks alone.
#[test]
fn clear_graphics_forgets_results() {
    let mut blocks = render(DOCUMENT);
    let mut id = 0;
    for_each_graphic(&mut blocks, |_, _, image| {
        *image = Some(Err(format!("r{id}")));
        id += 1;
    });
    let count = |blocks: &mut [md_viewer::Block]| {
        let mut stored = 0;
        for_each_graphic(blocks, |_, _, image| stored += image.is_some() as usize);
        stored
    };
    let before = count(&mut blocks);
    clear_graphics(&mut blocks[1]);
    assert_eq!(count(&mut blocks[1..2]), 0);
    assert!(count(&mut blocks) < before);
    for block in &mut blocks {
        clear_graphics(block);
    }
    assert_eq!(count(&mut blocks), 0);
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
    assert_eq!(progress.reset_unshown(jobs.len()), vec![2, 3, 4]);
    progress.complete(2);
    // The diagram's earlier result was forgotten, so its block waits.
    assert_eq!(progress.take_ready(), 1..3);
    progress.complete(3);
    progress.complete(4);
    assert_eq!(progress.take_ready(), 3..7);
    assert_eq!(progress.reset_unshown(jobs.len()), Vec::<usize>::new());
}

/// Only the graphics up to the frontier are re-rendered after a zoom: a
/// graphic beyond it (even one with a rendered, but unshown, result) is left
/// alone for lazy rendering.
#[test]
fn progress_reset_unshown_respects_frontier() {
    let mut blocks = render(DOCUMENT);
    let jobs = jobs(&mut blocks);
    let mut progress = Progress::new(blocks.len(), &jobs);
    for id in [0, 1, 3] {
        progress.complete(id);
    }
    assert_eq!(progress.take_ready(), 0..1);
    // The frontier stops before the diagram (graphic 3): only the table's
    // formula (2) is within it, so only it is queued for re-rendering, even
    // though the diagram also has an unshown result.
    assert_eq!(progress.reset_unshown(3), vec![2]);
    // A frontier covering the whole document reaches the diagram and the
    // last formula too.
    assert_eq!(progress.reset_unshown(jobs.len()), vec![2, 3, 4]);
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

/// Jobs submitted in several batches are all rendered, in order, by workers
/// that wait (rather than finish) between batches.
#[test]
fn pool_submit_batches_in_order() {
    let mut blocks = render(DOCUMENT);
    let jobs = jobs(&mut blocks);
    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    let settings = RasterSettings::default();
    let pool = Pool::start(Vec::new(), settings, 3, move |id, rendered| {
        sender.lock().unwrap().send((id, rendered)).unwrap();
    });
    // Submit in two batches: the first is collected in full before the
    // second is added, so the workers must stay alive while the queue is
    // empty in between.
    let split = jobs.len() / 2;
    pool.submit(jobs[..split].to_vec());
    let mut results = Vec::new();
    for _ in 0..split {
        results.push(receiver.recv_timeout(Duration::from_secs(60)).unwrap());
    }
    pool.submit(jobs[split..].to_vec());
    for _ in split..jobs.len() {
        results.push(receiver.recv_timeout(Duration::from_secs(60)).unwrap());
    }
    results.sort_by_key(|(id, _)| *id);
    assert_eq!(
        results.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        (0..jobs.len()).collect::<Vec<_>>()
    );
    assert!(results.iter().all(|(_, rendered)| rendered.is_ok()));
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

// --- Images ---------------------------------------------------------------

/// A document with a standalone image, an inline image and an image in a
/// table cell, next to plain text.
const IMAGE_DOCUMENT: &str = "![a](one.png)\n\n\
                              see ![b](two.png) here\n\n\
                              | x |\n|---|\n| ![c](three.png) |";

/// Local images everywhere become graphics: a standalone one in text is
/// marked so it is sized to the window width, while an inline one - in text
/// or in a table cell - is not.
#[test]
fn image_jobs_include_table_cells() {
    let mut blocks = render(IMAGE_DOCUMENT);
    md_viewer::resolve_images(&mut blocks, Path::new("/docs/notes.md"));
    let jobs = jobs(&mut blocks);
    let images: Vec<(usize, bool)> = jobs
        .iter()
        .filter_map(|job| match &job.graphic {
            Graphic::Image { inline, .. } => Some((job.block, *inline)),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 3);
    // The standalone paragraph image is the only one not inline.
    assert_eq!(
        images.iter().map(|(_, inline)| *inline).collect::<Vec<_>>(),
        vec![false, true, true]
    );
    let table = blocks
        .iter()
        .position(|block| matches!(block, Block::Table(_)))
        .unwrap();
    assert_eq!(images.last().unwrap().0, table);
}

/// A network image is never rendered (its placeholder is shown directly), so
/// it creates no job and does not hold its block back.
#[test]
fn remote_images_are_not_rendered() {
    let mut blocks = render("![a](https://example.com/a.png)");
    md_viewer::resolve_images(&mut blocks, Path::new("/docs/notes.md"));
    assert!(jobs(&mut blocks).is_empty());
}

/// A block waits for its image, like for a formula.
#[test]
fn progress_waits_for_images() {
    let mut blocks = render("![a](one.png)");
    md_viewer::resolve_images(&mut blocks, Path::new("/docs/notes.md"));
    let jobs = jobs(&mut blocks);
    assert_eq!(jobs.len(), 1);
    let mut progress = Progress::new(blocks.len(), &jobs);
    assert_eq!(progress.take_ready(), 0..0);
    progress.complete(0);
    assert_eq!(progress.take_ready(), 0..1);
    assert!(progress.all_shown());
}

/// `place` stores an image result on the image element, in order.
#[test]
fn place_stores_image_result() {
    let mut blocks = render("![a](one.png)");
    md_viewer::resolve_images(&mut blocks, Path::new("/docs/notes.md"));
    let job = jobs(&mut blocks).remove(0);
    assert!(place(
        &mut blocks,
        job.block,
        job.location,
        Err("broken".to_string())
    ));
    let Block::Text(inlines) = &blocks[0] else {
        panic!("expected a text block");
    };
    let Inline::Image(image) = &inlines[0] else {
        panic!("expected an image");
    };
    assert_eq!(image.image, Some(Err("broken".to_string())));
}

/// Every job's location reaches its own graphic: placing a numbered result
/// at each job's location stores it exactly where `for_each_graphic` finds
/// that graphic, in the showcase fixture (text, tables, lists, diagrams,
/// images) and the test document.
#[test]
fn every_location_reaches_its_graphic() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/showcase.md");
    let showcase = std::fs::read_to_string(&fixture).expect("read the showcase fixture");
    for source in [showcase.as_str(), DOCUMENT] {
        let mut blocks = render(source);
        md_viewer::resolve_images(&mut blocks, &fixture);
        let found = jobs(&mut blocks);
        assert!(!found.is_empty());
        for job in &found {
            assert!(place(
                &mut blocks,
                job.block,
                job.location,
                Err(format!("#{}", job.id))
            ));
        }
        let mut seen = Vec::new();
        for_each_graphic(&mut blocks, |_, _, image| {
            seen.push(image.clone().unwrap().unwrap_err());
        });
        let expected: Vec<_> = (0..found.len()).map(|id| format!("#{id}")).collect();
        assert_eq!(seen, expected);
    }
}

/// A graphic for the budget tests.
fn held(id: usize, key: usize, bytes: usize, distance: f64, arrived: u64) -> Held {
    Held {
        id,
        key,
        bytes,
        distance,
        arrived,
    }
}

/// A buffer shared by several graphics counts once.
#[test]
fn held_bytes_counts_shared_buffers_once() {
    let graphics = [
        held(0, 7, 100, 0.0, 0),
        held(1, 7, 100, 9.0, 1),
        held(2, 8, 50, 0.0, 2),
    ];
    assert_eq!(held_bytes(&graphics), 150);
    assert_eq!(held_bytes(&[]), 0);
}

/// Within the budget nothing happens; over it, the farthest buffers are
/// dropped first until it fits, never one with a graphic near the view
/// (distance 0) or one holding nothing.
#[test]
fn plan_budget_drops_farthest_first() {
    let graphics = [
        held(0, 0, 100, 0.0, 0),
        held(1, 1, 100, 50.0, 1),
        held(2, 2, 100, 900.0, 2),
        held(3, 3, 100, 300.0, 3),
        held(4, 4, 0, 5000.0, 4),
    ];
    assert_eq!(plan_budget(&graphics, 400), (vec![], vec![]));
    assert_eq!(plan_budget(&graphics, 350), (vec![2], vec![]));
    assert_eq!(plan_budget(&graphics, 150), (vec![2, 3, 1], vec![]));
}

/// A shared buffer is dropped whole (all its graphics), and kept whole
/// while any of its graphics is near the view.
#[test]
fn plan_budget_drops_shared_buffers_whole() {
    let graphics = [
        held(0, 9, 100, 800.0, 0),
        held(1, 9, 100, 700.0, 1),
        held(2, 5, 100, 10.0, 2),
        held(3, 6, 100, 0.0, 3),
        held(4, 6, 100, 600.0, 4),
    ];
    assert_eq!(plan_budget(&graphics, 200), (vec![0, 1], vec![]));
    assert_eq!(plan_budget(&graphics, 100), (vec![0, 1, 2], vec![]));
}

/// When what is near the view alone exceeds the budget, every far buffer
/// is dropped and then the newest near ones are limited until it fits.
#[test]
fn plan_budget_limits_newest_near_buffers() {
    let graphics = [
        held(0, 0, 100, 0.0, 5),
        held(1, 1, 100, 0.0, 9),
        held(2, 2, 100, 0.0, 7),
        held(3, 3, 100, 40.0, 1),
    ];
    assert_eq!(plan_budget(&graphics, 150), (vec![3], vec![1, 2]));
    assert_eq!(plan_budget(&graphics, 0), (vec![3], vec![1, 2, 0]));
}

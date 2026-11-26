# NOTES.md

Persistent facts and knowledge about `md_viewer`, for anyone (human or
agent) working on it. Rules for agents are in `AGENTS.md`; known bugs and
gaps are in `BUGS.md`. Keep this file current: when a fact here changes, or
you learn something a future session would need, update it.

## Project overview

`md_viewer` is a small GTK4/libadwaita desktop application for viewing
Markdown documents. It is a viewer only: it opens the file given on the
command line (or picked with the file chooser), renders it read-only in a
scrollable window, and never writes to it. The window title is "MD Viewer" (prefixed with the open file's name)
and the application ID is `io.github.TheHolm.md_viewer`. The package,
library and binary are all named `md_viewer`. The version is
declared in the `VERSION` file at the repository root, the single source of
the version (see "Versioning" below).

The viewer never fetches anything from the network, and the code must not
be able to: see "Hard decisions" in `AGENTS.md`, enforced by
`tests/no_network.rs`. Web links are only handed to the browser.

## Stack

- Rust 2021 edition, toolchain 1.98 (the version CI builds with)
- `gtk4` 0.11 (feature `v4_18`) - GUI toolkit
- `adw` / libadwaita 0.9 (feature `v1_7`) - GNOME application shell
- `glib-unix` - GLib's Unix API (SIGINT/SIGTERM handling)

The GTK/libadwaita feature levels are pinned to the oldest target, Debian
trixie (GTK 4.18, libadwaita 1.7); Ubuntu 26.04 and FreeBSD 15 ship newer.
Raise them only when every target platform ships the newer version.
- `pulldown-cmark` - CommonMark parser (with the tables, strikethrough,
  task list and math extensions enabled)
- `mathra` - LaTeX math (including mhchem `\ce{}`/`\pu{}`) to SVG, pure Rust,
  with its own bundled math font
- `merman` (features `svg`, `all-diagrams`) - Mermaid diagrams to SVG, pure
  Rust; used through its resvg-safe pipeline (labels as SVG text)
- `resvg` (features `text`, `system-fonts`, `memmap-fonts`,
  `raster-images`) - rasterizes every SVG; SVG text uses the system fonts,
  and raster images embedded in an SVG are decoded too
- `image` (default features off; `png`, `jpeg`, `gif`, `webp`) - decodes
  local image files for `![alt](path)`, pure Rust. Animated GIFs show only
  their first frame; `.svg` image files are rasterized with the resvg path
  instead
- `regex` - the find bar's regular-expression mode, pure Rust
- `url` - resolving and percent-decoding link destinations, pure Rust
- `clap` - command line argument parsing (the `FILE` argument)
- `tempfile` (dev-dependency) - temporary files for integration tests

Everything that renders formulas and diagrams is pure Rust: no JavaScript,
web view or extra system library. Keep it that way unless the user decides
otherwise - it is what keeps the FreeBSD cross-build simple.

## Target platforms

Linux desktops with GTK4 and libadwaita, and FreeBSD. Tagged releases build
and publish three packages, all amd64, and **all three are required** - a
failure in any of them fails the release:

- Debian trixie `.deb`
- Ubuntu 26.04 `.deb`
- FreeBSD 15 `.pkg` (cross-compiled on Linux against a FreeBSD sysroot)

The FreeBSD package is cross-built and checked for content in CI but never
run there; runtime verification on a real FreeBSD system is a manual step.

## Layout

- `src/main.rs` - the GTK4/libadwaita GUI: the application and its windows.
  Each window is a `DocWindow`: an `adw::ToolbarView` with a header bar
  (Open button on the left, a small loading `adw::Spinner` on the right)
  over either the empty start page (`adw::StatusPage`, shown when no `FILE`
  is given) or the document: a read-only `GtkTextView` in a
  `GtkScrolledWindow`, in a `GtkOverlay` whose `GtkRevealer` "cover" (a
  centred 48 px spinner, shown after `SPINNER_DELAY_MS`) hides the page
  until the first screen is full. The `win.open` action (Ctrl+O) runs a
  `gtk4::FileDialog`; the picked file opens in a new window, except that an
  empty window is reused (`md_viewer::open_target`). The file is read and
  parsed on `gio::spawn_blocking` before any window changes: a failure for
  the command line `FILE` goes to stderr with exit status 1, one for a
  picked file to an `adw::AlertDialog` (`md_viewer::Origin`). Ctrl+W closes
  a window. SIGINT/SIGTERM quit cleanly through the main loop.
  - Each shown document is a `Session`: it starts a `progressive::Pool`
    (one worker per CPU) over the graphics up to a *frontier*, collects
    results from an `mpsc` channel every 16 ms (`POLL_MS`, a
    `glib::timeout_add_local` that stops when idle), stores them with their
    blocks and appends each block once it and every block before it are
    complete (`progressive::Progress`). Content is only ever appended, so
    nothing on the page moves. The frontier advances as the user scrolls
    near the end of what is rendered (`Session::advance_frontier`), so a very
    large document only renders the graphics near the view; the workers wait
    on a condition variable for more jobs (`Pool::submit`) instead of
    finishing. `progressive::Phase` decides the cover; the header spinner
    shows while the run has outstanding work or a re-render is pending.
  - Text goes into the buffer with one `GtkTextTag` per `Style`; formulas
    are paintables (`ScaledTexture`: a device-pixel texture drawn at its
    logical size, replaceable in place), lowered by a per-formula `rise` tag,
    display math centred; tables are a `GtkGrid` of wrapping labels in a
    horizontal `GtkScrolledWindow`; code blocks are cards (`code_card`:
    language label, copy button, monospace selectable label in a horizontal
    scroller); diagrams are a `GtkPicture` on a white card. A standalone
    image is a `GtkPicture` in a `md-image` box (so a search match can
    highlight it) registered in `Resizable` (fit to the view width); an
    inline image is an `insert_paintable` (capped by the worker), and an image
    in a table cell is an inline `GtkPicture` in the cell's row of labels and
    pictures. A network, unsupported or unreadable image becomes
    `[image: alt — reason]` text with no `Slot`. A picture that is a link's
    content is clickable (a controller on its box; an inline one via a
    `LinkRange` over its `U+FFFC`), and every picture's alt text is a search
    entry (see the search bullet). Table and
    math-cell labels are plain text (`use_markup` off) styled with a single
    Pango attribute list built from `label_text`'s style runs (see
    `src/lib.rs`), never with markup. Every graphic on
    the page has a `Slot` (its `ScaledTexture`, rise tag, picture entry,
    buffer offset) so a re-render can replace it.
  - Anchored widgets only get their minimum size, so `Resizable` (run from
    a tick callback) sizes them from their content and the view width:
    tables at their natural width capped to the view (wider ones scroll),
    code cards as wide as the view minus their indent, diagrams and
    standalone images scaled down to fit.
  - Zoom (`md_viewer::zoom`): `win.zoom-in`/`zoom-out`/`zoom-reset`
    (Ctrl+plus/=/minus/0 and keypad), Ctrl+scroll and a pinch
    (`GestureZoom`), all on the toolbar in the capture phase. The text size
    is a per-window CSS class (`md-zoom-N`, `font-size` in px from the
    window's base font) on the text view, inherited by the cells and code
    cards. Pictures are stretched at once (`Slot::stretch`), the run is
    cancelled, and `REZOOM_DELAY_MS` after the last step all graphics are
    re-rendered at the new zoom in `progressive::zoom_order`. A mark at the
    line at the top of the window plus the fraction into it is held in
    place (`hold_anchor`/`keep_anchor`, every frame for `ANCHOR_HOLD_MS`
    after the last change; a normal scroll releases it).
  - Vertical wheel scrolling over a table is passed to the document
    (`pass_vertical_scrolls`); horizontal scrolling still scrolls the table.
  - Widget styling is the `CSS` constant; sizes and classes come from
    `md_viewer::layout`.
  - Search (Ctrl+F or `/`) shows a `GtkSearchBar` above the document with a
    search entry, count label, previous/next buttons and a mode button
    (smart case, ignore case, regex). A `Session` keeps a `Search`: the
    whole text buffer is one search source, and every table-cell label,
    code-card label, diagram (its extracted labels joined) and image (its alt
    text) is a `SearchEntry` registered as it is appended. On top of that,
    `search::document_units` lists the parsed document's render-independent
    text (span runs split around formulas and images, table cells, code,
    image alt) in document order, so a search covers blocks not yet rendered:
    matches there are counted (with the sentinel buffer offset `i32::MAX`,
    ordered by block) and, when navigated to, trigger `render_to` and a
    pending scroll. While any block is not on the page, the count label shows
    "…" (`SearchSummary::pending`). Matches are
    highlighted with text tags (buffer), a rebuilt Pango attribute list
    (labels, with their base style runs plus the highlight ranges), a CSS
    class (diagram cards and standalone/table-cell image boxes) or nothing
    visible (an inline image's alt text, whose picture is a paintable and
    only counts and marks the scrollbar); the current match scrolls into
    view, and a narrow `DrawingArea` over the scrollbar's right edge draws a
    mark per match.
    `/`, `n`, `N` and Escape are a capture-phase key controller on the
    window that lets keys through while the search entry is focused.
    Searching re-runs as blocks are appended while loading.
- `src/lib.rs` - all non-GUI logic, exposed publicly so the GUI and the
  integration tests share exactly one implementation: the app version
  (`version`, the `VERSION` file embedded at compile time), the application
  ID (`APP_ID`, which the `.desktop` file and icons are named after), the
  Markdown MIME types and file patterns (`MARKDOWN_MIME_TYPES`,
  `MARKDOWN_PATTERNS`), the decisions made when opening a document
  (`Origin`, `open_target`, `chooser_folder`), reading the document
  (`read_document`, and `open_document` = read + render), the error text for an unreadable file (`error_message`), the
  window title (`window_title`), the Markdown renderer (`render`) and Pango
  markup for table cells (`pango_markup`, `escape_markup`, and the shared
  colour constants), plus `label_text` (flattens inlines into plain text
  and `StyledRun`s so labels can be styled with Pango attributes instead of
  markup). `render` turns Markdown into a `Vec<Block>`:
  `Block::Text` (flowing `Inline`s: `Inline::Text(Span)` - text plus its
  `Style`s and an optional link destination - `Inline::Math(Math)` and
  `Inline::Image(ImageRef)`),
  `Block::Table(Table)` (per-column
  `Align`, header and rows of `Cell`s, i.e. `Vec<Inline>`) and
  `Block::Mermaid(Diagram)` and `Block::Code(CodeBlock)` (language from
  `code_language`, verbatim text, nesting `indent`; `clipboard_text` is what
  the copy button copies). `Math`, `Diagram` and `ImageRef` carry an `image`
  that is `None` after `render` and filled in by `graphics::render_document`
  or the GUI's worker pool. An `ImageRef` keeps its source as written, its
  alt text and the link destination it is the content of (`link`, `None`
  when it is not inside a link, used to make the picture clickable), plus an
  `ImageState`: `Unresolved` (straight after `render`),
  `Local(path)`, `Remote` (a network URL, never loaded) or
  `Unsupported(reason)`. `resolve_images` (called by `open_document` and the
  GUI, which know the document path) fills the state in with
  `links::resolve_image`; `image_is_standalone(inlines, index)` says whether
  an image is alone on its line, after any list marker (see the
  rendering-model note about merged paragraphs). `ImageRef::placeholder`
  builds the `[image: alt — reason]` text.
  `Style::tag_name` names each style's text tag; `Style::ALL` lists every
  style so all tags can be created up front. Private helpers (`Renderer`,
  `TableBuilder`, `push_inline`, `trailing_newlines`, `normalize`) have unit
  tests in the same file.
- `src/graphics.rs` (`md_viewer::graphics`) - turning formulas, diagrams and
  images into pixels: `math_svg`/`render_math` (mathra; math at `MATH_SCALE` =
  1.21x the text size like KaTeX, in the text colour, with its depth below
  the baseline), `mermaid_svg`/`render_mermaid` (merman), `rasterize` (resvg
  into premultiplied RGBA `Image`s at a device scale; the system font
  database is loaded once and generic `sans-serif` is pointed at an
  installed family, GNOME's Adwaita Sans first, see `pick_family`),
  `decode_image`/`load_image` for local image files (PNG/JPEG/GIF first
  frame/WebP through the `image` crate, `.svg` through `rasterize`; Exif
  orientation is applied, straight alpha is premultiplied; raster images
  keep the file's own pixels - never enlarged for HiDPI, which adds no
  detail and multiplied the memory - and one file pixel is one logical
  pixel; an inline image is shrunk to the pixels it is drawn with; the
  logical size grows with the zoom like math and diagrams) and
  `render_document`, which fills in every graphic of a rendered document one
  after another. `rasterize` also collects every SVG text node into
  `Image::labels` (usvg expands `<use>`, so a diagram's labels are found at
  their drawn positions); math clears them because it draws outlines. `RasterSettings` carries the text size, device scale (fractional with fractional scaling), text
  colour, zoom (math is typeset at `font_px * zoom`, diagrams drawn
  `zoom` times their natural size) and the size limit `max_px`.
  The size limit: nothing is ever drawn larger than `SCREEN_MULTIPLE` (10)
  times the screen in device pixels (`max_px_for`; `FALLBACK_MAX_PX` before
  the window is on a monitor, and in tests). `zoom_limit` checks a
  graphic's size at 100% before anything is allocated (in floating point,
  so a huge SVG size is not wrapped into a bogus "no area" error): over the
  limit it is refused with "too large (...)", shown in its place; otherwise
  `Image::max_zoom` is the largest zoom it fits at, and asked for more it is
  drawn at that zoom, never larger. A raster file is checked from its
  header (Exif orientation applied) before its pixels are decoded, with
  `image::Limits` as a backstop, so a decompression bomb costs nothing.
  Files are read through `md_viewer::read_limited`: regular files only
  (checked before opening, so a FIFO cannot block and `/dev/zero` cannot
  fill memory) and at most `MAX_FILE_BYTES` (512 MiB). This covers the
  document, image files and the files an SVG file's `<image href>` names;
  those resolve against the SVG's folder (`svg_options_for`, a custom
  usvg `ImageHrefResolver`), and formulas and diagrams cannot refer to files
  at all. Raster images embedded in an SVG (files or `data:` URLs) are
  checked from their header against the size limit before resvg decodes
  them.
- Memory: `Image::pixels` is an `Arc<Vec<u8>>` shared by the cache, the
  document and the GTK texture (`SharedPixels`, `glib::Bytes::from_owned`,
  no copy); a block's results are cleared from the parsed document once it
  is on the page (`progressive::clear_graphics`). The cache maps are also
  bounded in bytes (`CACHE_BYTES`, 96 MiB each). Each document has a budget
  of `DOCUMENT_BUDGET` (1 GiB) of pixels on the page: a graphic needing more
  at 100% is refused ("memory limit (...)"); otherwise, when the page holds
  more, `Session::enforce_budget` drops the graphics farthest from the view
  (`progressive::plan_budget`, never within `layout::KEEP_SCREENS` screens of
  it): their texture is freed and a spinner of the same size stands in
  (`ScaledTexture::show_spinner`, an `adw::SpinnerPaintable`), so nothing
  moves. `Session::redraw_near` asks the pool for dropped graphics coming
  near the view again (`Slot::held`: `Drawn`, `Dropped`, `Requested(run)`; a
  request from a run cancelled by a zoom is made again). Both run after each
  poll and, via `schedule_budget`, once idle after the adjustment's
  `changed`/`value-changed` signals. A zoom re-renders only drawn graphics.
  The budget does not limit the zoom. Diagram labels are measured with the
  same font files resvg draws them with (merman's `native-fonts`,
  `native_fonts`/`font_files`; merman's built-in estimates if no font files
  can be loaded). `Graphic` names one formula, diagram or image; `Cache` (one
  shared instance, `Cache::shared`) keeps rendered formulas by source and
  settings, laid-out diagram SVG by source, and decoded images by path and
  settings, each map emptied when it reaches its limit. An image `Graphic`
  carries an `inline` flag: inline images are capped to a few lines high
  (`cap_inline`), standalone ones keep their natural size (the GUI then fits
  them to the window like a diagram).
- `src/progressive.rs` (`md_viewer::progressive`) - progressive, parallel
  loading: `for_each_graphic`/`jobs`/`place` number the graphics in
  document order and store results (formulas and images everywhere, in text
  and table cells, plus diagrams; non-local images are skipped; an image in a
  table cell is always inline, one in a text block is standalone when alone
  on its line);
  `Progress` decides which blocks may go
  on the page (strictly in order, each once complete) and what to re-render
  after a zoom (`reset_unshown`, bounded by the frontier so far-off graphics
  stay unrendered); `zoom_order`; `Phase` (FirstScreen,
  Background, Done); `Pool`, the worker threads (a shared queue taken from
  the front, plus a condition variable the workers wait on when it empties,
  so `Pool::submit` can add more jobs for lazy rendering; cancelled on
  `cancel` or drop, which wakes the waiters; a result finished at that
  moment may still arrive, so the GUI tags runs with a generation).
- `src/zoom.rs` (`md_viewer::zoom`) - zoom levels (`STEPS`, 50%-300%),
  `Zoom` (step in/out, reset, free `set` for pinches, `scroll` accumulating
  wheel clicks and stopping at the ends, the document's `ceiling` that every
  change is clamped to, and the header bar `indicator` text),
  `wheel_clicks` (touchpad pixels to clicks) and the zoom CSS.
- `src/layout.rs` (`md_viewer::layout`) - the GUI's sizing and styling
  arithmetic as plain functions, so it is tested without a display: the
  width available to anchored widgets, table and diagram sizes, Pango font
  size to pixels, colour channels, heading scales, the baseline `rise` of
  inline math, column alignment and table cell CSS classes, code card
  widths, scroll distances passed on from tables, the zoom anchor
  arithmetic and a few GUI timings. `main.rs` only applies the results to
  widgets.
- `src/search.rs` (`md_viewer::search`) - the find bar's matching, pure
  and tested: `SearchMode` (smart case, ignore case, regex), `Matcher`
  (a query compiled once; `find` returns byte ranges; Unicode full case
  folding, from the table in `src/casefold.rs`, maps each folded byte back
  to its source character, and
  zero-width regex matches are skipped), `char_ranges` (byte to
  character-offset ranges for the text buffer) and the match navigation
  helpers (`next_index`, `first_at_or_after`).
- `src/links.rs` (`md_viewer::links`) - clickable links and image
  destinations, pure and tested:
  `resolve_link` (a destination resolved against the document into a web,
  local-file, Markdown-file or same-document-anchor target), `resolve_image`
  (`http`/`https` -> `Remote`, `file:`/paths -> `Local`, anything else ->
  `Unsupported` with a reason), `heading_starts` (the headings inside a
  text block: a text block holds many headings, paragraphs and lists, so a
  heading is a run of spans with the same `Style::Heading`, plus formulas
  or images next to it), `heading_slug` and `github_slug` (heading anchors: ours collapses
  whitespace runs, GitHub's keeps one hyphen per whitespace character),
  `Anchors` (the registry the GUI fills as blocks are appended, adding the
  duplicate `-N` suffixes in document order; fragments are percent-decoded
  by `resolve_link`, so `#%C3%BCber` finds `über`), `file_uri`, and
  `LinkRanges` (the
  character ranges of the clickable text, accumulated as blocks are
  appended).
- `tests/` - integration tests split by topic:
  - `render.rs` - the renderer: one group of tests per block type
    (paragraphs, headings, lists, block quotes, code blocks, rules, tables,
    math, Mermaid) and per inline style (emphasis, strong, strikethrough,
    inline code, links, images), plus standalone-image detection (including
    list items, block quotes, a formula or image next to the image, and
    trailing text), image resolution, image placeholders and
    the Pango markup for table cells
  - `links.rs` - link resolution (web, mail, local, relative, Markdown,
    anchor, ignored schemes, a `file:` URL with a host), image destination
    resolution (local, remote, unsupported, percent-decoding), fragment
    decoding, heading slugs (including dropping a trailing hyphen), finding
    the headings of a text block, the anchor registry, `file_uri`,
    `LinkRanges`, and the links inside label text
  - `graphics.rs` - rasterizing, math and mhchem, Mermaid diagrams (several
    types, labels, scale, errors), image decoding (PNG, JPEG, SVG,
    premultiplication, Exif orientation, no enlargement at a device scale,
    zoom, the inline cap and shrinking, and a raster image embedded in an
    SVG), the size limit (ten screens, `zoom_limit`, oversized SVGs,
    formulas, PNGs and a PNG bomb refused, zoom stopping at the limit for
    formulas, diagrams and images), filling in whole documents,
    that the showcase fixture renders, and that the root `showcase.md`
    renders with no fallbacks at all (every formula and diagram `Ok`)
  - `layout.rs` - the `md_viewer::layout` functions
  - `search.rs` - the find bar's matcher and navigation helpers (modes,
    smart case, ignore case, regex, Unicode folding)
  - `progressive.rs` - numbering and placing graphics (including images:
    text-only, non-local skipped, standalone flag), `Progress`,
    `zoom_order`, `Phase`, worker counts and the `Pool` (delivers every
    result, stops when cancelled or dropped)
  - `zoom.rs` - zoom steps, clamping, scroll accumulation and the zoom CSS
  - `cli.rs` - the binary end to end: `--version`, `--help` and argument
    errors always; with a display (`DISPLAY`/`WAYLAND_DISPLAY` set) also an
    unreadable file (status 1), the empty start window (no `FILE`),
    opening both fixtures, the root `showcase.md` and a large generated
    document, and closing that one while it is still loading, checking
    that the viewer stays up, prints no GTK criticals and exits 0 on
    SIGTERM. Without a display those tests print "skipped" and pass
  - `desktop.rs` - the files in `data/`: the `.desktop` file agrees with
    `APP_ID`, `APP_NAME` and `MARKDOWN_MIME_TYPES` and passes
    `desktop-file-validate` (skipped if not installed), the icons render at
    their sizes (the symbolic one with fills only), and both the `.deb`
    assets and the FreeBSD staging in `release.yaml` install every file
  - `fixtures/showcase.md` - a terse document using every supported
    feature (paragraph images inline and standalone, and an image in a table
    cell) plus one deliberately broken diagram; the tests' copy, with exact
    expected counts (the human demo is the root `showcase.md`)
  - `fixtures/cat.svg` - a small local image the showcase fixture references,
    so the image path (resolve, decode, render) is exercised end to end
  - `fixtures/fallbacks.md` - formulas and diagrams that fail to render and
    math in aligned table cells, for the GUI's fallback paths
  - `document_io.rs` - reading a document from disk, including failure paths,
    the window title, and the opening decisions (`Origin`, `open_target`,
    `chooser_folder`)
  - `version.rs` - that `VERSION` is one `X.Y.Z` line, that `version()`
    reports it, and that the copy in `Cargo.toml` matches it
- `showcase.md` - the demo for humans (`cargo run -- showcase.md`): a
  humorous coffee-machine incident report using every supported feature -
  all heading levels, inline styles, nested lists and quotes, task lists,
  code, four tables (aligned, wrapping, math, chemistry and image
  placeholders in cells), paragraph images (inline and standalone),
  inline and display math including differential equations, mhchem, and
  flowchart, sequence, Gantt, pie, class and state diagrams. Everything in
  it must render (no fallbacks; `root_showcase_renders` enforces this);
  failure paths belong in `tests/fixtures/fallbacks.md`. Check new math or
  diagrams in it visually, since mathra also returns `Ok` for unknown
  commands
- `data/` - desktop integration, installed by both packages:
  `io.github.TheHolm.md_viewer.desktop` (listed in the app grid, offered for
  `text/markdown`) and the icons under `icons/hicolor/` (`scalable/apps`,
  full colour, and `symbolic/apps`, fills only). GNOME matches a window to
  the `.desktop` file by the application ID, so every name here must be
  `APP_ID`
- `VERSION` - the app version, one `X.Y.Z` line; the single source of the
  version
- `Cargo.toml` - package metadata (its `version` is a copy of `VERSION`),
  dependencies, the `tempfile` dev-dependency and the `cargo-deb` metadata
  (including `assets`: the binary, docs and every file in `data/`)
- `.woodpecker/release.yaml` - the Woodpecker CI workflow: on `v*` tags it
  first checks the tag and `Cargo.toml` against `VERSION`, then builds a Debian trixie `.deb`, an Ubuntu 26.04 `.deb` and a FreeBSD `.pkg`,
  then publishes them to the GitHub mirror's Releases (needs a
  `github_token` secret in Woodpecker). Every package step is required and
  checks that its package contains the binary, the `.desktop` file and the
  icon.
- `scripts/` - CI helpers: `check-version.sh` (checks `VERSION` is `X.Y.Z`,
  that `Cargo.toml` matches it and, given a tag, that the tag is
  `v<VERSION>`), `build-freebsd-pkg.py` (writes a `.pkg` from a staged
  install tree; its version defaults to `VERSION`), `fetch-freebsd-gtk.py` (downloads the GTK4/libadwaita
  FreeBSD cross sysroot from `pkg.freebsd.org`) and `extract-release-notes.sh`
  (renders one release's section as a GitHub Release body)
- `AGENTS.md` - rules for coding agents working on this repository
- `NOTES.md` - this file: persistent facts and knowledge about the project
- `BUGS.md` - known bugs and limitations (planned features go in the
  README's TODO list instead)
- `RELEASE_NOTES.md` - per-version release notes; the matching `## vX.Y.Z`
  section is what a release publishes
- `README.md` - user-facing usage, build instructions and the TODO list of
  planned features
- `LICENSE` - AGPL v3

## Rendering model

- `render` never fails: anything it does not understand (raw HTML, footnote
  references) is shown as plain text rather than dropped.
- Blocks are separated by one blank line; list items by a single newline.
- List items are prefixed with `• ` (unordered) or `N. ` (ordered), indented
  four spaces per nesting level; task list items additionally get `[x] ` or
  `[ ] `.
- Tables are `Block::Table`s shown as grid widgets: bold, shaded header row,
  striped body rows, per-column alignment, inline styles kept in cells, long
  cells word-wrapped at about 40 characters, rows padded to the same column
  count. A table wider than the window scrolls horizontally.
- Math: `$…$` is inline, `$$…$$` is display math on its own centred line
  (inline inside a table cell). Only the dollar syntax is supported, not
  `\(…\)` or `\[…\]`. A lone `$`, `\$` and dollars in code are text.
  mhchem's `\ce{}` and `\pu{}` work inside math.
- Other code blocks (fenced or indented) are `Block::Code` cards: the
  language from the info string's first word in a small header, a copy
  button that copies the code exactly (no trailing newline) and shows a
  check mark for `COPY_FEEDBACK_MS`, monospace text whose long lines scroll
  sideways. A card always starts on its own line (in a list item, on the
  line after the marker) and is indented `CODE_INDENT` px per enclosing list
  or quote. A text selection cannot run from the surrounding text into a
  card (it is a separate widget).
- Loading is progressive: text is never shown before the graphics in it,
  but blocks appear in document order as soon as they are complete; the
  page is revealed (behind a spinner until then) once the first screen is
  full, and a header bar spinner shows until everything is on the page and
  while a zoom re-renders.
- Zoom (50%-300%) scales text, tables, code cards, math and diagrams;
  graphics are re-rendered sharp at the new size, visible ones first. The
  zoom belongs to the window and carries over to the next document opened
  in it; it is not saved between runs. A document can lower the maximum:
  the window's zoom stops where anything on the page (any graphic, or the
  largest heading) would exceed the size limit (ten screens, see
  `graphics`). Graphics are measured as they render, so a graphic arriving
  later can lower the ceiling below the current zoom; the window then zooms
  out to it (`Session::note_limit`/`report_limit` -> `DocWindow::limit_zoom`,
  reported after a poll so nothing is borrowed). The header bar shows the
  zoom percentage when it is not 100% ("(max)" at the ceiling); clicking it
  resets the zoom.
- A fenced code block whose info string starts with `mermaid`
  (case-insensitive) is a `Block::Mermaid` diagram, drawn on a white card
  in both themes (Mermaid's colours assume a light background).
- A formula or diagram that cannot be rendered is shown as its source in
  code style; display math and diagrams add a line with the reason, inline
  math a short "⚠ reason" after it (`Math::failure_note`). Nothing is ever
  dropped.
- Images: `![alt](path)` in a paragraph is loaded from a local file
  (resolved against the document, `file:` URLs allowed) and drawn. An image
  alone on its line (or after a list marker) is a standalone picture, scaled
  to the window width like a diagram; one among text is an inline paintable
  capped to a few lines high and sitting on the baseline. Raster formats go
  through the `image` crate (Exif orientation applied, the file's own
  pixels, scaled by the screen when drawn), `.svg` through resvg (including
  raster images embedded in the SVG). A picture larger than the size limit
  shows `[image: <alt> — too large (...)]`. Network destinations (`http`/`https`) are
  never fetched - that is deliberately not supported and not planned - and
  an empty source, unsupported scheme, missing file or unreadable format
  shows `[image: <alt> — <reason>]`. An image in a table cell is drawn inline
  (at about the line height) in the cell's row of labels and pictures.
- Adjacent text with identical styles is merged into one span.
- Links: a `Span` carries its destination, so the GUI makes the text
  clickable without re-parsing. A single click opens it (web/`mailto:` and
  non-Markdown local files via `GtkUriLauncher`, Markdown files in a new
  viewer window, relative paths resolved against the document); a
  `#fragment` scrolls to the matching heading, found by its slug
  (`heading_slug`) or its GitHub spelling (`github_slug`; duplicates get
  `-1`, `-2`, ... in each). Every heading has its
  own anchor, registered at its first character as its text block is
  appended (`links::heading_starts`). What a click does is decided by
  `links::link_action` (see "Link safety" under "Gotchas and techniques"):
  other schemes are refused, empty destinations ignored, and a
  formula that is a link's only content is not clickable (it is drawn as
  pixels). An image that is a link's content *is* clickable: a standalone or
  table-cell picture gets a click controller on its box, an inline one a
  link range over its buffer character. Hovering a link shows what a click
  does (`links::link_tooltip`) in a tooltip: both the
  text view and table-cell labels use `query-tooltip` (whose coordinates are
  widget-space, the same as `dest_at`/`label_link_at`) and set plain tooltip
  text, so destinations containing markup characters are safe.
- Search matches the rendered document *and* the parsed text of blocks not
  yet rendered: the whole text buffer (including fallback sources),
  table-cell text, code-card text, the labels inside diagrams and image alt
  text, plus `search::document_units`'s span runs, cells, code and alt for
  blocks still off the page; formula source is not searchable (a rendered
  formula is pixels). Matches are found per source and kept in document
  order by buffer offset (with `i32::MAX` and block order for unrendered
  ones); a diagram match highlights its whole card, an image alt-text match
  highlights a standalone or table-cell picture's box (an inline image's alt
  text only counts and marks the scrollbar), and a match in an unrendered
  block is counted but not highlighted until it is rendered. The find bar's
  count is followed by "…" while any block is still off the page.

## Versioning

- `VERSION` is the single source of the app version. The library embeds it
  (`md_viewer::version()`, used for `--version`), and the FreeBSD `.pkg`
  version and file name are read from it.
- `Cargo.toml`'s `version` is a copy, only because cargo requires a literal
  value there; the `.deb` packages take their version from it. Never let the
  two differ: `cargo test` (`tests/version.rs`) and the release pipeline
  (`scripts/check-version.sh`) both fail if they do.
- To change the version: edit `VERSION`, set `Cargo.toml`'s `version` to the
  same value, run `cargo build` so `Cargo.lock` picks it up, and add the
  matching `## vX.Y.Z` entry to `RELEASE_NOTES.md`. Release tags are
  `v<VERSION>`; the pipeline refuses any other tag.

## Commands

- Build/check: `cargo build`
- Run: `cargo run -- [FILE]` (requires a GTK4/libadwaita desktop session)
- Test: `cargo test`
- Format check: `cargo fmt --check`
- Version check: `bash scripts/check-version.sh [vX.Y.Z]` (run before
  tagging; pass the tag you are about to create)
- Coverage: `cargo llvm-cov --tests` (requires the `llvm-tools` component and
  the `cargo-llvm-cov` subcommand). Run it with a display so the GUI is
  covered too; headless, e.g. `Xvfb :77 & DISPLAY=:77 GDK_BACKEND=x11
  GSK_RENDERER=cairo cargo llvm-cov --tests`. With a display line coverage
  is about 80%: the library (`lib.rs`, `graphics.rs`, `links.rs`,
  `progressive.rs`, `search.rs`, `zoom.rs`, `layout.rs`) is about 98-100%,
  and `main.rs` is about 64%. The library's few uncovered lines are
  defensive branches that cannot be reached (no fonts installed, a `file:`
  URL with a host, parser events for extensions that are not enabled, and
  `Option` arms that cannot be `None`). Almost all of `main.rs`'s remainder
  is interactive GUI wiring that needs input events or a live window - the
  file chooser and error dialogs, click/zoom/search gestures, hover
  tooltips, and placing loaded images - so it is checked by hand (or with
  `xdotool` on Xvfb, see below). The display-independent helpers
  (`label_attributes` and friends) and the GTK-helper checks (`tag_for`/
  `create_tags`, `code_card`, `ScaledTexture::scale_size`,
  `label_link_at`) are unit-tested in `main.rs`'s test module
- Debian package: `cargo deb` (requires the `cargo-deb` subcommand)
- FreeBSD package: cross-build with `cargo build --release --target
  x86_64-unknown-freebsd` against a FreeBSD sysroot, then
  `python3 scripts/build-freebsd-pkg.py`
- Release: pushing a `v*` tag runs `.woodpecker/release.yaml`, which builds and
  publishes the packages (the tag needs a matching `## vX.Y.Z` entry in
  `RELEASE_NOTES.md`)

Build dependencies on Debian/Ubuntu:

```
apt-get install pkg-config libgtk-4-dev libadwaita-1-dev
```

One-time coverage tooling setup:

```
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
```

## Gotchas and techniques

Things learned the hard way; add to this list when you learn another.

- mathra returns `Ok` for LaTeX it does not understand: an unknown command
  such as `\notacommand` becomes transparent (invisible) text, not an error.
  A passing test therefore does not prove a formula looks right; check new
  math visually (see "Checking graphics visually" below).
- In mhchem `\pu{}`, write the degree sign directly (`\pu{100 °C}`);
  `\pu{100 ^\circ C}` draws a spaced-out ring operator ("100 ∘ C").
- merman sizes each diagram label as a nested `<svg>` with
  `overflow:hidden`, while resvg draws the text with whatever system font
  `sans-serif` resolves to. With merman's default (deterministic) text
  estimates, a wider real font clipped labels ("Appliance" drawn as
  "ppliand"). Fixed in 0.4.0 by merman's `native-fonts` feature, measuring
  with exactly the font files of the family `pick_family` chose (`font_files`);
  merman's own `with_system_fonts(true)` may pick a different family than
  ours and is not used. The unit test `labels_fit_their_boxes` checks every
  label box against resvg's drawn width. Diagram sizes therefore depend on
  the installed fonts. Cost: about 20-30% more time per diagram layout,
  some tens of ms to load the fonts once, +1.8 MB stripped binary.
- Which font draws what: body text, headings, lists and table cells use the
  GTK UI font (`gtk-font-name`: Adwaita Sans on GNOME 48, Cantarell before);
  inline code, code cards and the fallback sources use fontconfig's
  `monospace`; math and chemistry use STIX Two Math, built into mathra and
  drawn as outlines (identical everywhere); diagrams ask for
  `"Recursive Variable", arial, sans-serif`, which resolves to the first
  installed of Adwaita Sans, Cantarell, Noto Sans, DejaVu Sans, Liberation
  Sans, Inter (`pick_family`), at merman's own sizes (zoomed).
- Timings (release build, 12 CPUs, a generated document of 300 graphics:
  280 formulas, 20 flowcharts): rendering them one after another took about
  1.1 s, which 0.3.0 waited for before showing anything; the worker pool
  takes about 0.22 s in total, and the first section's graphics arrive
  within about 10 ms. Rasterizing dominates, not diagram layout: re-rendering
  everything at another zoom with every layout cached still took about
  1.05 s serially. In debug builds everything is several times slower.
- A `GtkScrolledWindow` with `vscrollbar_policy` `External` still counts as
  vertically scrollable and swallows mouse wheel scrolling (0.3.0's tables
  stopped the document from scrolling under the pointer). Use `Never` where
  the content's own height is right (code cards); tables need `External`
  (their height is measured for the right width by `Resizable`), so they
  pass vertical scrolls on with a capture-phase controller.
- A text view's vertical adjustment never has an `upper` below the window
  height, so "is the page taller than the window" must be measured at the
  end of the buffer (`line_yrange(end_iter)`).
- `TextView::iter_at_location` returns `None` inside the left margin (x = 0
  of `visible_rect`); use `line_at_y` to find the line at the top.
- Paintables in a text buffer (`insert_paintable`) and in a `GtkPicture`
  follow `invalidate_size`/`invalidate_contents`, so `ScaledTexture` can
  change picture and size in place.
- Driving a headless GUI for manual checks: `xdotool` works against the
  Xvfb display (`xdotool mousemove 400 500 click 5` scrolls,
  `keydown ctrl click 4 keyup ctrl` zooms in, `key ctrl+0`); combine with
  `import` screenshots. The automated tests cannot press keys, so zoom and
  the copy button are checked this way.
- Link safety (`links::link_action`, `local_file_risk`, `link_tooltip`):
  `http`/`https` go to the browser (a remote `.md` too: the viewer never
  fetches), `mailto:` to the mail client, local Markdown opens in the
  viewer, other schemes and `file://host/...` are refused with the reason
  in the tooltip. A local non-Markdown file opens directly only if it is a
  regular, non-executable file whose type by name and by content (GIO's
  `content_type_guess`, the first 4 KiB read through `read_head`, which
  does not block on a FIFO) is an image (not SVG), PDF, plain text, audio
  or video, and GIO does not call it executable (`text/plain` itself is
  exempt: GIO counts every text type as a possible script); otherwise
  `DocWindow::confirm_open` shows an `adw::AlertDialog` with the reason,
  Cancel as the default and "Open Anyway" as destructive. Every link's
  tooltip says what a click does. Gotcha: an `AlertDialog` heading that
  wraps makes libadwaita print "Trying to measure GtkLabel ... needs at
  least" warnings, so the heading is kept short ("Open Dangerous
  Content?"); the body carries the reason.
- Headings not on the page yet: `links::heading_blocks` maps every
  heading's anchor (same suffixes as `Anchors`) to its block when the
  document opens, so `Session::scroll_to_anchor` renders up to the block
  (`render_to`) and scrolls once the heading is appended; an anchor naming
  no heading leaves nothing pending, and `#` scrolls to the top.
- The frame callback (`Session::wake_ticks`) runs only while there is
  work: it stops after `layout::IDLE_FRAMES` frames that resize nothing
  with no held spot, and is woken by the adjustments' `changed` signals
  (the view's width shows in the horizontal one), new blocks, redrawn
  graphics and `hold_anchor`.
- While a document loads, an active search is refreshed at most every
  `layout::SEARCH_REFRESH_MS` (200 ms), at once when the last block is on
  the page, and before stepping to another match (`request_refresh`).
- The budget counts each pixel buffer once (`progressive::held_bytes`, by
  `Arc` pointer; a slot's texture keeps its buffer alive, so a key is not
  reused while counted) and drops buffers whole (`plan_budget`). The cache
  returns the entry another worker inserted first (`insert_bounded`), so
  one formula shown many times shares one buffer. If what is near the view
  alone exceeds the budget, the newest graphics there show "can't render:
  document memory limit reached" (`Held::Limited`,
  `ScaledTexture::show_message`) and are asked for again once they fit.
  The budget is planned from an idle callback after layout (`schedule_budget`),
  never right after appending: lines not yet laid out report positions
  that make everything look near the view.
- Graphics over the document limit (`Held::Limited`) are asked for again
  wherever they are on the page as soon as they fit in the room left
  (`Session::redraw_near`, nearest first, after counting the dropped
  graphics near the view that are being redrawn), so the message is only
  shown while the budget really is full.
- The image cache key includes the file's `graphics::file_stamp` (size,
  mtime, inode; `None` when missing), taken at each lookup, so a changed or
  newly created image file is decoded again on reopen or the next zoom, and
  the entry for its old version is removed. A file an SVG file refers to is
  not stamped.
- Case-insensitive search uses Unicode full case folding: `src/casefold.rs`
  is the `C` and `F` lines of the UCD's `CaseFolding.txt`, generated once
  (one `(char, &str)` pair per line, sorted, binary-searched; a character
  not listed folds to itself). Do not mix it with `char::to_lowercase`:
  folding maps Cherokee small letters to the capitals, the opposite of
  lowercasing, so a mix would never match them. To update, regenerate the
  table from a newer `CaseFolding.txt` (fetched by hand, outside the
  viewer) and update the version in its header.
- Monitors and scales (as GNOME does it): on Wayland each monitor has its
  own scale (100%/200% by default, 125%/150%/175% with fractional scaling)
  and a window keeps its logical size on every monitor; a window across two
  monitors gets the scale of the one it is mostly on. X11 has one global
  scale. GTK's `Widget::scale_factor` rounds a fractional scale up (1.25 and
  1.5 both give 2) and only notifies when that integer changes, so the
  viewer reads `gdk::Surface::scale` (`screen_scale`) and follows the
  surface's `notify::scale` and `enter-monitor` (`DocWindow::follow_monitor`,
  connected on realize). `Session::rescale` keeps every graphic's logical
  size and re-renders with the new device pixels, works the zoom limit out
  again (it can rise), and does not zoom the window out when the new limit
  is below the zoom (`Zoom::set_ceiling_keeping_level`, until the user next
  zooms); the window finding its first monitor is not a move and zooms out
  as usual. The unit tests turn following off (`follows_monitor`) and call
  `rescale` directly; a real monitor move is only checked by hand.
- `Anchors` keeps two maps, our slugs and GitHub's, each with its own `-N`
  duplicate counter; `lookup` tries ours first, so adding the GitHub
  spelling never changes where an existing link goes.
- The window's find shortcuts (`handle_key`: `/`, `n`, `N`, Escape) run in
  the capture phase over the whole window, so they step aside
  (`keys_belong_elsewhere`) while an `adw` dialog is shown in the window
  (`visible_dialog`) or the focus is in a popover; otherwise they would
  take Escape from the dialog and the menus.
- Link click handlers (text view, table-cell labels, linked pictures) act
  on a single click only (`layout::opens_link(n_press)`): the second
  release of a double click would open the link again.
- Regex search runs on each piece of text between U+FFFC object
  placeholders (`search::regex_ranges`), so `.`/`\W`/`\S` never match an
  embedded object and no match spans one. A leading byte-order mark is
  stripped in `render`. `ImageRef::failure_reason` is the one place an
  image placeholder's reason comes from (text view and table cells).
- The spot held after a zoom (`hold_anchor`) is let go of on user input
  only: Ctrl-less wheel scrolling (the window's scroll controller), a
  scrolling key (`layout::is_scroll_key`), a press on the vertical
  scrollbar and a touch drag (`Session::release_anchor_on_input`, named
  `RELEASE_CONTROLLER` so the tests find and emit them). Do not release on
  `value-changed`: GTK moves the value itself while a zoom resizes the page.
- Each `Job` carries a `progressive::Location` (table cell as row and
  column, index among the inlines), so `place` stores a result without
  walking the document; keep `for_each_graphic`'s order and the locations
  in step (`every_location_reaches_its_graphic`).
- A GTK controller's closures must not capture the widget the controller
  is added to (widget -> controller -> closure -> widget is a cycle that is
  never freed); take it from the controller (`widget()`) or the signal's
  arguments, as `make_label_clickable` does. A window's zoom CSS provider
  is added to the display, so its close-request handler removes it.
- Shrunk inline images are cached by image key and zoom
  (`Cache::inline_images`), so repeated inline uses share one buffer and
  the budget counts it once.
- `read_limited` checks the path, opens with `O_NONBLOCK` (a file swapped
  for a FIFO cannot block the open) and checks the opened file again.
- `Session::budget` is the document budget as a field only so the GUI unit
  test can lower it (a real 1 GiB document would be too heavy for a test).
- Headless GUI runs: start `Xvfb :77 &` and run with `DISPLAY=:77
  GDK_BACKEND=x11 GSK_RENDERER=cairo`. GTK then warns that it cannot
  acquire a session bus (no `dbus-launch`); that warning is harmless, and
  the CLI tests only reject `CRITICAL` and panics.
- Screenshot of a headless run: start the viewer in the background, wait a
  few seconds, `DISPLAY=:77 import -window root /tmp/shot.png`
  (ImageMagick).
- Stopping a viewer started in the background: use `pkill -x md_viewer`.
  `pkill -f md_viewer` also matches the shell running the command (its
  command line contains the pattern) and kills or hangs it.
- Text buffer offsets count characters, but `search::Matcher::find` returns
  byte ranges; `search::char_ranges` maps them. Pango `AttrList` indices
  (used to highlight table and code labels) are byte offsets into the
  label's text, so the byte ranges are used directly there.
- A focused `GtkSearchEntry` actually holds focus in its inner editable
  `GtkText`, not in the entry widget itself, so comparing the focus widget
  to the entry is always false and a window-level key handler steals the
  keys typed into the find bar (0.7.0 could not type `n`/`N`). Test
  structurally instead: the focus is the entry or a descendant of it
  (`focus.is_ancestor(&entry)`). `Widget::has_focus` is not reliable here:
  it requires the toplevel to have the *global* input focus, which is false
  under Xvfb with no window manager.
- `pango::Layout::xy_to_index` takes coordinates in **Pango units** (1/1024
  px), not pixels: multiply widget coordinates by `pango::SCALE` before
  hit-testing a label (see `label_link_at` in `main.rs`). Passing pixels
  makes every hit land near the first character.
- `Widget::query-tooltip` reports `x, y` in widget coordinates, and only
  fires when `has_tooltip` is true. Link hover tooltips use it on both the
  text view (`dest_at`) and the table-cell labels (`label_link_at`), with
  `Tooltip::set_text` (plain text, so URLs with markup characters are safe).
- Pango `AttrColor` channels are 16-bit (0-65535), unlike the 8-bit hex
  used by GDK/CSS; `layout::pango_channel` widens an 8-bit value by 257.
  Passing an 8-bit value straight through made the highlight background
  effectively black (matched text looked like black squares).
- Never mix `use_markup` with `GtkLabel::set_attributes`. GTK's
  `gtk_label_update_layout_attributes` merges the attribute lists with
  `_gtk_pango_attr_list_merge`, which aliases `markup_attrs` when the CSS
  style contributes no attributes and then folds the manually-set list into
  it; `set_attributes(NULL)` cannot remove them, so old highlights stuck
  around (single `c` letters stayed after the query changed). Table and
  math-cell labels therefore use plain text and one attribute list rebuilt
  from `label_text`'s `StyledRun`s plus the highlight ranges (`label_attributes`
  in `main.rs`).
- `TextBuffer::text` (`get_text`) omits the U+FFFC characters that represent
  embedded child anchors and paintables, so its indexes drift from buffer
  offsets; `TextBuffer::slice` (`get_slice`) keeps them. The find bar uses
  `slice` (`searchable_text` in `main.rs`) so highlights do not shift after
  tables, code cards, diagrams or formulas.
- A search started while a document is still loading only covers what is on
  the page so far; `append_ready` re-runs it as blocks arrive, so the count
  and marks grow until loading finishes.
- Consecutive paragraphs are not separate `Block::Text`s: a blank line only
  separates blocks when a table, diagram or code card sits between them, so
  two paragraphs (and any images in them) are inlines of the same block.
  Whether an image is "alone on its line" must therefore be decided from the
  inlines around it (`image_is_standalone`), not from the block: looking back,
  stop at the first newline (that is the start of the line); looking forward,
  stop at the first newline. Stopping only when the whole inline list is
  exhausted makes every image in a merged block look inline. The text before
  the image on its line may be a list marker (`• `, `12. `, `[x] `/`[ ] `,
  with nesting indentation), which `line_prefix_is_marker` accepts so an image
  alone in a list item is standalone; a block quote adds no marker text, so
  it already works.
- `image` crate pixels are straight (non-premultiplied) RGBA, but
  `gdk::MemoryFormat::R8g8b8a8Premultiplied` is what `ScaledTexture` stores;
  `graphics::premultiply` multiplies each channel by alpha first. Skipping it
  makes transparent areas show as dark fringes.
- Do not hold a `Cache` map's guard while calling `insert_bounded`: writing
  `match lock(&map).get(&key) { ... insert_bounded(&map, ...) }` keeps the
  scrutinee's guard alive for the whole `match` and deadlocks on the second
  lock. Read `.cloned()` in its own statement first (the image cache does).
- An image `Graphic`'s `inline` flag changes its sizing (inline is capped to
  a few lines, standalone keeps its natural size and is fit to the window by
  `Resizable`), so it is part of the job, not just the GUI: a zoom re-render
  reproduces the same size class.
- A standalone image is wrapped in a `md-image` box so a search match can
  outline it with a CSS `box-shadow`; `Resizable` still sizes the inner
  `GtkPicture` (the box sizes to it). Do not put a white card background on
  it: photos are not diagrams.
- An inline image is one `U+FFFC` buffer character, so a link over it is a
  `CharacterRange` of `offset..offset+1` in `LinkRanges`; the text view's
  `dest_at` then resolves a click on the picture. The picture itself is a
  paintable, not a widget, so an alt-text search match can only be counted
  and marked on the scrollbar, not visibly highlighted.
- A table cell that contains a *local* image must use the rich (non-wrapping)
  cell path even when the image failed to load: that image is a worker job, so
  it needs its `Slot`, and skipping the path would shift every later slot off
  its job id. A network or unsupported image has no job and can stay on the
  plain wrapping-label path.

### Checking graphics visually

To look at every formula and diagram of a document exactly as the app
renders them, without the GUI: create a scratch crate outside the repository
(e.g. under `/tmp`) that depends on `md_viewer` by path, build it with
`CARGO_TARGET_DIR` pointing at the repository's `target/` to reuse the
build, and in it call `open_document` + `graphics::render_document` (e.g.
with `scale: 2.0`), walk the blocks and write each `Image` as a PPM
(premultiplied RGBA composited on white: `channel + 255 - alpha`). Convert
and stack them with ImageMagick (`magick a.ppm b.ppm -append out.png`) and
view the PNGs. Never commit the scratch crate.

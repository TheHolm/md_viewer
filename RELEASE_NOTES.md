# Release Notes

History of tagged `md_viewer` releases. Each entry has a **User-facing
changes** summary (what also appears in the GitHub Release body) and a
**Details** section with the low-level technical narrative. New entries are
added at the top. `scripts/extract-release-notes.sh` prints just one release's
section for the GitHub Release body.

## v0.8.1 — Heading links and a size limit for graphics

### User-facing changes

- Links to headings (`[Usage](#usage)`, a table of contents,
  `other.md#section`) now work for every heading. Before, only a heading
  that happened to sit alone between two tables, code blocks or diagrams
  could be linked to; most such links did nothing. Headings with non-ASCII
  text (`#über-uns`) can be linked too.
- Very large graphics no longer crash the viewer or eat gigabytes of
  memory. Nothing is drawn larger than ten screens: an image, SVG, formula
  or diagram too large even at 100% shows "too large" with its size in its
  place (also for inline formulas, which now say why they could not be
  drawn), and a small PNG that claims a huge size is refused without being
  decoded.
- A document with a very large picture limits how far it can be zoomed in:
  the zoom stops where the picture reaches the limit, and if such a picture
  appears while you are zoomed in further, the window zooms out to it.
- The header bar shows the zoom percentage whenever it is not 100%
  ("(max)" at the largest zoom allowed); click it to reset to 100%.
- Raster images are no longer enlarged on HiDPI screens (that added no
  detail but multiplied their memory), and a large image used inline in the
  text keeps only the pixels it is drawn with.
- A document's pictures, formulas and diagrams hold at most 1 GiB of memory:
  when a document needs more, graphics far from the view are dropped (a
  spinner of the same size stands in) and drawn again as you scroll back to
  them. A single graphic needing more than that shows "memory limit" in its
  place.
- A document, image or picture referred to by an SVG that is not a regular
  file (a pipe, a device such as `/dev/zero`) or is larger than 512 MiB is
  refused instead of hanging or exhausting memory. Pictures inside an SVG
  file now load relative to the SVG's own folder.
- A bug in one of the rendering libraries no longer stops a document from
  finishing loading: the graphic shows "renderer failed" instead.

### Details

- Version 0.8.0 → 0.8.1 (bug fixes).
- Heading anchors were computed per text block: a block holds many
  headings, paragraphs and lists, so the one anchor registered was the slug
  of the whole block's text. `links::heading_starts` now finds each heading
  in a block (a run of spans with the same `Style::Heading`, plus adjacent
  formulas and images), the GUI registers each at the buffer offset of its
  first character, and `links::Anchors` (moved out of `main.rs`, tested)
  adds the `-1`, `-2` suffixes. `resolve_link` percent-decodes fragments
  (the URL parser encodes non-ASCII), using the `percent-encoding` crate
  that `url` already depended on.
- `graphics::zoom_limit` checks a graphic's size at 100% against
  `RasterSettings::max_px` (ten times the window's monitor in device
  pixels, `max_px_for`; `FALLBACK_MAX_PX` without a monitor) before any
  pixmap is allocated, in floating point (a huge SVG size used to wrap to a
  "no area" error). `rasterize` (SVG files, formulas, diagrams) refuses or
  caps at `Image::max_zoom`; `decode_image` checks the oriented size from
  the file header and sets `image::Limits` as a backstop, and no longer
  resizes for the device scale (the slow Lanczos resize over the whole
  file is gone; the texture is the file's pixels at scale 1). `cap_inline`
  resamples a shrunk inline image to its drawn size (at least 32 pixels on
  its shorter side). The settings key of both caches includes the limit.
- `Zoom` has a ceiling (`set_ceiling`, `max`, `at_max`) that all zoom
  changes are clamped to, and `scroll` stops at the ends instead of
  stepping once per accumulated click. The session lowers its zoom limit as
  graphics arrive (`note_limit`, also counting the largest heading via
  `layout::text_zoom_limit`) and reports it after each poll; the window
  then clamps its zoom, zooming out if needed. `Zoom::indicator` gives the
  header bar text.
- `Math::failure_note` gives the note after a formula's source, now also
  for inline math.
- `read_limited` (regular files only, at most `MAX_FILE_BYTES` = 512 MiB)
  reads the document, image files and SVG references; a custom usvg
  `ImageHrefResolver` resolves those against the SVG's folder, reads nothing
  for formulas and diagrams, and checks embedded raster headers.
- Worker pool: `Shared::cancel` sets the flag under the queue lock (a worker
  about to wait could miss it and sleep forever); panics in a renderer are
  caught (`render_caught`) and delivered as errors. Lazy rendering also
  advances on the adjustment's `value-changed`, not only `changed`.
- Memory: pixels are an `Arc<Vec<u8>>` shared by the cache, the document
  and the texture (no copies); results are cleared from the parsed document
  once their block is on the page; cache maps are bounded in bytes. A
  per-document budget (`DOCUMENT_BUDGET`) drops far graphics
  (`progressive::pick_drops`, `Session::enforce_budget`), showing an
  `adw::SpinnerPaintable` until `Session::redraw_near` re-renders them.
- Tests: heading detection and anchors (several headings per block,
  duplicates, markup, formulas, lists and quotes), fragment decoding, the
  size limit (oversized SVG, formula, diagram and PNG, a PNG bomb, Exif
  orientation, zoom stopping at the limit, the cache per limit, inline
  shrinking, no HiDPI enlargement), the zoom ceiling and indicator, a GUI
  session test of the anchors and of the zoom ceiling, and a CLI smoke test
  opening oversized graphics.

## v0.8.0 — Lazy rendering and complete search

### User-facing changes

- Very large documents render only what is near the view: graphics far below
  the window are drawn as you scroll to them, so opening a huge document no
  longer renders every formula and diagram up front. The header spinner turns
  while content near the view is being drawn and stops once enough is ready.
- Search now covers the whole document from the moment it opens, not just the
  part that has been rendered: text, table cells, code and image alt text far
  below the window are found at once. While more is still rendering (diagram
  labels and the sources of formulas that could not be rendered appear as
  they are drawn), the find bar shows "…" next to the match count until every
  result is in.

### Details

- Version 0.7.1 → 0.8.0 (new feature).
- A rendering "frontier" limits which graphics the worker pool renders: only
  the jobs up to a point a couple of screens past the bottom of the window are
  submitted, and the frontier advances (a batch of jobs at a time) as the
  user scrolls near the end of what is rendered. The worker pool no longer
  finishes when its queue empties; it waits (on a condition variable) for
  more jobs, and `Pool::submit` appends them. `Progress::reset_unshown` now
  takes the frontier, so a zoom only re-renders the graphics already on or
  near the page.
- Search is built from the parsed blocks (a canonical, ordered list of text,
  table-cell, code and image-alt units) rather than only from the appended
  buffer and widgets, so it is complete while the document is still being
  rendered. Diagram labels and formula fallback sources still only become
  searchable once they are drawn; the find bar reports that more is pending.

## v0.7.1 — Fix typing in the find bar

### User-facing changes

- Typing in the find bar no longer triggers the viewer's search shortcuts.
  Before, `n` and `N` stepped to the next/previous match instead of being
  typed, and `/` and Escape were also caught by the window's shortcut
  handler rather than handled by the text field.

### Details

- Version 0.7.0 → 0.7.1 (bugfix).
- The window has a capture-phase `EventControllerKey` (`handle_key`) that
  turns `/`, `n`, `N` and Escape into search actions, ignoring them while
  the find bar is focused. It decided that by comparing the focused widget
  to the `GtkSearchEntry` itself, but GTK focuses the entry's inner editable
  text, so the comparison never matched and the handler swallowed those keys
  while typing. The test now also accepts a focused widget that is a
  descendant of the entry (the inner text). `GtkWidget::has_focus` was not
  used because it depends on the toplevel holding global input focus, which
  is not reliably the case (for example without a window manager).

## v0.7.0 — Local images

### User-facing changes

- Images referenced by `![alt](path)` in a paragraph or a table cell are now
  loaded from disk and shown: an image alone on its line is scaled to the
  window width, one among text sits inline at about the line height, and one
  in a table cell is drawn inline in the cell. Relative paths resolve
  against the document; PNG, JPEG, GIF (first frame), WebP and SVG files
  work.
- An image that is a link's content (`[![alt](path)](dest)`) is clickable: a
  click opens the destination, with the pointer cursor and a tooltip, just
  like link text.
- An image's alt text is searchable: a match on a standalone or table-cell
  picture outlines the picture, and one on an inline picture is counted and
  marked on the scrollbar.
- Images are local-only: a network image (`http`/`https`) is **not
  supported and will not be** - the viewer loads nothing over the network -
  and says so in its placeholder.
- An image that cannot be shown (missing file, unreadable format, network
  address) appears as `[image: <alt> — <reason>]` instead of being dropped.
- Images scale with zoom like everything else, and a re-render sharpens the
  pictures after the zoom settles.
- Photos whose Exif data says which way is up (phones and cameras) are shown
  upright, raster images stay sharp on HiDPI displays, an image alone in a
  list item or block quote is scaled to the window width like any standalone
  image, and raster images embedded in an SVG file are drawn.

### Details

- Version 0.6.0 → 0.7.0 (new feature).
- The renderer no longer writes `[image: alt]` as text: it collects the alt
  text while inside `Tag::Image` and emits `Inline::Image(ImageRef)`
  (source, alt, `image`, `ImageState`). `Inline::text` is the alt text;
  `label_text`/`pango_markup` still show the placeholder, since a label
  cannot hold a picture (table cells and the GUI draw images as pictures).
- New `md_viewer::links::resolve_image` resolves a destination into
  `ImageState::Local(path)` / `Remote` / `Unsupported(reason)`; the new
  `resolve_images` walks a rendered document and fills the states in, called
  by `open_document` and the GUI (which know the document path).
  `image_is_standalone(inlines, index)` decides, line by line within the
  merged text block, whether the GUI should treat an image as a block,
  accepting a leading list marker (`• `, `12. `, `[x] `/`[ ] `) so an image
  alone in a list item counts.
- New `md_viewer::graphics::decode_image`/`load_image` decode a file into
  premultiplied RGBA with the pure-Rust `image` crate (new dependency:
  `default-features = false`, `png`, `jpeg`, `gif`, `webp`); the decoder's
  Exif orientation is applied, and raster images are rendered at the device
  scale (a Lanczos resize that keeps the logical size) so they stay sharp on
  HiDPI. `.svg` files use the existing resvg path, with resvg's
  `raster-images` feature enabled so raster images embedded in an SVG are
  drawn. Logical size grows with zoom, like math and
  diagrams. `Graphic::Image { path, inline }` joins the worker jobs; the
  shared `Cache` also keeps decoded images by path and settings, and inline
  images are capped to a few lines high.
- The renderer records an image's link destination (new `ImageRef.link`) when
  the image is a link's content, and `label_text` keeps it on the placeholder
  so even a failed image inside a link stays clickable.
- The GUI inserts a standalone image as a `GtkPicture` in a `md-image` box
  sized to the view (registered with `Resizable` alongside diagrams), an
  inline one as a buffer paintable, and a table-cell image as a picture in
  the cell's row of labels. Linked pictures get a click controller (or, for an
  inline one, a `LinkRange` over its buffer character).
  Network/unsupported/unreadable images are placeholder text and add no slot.
- Images in table cells are rendered too (`progressive::for_each_graphic`
  visits them as inline jobs). Every picture's alt text is a search entry;
  standalone and table-cell pictures outline on a match, inline ones only
  count and mark the scrollbar (a paintable is not a widget).

## v0.6.0 — Clickable links

### User-facing changes

- Links are clickable: a single click opens the destination in the text and
  in table cells, and the pointer changes to a hand over a link. Dragging to
  select still selects instead of opening. Hovering a link shows its raw
  destination in a tooltip.
- Web (`http`/`https`) and email (`mailto`) links open in the desktop's
  default handler.
- Local links open on disk: a Markdown file opens in the viewer (a new
  window), any other file opens in its default application, and relative
  paths are resolved against the document being viewed.
- `#fragment` links scroll to the matching heading in the same document;
  a Markdown link with a fragment opens that document and scrolls to the
  heading. Unsafe or unsupported destinations (`javascript:`, `data:`, ...)
  are ignored.

### Details

- Version 0.5.0 → 0.6.0 (new feature).
- `Span` gains a `link` destination, threaded through the renderer (so
  styled text inside a link, autolinks and empty links keep their
  destination, and adjacent spans only merge when their destinations match).
  `label_text` now also returns each link's byte range.
- New `md_viewer::links`: `resolve_link` (resolves a destination against the
  document into a web/local/Markdown/anchor target), `heading_slug`
  (GitHub-style heading anchors), `LinkRanges` (character ranges of the
  clickable text) and `file_uri`. The pure-Rust `url` dependency is added
  for resolution and percent-decoding.
- The GUI records link ranges while appending text and table cells, installs
  a click gesture and a motion controller on the view (and per-cell labels,
  hit-tested through the Pango layout), opens links with `GtkUriLauncher`,
  and keeps a per-session map of heading anchors (with duplicate `-N`
  suffixes) to scroll to. A fragment for a block not yet appended is
  remembered and applied when it arrives.

## v0.5.0 — Search

### User-facing changes

- Find text with Ctrl+F or `/`: matches are highlighted in the text, tables
  and code, and a match inside a diagram highlights the whole diagram.
- Step through matches with Enter / `n` / `N` (or Ctrl+G / Ctrl+Shift+G)
  and the up/down buttons in the find bar; every match is marked on the
  scrollbar, with the current one emphasised.
- Escape closes the find bar but keeps the highlights so `n`/`N` still
  work; a second Escape clears the search.
- A button next to the search field switches between smart case (the
  default), ignore case and regular expressions; an invalid pattern is
  reported in the bar.
- Fixed: highlights were offset (highlighting the wrong characters) after a
  table, code card, diagram or formula, because the buffer text searched
  omitted the embedded objects whose characters buffer offsets count.
- Fixed: matches inside tables and code cards were drawn as black squares;
  Pango colour channels are 16-bit, and the highlight colours were passed
  as 8-bit values.
- Fixed: changing the query left old single-letter highlights behind in
  table cells (e.g. every `c` stayed highlighted after searching `coffee`),
  because mixing `use_markup` with `GtkLabel::set_attributes` folds the
  highlight attributes into the label's markup list, where they cannot be
  cleared. Table and math-cell labels are now plain text styled with a
  single Pango attribute list, rebuilt for each search.

### Details

- Version 0.4.0 → 0.5.0 (new feature).
- New `md_viewer::search`: `SearchMode` (smart case, ignore case, regex),
  `Matcher` (a query compiled once; byte-range matches; Unicode case
  folding; zero-width regex matches skipped), `char_ranges` and the match
  navigation helpers. The `regex` dependency is added.
- `graphics::rasterize` now collects the SVG's text nodes into
  `Image::labels`, so the find bar can search diagram text; `render_math`
  clears them (formulas are outlines, so their TeX source is not
  searchable). A diagram match highlights its whole card.
- The GUI keeps a `Search` per `Session`: the text buffer plus one entry
  per table-cell label, code-card label and diagram, highlighted with text
  tags, a rebuilt Pango attribute list (base style runs plus highlights,
  from `label_text`) and a CSS class respectively. Marks are drawn on
  a `DrawingArea` over the scrollbar, and the search re-runs as blocks are
  appended while a document loads. The buffer is searched via
  `TextBuffer::slice`, which keeps embedded objects as U+FFFC so its
  character indexes match buffer offsets.

## v0.4.0 — Zoom, copyable code blocks, faster loading

### User-facing changes

- Zoom: Ctrl+mouse wheel, Ctrl+plus / Ctrl+minus, Ctrl+0 to reset, or a
  touchpad pinch, from 50% to 300%. Text, tables, code, formulas and
  diagrams all grow; formulas and diagrams are redrawn sharp at the new size,
  and the part being read stays in place. The zoom applies to the window
  and to the next document opened in it.
- Code blocks are cards with their language and a copy-to-clipboard button
  (it shows a check mark after copying); long lines scroll sideways.
- Large documents open much faster: formulas and diagrams are drawn on all
  CPU cores, the first screen appears as soon as it is ready (a spinner
  fills the window until then), and the rest is added below it while a
  small spinner turns in the header bar. Nothing already shown moves.
- Fixed: labels in diagrams (class and state diagrams especially) were cut
  off at their edges.
- Fixed: the mouse wheel over a table stopped the document from scrolling.
- Diagrams use GNOME's UI font (Adwaita Sans) when it is installed.

### Details

- Version 0.3.0 → 0.4.0 (new features).
- Diagram labels: merman's `native-fonts` feature measures label text with
  the same font files resvg draws it with (`graphics::native_fonts`,
  `font_files`), so a label's clipping box fits its text; merman's built-in
  estimates remain the fallback when no font files can be loaded. A unit
  test checks every label box against resvg's drawn width. Adwaita Sans
  now leads `pick_family`. Costs about 20-30% more per diagram layout and
  1.8 MB of binary.
- `Block::Code(CodeBlock)` (language, verbatim text, nesting indent) for
  fenced and indented code other than Mermaid, with `code_language` and
  `CodeBlock::clipboard_text`; a code card always starts on its own line.
  `Style::CodeBlock` is now only used for the sources of formulas and
  diagrams that could not be rendered.
- New `md_viewer::progressive`: graphics numbered in document order
  (`jobs`, `for_each_graphic`, `place`), `Progress` (blocks appended in
  order once complete), `Phase`, `zoom_order` and `Pool` (one worker thread
  per CPU over a shared queue, cancellable). `graphics::Cache` (shared)
  keeps rendered formulas and laid-out diagrams. `render_document` now uses
  the same walk. Measured on 300 graphics (release, 12 CPUs): 1.1 s one
  after another vs 0.22 s in the pool.
- New `md_viewer::zoom` (`Zoom`, `STEPS`, `wheel_clicks`, `css`) and
  `RasterSettings::zoom`; the GUI zooms text with a per-window CSS class,
  stretches pictures at once and re-renders them 150 ms after the last
  step, holding the line at the top of the window in place.
- The GUI reads and parses on the I/O pool, then builds the page from a
  `Session` that collects worker results every frame; the page sits behind
  a crossfading cover with a centred `adw::Spinner` until the first screen
  is full; a header bar spinner shows while loading or re-rendering.
- Tables pass vertical scrolling on to the document (an `External`
  scrollbar policy still swallowed the wheel); code cards use `Never`.
- Tests: new `tests/progressive.rs` and `tests/zoom.rs`; code block, layout
  and graphics (zoom, cache, fonts, label width) tests; GUI tests open a
  large generated document and close it while it is still loading.

## v0.3.0 — Open files from the GUI, app icon and menu entry

### User-facing changes

- The app has an icon and a menu entry: GNOME (and other desktops) show
  "MD Viewer" with its icon in the app grid, the dock and Alt+Tab, and it is
  offered under "Open With" for Markdown files.
- `FILE` is now optional. Started without one, the window offers an
  "Open…" button.
- An Open button in the header bar (and Ctrl+O) picks a document with the
  system file chooser. It opens in a new window; an empty window is reused.
  Ctrl+W closes a window.
- A document picked in the GUI that cannot be opened is reported in a
  dialog, and the app keeps running. A bad `FILE` on the command line still
  prints the error and exits with status 1.
- Needs GTK 4.18 and libadwaita 1.7 or newer (Debian trixie, Ubuntu 26.04
  and FreeBSD 15 all ship them).

### Details

- Dependencies: gtk4 0.9 → 0.11 (feature `v4_18`), libadwaita 0.7 → 0.9
  (`v1_7`), glib 0.20 → 0.22. The feature levels are pinned to the oldest
  target, Debian trixie. glib 0.22 moved `unix_signal_add_local` into the
  new `glib-unix` crate; `CssProvider::load_from_data` became
  `load_from_string` and `style_context().color()` became `color()`.
- `src/main.rs`: each window is a `DocWindow` (an `adw::ToolbarView` with a
  header bar over an `adw::StatusPage` start page or the document view) with
  a `win.open` action. `gtk4::FileDialog` picks the file (filtered to
  Markdown, starting in the current document's folder). The file is read on
  the I/O pool before any window changes, so a failure leaves the window as
  it was and shows an `adw::AlertDialog`. The default window icon is set to
  the application ID for desktops that use window icons.
- `src/lib.rs`: `APP_ID`, `MARKDOWN_MIME_TYPES`, `MARKDOWN_PATTERNS`,
  `Origin` (command line or chooser, and whether an error exits),
  `open_target` (reuse an empty window or open a new one) and
  `chooser_folder`.
- `data/`: `io.github.TheHolm.md_viewer.desktop`, a scalable icon and a
  symbolic icon. GNOME matches the window to the `.desktop` file by the
  application ID, which is why there was no icon before.
- Packaging: the `.deb` (cargo-deb `assets`) and the FreeBSD `.pkg` install
  them under `share/applications` and `share/icons/hicolor`; the release
  pipeline checks that both packages contain them.
- Tests: `tests/desktop.rs` (the `.desktop` file agrees with `APP_ID` and
  the MIME types, passes `desktop-file-validate` when installed, the icons
  render, and both packages install every file in `data/`); the CLI tests
  start the empty window; the opening decisions are tested in
  `tests/document_io.rs`. The chooser's response and the error dialog need
  user interaction and are checked by hand, so `main.rs` line coverage is
  about 87% (about 94% overall).

## v0.2.0 — Tables, math, chemistry and Mermaid diagrams

### User-facing changes

- Tables are real grids instead of monospace text: bold shaded header,
  striped rows, column alignment, bold/italic/code/links kept inside cells,
  long cells wrapped. Tables wider than the window scroll sideways.
- LaTeX math: `$…$` inline and `$$…$$` centred on its own line, sized like
  KaTeX and drawn in the theme's text colour (dark mode included).
- Chemistry with mhchem inside math: `\ce{…}` equations and `\pu{…}` units.
- Mermaid diagrams in ` ```mermaid ` blocks: flowcharts, sequence diagrams,
  Gantt charts, pie charts, class/state diagrams and the other Mermaid
  types, on a white card, scaled down to fit the window.
- A formula or diagram that cannot be rendered is shown as its source with
  the reason, never dropped.
- Everything renders natively (no browser engine or JavaScript); output is
  close to KaTeX and mermaid.js but not identical. The binary is larger
  (about 25 MB) because of the diagram engines.

### Details

- `src/lib.rs`: `render` now returns `Vec<Block>` (`Text`, `Table`,
  `Mermaid`) instead of a flat span list. Text blocks hold `Inline`s
  (styled `Span`s and `Math`); tables hold per-column `Align` and rows of
  styled cells. pulldown-cmark's math extension is enabled. Table cells are
  turned into Pango markup by `pango_markup`.
- `src/graphics.rs` (new): mathra typesets math (with mhchem) to SVG,
  merman lays out Mermaid through its resvg-safe pipeline, and resvg
  rasterizes both into premultiplied RGBA at the display scale, using the
  system fonts (generic sans-serif is mapped to an installed family).
  `render_document` fills in every graphic on the I/O thread pool.
- `src/main.rs`: tables are `GtkGrid`s of wrapping labels and diagrams are
  `GtkPicture`s, both anchored in the text view and sized by a tick-driven
  helper (anchored widgets otherwise only get their minimum size); formulas
  are scale-aware paintables lowered to the text baseline. Styling is a
  small CSS provider.
- New dependencies: `mathra` 1.2, `merman` 0.9 (SVG output and all diagram
  families only), `resvg` 0.48 (text and system fonts only). All pure Rust;
  no new system libraries, so CI and the FreeBSD cross-build are unchanged.
- Tests: `tests/render.rs` rewritten for the block model with table, math
  and Mermaid groups; new `tests/graphics.rs`; `tests/fixtures/showcase.md`
  covers every feature and is checked end to end.
- Coverage: the GUI's sizing and styling arithmetic moved into
  `md_viewer::layout` (tested in `tests/layout.rs`), and `tests/cli.rs`
  runs the binary: argument handling always, and with a display an
  unreadable file plus the showcase and a new `fallbacks.md` fixture.
  SIGINT/SIGTERM now quit the viewer cleanly. Line coverage with a display
  is about 98%.

## v0.1.0 — Initial release

### User-facing changes

- A GTK4/libadwaita program that opens the Markdown file given on the command
  line (`md_viewer <FILE>`) and shows it read-only in a scrollable window
  titled "<file name> — MD Viewer". The file is never modified.
- Renders headings, emphasis, strong text, strikethrough, inline code, code
  blocks, block quotes, ordered, unordered and task lists, horizontal rules,
  links (styled, not clickable) and tables (aligned monospace text). Images
  are shown as `[image: <alt text>]` placeholders and raw HTML as plain text.
- If the file cannot be opened the program prints an error and exits with
  status 1.
- Ready-to-install amd64 packages: Debian trixie `.deb`, Ubuntu 26.04 `.deb`
  and FreeBSD 15 `.pkg`.

### Details

- `src/lib.rs`: `render` drives a `pulldown-cmark` parser (tables,
  strikethrough and task list extensions) through a small `Renderer` state
  machine that outputs a flat `Vec<Span>`; each `Span` is text plus a sorted,
  deduplicated set of `Style`s, and adjacent spans with equal styles are
  merged. Tables are collected cell by cell and laid out by `layout_table`/
  `pad_cell`. Also `read_document`, `open_document`, `error_message` and
  `window_title`.
- `src/main.rs`: clap parses `FILE` before GTK starts (GTK only gets the
  program name). The window is an `adw::ApplicationWindow` with a header bar
  and a read-only `GtkTextView`, with one `GtkTextTag` per `Style`. The file
  is read and rendered on `gio::spawn_blocking`; the buffer is filled from a
  `glib::spawn_future_local` block on the main thread.
- `tests/render.rs` (one group per block type and inline style) and
  `tests/document_io.rs` (reading, failure paths, error message, window
  title), plus unit tests for the private helpers in `src/lib.rs`.
- `.woodpecker/release.yaml` builds all three packages on `v*` tags and
  publishes them to GitHub Releases; every package step is required, so a
  failing FreeBSD build fails the release.
- The `VERSION` file is the single source of the version: `md_viewer
  --version` (`md_viewer::version()`) and the FreeBSD `.pkg` read it, and
  `Cargo.toml` keeps a copy that `tests/version.rs` and
  `scripts/check-version.sh` require to match. The release pipeline runs
  `check-version.sh` first and refuses a tag that is not `v<VERSION>`.

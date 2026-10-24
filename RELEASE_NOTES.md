# Release Notes

History of tagged `md_viewer` releases. Each entry has a **User-facing
changes** summary (what also appears in the GitHub Release body) and a
**Details** section with the low-level technical narrative. New entries are
added at the top. `scripts/extract-release-notes.sh` prints just one release's
section for the GitHub Release body.

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
  tags, Pango attributes and a CSS class respectively. Marks are drawn on
  a `DrawingArea` over the scrollbar, and the search re-runs as blocks are
  appended while a document loads.

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

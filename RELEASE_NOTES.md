# Release Notes

History of tagged `md_viewer` releases. Each entry has a **User-facing
changes** summary (what also appears in the GitHub Release body) and a
**Details** section with the low-level technical narrative. New entries are
added at the top. `scripts/extract-release-notes.sh` prints just one release's
section for the GitHub Release body.

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

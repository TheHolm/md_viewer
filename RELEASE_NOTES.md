# Release Notes

History of tagged `md_viewer` releases. Each entry has a **User-facing
changes** summary (what also appears in the GitHub Release body) and a
**Details** section with the low-level technical narrative. New entries are
added at the top. `scripts/extract-release-notes.sh` prints just one release's
section for the GitHub Release body.

## v0.2.0 — Tables, math, chemistry and Mermaid diagrams

### User-facing changes

- Work in progress.

### Details

- Work in progress.

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

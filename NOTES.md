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
- `resvg` (features `text`, `system-fonts`, `memmap-fonts`) - rasterizes
  every SVG; SVG text uses the system fonts
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
  (Open button) over either the empty start page (`adw::StatusPage`, shown
  when no `FILE` is given) or a read-only `GtkTextView` inside a
  `GtkScrolledWindow`. The `win.open` action (Ctrl+O) runs a
  `gtk4::FileDialog`; the picked file opens in a new window, except that an
  empty window is reused (`md_viewer::open_target`). The file is read before
  any window changes: a failure for the command line `FILE` goes to stderr
  with exit status 1, one for a picked file to an `adw::AlertDialog`
  (`md_viewer::Origin`). Ctrl+W closes a window. Deliberately thin: it walks
  the library's blocks and handles errors and exit codes. Text goes into the buffer with one `GtkTextTag` per `Style`;
  formulas are inserted as paintables (`ScaledTexture`, which draws a
  device-pixel texture at its logical size for HiDPI), lowered by a `rise`
  tag to line up baselines, display math centred; tables become a `GtkGrid`
  of wrapping labels (Pango markup from the library) in a horizontal
  `GtkScrolledWindow`; diagrams become a `GtkPicture` on a white card. Tables
  and diagrams are anchored in the text view, which only gives anchored
  widgets their minimum size, so `Resizable` (run from a tick callback)
  sizes them from their content and the view width: tables at their natural
  width capped to the view (wider ones scroll), diagrams scaled down to fit.
  Widget styling is the `CSS` constant; sizes and classes come from
  `md_viewer::layout`. SIGINT/SIGTERM quit cleanly through the main loop.
  Reading, rendering and rasterizing run on `gio::spawn_blocking` (the I/O
  thread pool), not the GTK main loop, with `RasterSettings` (font size,
  scale factor, text colour) taken from the new view first; the results are
  awaited from a `glib::spawn_future_local` block so the widgets are built
  back on the main thread.
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
  colour constants). `render` turns Markdown into a `Vec<Block>`:
  `Block::Text` (flowing `Inline`s: `Inline::Text(Span)` - text plus its
  `Style`s - and `Inline::Math(Math)`), `Block::Table(Table)` (per-column
  `Align`, header and rows of `Cell`s, i.e. `Vec<Inline>`) and
  `Block::Mermaid(Diagram)`. `Math` and `Diagram` carry an `image` that is
  `None` after `render` and filled in by `graphics::render_document`.
  `Style::tag_name` names each style's text tag; `Style::ALL` lists every
  style so all tags can be created up front. Private helpers (`Renderer`,
  `TableBuilder`, `push_inline`, `trailing_newlines`, `normalize`) have unit
  tests in the same file.
- `src/graphics.rs` (`md_viewer::graphics`) - turning formulas and diagrams
  into pixels: `math_svg`/`render_math` (mathra; math at `MATH_SCALE` =
  1.21x the text size like KaTeX, in the text colour, with its depth below
  the baseline), `mermaid_svg`/`render_mermaid` (merman), `rasterize` (resvg
  into premultiplied RGBA `Image`s at a device scale; the system font
  database is loaded once and generic `sans-serif` is pointed at an
  installed family) and `render_document`, which fills in every graphic of a
  rendered document.
- `src/layout.rs` (`md_viewer::layout`) - the GUI's sizing and styling
  arithmetic as plain functions, so it is tested without a display: the
  width available to anchored widgets, table and diagram sizes, Pango font
  size to pixels, colour channels, heading scales, the baseline `rise` of
  inline math, column alignment and table cell CSS classes. `main.rs` only
  applies the results to widgets.
- `tests/` - integration tests split by topic:
  - `render.rs` - the renderer: one group of tests per block type
    (paragraphs, headings, lists, block quotes, code blocks, rules, tables,
    math, Mermaid) and per inline style (emphasis, strong, strikethrough,
    inline code, links, images), plus the Pango markup for table cells
  - `graphics.rs` - rasterizing, math and mhchem, Mermaid diagrams (several
    types, labels, scale, errors), filling in whole documents, that the
    showcase fixture renders, and that the root `showcase.md` renders with
    no fallbacks at all (every formula and diagram `Ok`)
  - `layout.rs` - the `md_viewer::layout` functions
  - `cli.rs` - the binary end to end: `--version`, `--help` and argument
    errors always; with a display (`DISPLAY`/`WAYLAND_DISPLAY` set) also an
    unreadable file (status 1), the empty start window (no `FILE`) and
    opening both fixtures and the root `showcase.md`, checking that the viewer stays up, prints no GTK
    criticals and exits 0 on SIGTERM. Without a display those tests print
    "skipped" and pass
  - `desktop.rs` - the files in `data/`: the `.desktop` file agrees with
    `APP_ID`, `APP_NAME` and `MARKDOWN_MIME_TYPES` and passes
    `desktop-file-validate` (skipped if not installed), the icons render at
    their sizes (the symbolic one with fills only), and both the `.deb`
    assets and the FreeBSD staging in `release.yaml` install every file
  - `fixtures/showcase.md` - a terse document using every supported
    feature plus one deliberately broken diagram; the tests' copy, with exact
    expected counts (the human demo is the root `showcase.md`)
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
  code, three tables (aligned, wrapping, math and chemistry in cells),
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
- A fenced code block whose info string starts with `mermaid`
  (case-insensitive) is a `Block::Mermaid` diagram, drawn on a white card
  in both themes (Mermaid's colours assume a light background).
- A formula or diagram that cannot be rendered is shown as its source in
  code style; display math and diagrams add a line with the reason. Nothing
  is ever dropped.
- Images show as `[image: <alt text>]`; the image itself is not loaded.
- Adjacent text with identical styles is merged into one span.

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
  is about 94% (the library about 99%); the remaining lines are the file
  chooser and error dialog paths in `main.rs`, which need user interaction,
  and defensive branches that cannot be reached with the enabled parser
  options
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
- merman lays out text with its own font metrics, while resvg draws it with
  whatever system font `sans-serif` resolves to (see `pick_family` in
  `src/graphics.rs`). Each label is a nested `<svg>` with
  `overflow:hidden` sized by merman, so a wider real font clips labels (see
  `BUGS.md`). Diagram appearance therefore depends on the installed fonts.
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

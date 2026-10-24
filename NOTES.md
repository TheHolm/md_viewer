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
- `regex` - the find bar's regular-expression mode, pure Rust
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
    (one worker per CPU) over all graphics, collects results from an
    `mpsc` channel every 16 ms (`POLL_MS`, a `glib::timeout_add_local`
    that stops when idle), stores them with their blocks and appends each
    block once it and every block before it are complete
    (`progressive::Progress`). Content is only ever appended, so nothing on
    the page moves. `progressive::Phase` decides the cover / header spinner.
  - Text goes into the buffer with one `GtkTextTag` per `Style`; formulas
    are paintables (`ScaledTexture`: a device-pixel texture drawn at its
    logical size, replaceable in place), lowered by a per-formula `rise` tag,
    display math centred; tables are a `GtkGrid` of wrapping labels in a
    horizontal `GtkScrolledWindow`; code blocks are cards (`code_card`:
    language label, copy button, monospace selectable label in a horizontal
    scroller); diagrams are a `GtkPicture` on a white card. Every graphic on
    the page has a `Slot` (its `ScaledTexture`, rise tag, diagram entry,
    buffer offset) so a re-render can replace it.
  - Anchored widgets only get their minimum size, so `Resizable` (run from
    a tick callback) sizes them from their content and the view width:
    tables at their natural width capped to the view (wider ones scroll),
    code cards as wide as the view minus their indent, diagrams scaled down
    to fit.
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
    code-card label and diagram (its extracted labels joined) is a
    `SearchEntry` registered as it is appended. Matches are highlighted
    with text tags (buffer), Pango attributes (labels) or a CSS class
    (diagram cards), the current match scrolls into view, and a narrow
    `DrawingArea` over the scrollbar's right edge draws a mark per match.
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
  colour constants). `render` turns Markdown into a `Vec<Block>`:
  `Block::Text` (flowing `Inline`s: `Inline::Text(Span)` - text plus its
  `Style`s - and `Inline::Math(Math)`), `Block::Table(Table)` (per-column
  `Align`, header and rows of `Cell`s, i.e. `Vec<Inline>`) and
  `Block::Mermaid(Diagram)` and `Block::Code(CodeBlock)` (language from
  `code_language`, verbatim text, nesting `indent`; `clipboard_text` is what
  the copy button copies). `Math` and `Diagram` carry an `image` that is
  `None` after `render` and filled in by `graphics::render_document` or the
  GUI's worker pool.
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
  installed family, GNOME's Adwaita Sans first, see `pick_family`) and
  `render_document`, which fills in every graphic of a rendered document one
  after another. `rasterize` also collects every SVG text node into
  `Image::labels` (usvg expands `<use>`, so a diagram's labels are found at
  their drawn positions); math clears them because it draws outlines. `RasterSettings` carries the text size, device scale, text
  colour and zoom (math is typeset at `font_px * zoom`, diagrams drawn
  `zoom` times their natural size). Diagram labels are measured with the
  same font files resvg draws them with (merman's `native-fonts`,
  `native_fonts`/`font_files`; merman's built-in estimates if no font files
  can be loaded). `Graphic` names one formula or diagram; `Cache` (one
  shared instance, `Cache::shared`) keeps rendered formulas by source and
  settings and laid-out diagram SVG by source, each map emptied when it
  reaches its limit.
- `src/progressive.rs` (`md_viewer::progressive`) - progressive, parallel
  loading: `for_each_graphic`/`jobs`/`place` number the graphics in
  document order and store results; `Progress` decides which blocks may go
  on the page (strictly in order, each once complete) and what to re-render
  after a zoom (`reset_unshown`); `zoom_order`; `Phase` (FirstScreen,
  Background, Done); `Pool`, the worker threads (a shared queue taken from
  the front, cancelled on `cancel` or drop; a result finished at that
  moment may still arrive, so the GUI tags runs with a generation).
- `src/zoom.rs` (`md_viewer::zoom`) - zoom levels (`STEPS`, 50%-300%),
  `Zoom` (step in/out, reset, free `set` for pinches, `scroll` accumulating
  wheel clicks), `wheel_clicks` (touchpad pixels to clicks) and the zoom CSS.
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
  (a query compiled once; `find` returns byte ranges; Unicode case
  folding maps each folded byte back to its source character, and
  zero-width regex matches are skipped), `char_ranges` (byte to
  character-offset ranges for the text buffer) and the match navigation
  helpers (`next_index`, `first_at_or_after`).
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
  - `search.rs` - the find bar's matcher and navigation helpers (modes,
    smart case, ignore case, regex, Unicode folding)
  - `progressive.rs` - numbering and placing graphics, `Progress`,
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
  in it; it is not saved between runs.
- A fenced code block whose info string starts with `mermaid`
  (case-insensitive) is a `Block::Mermaid` diagram, drawn on a white card
  in both themes (Mermaid's colours assume a light background).
- A formula or diagram that cannot be rendered is shown as its source in
  code style; display math and diagrams add a line with the reason. Nothing
  is ever dropped.
- Images show as `[image: <alt text>]`; the image itself is not loaded.
- Adjacent text with identical styles is merged into one span.
- Search matches the rendered document: the whole text buffer (including
  fallback sources), table-cell text, code-card text and the labels inside
  diagrams; formula source is not searchable (a rendered formula is
  pixels). Matches are found per source and kept in document order by
  buffer offset; a diagram match highlights its whole card.

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
- A search started while a document is still loading only covers what is on
  the page so far; `append_ready` re-runs it as blocks arrive, so the count
  and marks grow until loading finishes.

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

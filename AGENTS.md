# AGENTS.md

## Repository visibility

**This repository is pushed to a public GitHub repo.** Before committing,
pushing, or writing anything into a tracked file (code, docs, scripts, test
fixtures, commit messages), make sure it contains no sensitive or identifying
information: no real hostnames/IPs, credentials/passwords/API keys/private
keys, personal file paths, or other details tied to a particular person's or
machine's identity. Genuinely throwaway material (e.g. ad hoc scripts written
for a single session) belongs outside the repo entirely (e.g. under `/tmp`),
never committed - see the "Never commit changes unless the user explicitly
asks to commit" rule below, which exists partly for this reason.

## Project overview

`md_viewer` is a small GTK4/libadwaita desktop application for viewing
Markdown documents. It is a viewer only: it opens the file given on the
command line, renders it read-only in a scrollable window, and never writes
to it. The window title is "MD Viewer" (prefixed with the open file's name)
and the application ID is `io.github.TheHolm.md_viewer`. The package,
library and binary are all named `md_viewer`. Version: declared in the
`VERSION` file at the repository root, the single source of the version (see
"Versioning" below).

## Stack

- Rust 2021 edition, toolchain 1.98 (the version CI builds with)
- `gtk4` (feature `v4_8`) - GUI toolkit
- `adw` / libadwaita (feature `v1_2`) - GNOME application shell
- `pulldown-cmark` - CommonMark parser (with the tables, strikethrough and
  task list extensions enabled)
- `clap` - command line argument parsing (the `FILE` argument)
- `tempfile` (dev-dependency) - temporary files for integration tests

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

- `src/main.rs` - the GTK4/libadwaita GUI: the application, a window with a
  header bar and a read-only `GtkTextView` inside a `GtkScrolledWindow`.
  Deliberately thin: it maps the library's styled spans onto `GtkTextTag`s
  and handles errors and exit codes. File reading and Markdown rendering run
  on `gio::spawn_blocking` (the I/O thread pool), not the GTK main loop; the
  result is awaited from a `glib::spawn_future_local` block so the text
  buffer is filled back on the main thread.
- `src/lib.rs` - all non-GUI logic, exposed publicly so the GUI and the
  integration tests share exactly one implementation: the app version
  (`version`, the `VERSION` file embedded at compile time), reading the document
  (`read_document`, and `open_document` = read + render, which is what the
  GUI calls), the error text for an unreadable file (`error_message`), the
  window title (`window_title`) and the Markdown renderer (`render`), which
  turns Markdown into a flat list of `Span`s, each a piece of text plus the
  `Style`s that apply to it. The GUI only needs to insert spans in order and
  apply one text tag per style (`Style::tag_name`; `Style::ALL` lists every
  style so all tags can be created up front). Private helpers (`Renderer`,
  `layout_table`, `pad_cell`, `trailing_newlines`, `normalize`) have unit
  tests in the same file.
- `tests/` - integration tests split by topic:
  - `render.rs` - the renderer: one group of tests per block type
    (paragraphs, headings, lists, block quotes, code blocks, rules, tables)
    and per inline style (emphasis, strong, strikethrough, inline code,
    links, images)
  - `document_io.rs` - reading a document from disk, including failure paths,
    and the window title
  - `version.rs` - that `VERSION` is one `X.Y.Z` line, that `version()`
    reports it, and that the copy in `Cargo.toml` matches it
- `VERSION` - the app version, one `X.Y.Z` line; the single source of the
  version
- `Cargo.toml` - package metadata (its `version` is a copy of `VERSION`),
  dependencies, the `tempfile` dev-dependency and the `cargo-deb` metadata
- `.woodpecker/release.yaml` - the Woodpecker CI workflow: on `v*` tags it
  first checks the tag and `Cargo.toml` against `VERSION`, then builds a Debian trixie `.deb`, an Ubuntu 26.04 `.deb` and a FreeBSD `.pkg`,
  then publishes them to the GitHub mirror's Releases (needs a
  `github_token` secret in Woodpecker). Every package step is required.
- `scripts/` - CI helpers: `check-version.sh` (checks `VERSION` is `X.Y.Z`,
  that `Cargo.toml` matches it and, given a tag, that the tag is
  `v<VERSION>`), `build-freebsd-pkg.py` (writes a `.pkg` from a staged
  install tree; its version defaults to `VERSION`), `fetch-freebsd-gtk.py` (downloads the GTK4/libadwaita
  FreeBSD cross sysroot from `pkg.freebsd.org`) and `extract-release-notes.sh`
  (renders one release's section as a GitHub Release body)
- `RELEASE_NOTES.md` - per-version release notes; the matching `## vX.Y.Z`
  section is what a release publishes
- `README.md` - user-facing usage, build instructions and TODO list
- `LICENSE` - AGPL v3

## Rendering model

- `render` never fails: anything it does not understand (raw HTML, footnote
  references) is shown as plain text rather than dropped.
- Blocks are separated by one blank line; list items by a single newline.
- List items are prefixed with `• ` (unordered) or `N. ` (ordered), indented
  four spaces per nesting level; task list items additionally get `[x] ` or
  `[ ] `.
- Tables are laid out as aligned monospace text (cells padded to a common
  column width, honouring the column alignment) with a `-` separator line
  under the header.
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

## Status / known gaps

First version (v0.1.0). Current known gaps:

- Errors (unreadable file, invalid UTF-8) are written to STDERR and the
  program exits with status 1; a popup would be better.
- Links are styled but not clickable; images are placeholders only.
- No way to open another file from the GUI, and no reload when the file
  changes on disk.
- Table column widths count characters, not display width, so wide (e.g.
  CJK) characters can misalign columns.
- The `.deb`/`.pkg` install only the binary and docs; there is no `.desktop`
  file or icon yet.

## Commands

- Build/check: `cargo build`
- Run: `cargo run -- <FILE>` (requires a GTK4/libadwaita desktop session)
- Test: `cargo test`
- Format check: `cargo fmt --check`
- Version check: `bash scripts/check-version.sh [vX.Y.Z]` (run before
  tagging; pass the tag you are about to create)
- Coverage: `cargo llvm-cov --tests` (requires the `llvm-tools` component and
  the `cargo-llvm-cov` subcommand)
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

## Conventions

- `cargo fmt` style, no external formatters
- Every function and every test is documented with a `///` doc comment
  describing its purpose and any non-obvious behavior
- All new code must be covered by tests - integration tests in `tests/` for
  public behaviour and unit tests for private helpers; never add production
  code without accompanying tests
- Keep GUI code in `main.rs` and logic in `lib.rs`; anything testable belongs
  in the library
- The viewer never modifies the documents it opens
- Commits include a detailed description of what changed and why
- Change the version only as described under "Versioning" (`VERSION` first,
  then the copy in `Cargo.toml`)
- Add a `## vX.Y.Z` entry to `RELEASE_NOTES.md` for each release (the release
  pipeline fails a tag with no matching entry rather than publish an empty one)
- When starting work on each new branch, ask the user whether to bump the
  version number (and if so, to what value) before writing any code
- Version numbers follow `X.Y.Z`: `X` (major) is bumped only when the user
  explicitly asks for it; `Y` (minor) is bumped when a change adds a new
  feature; `Z` (patch) is bumped for bugfixes and other changes that don't
  add, remove, or change functionality
- Never commit changes unless the user explicitly asks to commit
- Pushing to the upstream remote (`origin`), including tags, is done by the
  user; prepare commits, merges and tags locally but never push

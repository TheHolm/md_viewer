# md_viewer

**!!! WORK IN PROGRESS !!!**

A simple GTK4/libadwaita program for viewing Markdown documents. It opens a
file, renders it read-only in a scrollable window, and never modifies it.

# How to use

```
md_viewer [FILE]
```

Without a file the window offers an "Open…" button. The Open button in the
header bar (or Ctrl+O) picks another document, which opens in a new window;
Ctrl+W closes a window. Installed from a package, MD Viewer is also in the
desktop's app grid and in "Open With" for Markdown files.

The document is shown with headings, emphasis, strong text, strikethrough,
inline code and code blocks, block quotes, ordered/unordered and task lists,
horizontal rules, and:

* **Tables** as real grids: shaded header, striped rows, column alignment,
  formatting inside cells, and long cells wrapped. Tables wider than the
  window scroll sideways.
* **Math** in LaTeX: `$…$` inline and `$$…$$` on its own line, as in KaTeX.
* **Chemistry** with mhchem inside math: `$\ce{2H2 + O2 -> 2H2O}$`,
  `$\pu{123 kJ/mol}$`.
* **Mermaid diagrams** in ` ```mermaid ` code blocks: flowcharts, sequence
  diagrams, Gantt charts, pie charts, class and state diagrams and the other
  Mermaid diagram types.

A formula or diagram that cannot be rendered is shown as its source, with
the reason. Images are shown as `[image: <alt text>]` placeholders, and raw
HTML is shown as plain text.

`showcase.md` at the top of the repository shows every feature in one
(slightly silly) document - an incident report about a coffee machine, with
tables, differential equations, chemistry, and flowchart, sequence, Gantt,
pie, class and state diagrams:

```
md_viewer showcase.md
```

(`tests/fixtures/showcase.md` is a terser fixture used by the test suite.)

Everything is rendered natively in Rust (math with
[mathra](https://crates.io/crates/mathra), diagrams with
[merman](https://crates.io/crates/merman), drawn with
[resvg](https://crates.io/crates/resvg)): no browser engine or JavaScript is
involved. The output is close to KaTeX and mermaid.js but not pixel
identical.

# Target platforms

Tagged releases ship ready-to-install amd64 packages on the GitHub Releases
page:

* Debian trixie `.deb`
* Ubuntu 26.04 `.deb`
* FreeBSD 15 `.pkg`

All three are built for every release; the FreeBSD package is cross-compiled
on Linux against FreeBSD 15 packages.

# Compiling

Needs Rust 1.98 or newer and the development files of GTK 4.18 and
libadwaita 1.7 or newer (the versions in Debian trixie, the oldest supported
platform). On Debian/Ubuntu:

```
sudo apt-get install pkg-config libgtk-4-dev libadwaita-1-dev
git clone git@github.com:TheHolm/md_viewer.git
cd md_viewer
cargo build --release
```

The compiled executable will be in `target/release/md_viewer`.

The packages also install a `.desktop` file and icons from `data/`; desktops
use them for the app's name and icon (GNOME shows a generic icon without
them). To get them for a binary built from source, install them for your
user:

```
install -Dm644 data/io.github.TheHolm.md_viewer.desktop -t ~/.local/share/applications
install -Dm644 data/icons/hicolor/scalable/apps/io.github.TheHolm.md_viewer.svg -t ~/.local/share/icons/hicolor/scalable/apps
install -Dm644 data/icons/hicolor/symbolic/apps/io.github.TheHolm.md_viewer-symbolic.svg -t ~/.local/share/icons/hicolor/symbolic/apps
```

The `.desktop` file runs `md_viewer`, so it must be on your `PATH`.

# Versioning

The app version lives in the `VERSION` file and nowhere else; `md_viewer
--version` prints it. `Cargo.toml` carries a copy because cargo needs one, and
`cargo test` fails if the two differ. Before tagging a release, check that
everything agrees:

```
bash scripts/check-version.sh v$(cat VERSION)
```

# Testing

The non-GUI logic (file reading and Markdown rendering) lives in a library
crate (`src/lib.rs`) so it can be tested without a display. Run the test
suite with:

```
cargo test
```

Line coverage is generated with [cargo-llvm-cov](https://github.com/taiki-e/cargo-llvm-cov):

```
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
cargo llvm-cov --tests
```

`tests/cli.rs` also starts the GUI when a display is available and skips
those tests otherwise. To cover the GUI on a headless machine, run the tests
under a virtual display:

```
Xvfb :77 &
DISPLAY=:77 GDK_BACKEND=x11 GSK_RENDERER=cairo cargo llvm-cov --tests
```

# TODO

Planned features. Known bugs are listed in `BUGS.md`.

* Clickable links.
* Inline images.
* Reload a document when it changes on disk.
* An error popup instead of a message on STDERR for a bad `FILE` argument.

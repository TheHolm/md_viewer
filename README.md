# md_viewer

**!!! WORK IN PROGRESS !!!**

A simple GTK4/libadwaita program for viewing Markdown documents. It opens a
file, renders it read-only in a scrollable window, and never modifies it.

# How to use

```
md_viewer <FILE>
```

The document is shown with headings, emphasis, strong text, strikethrough,
inline code and code blocks, block quotes, ordered/unordered and task lists,
horizontal rules and tables (laid out as aligned monospace text). Images are
shown as `[image: <alt text>]` placeholders, and raw HTML is shown as plain
text.

# Target platforms

Tagged releases ship ready-to-install amd64 packages on the GitHub Releases
page:

* Debian trixie `.deb`
* Ubuntu 26.04 `.deb`
* FreeBSD 15 `.pkg`

All three are built for every release; the FreeBSD package is cross-compiled
on Linux against FreeBSD 15 packages.

# Compiling

Needs Rust 1.98 or newer and the GTK4 and libadwaita development files. On
Debian/Ubuntu:

```
sudo apt-get install pkg-config libgtk-4-dev libadwaita-1-dev
git clone git@github.com:TheHolm/md_viewer.git
cd md_viewer
cargo build --release
```

The compiled executable will be in `target/release/md_viewer`.

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

The GUI in `src/main.rs` is a thin wrapper and is not covered by the tests.

# TODO

* Clickable links.
* Inline images.
* Open a file from the GUI and reload it when it changes on disk.
* Error popups instead of messages on STDERR.

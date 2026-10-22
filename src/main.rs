//! The GTK4/libadwaita front-end of `md_viewer`.
//!
//! Deliberately thin: it parses the command line, builds a window around a
//! read-only text view, and maps the library's styled spans onto text tags.
//! All Markdown handling lives in the library (`src/lib.rs`).

use adw::prelude::*;
use clap::Parser;
use gtk4::glib::translate::IntoGlib;
use gtk4::{gdk, gio, glib, pango};
use md_viewer::{Span, Style};
use std::path::PathBuf;

/// The application ID registered with GTK.
const APP_ID: &str = "io.github.TheHolm.md_viewer";

/// Command line arguments.
#[derive(Parser, Debug)]
#[command(version, about = "View a Markdown document")]
struct Args {
    /// The Markdown file to view.
    file: PathBuf,
}

/// Parses the command line and runs the application.
///
/// The arguments are parsed with clap before GTK starts, and GTK is then
/// given only the program name, so GApplication never tries to interpret
/// them itself.
fn main() -> glib::ExitCode {
    let args = Args::parse();
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| build_window(app, args.file.clone()));
    app.run_with_args(&[env!("CARGO_PKG_NAME")])
}

/// Builds and shows the main window, then loads `path` into it.
fn build_window(app: &adw::Application, path: PathBuf) {
    let view = gtk4::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(16)
        .right_margin(16)
        .build();
    let scrolled = gtk4::ScrolledWindow::builder()
        .vexpand(true)
        .child(&view)
        .build();
    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    content.append(&adw::HeaderBar::new());
    content.append(&scrolled);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(md_viewer::window_title(&path))
        .default_width(800)
        .default_height(900)
        .content(&content)
        .build();
    create_tags(&view.buffer());
    window.present();

    // Read and render on the I/O thread pool, then fill the buffer back on
    // the main thread.
    glib::spawn_future_local(async move {
        let worker_path = path.clone();
        let result = gio::spawn_blocking(move || md_viewer::open_document(&worker_path)).await;
        match result {
            Ok(Ok(spans)) => fill_buffer(&view.buffer(), &spans),
            Ok(Err(error)) => {
                eprintln!("{}", md_viewer::error_message(&path, &error));
                std::process::exit(1);
            }
            Err(_) => {
                eprintln!("Rendering {} failed unexpectedly", path.display());
                std::process::exit(1);
            }
        }
    });
}

/// Inserts `spans` at the end of `buffer`, tagging each with its styles.
fn fill_buffer(buffer: &gtk4::TextBuffer, spans: &[Span]) {
    for span in spans {
        let names: Vec<&str> = span.styles.iter().map(|style| style.tag_name()).collect();
        buffer.insert_with_tags_by_name(&mut buffer.end_iter(), &span.text, &names);
    }
}

/// Registers one text tag per [`Style`] in `buffer`'s tag table.
fn create_tags(buffer: &gtk4::TextBuffer) {
    let table = buffer.tag_table();
    for style in Style::ALL {
        table.add(&tag_for(style));
    }
}

/// Builds the text tag implementing `style`.
fn tag_for(style: Style) -> gtk4::TextTag {
    let muted = gdk::RGBA::new(0.5, 0.5, 0.5, 1.0);
    let code_background = gdk::RGBA::new(0.5, 0.5, 0.5, 0.15);
    let link = gdk::RGBA::new(0.21, 0.52, 0.89, 1.0);
    let builder = gtk4::TextTag::builder().name(style.tag_name());
    match style {
        Style::Heading(level) => {
            let scale = match level {
                1 => 2.0,
                2 => 1.6,
                3 => 1.3,
                4 => 1.15,
                5 => 1.0,
                _ => 0.9,
            };
            builder
                .weight(pango::Weight::Bold.into_glib())
                .scale(scale)
                .pixels_above_lines(6)
        }
        Style::Emphasis => builder.style(pango::Style::Italic),
        Style::Strong => builder.weight(pango::Weight::Bold.into_glib()),
        Style::Strikethrough => builder.strikethrough(true),
        Style::Code => builder
            .family("monospace")
            .background_rgba(&code_background),
        Style::CodeBlock => builder
            .family("monospace")
            .paragraph_background_rgba(&code_background)
            .wrap_mode(gtk4::WrapMode::None)
            .left_margin(28),
        Style::Quote => builder
            .style(pango::Style::Italic)
            .foreground_rgba(&muted)
            .left_margin(36),
        Style::Link => builder
            .underline(pango::Underline::Single)
            .foreground_rgba(&link),
        Style::Image => builder.style(pango::Style::Italic).foreground_rgba(&muted),
        Style::Table => builder.family("monospace").wrap_mode(gtk4::WrapMode::None),
        Style::Rule => builder.foreground_rgba(&muted),
    }
    .build()
}

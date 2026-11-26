# BUGS.md

Known bugs and limitations of `md_viewer`: things that are wrong or behave
badly. Add an entry when you find a problem you are not fixing right away;
remove it when it is fixed (and mention the fix in `RELEASE_NOTES.md`).
Planned features are not bugs: they go in the TODO list in `README.md`.
Background on the rendering crates is in `NOTES.md`.

## Bugs

- mathra and merman are young crates: output is close to, but not identical
  with, KaTeX and mermaid.js. Known differences: mathra has no automatic
  line breaking and no mhchem text mode; long Gantt date axes crowd, and
  label sizes depend on the installed fonts. Rendering bugs there are upstream issues.
- mathra silently accepts unknown LaTeX commands (they come out invisible,
  with no error), so a misspelled command is neither reported nor shown as
  the fallback source.
- Formulas are rendered once with the theme's text colour at load time;
  switching between light and dark while a document is open leaves them in
  the old colour.
- Table cells containing math or images do not wrap (they are a row of labels
  and pictures, not one wrapping label).
- A slow graphic holds back everything after it while a document loads
  (blocks are appended strictly in order, so nothing on the page moves);
  scrolling to the end of a loading document stops at what is there so far.
- After a zoom, re-rendered pictures can differ from the stretched ones by
  a pixel or so (rounding), so the page may shift very slightly while the
  sharp pictures arrive; the line at the top of the window is held in
  place for a moment, but not after the user scrolls.

- The memory budget is enforced on the GUI thread, after results arrive:
  the render workers do not reserve their bytes before allocating, so while
  rendering the document can briefly hold up to one graphic per worker
  over the budget (accepted as not a problem).
- An image file changed on disk shows the change only when the document is
  reopened or zoomed (nothing watches the file), and a change to a file an
  SVG image refers to (`<image href>`) is not noticed at all until restart.
- Regex mode's `(?i)` uses the regex crate's simple case folding (`ß`
  does not match `ss` there, unlike in the other modes).

## Limitations

- Heading anchors are matched in two spellings: ours (runs of whitespace
  collapse to one hyphen, a trailing hyphen is dropped; GitLab's rule is
  close to it) and GitHub's (`Install - Linux` is `install---linux`).
  Combining marks are dropped in both, so a heading in a script that uses
  them (Devanagari vowel signs, for instance) has a different anchor than
  on GitHub.

- Lists (and, for code cards, block quotes) are indented at most
  `MAX_LIST_INDENT` (4) levels; deeper items line up at the fourth level.

- merman's diagram engines make the binary much larger (about 26.5 MB
  stripped in 0.4.0, 24.7 MB in 0.3.0, about 1.3 MB for 0.1.0).
- A text selection cannot run from the text into a code card or out of it:
  each card is its own widget.
- The `.desktop` file and icons are only installed by the packages; a binary
  built from source shows a generic icon under GNOME unless they are copied
  into `~/.local/share` (see the README).
- Search does not cover the source of a rendered formula (it is drawn as
  pixels); only the fallback source shown for a formula that could not be
  rendered is searchable.
- Network images (`http`/`https`) are not supported, and that is not
  planned: the viewer deliberately loads nothing over the network. Such an
  image shows a placeholder saying so.
- An animated GIF shows only its first frame.
- Nothing is drawn larger than ten screens. A graphic too large even at 100%
  is refused ("too large" in its place); a very large one lowers the
  document's maximum zoom, so the whole page stops zooming in where that
  graphic reaches the limit. At a ceiling of exactly 100% the zoom indicator
  is hidden (as at every 100%), so nothing says why zooming in does nothing.
- A standalone raster image keeps the file's full resolution even though
  it is shown at the window width.
- A match inside a diagram highlights the whole diagram card, not the exact
  label characters. Likewise a match on a standalone or table-cell image's
  alt text highlights the whole picture, and a match on an inline image's alt
  text only counts and marks the scrollbar (an inline picture is a paintable,
  not a widget, so there is nothing to highlight).
- A search started while a document is still loading covers the whole
  document's text, but the sources of rendered formulas and diagram labels
  only become searchable once they are drawn, so a search for those can gain
  results as blocks are appended; the find bar shows "…" meanwhile.
- A match in a block not yet rendered has no position on the page: it is
  counted and can be navigated to (which renders up to its block and scrolls
  there), but it is not highlighted or marked on the scrollbar until the
  block is drawn. Stepping through a match that was just rendered may not
  keep it selected.
- A formula that is a link's only content is not clickable (it is drawn as
  pixels, so there is no text to hit); the link still has the link style.

## Testing gaps

- The link confirmation dialog's "Open Anyway" (which hands the file to
  its default application) is not exercised by the tests, to launch
  nothing; the tests check that a dangerous file shows the dialog with
  Cancel as the default, and that nothing else is launched.
- Moving a window to another monitor (the surface's `notify::scale` and
  `enter-monitor` signals) is not tested: headless X has one monitor and
  one scale. The GUI unit tests call `Session::rescale` directly (whole and
  fractional scales, a lower and a higher limit) and check following the
  headless monitor once.
- A real window resize is not tested (headless X without a window manager
  does not resize windows); the tests check that the width signal wakes the
  frame callback.
- The file chooser's response and the error dialog need user interaction,
  so the tests do not reach them; check them by hand after changing them.
- Zooming (Ctrl+wheel, Ctrl+plus/minus/0, pinch) and the code cards' copy
  button need input events, so the GUI tests do not reach them; the
  library logic behind them is tested, and the GUI is checked by hand (or
  with `xdotool` on Xvfb, see `NOTES.md`).
- The find bar (Ctrl+F, `/`, highlighting, marks and the mode popover)
  needs input events too, so the GUI tests only confirm the viewer starts
  and exits cleanly with it installed; the matching logic is fully tested
  and the interaction is checked by hand.
- Clicking links (the view's click gesture, the pointer cursor, the hover
  tooltip, table-cell link hit-testing, `GtkUriLauncher` and
  cross-file/anchor scrolling) needs input events and a running desktop
  session, so the GUI tests do not reach it; the resolution and range logic
  is fully tested, and the interaction is checked by hand (or with `xdotool`
  on Xvfb, see `NOTES.md`). That only a single click opens a link (not the
  second click of a double click) is tested through `layout::opens_link`,
  not with real clicks. The GUI unit tests do open a real document
  session and check that every heading is registered at its own buffer
  offset and that a clicked fragment is resolved and found, but not the
  scrolling itself.
- The zoom ceiling is tested end to end in the GUI unit tests (a picture
  lowering it zooms the window out, the indicator shows "(max)", zooming in
  is refused), but the indicator's click and keyboard/wheel zooming against
  the ceiling are checked by hand.
- How a loaded image is placed (standalone at the window width, inline at
  the line height, in a table cell, and its place in a zoom re-render) needs
  a display and input events, as do clicking a linked image and the
  alt-text search highlight on pictures, so the GUI tests only confirm the
  viewer starts and exits cleanly with images in the document (including a
  table cell); the decoding, sizing arithmetic, resolution, link and
  searchable-alt logic are tested, and the interaction is checked by hand.
  The pure/layout helpers around pictures are unit-tested in `main.rs`
  (text tags, code cards, `ScaledTexture::scale_size`, label link
  hit-testing), but the placement itself (`insert_image`,
  `make_picture_clickable`, `rich_cell_widget`) is only smoke-tested: the
  viewer opens a document with images and exits cleanly, without the
  placement being asserted.

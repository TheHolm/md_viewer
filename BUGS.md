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
- Table cells containing math do not wrap (they are a row of labels and
  pictures, not one wrapping label).
- A slow graphic holds back everything after it while a document loads
  (blocks are appended strictly in order, so nothing on the page moves);
  scrolling to the end of a loading document stops at what is there so far.
- After a zoom, re-rendered pictures can differ from the stretched ones by
  a pixel or so (rounding), so the page may shift very slightly while the
  sharp pictures arrive; the line at the top of the window is held in
  place for a moment, but not after the user scrolls.

## Limitations

- merman's diagram engines make the binary much larger (about 26.5 MB
  stripped in 0.4.0, 24.7 MB in 0.3.0, about 1.3 MB for 0.1.0).
- A text selection cannot run from the text into a code card or out of it:
  each card is its own widget.
- The `.desktop` file and icons are only installed by the packages; a binary
  built from source shows a generic icon under GNOME unless they are copied
  into `~/.local/share` (see the README).

## Testing gaps

- The file chooser's response and the error dialog need user interaction,
  so the tests do not reach them; check them by hand after changing them.
- Zooming (Ctrl+wheel, Ctrl+plus/minus/0, pinch) and the code cards' copy
  button need input events, so the GUI tests do not reach them; the
  library logic behind them is tested, and the GUI is checked by hand (or
  with `xdotool` on Xvfb, see `NOTES.md`).

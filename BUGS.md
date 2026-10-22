# BUGS.md

Known bugs and limitations of `md_viewer`: things that are wrong or behave
badly. Add an entry when you find a problem you are not fixing right away;
remove it when it is fixed (and mention the fix in `RELEASE_NOTES.md`).
Planned features are not bugs: they go in the TODO list in `README.md`. Background on the rendering crates
is in `NOTES.md`.

## Bugs

- Class and state diagram labels are clipped at their edges (e.g.
  "Appliance" drawn as "ppliand", the edge label "button pressed" as
  "utton presse"; visible in the root `showcase.md`). merman sizes each
  label box with its own text metrics, and the label is a nested `<svg>`
  with `overflow:hidden`; resvg draws the text with the system's
  `sans-serif` font, which can be wider. Seen with DejaVu Sans as the only
  installed font; not yet checked with Cantarell or Noto Sans. Upstream
  (merman) issue, possibly worked around by measuring or choosing fonts.
- mathra and merman are young crates: output is close to, but not identical
  with, KaTeX and mermaid.js. Known differences: mathra has no automatic
  line breaking and no mhchem text mode; merman sizes text with its own
  metrics, so e.g. edge label backgrounds can be slightly off and long Gantt
  date axes crowd. Rendering bugs there are upstream issues.
- mathra silently accepts unknown LaTeX commands (they come out invisible,
  with no error), so a misspelled command is neither reported nor shown as
  the fallback source.
- Formulas are rendered once with the theme's text colour at load time;
  switching between light and dark while a document is open leaves them in
  the old colour.
- Table cells containing math do not wrap (they are a row of labels and
  pictures, not one wrapping label).

## Limitations

- merman's diagram engines make the binary much larger (about 25 MB
  stripped, against about 1.3 MB for 0.1.0).
- The `.desktop` file and icons are only installed by the packages; a binary
  built from source shows a generic icon under GNOME unless they are copied
  into `~/.local/share` (see the README).

## Testing gaps

- The file chooser's response and the error dialog need user interaction,
  so the tests do not reach them; check them by hand after changing them.

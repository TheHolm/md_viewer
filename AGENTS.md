# AGENTS.md

Rules for coding agents working on this repository. The project's facts
(overview, stack, layout, rendering model, versioning, commands) and
hard-won knowledge are in `NOTES.md`; known bugs and gaps are in `BUGS.md`.
Read both before starting work, and keep them current: record new
persistent facts and lessons in `NOTES.md`, and new or fixed bugs in
`BUGS.md`. Session-specific or transient details belong in neither.

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
- Change the version only as described under "Versioning" in `NOTES.md`
  (`VERSION` first, then the copy in `Cargo.toml`)
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
- Before finishing a change, run `cargo fmt --check` and `cargo test`, with
  a display (see "Gotchas and techniques" in `NOTES.md` for a headless one)
  so the GUI tests run instead of being skipped

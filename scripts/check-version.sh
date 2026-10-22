#!/usr/bin/env bash
# Checks that the VERSION file - the single source of the app version - is
# well formed and that everything that has to repeat it agrees with it:
#
#   - VERSION holds exactly one `X.Y.Z` line;
#   - the `[package]` version in Cargo.toml (a copy cargo needs) matches it;
#   - if a tag is given, the tag is exactly `v<VERSION>`.
#
# Run it before tagging a release; the release pipeline runs it first and
# publishes nothing if it fails.
#
# Usage: check-version.sh [tag]
# Run from the repository root. Exits 1 with a message on any mismatch.
set -euo pipefail

if [[ $# -gt 1 ]]; then
    echo "usage: $(basename "$0") [tag]" >&2
    exit 1
fi

if [[ ! -f VERSION ]]; then
    echo "error: VERSION not found (run from the repository root)" >&2
    exit 1
fi

if [[ "$(wc -l < VERSION)" -ne 1 ]]; then
    echo "error: VERSION must contain exactly one line" >&2
    exit 1
fi
version="$(< VERSION)"

if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: VERSION \"${version}\" is not in X.Y.Z form" >&2
    exit 1
fi

# The first `version = "..."` line inside the [package] table.
cargo_version="$(awk '
    /^\[/ { in_package = ($0 == "[package]") }
    in_package && /^version[[:space:]]*=/ {
        sub(/^version[[:space:]]*=[[:space:]]*"/, ""); sub(/".*/, ""); print; exit
    }
' Cargo.toml)"

if [[ "$cargo_version" != "$version" ]]; then
    echo "error: Cargo.toml version \"${cargo_version}\" differs from VERSION \"${version}\"" >&2
    exit 1
fi

if [[ $# -eq 1 && "$1" != "v${version}" ]]; then
    echo "error: tag \"$1\" does not match VERSION (expected \"v${version}\")" >&2
    exit 1
fi

echo "version ${version} OK"

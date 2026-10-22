#!/usr/bin/env python3
"""Populate a FreeBSD cross-compilation sysroot with GTK4 and libadwaita.

Cross-linking `md_viewer` for FreeBSD needs the native libraries the binary
links against (GTK4, libadwaita and their transitive dependencies), which a
`base.txz` sysroot does not contain. This helper downloads those packages from
the official FreeBSD package repository and unpacks them into the sysroot. It
needs no FreeBSD host and never runs a FreeBSD binary - it only downloads and
extracts archives.

The repository uses the pkg 2.x layout: `packagesite.pkg` is a zstd tar holding
`packagesite.yaml` (one JSON manifest per line, each with a `path` under
`All/Hashed/`), and that `path` is directly downloadable as a normal `.pkg`.
This layout is not a stable public interface. The FreeBSD package is a required
release artifact, so if the layout changes this helper fails, the Woodpecker
release fails with it, and the helper has to be updated before releasing. See TheHolm/dak's NOTES.md section 2 for the `.pkg`
format itself.

Usage:
    fetch-freebsd-gtk.py --sysroot /opt/freebsd-sysroot \
        --deps-output /opt/freebsd-deps.json

    # Offline / tested against a pre-downloaded manifest:
    fetch-freebsd-gtk.py --sysroot ./root --manifest ./packagesite.yaml \
        --root gtk4 --root libadwaita

Requires `python3`, GNU `tar` built with zstd support, and `zstd` on PATH.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request

DEFAULT_REPO = "https://pkg.freebsd.org/FreeBSD%3A15%3Aamd64/latest"
DEFAULT_ROOTS = ["gtk4", "libadwaita"]
# FreeBSD package name -> pkg-config module name, where they differ.
PKG_CONFIG_MODULE = {"gtk4": "gtk4", "libadwaita": "libadwaita-1"}

MISSING_QUOTED = re.compile(r"Package '([^']+)', required by '[^']+', not found")
MISSING_BARE = re.compile(r"Package ([^ ]+) was not found")
STUB_MARKER = "auto-generated stub for a build-only dependency"


def download(url: str, dest: str) -> None:
    """Download `url` to `dest` (streamed), failing on any HTTP error."""
    request = urllib.request.Request(url, headers={"User-Agent": "md_viewer-ci"})
    with urllib.request.urlopen(request) as response, open(dest, "wb") as out:
        shutil.copyfileobj(response, out)


def package_url(repo: str, path: str) -> str:
    """Return the download URL for a manifest `path` under `repo`."""
    return repo.rstrip("/") + "/" + urllib.parse.quote(path, safe="/~$")


def load_manifests(manifest_path: str) -> dict:
    """Parse a `packagesite.yaml` into a name -> manifest dict."""
    packages = {}
    with open(manifest_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            entry = json.loads(line)
            packages[entry["name"]] = entry
    return packages


def resolve_closure(packages: dict, roots: list) -> list:
    """Return the dependency closure of `roots`, in discovery order.

    Raises if a root is missing from the repository; a missing transitive
    dependency is reported on stderr and skipped, so one renamed package does
    not take the whole sysroot down.
    """
    seen = set()
    ordered = []
    stack = list(roots)
    while stack:
        name = stack.pop()
        if name in seen:
            continue
        seen.add(name)
        entry = packages.get(name)
        if entry is None:
            if name in roots:
                raise SystemExit(f"error: package {name!r} not found in repository")
            print(f"warning: dependency {name!r} not in repository, skipping", file=sys.stderr)
            continue
        ordered.append(name)
        for dep in entry.get("deps", {}):
            if dep not in seen:
                stack.append(dep)
    return ordered


def extract(pkg_file: str, sysroot: str) -> None:
    """Unpack a `.pkg` (zstd tar) into `sysroot`, skipping the metadata members.

    Package members are absolute paths like `/usr/local/lib/...`; GNU tar strips
    the leading `/` and extracts them relative to `--directory`, which is what
    lays them out correctly under the sysroot.
    """
    subprocess.run(
        [
            "tar", "--zstd", "-xf", pkg_file, "-C", sysroot,
            "--no-same-owner", "--exclude=+*",
        ],
        check=True,
    )


def pkg_config_dirs(sysroot: str) -> list:
    """Return the sysroot's pkg-config directories, creating them if needed."""
    dirs = []
    for sub in ("usr/local/libdata/pkgconfig", "usr/local/lib/pkgconfig"):
        path = os.path.join(sysroot, sub)
        os.makedirs(path, exist_ok=True)
        dirs.append(path)
    return dirs


def ensure_pkg_config_resolvable(sysroot: str, modules: list) -> int:
    """Make `pkg-config` resolve `modules` against the sysroot alone.

    A `.pc` file's `Requires.private` lists build-only dependencies that are not
    in a package's runtime dependency closure, so they are absent from the
    sysroot even though `pkg-config --cflags` insists on resolving them (e.g.
    `libcurl`, reached via libadwaita -> AppStream, wants `mit-krb5-gssapi`).
    Rather than chase every such package, write a minimal stub `.pc` for each
    one that is missing and retry until `pkg-config` succeeds. Only the real
    link libraries matter, and those come from the actual closure.

    Returns the number of stubs written. `PKG_CONFIG_LIBDIR` is pinned to the
    sysroot so host `.pc` files are never consulted.
    """
    dirs = pkg_config_dirs(sysroot)
    env = dict(os.environ)
    env["PKG_CONFIG_ALLOW_CROSS"] = "1"
    env["PKG_CONFIG_SYSROOT_DIR"] = sysroot
    env["PKG_CONFIG_LIBDIR"] = os.pathsep.join(dirs)
    env.pop("PKG_CONFIG_PATH", None)

    # Drop any stubs a previous run left behind, so this stays idempotent.
    for directory in dirs:
        for name in os.listdir(directory):
            if not name.endswith(".pc"):
                continue
            path = os.path.join(directory, name)
            with open(path, encoding="utf-8", errors="ignore") as f:
                if STUB_MARKER in f.read():
                    os.remove(path)

    stubs = 0
    for _ in range(200):
        missing = set()
        for module in modules:
            result = subprocess.run(
                ["pkg-config", "--cflags", "--libs", module],
                env=env, capture_output=True, text=True,
            )
            if result.returncode == 0:
                continue
            for pattern in (MISSING_QUOTED, MISSING_BARE):
                missing.update(pattern.findall(result.stderr))
        if not missing:
            return stubs
        for name in missing:
            with open(os.path.join(dirs[0], name + ".pc"), "w", encoding="utf-8") as f:
                f.write(f"Name: {name}\n")
                f.write(f"Description: {STUB_MARKER}\n")
                # A high version so any `>=` requirement on a build-only package
                # (e.g. `renderproto >= 0.9`) is satisfied; nothing here is
                # actually linked, so the value only has to pass comparison.
                f.write("Version: 9999\n")
            stubs += 1
    raise SystemExit("error: pkg-config dependency resolution did not converge")


def parse_args(argv: list) -> argparse.Namespace:
    """Parse command-line arguments."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sysroot", required=True, help="sysroot to unpack into")
    parser.add_argument("--repo", default=DEFAULT_REPO, help="pkg repository base URL")
    parser.add_argument(
        "--manifest",
        help="use a local packagesite.yaml instead of downloading one (testing)",
    )
    parser.add_argument(
        "--root",
        action="append",
        default=None,
        help="package whose closure to fetch (repeatable; default: gtk4, libadwaita)",
    )
    parser.add_argument(
        "--deps-output",
        help="write the roots' {origin, version} deps JSON here (for the .pkg manifest)",
    )
    return parser.parse_args(argv)


def main(argv: list) -> int:
    """Entry point: fetch the closure and unpack it into `--sysroot`."""
    args = parse_args(argv)
    roots = args.root if args.root else DEFAULT_ROOTS
    os.makedirs(args.sysroot, exist_ok=True)

    with tempfile.TemporaryDirectory() as work:
        if args.manifest:
            manifest_path = args.manifest
        else:
            manifest_path = os.path.join(work, "packagesite.yaml")
            print(f"fetching {args.repo}/packagesite.pkg", flush=True)
            download(args.repo.rstrip("/") + "/packagesite.pkg", os.path.join(work, "ps.pkg"))
            subprocess.run(
                ["tar", "--zstd", "-xf", os.path.join(work, "ps.pkg"),
                 "-C", work, "packagesite.yaml"],
                check=True,
            )

        packages = load_manifests(manifest_path)
        names = resolve_closure(packages, roots)

        total = 0
        for index, name in enumerate(names, start=1):
            entry = packages[name]
            pkg_file = os.path.join(work, "pkg.pkg")
            print(f"[{index}/{len(names)}] {name} {entry['version']}", flush=True)
            download(package_url(args.repo, entry["path"]), pkg_file)
            total += os.path.getsize(pkg_file)
            extract(pkg_file, args.sysroot)
            os.remove(pkg_file)

        if args.deps_output:
            deps = {
                name: {"origin": packages[name]["origin"], "version": packages[name]["version"]}
                for name in roots
                if name in packages
            }
            with open(args.deps_output, "w", encoding="utf-8") as f:
                json.dump(deps, f, indent=2)
                f.write("\n")

        stubs = ensure_pkg_config_resolvable(
            args.sysroot, [PKG_CONFIG_MODULE.get(root, root) for root in roots]
        )

    print(
        f"unpacked {len(names)} packages ({total / 1e6:.1f} MB) into {args.sysroot} "
        f"({stubs} pkg-config stubs)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

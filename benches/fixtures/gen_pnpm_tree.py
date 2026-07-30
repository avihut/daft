#!/usr/bin/env python3
"""Generate a pnpm-shaped node_modules tree, deterministically.

The copy benchmark's fixture (#801): reproduces the structural properties
that make a pnpm virtual store the copier's adversarial case, without the
network. Structure, not bytes, is what the copier's cost tracks, so the
proportions matter more than the sizes:

  * content lives in a "store" OUTSIDE the tree; every regular file in
    node_modules is a HARDLINK into it (nlink >= 2, like a real pnpm install)
  * the virtual store ``.pnpm/<pkg>@<ver>/node_modules/<pkg>`` holds the
    files, with one SYMLINK per dependency edge beside each consumer
  * one symlink per direct dependency at the top level, plus ``.bin`` shims
  * package sizes follow a long tail (a few typescript-sized monsters, many
    4-15-file leaves, ~12 files per directory)

Deterministic: seeded PRNG, no timestamps in content. Same arguments, same
tree. A first-generation piece of the fixture framework #805 proposes.

Usage: gen_pnpm_tree.py <dest-dir> [--scale small|medium|large] [--packages N] [--seed N]

  <dest-dir>/store         the fake global store (hardlink source)
  <dest-dir>/node_modules  the tree to point the copier at
"""

import argparse
import os
import random
import sys

SCALES = {
    # packages, direct deps, edge-density, monster packages.
    # max_edges targets real pnpm virtual-store density: one symlink per
    # dependency edge, avg ~8 deps/package in real lockfiles.
    "small": dict(packages=150, direct=25, max_edges=8, monsters=1),
    "medium": dict(packages=600, direct=60, max_edges=16, monsters=3),
    "large": dict(packages=1400, direct=90, max_edges=20, monsters=6),
}


def file_count_for(rng, is_monster):
    """Long-tail: most packages are small, a few are typescript-sized."""
    if is_monster:
        return rng.randint(2500, 4500)
    r = rng.random()
    if r < 0.55:
        return rng.randint(4, 15)  # leaf utility
    if r < 0.85:
        return rng.randint(16, 60)  # typical lib
    if r < 0.97:
        return rng.randint(61, 300)  # framework-ish
    return rng.randint(301, 900)  # lodash-ish


def content_for(rng, i):
    """Small JS-ish payloads; a few larger blobs (sourcemaps, wasm)."""
    r = rng.random()
    if r < 0.80:
        size = rng.randint(200, 4000)
    elif r < 0.97:
        size = rng.randint(4001, 60_000)
    else:
        size = rng.randint(60_001, 800_000)
    seed_line = f"// blob {i} {rng.randint(0, 1 << 30)}\n".encode()
    return seed_line + b"x" * (size - len(seed_line))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dest")
    ap.add_argument("--scale", choices=SCALES, default="small")
    ap.add_argument("--packages", type=int, help="override package count")
    ap.add_argument("--seed", type=int, default=801)
    args = ap.parse_args()

    cfg = dict(SCALES[args.scale])
    if args.packages:
        cfg["packages"] = args.packages
    rng = random.Random(args.seed)

    dest = os.path.abspath(args.dest)
    store = os.path.join(dest, "store")
    nm = os.path.join(dest, "node_modules")
    if os.path.exists(nm) or os.path.exists(store):
        sys.exit(f"refusing: {dest} already holds store/ or node_modules/")
    os.makedirs(store)
    pnpm_dir = os.path.join(nm, ".pnpm")
    os.makedirs(pnpm_dir)

    n_pkgs = cfg["packages"]
    scopes = ["", "", "", "@babel/", "@types/", "@acme/"]
    names = [f"{rng.choice(scopes)}pkg-{i}" for i in range(n_pkgs)]

    store_blob = 0

    def put_file(path, content):
        # store is content-addressed-ish: one blob per file, hardlinked out
        nonlocal store_blob
        blob = os.path.join(store, f"{store_blob:07d}")
        store_blob += 1
        with open(blob, "wb") as f:
            f.write(content)
        os.link(blob, path)

    monsters = set(rng.sample(range(n_pkgs), cfg["monsters"]))
    pkg_roots = {}
    for i, name in enumerate(names):
        ver = f"{rng.randint(0, 9)}.{rng.randint(0, 20)}.{rng.randint(0, 9)}"
        safe = name.replace("/", "+")
        vdir = os.path.join(pnpm_dir, f"{safe}@{ver}", "node_modules")
        root = os.path.join(vdir, name)
        pkg_roots[i] = (vdir, root)

        files = file_count_for(rng, i in monsters)
        # spread files over a dir tree, ~12 files per dir like real packages
        subdirs = [""]
        for d in range(max(1, files // 12)):
            subdirs.append(os.path.join(rng.choice(subdirs), f"d{d}"))
        for sub in subdirs:
            os.makedirs(os.path.join(root, sub), exist_ok=True)
        for fidx in range(files):
            put_file(
                os.path.join(root, rng.choice(subdirs), f"f{fidx}.js"),
                content_for(rng, fidx),
            )
        put_file(
            os.path.join(root, "package.json"),
            f'{{"name":"{name}","version":"{ver}"}}\n'.encode(),
        )

    # dependency edges: symlinks inside the virtual store
    for i in range(n_pkgs):
        vdir, _root = pkg_roots[i]
        for d in rng.sample(range(n_pkgs), rng.randint(0, cfg["max_edges"])):
            if d == i:
                continue
            dep_name = names[d]
            _dvdir, droot = pkg_roots[d]
            link = os.path.join(vdir, dep_name)
            if os.path.lexists(link):
                continue
            if "/" in dep_name:
                os.makedirs(os.path.dirname(link), exist_ok=True)
            os.symlink(os.path.relpath(droot, os.path.dirname(link)), link)

    # direct dependencies: top-level symlinks into the virtual store
    direct = rng.sample(range(n_pkgs), cfg["direct"])
    for d in direct:
        dep_name = names[d]
        _vdir, droot = pkg_roots[d]
        link = os.path.join(nm, dep_name)
        if os.path.lexists(link):
            continue
        if "/" in dep_name:
            os.makedirs(os.path.dirname(link), exist_ok=True)
        os.symlink(os.path.relpath(droot, os.path.dirname(link)), link)

    # .bin shims
    bin_dir = os.path.join(nm, ".bin")
    os.makedirs(bin_dir)
    for d in direct[: max(5, len(direct) // 3)]:
        _vdir, droot = pkg_roots[d]
        target = os.path.join(droot, "package.json")  # any real file
        os.symlink(os.path.relpath(target, bin_dir), os.path.join(bin_dir, f"tool-{d}"))

    with open(os.path.join(nm, ".modules.yaml"), "w") as f:
        f.write("hoistPattern: []\nvirtualStoreDir: .pnpm\n")

    # report real totals, counted from disk
    total_dirs = total_files = total_links = 0
    total_bytes = 0
    for root, dirs, files in os.walk(nm):
        real_dirs = []
        for d in dirs:
            p = os.path.join(root, d)
            if os.path.islink(p):
                total_links += 1
            else:
                real_dirs.append(d)
                total_dirs += 1
        dirs[:] = real_dirs
        for fl in files:
            p = os.path.join(root, fl)
            if os.path.islink(p):
                total_links += 1
            else:
                total_files += 1
                total_bytes += os.lstat(p).st_size
    print(
        f"generated {nm}: dirs={total_dirs} files={total_files} "
        f"symlinks={total_links} total={total_dirs + total_files + total_links} "
        f"bytes={total_bytes / 1e6:.0f}MB (every file hardlinked from store/)"
    )


if __name__ == "__main__":
    main()

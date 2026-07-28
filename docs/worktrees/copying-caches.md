---
title: Copying build caches into new worktrees
description:
  Declare gitignored build caches with the copy key in daft.yml so every new
  worktree starts warm — copy-on-write where the filesystem supports it, honest
  expectations per toolchain.
---

# Copying build caches into new worktrees

Full isolation has a bill, and it arrives the moment a worktree is created: no
`node_modules/`, no `target/`, no `.venv/`. The first thing you do in a
brand-new worktree is wait for a build you have already run, in a directory two
levels up, against nearly the same dependency graph.

`copy:` in `daft.yml` is the declaration that removes that wait. It names the
gitignored paths daft should replicate from the source worktree into each new
one — and on a copy-on-write filesystem, replicating them is nearly free.

```yaml
copy:
  - target/
  - node_modules/
```

That is the whole configuration. `daft start feature-x` now creates the
worktree, copies both directories in, and only then runs the
`worktree-post-create` hooks — so the `npm install` in your hooks reconciles a
warm tree instead of building one from nothing.

**The source worktree is the one you ran the command from.** Create a worktree
while standing in `feature-a` and its caches are what gets copied; only when you
run from the bare container root — outside any worktree — does daft fall back to
a resident worktree, preferring the base branch's if you named one and the
default branch's otherwise. Two consequences worth internalizing: you inherit
the warmth of wherever you were standing, and the `copy:` configuration is read
from that same source worktree — so if its merged config has no `copy:` key,
nothing is planned, whatever the branch you are creating would have said.

## The four ways content reaches a new worktree

`copy:` is the last piece of a set. Each mechanism moves a different kind of
content, and the differences are the point:

| Mechanism                                             | What moves                                      | What the new worktree gets     |
| ----------------------------------------------------- | ----------------------------------------------- | ------------------------------ |
| **Carry** (`daft carry`, `daft.checkoutBranch.carry`) | Uncommitted tracked changes                     | The same edits, relocated      |
| **Visitor propagation**                               | Untracked daft config (`daft.yml` and siblings) | A private copy daft tracks     |
| **`shared:`** (`daft shared`)                         | Config that must be identical everywhere        | A symlink to one central file  |
| **`copy:`**                                           | Gitignored build caches                         | An independent private replica |

The distinction that matters is the last two. `shared:` gives every worktree
_the same file_; `copy:` gives every worktree _its own_. That is not a style
preference — a build cache must not be shared. Two worktrees compiling into one
`target/` or installing into one `node_modules/` corrupt each other's artifacts,
which is the failure mode
[shared mutable state](/recipes/anti-patterns/shared-mutable-state) documents.

`copy:` is also distinct from sharing a _global_ cache. pnpm's store, cargo's
registry, and the Go module cache are content-addressed and safe to share across
every worktree on the machine; see
[Sharing caches across worktrees](/recipes/sharing-caches) for the per-tool
answers. `copy:` handles the other column of that page — the per-worktree
directories that page tells you never to share.

## What it costs

On a filesystem with copy-on-write support, a copy is a metadata operation: the
two directories point at the same blocks until one of them is written to.

| Filesystem              | Copy-on-write |
| ----------------------- | ------------- |
| APFS (macOS)            | Yes           |
| btrfs                   | Yes           |
| XFS mounted `reflink=1` | Yes           |
| OpenZFS 2.2+            | Yes           |
| bcachefs                | Yes           |
| ReFS (Windows)          | Yes           |
| ext4, HFS+, NTFS, tmpfs | No            |

daft does not guess from the filesystem type — it attempts a reflink and uses
the answer, because a single `copy:` entry can straddle mount points. Where the
attempt fails, `fallback:` decides what happens next:

```yaml
copy:
  paths: [target/, node_modules/]
  fallback: copy # copy | skip — default is copy
  max_size: 5GB # per-entry cap, byte-copy fallback only
```

- `fallback: copy` (default) pays for a real byte copy. A warm cache is worth
  the bytes on most trees.
- `fallback: skip` leaves the entry out and reports a yellow skip. Choose it for
  trees where a non-CoW copy would cost more than the rebuild it saves.
- `max_size` caps that byte copy per entry. It never applies to a reflink, which
  is near-free by construction — so the same config is generous on APFS and
  cautious on ext4 without a second spelling.

The full schema is in the
[`daft.yml` reference](/hooks/yaml-reference#copied-paths).

## What actually stays warm

A copied cache is a head start, not a guarantee. Toolchains vary in how much
absolute-path knowledge they bake into their artifacts, and that is what decides
how much survives the move. Run your normal install or build afterwards — it
will simply do far less work.

### cargo — `target/`

Expect the dependency half to survive and your own crates to rebuild. Registry
dependencies are compiled once per feature set and toolchain, and that is the
bulk of a cold `target/`; they come across warm. Workspace-local crates do not:
rustc embeds absolute paths in debug info and metadata, and cargo's fingerprints
for path dependencies are tied to where those crates live. Moving `target/` to a
new directory invalidates them.

This is still the largest single win in a Rust repo — the dependency graph is
usually minutes and your own crates are usually seconds.

### JavaScript — `node_modules/`

**pnpm is the cheapest case, on Unix.** The top layer of a pnpm `node_modules/`
is symlinks into `node_modules/.pnpm/`, and on Unix symlinks copy as symlinks —
the link is recreated, the target is not walked. The bytes live one layer down
in `.pnpm/`, hardlinked from pnpm's global store, and those arrive as real files
(reflinked where the filesystem allows).

On Windows the calculus is different: daft does not yet replicate symlinks
there, so an entry containing one fails and is reported as a yellow attention
row. Whatever had already landed at the destination is then cleared away, which
means the next creation or `daft warm` retries the entry cleanly rather than
finding a half-copy and reporting `already present` over it forever. ReFS gives
Windows block cloning for ordinary files, but a symlink-based `node_modules/` is
not currently a copyable entry — declare the flat-install directories instead.

**npm and yarn's flat trees** are real files all the way down. This is precisely
the case reflink exists for: near-free on APFS or btrfs, a genuine
several-hundred-megabyte copy without it. If your team is split across both
kinds of machine, `max_size` is how you express "warm where it's cheap, cold
where it isn't."

Native modules are the caveat in both cases. Anything compiled by `node-gyp`
against a specific path or ABI may need rebuilding; your post-create
`pnpm install` / `npm install` will notice and fix it.

### Python — `.venv/`

Do not copy a virtualenv unless you have made it relocatable first. A `.venv/`
records its own absolute location in `pyvenv.cfg`, in the shebang of every
script in `bin/`, and in `bin/activate` — copy it to a new path and it keeps
pointing at the worktree it came from.

Two better options, in order:

1. **Don't copy it.** Share the _package_ cache instead (`~/.cache/uv`,
   `~/.cache/pip` — both safe to share and shared by default) and let `uv sync`
   rebuild the venv in the new worktree. With a warm uv cache that is usually a
   second or two.
2. **Make it relocatable.** `uv venv --relocatable` creates a venv whose
   entrypoint and activation scripts use relative paths, which survives being
   copied. Only then is `.venv/` a reasonable `copy:` entry.

### Anything else

The question to ask of a cache is: _does it record its own absolute path?_ If
yes, expect partial reuse. If no — Vite's `node_modules/.vite/`, webpack 5's
`node_modules/.cache/`, most output directories — expect it to work as-is.

One thing no cache carries across: **special files are silently omitted.** Unix
sockets, FIFOs, and device nodes are not reproduced — regular files,
directories, and (on Unix) symlinks are. Daemons that leave a socket behind in
`node_modules/.cache/` or `.venv/` therefore arrive without it, which is what
you want: a stale socket pointing at a process that was never in this worktree
is worse than an absent one, and the tool recreates it on next start.

## Entries must be gitignored

`copy:` replicates caches, not the working tree. Every entry is checked with
`git check-ignore`, and daft additionally verifies that nothing underneath it is
tracked — which catches the force-added file inside an otherwise-ignored
directory.

An entry that fails either check gets a yellow row on the creation rail
(`must be gitignored — tracked content is never copied`) and is skipped. The
worktree is still created. That is the general rule for this stage: **a cache
copy is an optimization, and an optimization never costs you the worktree you
asked for.** A tracked entry, an unreadable source, a full disk — each becomes a
warning row and creation continues.

The check asks the **source** worktree only, which is the right question for
"may daft copy this?" — but it means the destination's own `.gitignore` never
gets a vote. Warm a branch that does not ignore `target/` and you land a large
untracked directory in that worktree's `git status`. Keep the ignore rules
consistent across branches you copy between.

### Reading the copied-paths section

Every declared entry leaves exactly one row, so the section reads as a receipt.
Dim `○` rows are the stage working as designed; yellow `↓` rows are the config
asking for something that did not happen. No copy row is ever red.

A row's label is the entry, so its phrase never repeats it — and when a glob
expands, the phrase names the one match that offended, which is how a single row
can report on a thirty-way expansion.

| Row                                                      | Meaning                                                                                                                                                                   |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `✓ target  1.2 GB · reflinked · 0.3s`                    | Copied. A multi-match entry counts them (`3 paths · …`), a mixed entry reports `part reflinked`, and an expansion that could not read everywhere appends `· 2 unreadable` |
| `○ nothing to copy yet`                                  | Declared, but the cache has never been built in the source                                                                                                                |
| `○ already present`                                      | The destination already has it — nothing to do                                                                                                                            |
| `○ matched nothing`                                      | A glob entry expanded to no paths in the source worktree                                                                                                                  |
| `↓ must be gitignored — tracked content is never copied` | Not gitignored, or git tracks content under it                                                                                                                            |
| `↓ 2.1 GB — over the 1 GB max_size`                      | Byte-copy fallback exceeded the cap                                                                                                                                       |
| `↓ no reflink support — fallback: skip`                  | The filesystem cannot reflink and `fallback: skip` declined the byte copy                                                                                                 |
| `↓ already present as … — not replaced`                  | Something is at the destination, but the wrong shape — a symlink where a directory belongs, a file where a tree belongs, a dangling link                                  |
| `↓ … — not copied`                                       | Containment refusal: the entry is absolute, contains `..`, or names the worktree itself                                                                                   |
| `↓ could not be read — …`                                | The source entry could not be read                                                                                                                                        |
| `↓ the destination could not be read — …`                | The destination could not be read, so nothing about it could be established                                                                                               |
| `↓ could not be classified by git — …`                   | `check-ignore` / `ls-files` failed on the source, so the gitignore question went unanswered                                                                               |
| `↓ the destination could not be classified by git — …`   | The same failure on the destination side                                                                                                                                  |
| `↓ is tracked in this worktree — refusing to replace it` | `--force` would have deleted content the **target** tracks                                                                                                                |
| `↓ source and target are the same worktree`              | `daft warm` was pointed at its own source                                                                                                                                 |
| `↓ failed — …`                                           | The copy was attempted and broke — I/O error, permissions, disk full                                                                                                      |

## Re-warming a worktree with `daft warm`

The creation-time stage is not the only way to run it.
[`daft warm`](/reference/cli/daft-warm) replays the same `copy:` declarations on
demand:

```bash
daft warm                 # warm the current worktree from the default branch's
daft warm feature-x       # warm that worktree from where you are standing
daft warm --from main     # name the source explicitly
daft warm --force         # replace entries that already exist in the target
daft warm -v              # add the engine's per-entry narration
```

Standing still and naming nothing warms **you**, from the default branch's
worktree; naming a target warms **it**, from where you stand. `--from` overrides
the source either way, and all three slots accept a worktree directory name, a
branch name, or a path under the project root.

Whichever pair it resolves, the result line names both ends —
`Copied 1 of 2 declared paths (1 KB) into 'develop' from 'main'` — at ordinary
verbosity, so a name that resolved to something you did not mean is visible
without `-v`. `-v` adds the engine's narration on top; it is not what makes the
pair visible.

Without `--force`, entries already present in the target are left alone — so
`daft warm` twice in a row is a no-op, and it never clobbers what a post-create
hook already built.

Reach for it when a worktree was created before you added the `copy:` key, when
you have just built something expensive on the default branch and want it in the
worktrees that predate it, or when a cache went stale and you want the current
one instead. It targets one worktree per run — there is no fleet form, so
spreading a fresh cache means running it per worktree or driving it with
[`daft exec`](/worktrees/running-commands).

`daft warm` does not move your shell; it only writes into the target worktree.

### What `--force` will still refuse

`--force` is the only thing here that deletes, so it is the only thing that asks
the **target** what it is about to delete:

- **Content the target tracks is never replaced.** A path one branch gitignores
  can be committed content on another, and the source's opinion says nothing
  about the destination. The entry is refused with
  `is tracked in this worktree — refusing to replace it`.
- **The bare container is refused as either end.** It has no working tree to
  copy from or into.
- **Nothing is removed until the copy is actually going to proceed.** Every
  other refusal — containment, unreadable source, size cap — fires with the
  destination still intact, so `--force` can never report a skip over a cache it
  has already destroyed.

A source and target that resolve to the same directory are refused outright:
harmless under a normal run, and under `--force` it would delete the very caches
it was asked to replicate.

One edge to know about: if `--force`'s removal itself fails partway (a
permission-locked file inside the entry), that run warns that what is left "will
be mistaken for a finished copy" — and later runs will indeed report the remnant
as `already present`. The warning run names the path; clear it by hand and
re-run.

## What `copy:` does not do

- **It does not run on `daft clone`.** A fresh clone has no source worktree to
  copy from. The first build happens in your `post-clone` or
  `worktree-post-create` hooks; every worktree branched off afterwards inherits
  it.
- **It is not transactional.** The source tree is read live and is not quiesced.
  Copying a `target/` while a build is writing to it yields a torn snapshot — a
  cache that is internally inconsistent. This is accepted rather than defended
  against, because a build cache is regenerable: run the build again, or
  `daft warm --force`, and the tear is gone. Avoid creating worktrees mid-build
  if you would rather not think about it.
- **It does not overwrite.** An entry already present at the destination is
  skipped. `daft warm --force` is the explicit opt-in to replace.
- **It never blocks creation.** See above — warnings only, in every failure
  mode.
- **It does not deduplicate across a fan-out.** `daft start --fork -n 3` runs
  the stage once per fork, so on a reflinking filesystem you get three near-free
  clones — but on one without reflink and `fallback: copy`, three real byte
  copies. Size the fan-out accordingly, or set `fallback: skip` there.

## Migrating from worktrunk's `.worktreeinclude`

If you are coming from worktrunk, its `.worktreeinclude` file lists the
gitignored paths to bring into each new worktree — the same idea, one path per
line. Convert it into a `copy:` block and paste the result into `daft.yml`:

```bash
{ echo 'copy:'; grep -Ev '^[[:space:]]*(#|$)' .worktreeinclude | sed "s/'/''/g; s/.*/  - '&'/"; }
```

The entries come out single-quoted deliberately. A bare `*.log`, `**/dist/`, or
`!keep/` is not a valid plain YAML scalar, and daft.yml is parsed as one
document — a scalar that fails to scan takes the whole file down with it,
dropping every YAML hook in the repo to the legacy-script fallback. The first
`sed` expression doubles any embedded single quote, which is how a single-quoted
YAML scalar escapes one.

Two behavioral differences to know once you switch: daft reflinks where the
filesystem allows instead of always copying bytes, and daft refuses entries that
are not gitignored rather than copying them.

## Where to next

- **The key's full schema:**
  [`copy:` in the daft.yml reference](/hooks/yaml-reference#copied-paths)
- **Global caches instead of per-worktree ones:**
  [Sharing caches across worktrees](/recipes/sharing-caches)
- **Why a cache must not be shared:**
  [Anti-pattern: shared mutable state](/recipes/anti-patterns/shared-mutable-state)
- **Building in the background instead of copying:**
  [Background warmup](/recipes/background-warmup)
- **Config files rather than caches:**
  [`daft shared`](/reference/cli/daft-shared)
- **Reading the creation rail:**
  [Progress timeline](/reference/progress-timeline)
- **The command's full surface:** [`daft warm`](/reference/cli/daft-warm)

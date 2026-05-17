---
title: daft
description: Top-level daft binary and global options
---

# daft

The top-level `daft` binary dispatches to subcommands (`daft list`, `daft go`,
`daft checkout`, …) and equivalent `git worktree-*` symlinks. Run `daft` with
no arguments for a categorized overview, or `daft <command> --help` for help
on a specific subcommand.

## Global options

These options are parsed before subcommand dispatch and apply to every daft
subcommand and `git-worktree-*` symlinked entry.

### `-C <path>`

Run as if `daft` had been started in `<path>` instead of the current working
directory. The chdir happens before repo discovery, layout resolution, hook
lookup, and `daft.yml` reading — so the entire invocation behaves as if you
had `cd`-ed into `<path>` first.

```bash
daft -C ~/repos/foo list         # equivalent to:  cd ~/repos/foo && daft list
daft -C ~/repos/foo go feat/x    # creates the worktree inside ~/repos/foo
git-worktree-list -C ~/repos/foo # works for symlinked entries too
```

#### Composition

Multiple `-C` flags **compose**, matching `git -C`. Each subsequent
non-absolute `-C <path>` is interpreted relative to the previously applied
cwd:

```bash
daft -C ~/repos -C foo list      # equivalent to:  daft -C ~/repos/foo list
daft -C /tmp -C a -C b list      # lands in /tmp/a/b
```

The first relative `-C` is resolved against the cwd at invocation time;
subsequent ones compose against the prior `-C`.

#### Empty argument

`-C ""` is a no-op (cwd unchanged), matching `git -C ""`.

#### Errors

If `<path>` doesn't exist or isn't a directory, daft prints a terse error and
exits with status 2 (clap usage-error convention):

```
daft: -C: '/missing/path': not a directory
```

#### Why it exists

Agent-driven workflows: an agent operating across multiple worktrees in a
single session often can't reliably `cd`, so each invocation needs to be
self-contained. `-C <path>` turns every daft call into "do X in path Y",
matching the pattern users already know from `git -C`, `make -C`, and
`pnpm -C`.

Humans get a quality-of-life win too: `daft -C ~/repos/foo list` is shorter
than `cd ~/repos/foo && daft list`.

#### Interactions

- **Shell-integration cd redirect** (`DAFT_CD_FILE`) works through `-C`. The
  wrapper strips `-C <path>` from the front of args before dispatching to the
  verb, then reattaches them so the binary applies the chdir. `daft -C
  ~/repo go newbranch` leaves your shell inside the new worktree under
  `~/repo`.
- **Hooks** run from the post-`-C` cwd. The `.daft/hooks/` directory and
  `daft.yml` are resolved from the new working directory's repo root.
- **XDG state directories** (`DAFT_CONFIG_DIR` / `DAFT_DATA_DIR`) are
  environment-based, not cwd-based, and are unaffected by `-C`.
- **Relative path arguments** to the subcommand are resolved against the
  post-`-C` cwd. `daft -C ~/repos exec ./script.sh` runs `~/repos/script.sh`.

### `--version`, `-V`

Print the daft version and exit.

### `--help`, `-h`

Print the top-level command overview.

## See also

- [Configuration reference](/reference/configuration)
- [Shell integration](/getting-started/shell-integration)
- `daft <command> --help` for per-subcommand documentation.

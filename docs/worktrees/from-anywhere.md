---
title: Running daft from anywhere with -C
description:
  Use the top-level -C <path> flag to operate on a worktree or repo without
  cd-ing into it. Primary use case is agentic workflows; humans benefit too.
---

# Running daft from anywhere with `-C`

`daft -C <path> <subcommand>` runs the subcommand as if you had `cd`-ed into
`<path>` first. The chdir happens before repo discovery, layout resolution, hook
lookup, and `daft.yml` reading — so the entire invocation behaves as if you had
been there from the start.

Same semantics as `git -C`, `make -C`, and `pnpm -C`.

```bash
daft -C ~/repos/foo list             # equivalent to:  cd ~/repos/foo && daft list
daft -C ~/repos/foo go feat/x        # creates worktree inside ~/repos/foo
git-worktree-list -C ~/repos/foo     # works via the symlink entry too
```

## When to reach for it

### Coding agents

The motivating use case. When an agent works across multiple daft worktrees in
one session, it often can't reliably `cd` — the tool surface either doesn't
expose `cd` or expects each command to be self-contained. `-C` turns every
invocation into "do X in path Y":

```bash
# Agent batch-running tests across three feature worktrees:
daft -C ~/repos/api/feat/auth   exec --all -- cargo test
daft -C ~/repos/api/feat/billing exec --all -- cargo test
daft -C ~/repos/api/feat/search  exec --all -- cargo test
```

Without `-C`, the agent would need to spawn a shell for each command
(`bash -c 'cd … && daft …'`), or fight daft's repo-discovery walk-up.

### Humans

`daft -C ~/repos/foo list` is shorter than `cd ~/repos/foo && daft list`, and
keeps your shell wherever it was.

## Composition (matches git)

Multiple `-C` flags compose. Each subsequent non-absolute path is interpreted
relative to the previously applied cwd:

```bash
daft -C ~/repos -C foo list          # = daft -C ~/repos/foo list
daft -C /tmp -C a -C b list          # lands in /tmp/a/b
```

This is **not** "last wins" — that would silently break scripts that build up
`-C` arguments incrementally.

## With the shell integration

`-C` cooperates with `DAFT_CD_FILE` redirection. If you have the shell wrapper
installed (`eval "$(daft shell-init bash)"`), running:

```bash
daft -C ~/repos/foo go feat/x
```

leaves your shell inside `~/repos/foo/feat/x`, not in `~/repos/foo` and not in
your previous cwd. The wrapper strips `-C <path>` from the front of the args
before dispatching to the verb, then reattaches them so the binary applies the
chdir — so the cd-redirect path is preserved.

## Edge cases

- `-C ""` is a no-op (cwd unchanged), matching `git -C ""`.
- Missing/non-directory path: terse error and exit 2.
- Relative paths in subcommand arguments are resolved against the post-`-C` cwd.
  `daft -C ~/repos exec ./script.sh` runs `~/repos/script.sh`.
- `-C` is parsed before subcommand dispatch, so a subcommand-local `-C` flag
  (e.g. an inner shell command's `-C`) is not consumed: only the leading
  `-C <path>` pairs (those appearing before the verb) are processed.

## See also

- [`daft` CLI reference — global options](/reference/cli/daft#global-options)
- [Shell integration](/getting-started/shell-integration)

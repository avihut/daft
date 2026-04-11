# `daft go` Completion Overhaul

## Problem

`daft go <TAB>` currently produces a confusing, flat list that mixes every local
branch, every `origin/*` branch (with the prefix stripped), and every
command-line flag together. Two concrete problems:

1. **Flags leak into branch completion in zsh.** The zsh generator appends
   `compadd -a flags` unconditionally, so flags appear alongside branches even
   when the user has typed no leading `-`. Bash already early-returns when
   `$cur` doesn't start with `-`, so this bug is zsh-specific.
2. **Names are ambiguous and occasionally nonsensical.** `origin/*` branches are
   shown with the `origin/` prefix stripped, which means a remote-only branch
   looks identical to a local one. In at least one observed case a bare `origin`
   token surfaces in the list — a token that is not a valid navigation target.

There is also no hint of which branches actually have a worktree to navigate to,
no metadata (e.g. how recently the branch was touched), and no visual grouping —
everything is one flat alphabetical list.

## Goals

- Group completions as **worktrees → local branches → remote branches**, in that
  order, without group headers. Sorted within each group.
- Disambiguate names: don't collide local and remote branches; surface the
  remote prefix only in multi-remote mode.
- Show the last-commit relative age next to each name on shells that support
  per-item descriptions (zsh, fish).
- Color each group distinctly in zsh. Fish gets implicit differentiation from
  its native description rendering. Bash gets no color.
- Only show flags when the user has typed a leading `-`.
- When the user types a prefix that matches nothing locally, run `git fetch`
  with a spinner drawn to the terminal, then re-resolve — so that completing to
  a remote-only branch "just works" even if the local ref cache is stale.

## Non-goals

- Dirty-state indicator (`●`), ahead/behind counts, or any metadata beyond
  last-commit age. Speed is the preference; these require per-worktree
  `git status` / `rev-list` calls that blow the perf budget.
- Bash coloring. Bash completion has no per-item color mechanism worth building
  around.
- Fetching from non-default remotes in multi-remote mode. The fetch-on-miss path
  only fetches from the configured default remote. Other remotes are listed from
  whatever is already cached locally.
- A fuzzy/fzf-style picker on TAB. That is a separate, larger feature.
- Changing Fig / Amazon Q completions. Fig specs are declarative and do not
  participate in the dynamic completion path; `fig.rs` stays as-is.

## Architecture

There are three layers:

1. **The data layer** — `daft __complete daft-go` in `src/commands/complete.rs`
   collects refs and worktrees, groups and dedupes them, emits one line per
   suggestion in a tab-separated format.
2. **The shell layer** — generated completion scripts in
   `src/commands/completions/{bash,zsh,fish}.rs` call `daft __complete`, parse
   its output, and push it into the shell's completion system (grouped and
   colored where supported).
3. **The fetch-on-miss layer** — when invoked with `--fetch-on-miss` and the
   prefix has no matches in the local+cached data, `daft __complete` runs
   `git fetch` in-process while drawing a single-line braille-dot spinner to
   `/dev/tty`, then re-resolves and emits results.

### Data protocol

`daft __complete daft-go <prefix>` emits one line per candidate in tab-separated
form:

```
<name>\t<group>\t<description>
```

Where:

- `<name>` is the token the shell should complete to.
- `<group>` is one of `worktree`, `local`, `remote`.
- `<description>` is the last-commit relative age (e.g. `3 days ago`,
  `just now`). May be empty.

Example:

```
master<TAB>worktree<TAB>2 hours ago
fix/go-completions<TAB>worktree<TAB>just now
feat/bar<TAB>local<TAB>4 days ago
bug/xyz<TAB>remote<TAB>3 weeks ago
```

Older shell scripts that only read column 1 still get correct (if un-grouped,
un-described) completion, which matters while users have a mix of old and
newly-regenerated completion scripts in flight.

A new flag `--fetch-on-miss` triggers the fetch path described below. It is
ignored by any caller that does not pass it — shell scripts that predate this
change continue to work.

### Population rules

**Worktree group.** `git worktree list --porcelain` → `(branch, path)` pairs. If
completion is invoked from inside a worktree, that worktree's branch is excluded
from the list — `daft go` to the branch you are already on is a no-op. If
completion is invoked from outside any worktree (e.g. from a bare repository
root), nothing is excluded. Sorted by branch name.

**Local group.**
`git for-each-ref refs/heads/ --format='%(refname:short) \t%(committerdate:relative)%(if)%(worktreepath)%(then)\tHAS_WT%(end)'`.
Entries tagged `HAS_WT` are dropped — they already appear in the worktree group.
Sorted by branch name.

**Remote group.**
`git for-each-ref refs/remotes/ --format='%(refname:short)\t%(committerdate:relative)'`.
Filter:

- Drop HEAD symrefs (`origin/HEAD`, etc.).
- Drop any entry whose name (after potential prefix stripping) matches a
  worktree or local branch — local shadows remote.
- In **single-remote mode** (the default, i.e. when
  `multi_remote_enabled == false`), strip the leading `<default_remote>/` prefix
  from the name. The description column is the relative age.
- In **multi-remote mode** keep the full `<remote>/<branch>` form verbatim. No
  stripping — the prefix is part of the navigation target.
- Sorted by name after prefix handling.

All three groups come from at most three `git for-each-ref` /
`git worktree list` calls — no per-branch git invocations. The perf target is
unchanged: < 50 ms for the local path (no fetch).

### Fetch-on-miss with spinner

Triggered only when:

- `--fetch-on-miss` is passed by the shell script (always passed for `daft-go`),
  AND
- The prefix is non-empty (empty prefix = browsing, not searching), AND
- The local+cached resolution produces zero candidates, AND
- The cooldown marker (`<git-common-dir>/daft_complete_last_fetch`) is either
  missing or older than 30 seconds, AND
- `daft.go.fetchOnMiss` is not explicitly set to `false`.

When triggered:

1. Open `/dev/tty` for writing. If that fails (non-interactive shell, CI, piped
   completion) skip straight to silent fetch.
2. Spawn a background thread that writes braille-dot frames
   (`⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏`) at ~10 Hz to `/dev/tty`, each frame prefixed with
   `\r` so they overwrite in place. Frame text:
   `<frame> Fetching refs from <remote>…`.
3. On the main thread, run `git fetch --quiet --no-tags <default_remote>` with a
   5-second timeout.
4. When the fetch returns (or times out), signal the spinner thread to stop,
   wait for it to join, clear the line with `\r\033[K`, and close `/dev/tty`.
5. Touch the cooldown marker.
6. Re-run the population rules against the now-updated refs, emit results to
   stdout as normal.

On fetch error or timeout, clear the spinner and return whatever the local
resolution found (possibly empty). No error output — shell completion is not the
right channel for errors.

### Settings

Add `go_fetch_on_miss: bool` to `DaftSettings` in `src/core/settings.rs`,
alongside the existing `go_auto_start`. Sourced from the git config key
`daft.go.fetchOnMiss`, default `true`. Documented on the config reference page.

### Fixing the flag-gating bug

In the generated zsh script, the existing sequence:

```zsh
# Branch completions
if [[ $curword != -* ]]; then
    compadd -a branches
fi
# ...
compadd -a flags
```

becomes:

```zsh
if [[ $curword == -* ]]; then
    compadd -a flags
    return
fi
# grouped worktree/local/remote via _describe
```

This is the regression fix that CLAUDE.md's "every bugfix needs a repro test"
rule covers.

## Shell rendering

### zsh

The `daft-go` branch of the zsh generator is the biggest rewrite:

```zsh
__daft_go_impl() {
    local curword="${words[$CURRENT]}"

    # Flags only when user has typed '-'
    if [[ "$curword" == -* ]]; then
        local -a flags
        flags=( ... )     # from clap introspection, same as before
        compadd -a flags
        return
    fi

    # Fetch candidates from daft __complete (tab-separated)
    local -a raw wt_items local_items remote_items
    raw=(${(f)"$(daft __complete daft-go "$curword" --position "$cword" \
                    --fetch-on-miss 2>/dev/null)"})

    local line name group desc
    for line in "${raw[@]}"; do
        name="${line%%$'\t'*}"
        local rest="${line#*$'\t'}"
        group="${rest%%$'\t'*}"
        desc="${rest#*$'\t'}"
        case "$group" in
            worktree) wt_items+=("$name:$desc") ;;
            local)    local_items+=("$name:$desc") ;;
            remote)   remote_items+=("$name:$desc") ;;
        esac
    done

    _describe -t worktree '' wt_items
    _describe -t local    '' local_items
    _describe -t remote   '' remote_items
}
```

Empty-string tag descriptions (`''`) suppress the group headers. The three
`_describe` calls execute in order, so worktrees appear first in the menu. The
`-t <tag>` parameter is what lets zstyle color each group.

Shell-init emits a scoped zstyle block alongside the generated functions:

```zsh
zstyle ':completion:*:*:daft-go:*:worktree' list-colors '=(#b)(*)=0=1;32'
zstyle ':completion:*:*:daft-go:*:local'    list-colors '=(#b)(*)=0=1;34'
zstyle ':completion:*:*:daft-go:*:remote'   list-colors '=(#b)(*)=0=2;37'
```

worktree = bright green, local = bright blue, remote = dim gray. Scoped to the
`daft-go` completion context so the user's global completion colors are
untouched. Users who dislike these colors can `zstyle -d` them.

### bash

Bash has no per-item description or color support. The generator parses column 1
only and concatenates `worktree + local + remote` in that order:

```bash
if [[ "$cur" != -* ]]; then
    local raw
    raw=$(daft __complete daft-go "$cur" --position "$cword" \
              --fetch-on-miss 2>/dev/null | cut -f1)
    if [[ -n "$raw" ]]; then
        COMPREPLY=( $(compgen -W "$raw" -- "$cur") )
        compopt -o nosort 2>/dev/null || true
        return 0
    fi
fi
```

`compopt -o nosort` (bash ≥ 4.4) preserves the group ordering emitted by
`daft __complete`. On older bash it silently falls through and the user gets
alphabetical ordering — correctness is preserved, only group ordering is lost.

The flag-gating `if [[ "$cur" == -* ]]` branch is already correct in bash and is
unchanged.

### fish

Fish's existing pattern for `daft-go` is a single
`complete -f -a "(daft __complete daft-go '' 2>/dev/null)"` line. Extend it to
pass the description column — fish natively reads `name\tdescription` from
completion helpers:

```fish
complete -c daft -n '__fish_seen_subcommand_from go' -f -a \
    "(daft __complete daft-go (commandline -ct) --fetch-on-miss 2>/dev/null \
      | awk -F'\\t' '{printf \"%s\\t%s · %s\\n\", \$1, \$3, \$2}')"
```

The `awk` reshuffles tab-separated columns into fish's `name\tdescription`
format with the description as `<age> · <group>`. Fish colors descriptions
distinctly from the main completion, which provides the visual grouping
automatically. The exact shell-quoting of the inline awk expression will be
finalized during implementation — the snippet above is illustrative.

## Testing

- **Unit tests on the data layer.** `complete_daft_go()` driven by fixture git
  output (ref lists, worktree porcelain). Assertions on exact tab-separated
  output for: grouping order, dedupe (local shadows remote, worktree shadows
  local), current-worktree exclusion, single- vs multi-remote naming, empty
  prefix, matching prefix, non-matching prefix.
- **Cooldown marker.** Unit test that touches the marker and asserts fetch is
  skipped within 30s, and runs again after.
- **Generator snapshot tests.** For each of `bash.rs` / `zsh.rs` / `fish.rs`,
  assert that the generated script for `daft-go` contains:
  - The flag-gating on `-`.
  - `_describe -t worktree`, `-t local`, `-t remote` (zsh).
  - `compopt -o nosort` (bash).
  - The zstyle block (zsh, in the daft subcommand completions).
- **Regression test** for the zsh flag-leak bug: snapshot assertion that the
  generated zsh script contains the `curword == -*` gate and does **not**
  contain an unconditional `compadd -a flags` after the branches.
- **YAML scenario** in `tests/manual/scenarios/go/` that sets up a repo with (a)
  a current worktree, (b) another worktree on a different branch, (c) a local
  branch with no worktree, (d) a remote-only branch, and asserts
  `daft __complete daft-go '' --position 1` emits the expected three-group
  output.
- **Spinner.** The tty-drawing side is awkward to test end-to-end. Unit tests
  cover the frame generator (returns the right characters at the right cadence)
  and the cooldown-file logic. The actual drawing is verified via a manual test
  plan entry in `test-plans/`.

## Migration and backward compatibility

- Old callers of `daft __complete daft-go <word>` that parse only the first
  column still work — they lose grouping and descriptions but the completion
  targets are correct.
- `--fetch-on-miss` is a new optional flag; completion scripts that don't pass
  it simply don't get the fetch behavior. Shell-init regenerates the wrapper
  scripts on every shell start, so users pick up the new behavior automatically
  on next shell.
- The zstyle block and `compopt -o nosort` are additive. Users with customized
  completion styles keep theirs.
- `daft.go.fetchOnMiss` defaults to `true`. Existing users who never set it get
  the new fetch behavior.
- `man daft-go` and `docs/cli/daft-go.md` gain a short "Completion behavior"
  section. Man page regenerated via `mise run man:gen`.
- **Not touched**: `fig.rs`. Fig specs are declarative and don't participate in
  the dynamic completion path.

## Files touched

- `src/commands/complete.rs` — new `complete_daft_go()` function;
  `--fetch-on-miss` flag; cooldown-marker I/O; spinner module (inline or
  `src/completion_spinner.rs` — decide during implementation).
- `src/commands/completions/zsh.rs` — rewrite the branch branch of the
  generator; `_describe -t <tag>` per group; flag gating; zstyle block.
- `src/commands/completions/bash.rs` — `compopt -o nosort`; consume new
  tab-separated output from column 1.
- `src/commands/completions/fish.rs` — awk reshuffle into fish's
  `name\tdescription` format for `daft-go`.
- `src/core/settings.rs` — `go_fetch_on_miss: bool` field on `DaftSettings`
  (alongside the existing `go_auto_start`), sourced from the git config key
  `daft.go.fetchOnMiss`, default `true`.
- `tests/manual/scenarios/go/` — new completion scenario YAML.
- `docs/cli/daft-go.md`, `man/daft-go.1` — completion behavior paragraph.
- `test-plans/go-completions.md` — manual checklist for the spinner path.

## Open questions

None. All scope decisions are locked in:

- Grouping without headers, sorted within groups, ordered worktree → local →
  remote.
- Strip the remote prefix only in single-remote mode; keep it verbatim in
  multi-remote mode.
- Last-commit relative age as the description column.
- zsh colors: worktree=bright green, local=bright blue, remote=dim gray.
- Fetch-on-miss is in scope. 5-second fetch timeout. Default on; opt-out via
  `daft.go.fetchOnMiss = false`. 30-second cooldown marker.
- Spinner drawn by `daft __complete` to `/dev/tty`, not by shell scripts.

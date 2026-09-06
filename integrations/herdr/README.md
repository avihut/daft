# daft for herdr

A [herdr](https://herdr.dev) plugin that makes daft the worktree engine beneath
herdr's workspaces. daft creates, opens, forks and removes worktrees (layouts,
`copy:` caches, `shared:` links, `daft.yml` hooks, push and tracking, PR
checkouts, sandboxes); herdr shows each one as a grouped child of its repository
row, exactly as it shows its own worktrees, with daft's status next to it.

## What you get

| Key (suggested)  | Action          | What happens                                                                                              |
| ---------------- | --------------- | --------------------------------------------------------------------------------------------------------- |
| `prefix+shift+g` | `start`         | Popup: branch name and base, then `daft start`; the worktree opens as a grouped workspace with its layout |
| `prefix+shift+o` | `go`            | Popup: a branch, `repo branch`, `pr:123`, a commit, or `-`; `daft go`, then open or focus it              |
| `prefix+shift+f` | `fork`          | Popup: `daft start --fork [-n N]`; every fork opens grouped, the first one focused                        |
| `prefix+shift+x` | `remove`        | Popup: confirm, then `daft remove`; the workspace closes once daft is done                                |
| `prefix+shift+r` | `repo`          | Popup: pick a cataloged repository; its default-branch worktree opens                                     |
| `prefix+shift+a` | `adopt`         | Group the current checkout under its repository row (for worktrees you entered by hand)                   |
| `prefix+shift+s` | `layout`        | Apply the repository's layout to the pane you are in                                                      |
|                  | `tokens`        | Refresh the `$daft` sidebar status for this repository                                                    |
|                  | `install-hooks` | Install daft's user-global hooks now, without waiting for the next server start                           |

And two things that need no key:

- **Every daft-made worktree appears in herdr**, whichever pane, agent or script
  ran `daft start`, `daft go -b`, `daft clone` or `daft sync`. The plugin
  installs two hook scripts in daft's user-global hooks directory, which daft
  runs without consulting its trust database — that gate is for a repository's
  own `.daft/hooks` (see Caveats for the one case where it still applies).
  Removals close the matching workspace.
- **A `$daft` token on every worktree row**: `↑3 ↓1` against the base branch,
  `+2 ~1 ?4 !0` for staged, modified, untracked and conflicted files, `⇡2 ⇣0`
  against the remote, a paused operation such as `rebasing`, optionally the PR
  (`#42 open ✓`), or `✓` when there is nothing to report. herdr hides its own
  branch and status tokens on indented worktree rows, so this is the row's
  status.

## Requirements

- herdr 0.8.0 or newer (developed against 0.8.2)
- daft 1.27 or newer
- `jq`
- macOS or Linux (daft on Windows is WSL-only)

Plugin commands inherit the herdr **server's** environment. When herdr runs as a
service its PATH is bare, so the plugin resolves `daft` and `jq` from the usual
tool homes itself; if that fails, pin the binary in the config (below).

## Install

Link the directory (no build step; the scripts run in place):

```sh
herdr plugin link /path/to/daft/integrations/herdr
herdr plugin list
```

Bind the actions in `~/.config/herdr/config.toml`, and hand herdr's own worktree
keys to the plugin so both paths agree:

```toml
[keys]
new_worktree    = ""   # herdr's native create would bypass daft
open_worktree   = ""
remove_worktree = ""

[[keys.command]]
key = "prefix+shift+g"
type = "plugin_action"
command = "daft.start"
description = "daft start: new branch + worktree"

[[keys.command]]
key = "prefix+shift+o"
type = "plugin_action"
command = "daft.go"
description = "daft go: open a branch, PR or repo"

[[keys.command]]
key = "prefix+shift+f"
type = "plugin_action"
command = "daft.fork"
description = "daft start --fork: throwaway worktrees"

[[keys.command]]
key = "prefix+shift+x"
type = "plugin_action"
command = "daft.remove"
description = "daft remove: this worktree and its branch"

[[keys.command]]
key = "prefix+shift+r"
type = "plugin_action"
command = "daft.repo"
description = "daft: open a cataloged repository"

[[keys.command]]
key = "prefix+shift+a"
type = "plugin_action"
command = "daft.adopt"
description = "daft: group this checkout under its repo"

[[keys.command]]
key = "prefix+shift+s"
type = "plugin_action"
command = "daft.layout"
description = "daft: apply the repo layout to this pane"
```

Show the token in the sidebar (herdr renders only the tokens a row names):

```toml
[ui.sidebar.spaces]
rows = [["state_icon", "workspace"], ["branch", "git_status"], ["$daft"]]
```

Then `herdr server reload-config`. The hooks and tokens are installed and seeded
by the plugin's startup hook, which runs when the server starts; run the
`install-hooks` action (`herdr plugin action invoke daft.install-hooks`) to do
it right away, and check with `daft hooks status` in any repository.

## Layouts

After a worktree opens, the plugin sources a layout file and calls

```sh
after_open PANE CHECKOUT BRANCH SLUG
```

with these helpers available (every pane opens in `CHECKOUT`):

```sh
split_right PANE           # prints the new pane id
split_down  PANE           # prints the new pane id
run PANE "cmd"             # types cmd + Enter into PANE, returns at once
start_agent NAME KIND PANE # herdr agent start; blocks until the agent is idle
```

The first readable file wins:

1. `<plugin config dir>/layouts/<repo>.sh`, by daft's catalog name for the
   repository
2. `<plugin config dir>/layouts/default.sh`
3. `~/.config/herdr/layouts/default.sh`

Every candidate is in a directory you own, and no candidate is inside a
checkout. A layout file is shell that the plugin sources, and in daft's default
**sibling** layout the project root is the default branch's checkout — so a
`herdr-layout.sh` there would arrive with `git clone` and run on the first
`daft go` into a repository you had only just fetched. daft gates a repository's
own `.daft/hooks` behind a trust database; this plugin has no such gate, so it
never reads a layout out of a repository.

`herdr plugin config-dir daft` prints the plugin config dir. A minimal layout:

```sh
#   ┌──────────┬──────────┐
#   │  claude  │  shell   │
#   └──────────┴──────────┘
after_open() {
  local pane=$1 checkout=$2 branch=$3 slug=$4
  split_right "$pane" >/dev/null
  run "$pane" "claude"
}
```

Layouts run from the popup actions and from `prefix+shift+s`. Worktrees that
arrive through daft's hooks only get one when `layout_on_hook = true`, so an
agent typing `daft start` does not spawn another agent. The file is sourced in a
subshell: a layout that exits non-zero or trips over an unset variable is logged
and skipped, and never fails the `daft start` that created the worktree.

## Configuration

`<plugin config dir>/config.toml`, written with commented defaults on the first
start — including a start where `jq` or `daft` could not be found, since that is
the file you fix it in. Flat `key = value` lines only: everything below a
`[table]` header is ignored.

| Key                     | Default    | Meaning                                                                    |
| ----------------------- | ---------- | -------------------------------------------------------------------------- |
| `daft`                  | resolved   | Path to the daft binary, when the server's PATH cannot find it             |
| `jq`                    | resolved   | Path to `jq`, likewise                                                     |
| `popup_placement`       | `popup`    | `popup` (modal, over the layout) or `split` (a pane below the current one) |
| `popup_width`           | `70%`      | Cells or a percentage                                                      |
| `popup_height`          | `60%`      | Cells or a percentage                                                      |
| `layout_on_hook`        | `false`    | Apply the layout to worktrees that arrive through daft's hooks             |
| `tokens`                | `true`     | Report the `$daft` token                                                   |
| `tokens_pr`             | `false`    | Include the pull request and CI state (one forge call per refresh)         |
| `tokens_ttl_ms`         | `86400000` | Token lifetime; herdr caps it at 24 h                                      |
| `refresh_debounce_secs` | `15`       | Minimum gap between refreshes of one repository                            |

Tokens refresh when a worktree opens, when a workspace gains focus, and when an
agent in it goes idle, finishes or blocks; the startup hook re-seeds them after
a server restart. Refreshes run `daft list --format json` once per repository.

## How it works

Two seams carry the integration.

**herdr to daft.** A keybinding invokes a plugin action, which is headless, so
the action opens a popup pane where daft has a real TTY. The popup runs daft
with `DAFT_CD_FILE` set, reads the destination from it, and calls
`herdr worktree open --cwd <repo root> --path <checkout>`, which groups the
workspace under the repository row with worktree provenance. Then it applies the
layout and refreshes tokens.

**daft to herdr.** daft runs executable scripts from its user-global hooks
directory (`daft __dirs` shows the config dir) in every repository. The plugin
generates them at startup, each one a four-line script that execs
`bin/daft-hook.sh` from the plugin root. The installed `worktree-post-create`
hook calls `herdr worktree open` when `HERDR_ENV=1`; `worktree-pre-remove`
closes the matching workspace once daft has exited and the directory is really
gone, so a removal typed inside that very workspace does not kill its own daft,
and a removal that is refused leaves the workspace open. The popup flows set
`DAFT_HERDR_PLUGIN_ACTIVE=1` so a creation is mirrored once.

Both hooks stand down for a move. `daft rename` and `daft layout transform`
replay the whole remove-then-create sequence with `DAFT_IS_MOVE=true`, with the
same worktree on either side; acting on it would close the workspace you are
working in and open a second one for the new path. herdr 0.8.2 cannot re-point a
live workspace, so the row keeps its old path until you re-group it with
`adopt`.

## Caveats

- herdr's right-click entries **New worktree** and **Delete worktree checkout…**
  still run herdr's own `git worktree` path; on 0.8.2 a plugin cannot replace
  them. Keep `[worktrees].directory` pointed at your daft projects root as a
  safety net.
- `daft go` to an already existing worktree fires no daft hook, so a `daft go`
  typed in a pane is not mirrored; use `adopt`.
- In a repository with its own `daft.yml` hooks that is **not** trusted, daft
  skips the whole hook phase, including these user hooks. `daft hooks trust`
  fixes it.
- `DAFT_CD_FILE` follows `daft.autocd`; with it off, the popups cannot learn
  where daft landed.
- A renamed or relocated worktree keeps its old path in the sidebar until
  `adopt` re-groups it (see How it works).

## Uninstall

```sh
herdr plugin unlink daft
rm "$(daft __dirs | awk -F'\t' '$1 == "config" { print $2 }')"/hooks/worktree-{post-create,pre-remove}
```

The plugin only removes hook files that carry its marker line.

## Development

The scripts are bash 3.2 (macOS `/bin/bash`) and call herdr through
`$HERDR_BIN_PATH`. `tests/integration/test_herdr_plugin.sh` in the daft
repository exercises them against a stub `herdr` that records every call and the
real daft binary, in an isolated `DAFT_CONFIG_DIR`:

```sh
bash tests/integration/test_herdr_plugin.sh
```

Do not run `bin/startup.sh` by hand outside that harness: it installs hooks into
whichever daft config directory `daft __dirs` reports, which unset means your
real one. It refuses to run unless `HERDR_PLUGIN_ROOT` is set (herdr always sets
it), so a bare invocation stops before writing anything; to drive it manually,
point `HERDR_PLUGIN_ROOT`, `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_STATE_DIR`
and `DAFT_CONFIG_DIR` at scratch directories first.

The suite runs on Linux in CI (`integration-tests` in `test.yml`). The plugin's
BSD-vs-GNU-divergent calls — `ps -o comm=`, `find -perm`/`-mmin`, `sed -E`,
`sort -V`, `tr -c` — are exercised on macOS only when you run it locally.

`herdr plugin log list --plugin daft` shows each action's output;
`<plugin state dir>/plugin.log` has the plugin's own log.

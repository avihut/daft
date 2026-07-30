---
branch: daft-470/config-management-interface
---

# Config management interface

Everything below wants a scratch repository, never this one. The screen only
opens on a TTY, so run it from a real terminal rather than through a pipe.

```bash
cd $(mktemp -d) && git init -q . && daft config
```

## The browser

- [ ] `daft config` opens full screen and lists every setting, grouped by
      category
- [ ] The header names the repository and shows the write scope
- [ ] `j`/`k` and the arrows move; neither ever lands on a category heading or
      in the gap between two
- [ ] The current row is lit across its whole width, out to the right edge
- [ ] A blank line separates every category from the one above it, and the top
      of the list is never that blank line
- [ ] `tab` moves between the rail and the list; `h`/`l` do the same
- [ ] With the rail focused, the list keeps its lit row but drops the cyan bar,
      so a category jump is visible where it lands
- [ ] Walking the rail's top three entries changes what the list holds (All /
      Modified / Issues)
- [ ] Walking the rail's categories moves the cursor without hiding anything
- [ ] `g` and `G` reach both ends; `PageUp`/`PageDown` move by a screenful
- [ ] `]` and `[` (and `}`/`{`) walk the categories; `[` from mid-section goes
      to the top of that section first
- [ ] The footer fits on one line at 80, 100, 101 and 120 columns, with and
      without a filter — `q quit` is never the thing that falls off
- [ ] `q` exits and the terminal is left exactly as it was found — scrollback
      intact, cursor visible, no leftover raw mode

## Provenance

- [ ] With a value set at both global and local scope, the ladder shows both and
      marks the local one as the winner
- [ ] With nothing set, the ladder marks the default — the mark and the
      effective line always name the same layer
- [ ] The detail panel names the file each value came from
- [ ] `daft.checkout.pushVerify` with nothing set reads "inherited from
      daft.pushVerify"
- [ ] With `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=daft.remote` and
      `GIT_CONFIG_VALUE_0=x` exported, the environment rung wins and the panel
      says no config file can change it

## Filtering

- [ ] `/` opens the prompt; typing narrows the list as you go
- [ ] Typing `sync` does not flip the write scope on the `s`
- [ ] `enter` keeps the filter and hands the letter keys back to the commands
- [ ] `esc` clears the filter; a second `esc` exits
- [ ] A filter matching nothing says so rather than painting an empty pane

## Editing

- [ ] `enter` on an enum opens the editor on its current value
- [ ] The scope row shows both scopes; `tab` switches; the write follows it
- [ ] `unset` is the last option and names what it would reveal
- [ ] Applying narrates the value and the scope, and the list updates
- [ ] `space` on a boolean flips it without opening the editor
- [ ] `space` on a duration says it is not a toggle
- [ ] `u` clears the value at the pill scope
- [ ] A text setting types, edits with the arrows, and shows its format hint
- [ ] Typing something the type rejects shows the reason live, in red
- [ ] With `daft.merge.cleanup = remove-branch`, setting
      `daft.merge.commit = false` is refused **inside the box**, which stays
      open on the value that caused it
- [ ] `daft.updateCheck` at local scope warns inline and refuses to apply

## Diagnostics

- [ ] `git config daft.checkout.fetch maybe` shows the row red, the ladder marks
      it, and the effective value falls back to the default
- [ ] `git config daft.checkoutbranch.carry false` (lower-case `b`) appears
      under Issues as a key that is set and does nothing, and the panel suggests
      `daft.checkoutBranch.carry`
- [ ] `git config daft.fetch.args --rebase` reports the retired spelling and
      names its replacement

## Layout

- [ ] The Layout row shows all six layers
- [ ] With a `layout:` in `daft.yml` and a different one in the repo store, the
      repo store wins and the ladder makes that visible
- [ ] Editing it offers "repo store" and "global toml", not "local"/"global"
- [ ] The change is recorded; no worktrees move

## daft.yml

- [ ] Editing `log.retention` writes `daft.local.yml` at local scope and the
      committed file at the other
- [ ] `git diff` after an edit shows exactly one changed line
- [ ] Comments and blank lines either side of the edited key survive
- [ ] A `daft.yml` using `log: {retention: 7d}` is refused, naming the file
- [ ] `merge.ff` cannot be relaxed from either file
- [ ] `shared` says it is managed by `daft shared`

## Terminal shapes

- [ ] Below 100 columns the rail disappears and the footer says so
- [ ] The editor still fits and is usable at that width
- [ ] Resizing while the screen is open reflows without corruption
- [ ] A very small terminal (say 20x6) does not panic

## Outside a repository

- [ ] `cd ~ && daft config` opens, says there is no repository, and still shows
      what `~/.gitconfig` sets
- [ ] The write scope is global and `s` explains why it will not move
- [ ] The editor offers only the one scope

## The command line

- [ ] `daft config list | head` works when piped (no screen, no hang)
- [ ] `daft config get daft.autocd` prints only the value
- [ ] `daft config get daft.merge.edit` exits 1 when nothing sets it
- [ ] `daft config set daft.merge.style SQUASH` stores `squash`
- [ ] `daft config set daft.update.args --ff-only` takes the flag as a value
- [ ] `daft config get daft.merge.stile` suggests `daft.merge.style`
- [ ] `daft config <TAB>` completes the verbs; `daft config set <TAB>` completes
      keys; `daft config set daft.merge.style <TAB>` completes its variants
- [ ] `daft config list --category <TAB>` completes the category names, and
      `daft config list --format <TAB>` completes the seven row formats while
      `daft config get x --format <TAB>` completes only the four document ones
- [ ] `daft config get x --local <TAB>` and `--global` complete on all four
      verbs

## Choosing a layer

- [ ] With `daft.remote` set globally to one value and locally to another, `get`
      prints the local one, `get --global` the global one, and `get --local` the
      local one
- [ ] `get <key> --local` exits 1 for a key that layer does not set, while the
      same `get` without the flag still prints the resolved default
- [ ] `--local --global` together is refused, and so is `--origin --local`
- [ ] `list --global` shows only what global sets, showing global's own values,
      and marks the row local outranks with `outranked by local`
- [ ] For a `daft.yml` setting, `set --global` edits the committed file
      (comments intact) and `set --local` creates `daft.local.yml`; reading each
      layer back returns the right one of the two
- [ ] For the layout row, `--local` is the repo's own entry and `--global` the
      default in the global config, and each write says which store it used
- [ ] From outside a repository, `--local` is an error naming the reason, while
      `--global` still answers
- [ ] `get daft.updateCheck --local` prints a value set there even though daft
      never reads it, and `list --local` counts it in the attention line

## Machine-readable output

- [ ] `get <key> --format json` includes `layers`, `diagnostics`, and
      `writable_scopes` without `--origin` being passed
- [ ] `get <key> --local --format json` sets `value` to that layer's own,
      `effective` to the resolved one, and `outranked_by` when something is
      above
- [ ] `get <key> --local --format json` on a silent layer still prints a
      document **and** exits 1
- [ ] `set remote-sync on --format json` lists all three keys in `written` and
      the resulting `state`
- [ ] `unset <key> --format json` for a key that was not set reports
      `changed: false` and exits 0
- [ ] `set --local rc x --format json` names `daft.local.yml` in `file` and
      `local overlay` in `store`
- [ ] `list --global --format json` carries an `outranked_by` column

## Behaviors

- [ ] The list and the browser both open with `Behaviors` above the categories
- [ ] `daft config get remote-sync` prints `off` in an untouched repository
- [ ] `daft config set remote-sync on` writes all three settings in one line,
      and says which state it left the behavior in
- [ ] `daft config get remote-sync --local` exits 1 when local sets only one of
      the three members, and prints the state once a behavior write made it
      whole
- [ ] Setting one member alone flips the row to `Custom`, and the detail panel
      names which member is out of step
- [ ] `space` on the behavior row steps to the state the panel says it is
      closest to
- [ ] `→` narrows the list to the three member settings; the header says which
      behavior they belong to; `←` and `esc` each come back, and a write while
      narrowed keeps the narrowed list
- [ ] `enter` opens a preset selector, not a value editor: the states are listed
      by name (`Local only`, `Full sync`), with the word you would type for each
      beside it and `unset` set apart below them
- [ ] The editor names the state the behavior is in now, and when that is
      `Custom` says what it is closest to and which member differs
- [ ] Moving between presets updates the table under it: every member, what it
      reads now and where from, and what the highlighted preset would write —
      `unset` shows what it would clear instead
- [ ] Nothing in the editor is cut off mid-sentence at 80 columns, and at 24
      rows it drops explanation rather than the key hints or a refusal
- [ ] Opening the editor on a `Custom` behavior starts on the preset the panel
      says it is closest to, so `enter` resolves the disagreement rather than
      reverting the members that were set deliberately
- [ ] With a member set locally, `tab` to global marks that member's row as
      outranked by local — and the write is still allowed
- [ ] `daft config unset remote-sync` at local scope reports the state the
      global values reveal, not just the removal
- [ ] `daft config set remote-sync custom` is refused and names the real states
- [ ] `daft config set --global remote-sync on` in a repo whose local config
      disagrees says the local value still wins
- [ ] `daft config set remote-sync <TAB>` completes `on`/`off`, not
      `true`/`false`

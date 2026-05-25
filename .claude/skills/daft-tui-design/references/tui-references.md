# TUI Design DNA — 6 Reference Apps

Source-grounded, filtered for daft (git-worktree, ratatui, sync loop).

## gitui

**Stack:** Rust + tui-rs.

**Layout:** Numeric top tabs (`1`–`5`) over two-column work area; modal popups.

**Color philosophy:** Named ratatui colors, RON-serialized, user-themeable. Small semantic palette in `src/ui/style.rs`: `selection_bg`, `diff_file_*`, `disabled_fg`, `danger_fg`. Selected = bg highlight; focused block = bold + colored title, unfocused dims to `disabled_fg`. Red = destructive; same red reused for off-state options.

**Keybinding doctrine:** Always-available single-letter mnemonics (`c` commit, `h` help, `o` options); tabs by digit; hjkl + arrows both wired (`src/keys/key_list.rs`). No chord state. Help via bottom command bar of currently-valid bindings (`src/cmdbar.rs`).

**Microcopy:** Title-case panels ("Unstaged Changes"). Tabs embed the key: `Status [1]`, `Log [2]` (`src/strings.rs`). Lowercase progress ("preparing…", "pushing (3/3)", "done").

**Errors/empty:** Modal popups for failures (`POPUP_FAIL_COPY: "Failed to copy text"`). Empty panels: blank.

**Steal:** `Tab [N]` label format. Currently-valid-only command bar.

**Skip:** 90-field KeysList — daft's surface is too small to justify.

## lazygit

**Stack:** Go + gocui.

**Layout:** Persistent five-panel IDE. Each panel is a context with its own bindings; Enter drills, Esc pops.

**Color philosophy:** 16-color basic palette (`pkg/gui/style/basic_styles.go`). Yellow = warning/pending, red = danger, green = success, cyan = focus. Modes **promote keybinding colors** in the option bar (`pkg/gui/options_map.go`): cyan paste during cherry-pick, green during bisect. Explicit style guide in that file: a non-default color is reserved for keys the user is likely to want to press *right now*.

**Keybinding doctrine:** Single-letter context-sensitive + universals (`q`, `?`, `/`). Footer renders *only currently-applicable* bindings (`renderContextOptionsMap`).

**Microcopy:** Title-case panels. Per-binding tooltips (`pkg/i18n/english.go`). Verb-first commands.

**Errors/empty:** Confirm popups for destructive ops. Filter mode forces popup before exit. Empty panels show title only.

**Steal:** Color-promote keybindings relevant to current mode. Per-context hint bar with only applicable keys.

**Skip:** Five-panel IDE — daft's flows are transactional.

## bottom

**Stack:** Rust + ratatui.

**Layout:** User-defined grid dashboard.

**Color philosophy:** Rotating `Light*` palette (`src/options/config/style/themes/default.rs`). `HIGHLIGHT_COLOUR = LightBlue` is reused for borders + table headers + selection — one color triples for "focused". Battery green/yellow/red. Invalid query → red. Disabled → DarkGray. Three built-in themes; TOML-customizable.

**Keybinding doctrine:** Vim-flavored. `hjkl`, `g/G`, `?` help, `dd` chord to kill. Widget focus via Shift+arrows or tab.

**Microcopy:** Terse uppercase ("RAM", "SWAP", "Rx", "Tx"). Headers BOLD on highlight color.

**Errors/empty:** Invalid filter = red inline text. No-data widgets = empty axes.

**Steal:** One highlight color reused everywhere focused (border + header + selection).

**Skip:** Configurable grid layouts.

## atuin

**Stack:** Rust + ratatui (`crates/atuin/src/command/client/search/interactive.rs`).

**Layout:** Inline overlay above prompt. `Compactness` ladder (Ultracompact/Compact/Full) auto-collapses below 14 lines. Two-tab inspector (`["Search", "Inspect"]`).

**Color philosophy:** Semantic `Meaning` enum themed by user (`crates/atuin-client/src/theme.rs`) — every color is a meaning name. Search hits highlighted via `HistoryHighlighter`.

**Keybinding doctrine:** Five keymaps (emacs/vim-normal/vim-insert/inspector/prefix — `search/keybindings/defaults.rs`), user-configurable, with conditional rules like `ListAtStart → Exit, otherwise → scroll`. `ctrl-c`/`ctrl-g` exit, `tab` accept-without-execute. Prefix key acts as leader.

**Microcopy:** Short tab labels. Terse lowercase status.

**Errors/empty:** Red `invalid_query_style` for bad input. Empty results = empty list; prompt stays usable.

**Steal:** Compactness ladder. Boundary-aware bindings (`ListAtStart → Exit`).

**Skip:** Five keymaps — pick one for daft's audience.

## yazi

**Stack:** Rust + ratatui.

**Layout:** Miller columns (parent / current / preview); `h`/`l` walks.

**Color philosophy:** Mode-keyed bg blocks (`yazi-config/preset/theme-dark.toml`): `normal_main = bg blue bold`, `select_main = bg red bold`. Markers (`marker_copied`/`cut`/`selected`) each get a dedicated color. Notifications: `title_info = green`, `title_warn = yellow`, `title_error = red`. Border = blue focused, gray unfocused. Dark/light auto-selected.

**Keybinding doctrine:** Vim-modal. `h/j/k/l`, `gg`/`G`, `v` visual, leader chords (`[ "g", "g" ]` in TOML). Every binding carries a `desc` field for help/which-key.

**Microcopy:** Imperative verbs in `desc` ("Previous file", "Trash selected files"). Button mnemonics: `"  [Y]es  "`, `"  (N)o  "`.

**Errors/empty:** Notification toasts with semantic title color. Confirm uses `btn_yes = { reversed = true }`.

**Steal:** Per-binding `desc` field — descriptions live *with* bindings, not in separate copy. `[Y]es`/`(N)o` button mnemonics. Notify-toast severity titles.

**Skip:** Miller columns.

## k9s

**Stack:** Go + tview.

**Layout:** Single content table + top crumbs/info + bottom hint menu + `:command` prompt overlay.

**Color philosophy:** 15+ skins. Status colors as **named semantic slots** (`skins/dracula.yaml`): `newColor`, `modifyColor`, `addColor`, `errorColor`, `highlightColor`, `killColor`, `completedColor`. Logo varies by severity (`LogoColorInfo/Warn/Error` in `internal/config/styles.go`).

**Keybinding doctrine:** Single-letter commands + numeric shortcuts for favorites. Config-driven hot keys (`HotKey{ ShortCut, Description, Command }` — `internal/config/hotkey.go`). Bottom menu (`internal/ui/menu.go`) auto-hydrates from current view's `Hints()`.

**Microcopy:** ALL-CAPS resource crumbs ("PODS"). Short menu verbs. Numeric prefixes for digit shortcuts.

**Errors/empty:** Flash bar (`internal/ui/flash.go`) for transient severity messages. Empty table = headers only.

**Steal:** Bottom menu hydrated from per-view `Hints()` — no central registry. Named status slots — one color per *state*, not per widget.

**Skip:** Typed `:command` prompt.

---

## Convergence (5+ of 6)

- **hjkl + arrows.** All six. Daft's TUI rule already aligns.
- **Bottom hint bar of currently-valid bindings.** gitui (cmdbar), lazygit (options_map), bottom, yazi (which-key), k9s (menu). Atuin alone skips it.
- **Semantic color slots, not raw colors.** All six expose named slots. Selected = bg highlight; focused border distinct; red = destructive only; yellow = warning; green = success/added; cyan/blue = focus.
- **Modal popups for destructive confirmation.** gitui, lazygit, yazi. Bottom/k9s use inline chord confirm.
- **Single-letter mnemonics + modifier-for-danger.** `q` quit, `?` help, `/` filter; lowercase safe, uppercase/ctrl stronger (yazi `d` vs `D`; gitui `f` vs `F`).
- **Themeable via serialized config** (RON/YAML/TOML), 2–3 built-in themes.

## Divergence — questions daft must answer

- **Help discovery.** Bottom hint bar (gitui/lazygit/k9s) vs `?` overlay (bottom/yazi). *Always-visible or toggled?*
- **Keybinding ambition.** Tiny (atuin, bottom subset) vs sprawling per-context (gitui, lazygit). *How much surface?*
- **Tab dispatch.** `[N]`-embedded numeric (gitui) vs Tab-cycle (atuin) vs no tabs/drill-down (yazi, k9s). *Daft's multi-tab pickers — which?*
- **Layout density.** Multi-panel persistent (lazygit/gitui) vs inline-and-go (atuin) vs full-screen (yazi/k9s). *Where do sync/prune progress tables live?*
- **Confirmation style.** Modal (gitui/lazygit/yazi) vs inline chord (bottom/k9s) vs none (atuin). *`worktree remove`?*
- **Theming.** Hardcoded slots (atuin) vs full user theming (other five). *Slots-only, or theming infra day-one?*

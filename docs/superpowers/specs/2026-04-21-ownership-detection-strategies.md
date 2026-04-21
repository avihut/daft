# Branch Ownership Detection Strategies

## Problem

Branch ownership is currently decided by a single signal: **the author email of
the branch tip commit** (`git log -1 --format='%ae' <branch>`, compared against
`git config user.email`). See `src/core/worktree/list.rs:439` and
`src/commands/sync.rs:64-93`.

This heuristic is wrong often enough to erode trust in the ownership signal. Any
of these flips a branch you worked on to "not yours":

1. **A teammate touched the tip** — a fixup, a merge commit, a squash-merge, a
   bot PR (Renovate, Dependabot, release-plz autocommits). You wrote 10 commits,
   someone else pushed one on top, the branch is now "unowned."
2. **A rebase by someone else** — `git commit --amend --reset-author` or an
   interactive rebase that rewrites commit authorship.
3. **Identity drift across machines** — different `user.email` values resolve to
   different people locally.
4. **Noreply address variants** — `12345+avihut@users.noreply.github.com` vs
   `avihu@example.com` are treated as different owners.

The original design
(`docs/superpowers/specs/2026-03-15-branch-ownership-and-local-branch-sync-design.md`)
cited "GitHub's Your branches heuristic" as precedent, but that was a
misattribution — GitHub uses server-side **pusher identity**, not commit author.
A survey of other tools (git-town, Graphite, GitLab, Bitbucket, git-branchless,
Linear) turned up zero tools that rely on tip-author-email for ownership; every
robust implementation either declares ownership explicitly, uses a server-side
signal, or gives up.

The direction chosen: stay with **pure commit-history deduction** (no persisted
state, no remote calls), but broaden the signal from a single commit to the full
range of commits that belong to a branch, with multiple aggregation strategies
the user can choose between.

## Design

### 1. Window: `base..branch`, not "last N"

Ownership is computed over commits returned by:

```sh
git log <base>..<branch> --format='%H%x09%an%x09%ae%x09%ct'
```

Where `<base>` is the repo's default branch (already resolved by daft via
`remote::get_default_branch_local`; used today for `ahead/behind`). This is the
same window daft already reasons about for every branch.

"Last N commits" was considered and rejected: N is arbitrary, too wide for short
branches (walks past the branch point into shared history), too narrow for long
ones. The `base..branch` range is the principled window — it adapts to branch
length and ignores shared history automatically.

For the default branch itself, the range is empty — the default branch is always
treated as unowned (consistent with current behavior where `is_branch_included`
only matches on the owner-email-equals-user-email path, and the default branch's
tip author is rarely also the viewer).

For detached worktrees / sandboxes, no branch ref → no ownership (current
behavior; unchanged).

### 2. Aggregation strategies

Given the list of commits in `base..branch` (each a
`(hash, name, email, committer_timestamp)` tuple), the **strategy** decides who
owns the branch. Five strategies are supported:

| Key                 | Rule                                                                                                                                                                                                              |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tip`               | Owner = author of the newest commit in range. Equivalent to current behavior when the range is non-empty.                                                                                                         |
| `any`               | Owner = current user if **any** commit in range was authored by them. If not, owner = author of the newest commit. Permissive.                                                                                    |
| `first`             | Owner = author of the **oldest** commit in range — "who started this branch."                                                                                                                                     |
| `plurality`         | Owner = author with the most commits. Ties broken by most-recent-commit-by-that-author. Stable.                                                                                                                   |
| `majority`          | Owner = author with `> 50%` of commits. No majority ⇒ no owner.                                                                                                                                                   |
| `recency-plurality` | Owner = author with the highest **recency-weighted score**, where each commit at rank `k` from the tip (`k=0` = tip) contributes weight `1/(k+1)`. Ties broken by most-recent-commit-by-that-author. **Default.** |

All strategies look at **author** (`%an` / `%ae`), not committer. A teammate
rebasing your branch should not steal ownership — the author is preserved, the
committer is not.

Empty range ⇒ no owner (not even a fallback to tip — the branch has no commits
it can claim).

Single-commit range ⇒ all strategies collapse to the same answer.

#### Why `recency-plurality` as default

It's the strategy that most closely matches the intuition behind the user's
original request (favor recent work) while still surviving a teammate's drive-by
commit on top. Concrete example with 6 commits in range where you authored the
first 5 and a teammate authored the tip:

- Your score: `1/2 + 1/3 + 1/4 + 1/5 + 1/6 ≈ 1.45`
- Their score: `1/1 = 1.00`
- You own the branch.

Switch in one commit of yours on top: your score gains `1.0`, you win by even
more. Switch in a teammate's rebase that puts three of their commits on top of
yours: you still win if your commit count is high enough. The decay is gentle —
`1/(k+1)` not `0.5^k` — so old work still counts.

### 3. Config

A new config key selects the strategy. `daft.ownership.strategy` in git config,
with the following accepted values:

```
tip | any | first | plurality | majority | recency-plurality
```

Default: `recency-plurality`. Unrecognized values fall back to the default with
a one-line warning on stderr (consistent with how `PruneCdTarget::parse` treats
unknown values today by returning `None` and keeping the default).

Loaded via the same pattern as every other setting in `src/core/settings.rs`:

- New `DaftSettings::ownership_strategy: OwnershipStrategy` field.
- New `keys::OWNERSHIP_STRATEGY = "daft.ownership.strategy"`.
- New `defaults::OWNERSHIP_STRATEGY = OwnershipStrategy::RecencyPlurality`.
- New `OwnershipStrategy::parse(&str) -> Option<Self>` mirroring
  `PruneCdTarget::parse` / `Stat::parse`.

Both `DaftSettings::load` and `DaftSettings::load_global` pick it up.

No command-line flag in v1 — the strategy is a repo-wide / user-wide preference,
not a per-invocation knob. If per-invocation override is ever needed, we can add
`--ownership-strategy <x>` on `list`/`sync`/`prune` later.

### 4. Data model changes

`WorktreeInfo` currently carries a single `owner_email: Option<String>` field.
Replace it with a richer struct:

```rust
pub struct BranchOwner {
    /// Author name of the determined owner — shown in the Owner column.
    pub name: String,
    /// Author email of the determined owner — matched by `--include <email>`.
    pub email: String,
    /// True iff this branch is owned by the user running daft (per the
    /// configured strategy). Precomputed so display code doesn't need to
    /// reach for `user.email` again.
    pub is_current_user: bool,
}

pub owner: Option<BranchOwner>   // None = no commits in range / unresolvable
```

The `is_current_user` flag is precomputed against `git config user.email` at the
same place the strategy is evaluated, which keeps the partition / filter /
divider logic from having to plumb `user_email` through every call site (sync.rs
currently threads it through four layers).

`IncludeFilter::Email(email)` continues to work — it matches against
`owner.email`. `IncludeFilter::Unowned` / `Branch` are unchanged.

`is_branch_included` simplifies to:

```rust
fn is_branch_included(branch: &str, owner: Option<&BranchOwner>,
                      filters: &[IncludeFilter]) -> bool {
    if owner.is_some_and(|o| o.is_current_user) { return true; }
    for filter in filters {
        match filter {
            IncludeFilter::Unowned => return true,
            IncludeFilter::Email(e) => {
                if owner.is_some_and(|o| o.email == *e) { return true; }
            }
            IncludeFilter::Branch(n) => {
                if branch == n { return true; }
            }
        }
    }
    false
}
```

### 5. Display: author name, not email

The Owner column today renders the raw email (`src/output/format.rs:298`,
`src/core/sort.rs:143`, `src/commands/list.rs:398`). Change it to render
`owner.name` — the author name of the winning commit's author, matching how `go`
completions display authorship (`src/commands/complete.rs:270-309` —
`RefInfo { age, author }`, author is `%an`).

Specifics:

- **TUI & CLI list:** Show `owner.name`. Column stays named `owner`. Width
  budgeting unchanged (names are not materially longer than emails for the
  common case).
- **JSON output (`daft list --json`):** The shape changes from a flat
  `"owner": "<email>"` to `"owner": { "name": "...", "email": "..." }` or
  `"owner": null`. This is a **breaking change** for scripted consumers; see
  "Migration" below.
- **Sort by `owner`:** sort by name (case-insensitive), ties broken by email.
  `src/core/sort.rs:142-146` currently sorts on email.
- **Unresolved owner (`None`):** renders as empty string (current behavior for
  `None`-email rows). Sort puts them last.

When multiple commits by the same email have different names (e.g. "Avihu" vs
"Avihu Turzion" across rebased/amended commits), use the **name from the most
recent commit** by that email in the range. Deterministic, intuitive.

### 6. The "owned" partition (sync)

Today sync partitions rows into "full sync" (owned + explicitly included) and
"update only" (everything else) — the divider rendered in the TUI as
`── other branches ──`, see `src/commands/sync.rs:511-541`. This logic stays;
only its input changes:

- Ownership is now the strategy-computed `owner.is_current_user`, not
  `owner_email == user.email` on the tip.
- `user_email.is_some()` gating (the "don't show a divider if user.email is not
  configured") is retained, implemented as "skip ownership computation entirely
  if `git config user.email` is empty — all branches become unowned, and
  `is_branch_included` returns true only for explicit `--include` matches."

### 7. Performance

The new computation runs `git log <base>..<branch> --format=…` once per branch,
instead of `git log -1`. For typical daft usage (≤ 50 branches, ≤ 100 commits
per branch since base) the extra work is trivial — tens of ms at most across a
full list. For monorepos with long-lived branches we may want to cap range
length, but that's a v2 tuning decision (see Open Questions).

The call replaces the existing `get_author_email_for_ref`; there's one `git log`
per branch either way.

### 8. Unconfigured `user.email`

Current behavior: if `git config user.email` is unset, all branches are unowned;
rebase/push require `--include` to proceed. **Preserved unchanged.** The new
strategies all depend on comparing the winning author email to `user.email`;
with no `user.email`, nothing can be "mine."

The Owner column still renders the winning author name — users get to see who
owns each branch even when daft has no notion of "me."

## Files touched

- **`src/core/ownership.rs`** — new module. Contains:
  - `OwnershipStrategy` enum with `parse()`.
  - `BranchOwner` struct.
  - `resolve_owner(branch: &str, base: &str, cwd: &Path, strategy: OwnershipStrategy, user_email: Option<&str>) -> Option<BranchOwner>`.
  - Pure-function inner helpers for each strategy, driven by `&[CommitRecord]`
    so they're trivially unit-testable without git fixtures.
- **`src/core/settings.rs`** — `ownership_strategy` field, defaults/keys module
  entries, `load()` and `load_global()` wiring.
- **`src/core/worktree/list.rs`** — replace `owner_email: Option<String>` on
  `WorktreeInfo` with `owner: Option<BranchOwner>`. Replace
  `get_author_email_for_ref` call sites (three of them at lines ~834, ~984,
  ~1076) with `resolve_owner(...)`. Delete `get_author_email_for_ref` once
  unused.
- **`src/commands/sync.rs`** — update `is_branch_included` signature as
  specified in §4. Drop the `user_email` plumbing. Update the two
  divider-computation call sites (~lines 267-306, 511-541) and the orchestrator
  ownership partition (~line 672).
- **`src/commands/prune.rs`** — update `get_author_email_for_ref` call site
  (~line 283) and the `WorktreeInfo::local_branch_stub` constructor call. Update
  `local_branch_stub` signature in `list.rs` to take `Option<BranchOwner>`
  instead of `Option<String>`.
- **`src/core/sort.rs`** — update `SortColumn::Owner` comparison (~line 142) to
  sort on name then email. Update the test helper at line 458.
- **`src/output/format.rs`** — change Owner column rendering (~line 298) from
  `info.owner_email.clone()` to `info.owner.as_ref().map(|o| o.name.clone())`.
- **`src/commands/list.rs`** — JSON rendering at line 398 emits `{name, email}`
  object or `null`.
- **`src/output/tui/render.rs`**, **`src/output/tui/columns.rs`**,
  **`src/output/tui/state.rs`**, **`src/output/tui/operation_table.rs`**,
  **`src/output/tui/driver.rs`** — audit usages of `owner_email`, replace with
  `owner.as_ref().map(|o| o.name.as_str())` for display and
  `owner.as_ref().map(|o| o.is_current_user).unwrap_or(false)` for partition
  logic.
- **`src/core/worktree/sync_dag.rs`**, **`src/hooks/job_adapter.rs`**,
  **`src/output/buffering.rs`**, **`src/core/columns.rs`**,
  **`src/doctor/installation.rs`**, **`src/commands/clone.rs`**,
  **`src/commands/release_notes.rs`** — audit for `owner_email` / `owner`
  references flagged in the earlier grep. Most are likely read-only display
  references and swap cleanly.
- **`docs/cli/`** — document the new `daft.ownership.strategy` config key on the
  config reference page. Note the Owner column now shows author name.
- **`SKILL.md`** — one-line note that ownership is strategy-based as of this
  change.
- **`tests/manual/scenarios/list/`, `tests/manual/scenarios/sync/`,
  `tests/manual/scenarios/prune/`** — new YAML scenarios, one per strategy,
  seeded with a multi-author commit history.

## Testing

- **Unit tests in `src/core/ownership.rs`** driven by fixture
  `Vec<CommitRecord>` slices. One test per strategy covering:
  - Empty range (all strategies → `None`).
  - Single-commit range (all strategies → same result).
  - Plurality with a clear winner.
  - Plurality with a tie (ties broken by most-recent-commit-of-tied-author).
  - Majority with no majority (`None`).
  - Majority with a clear majority.
  - Recency-plurality where a teammate's tip commit loses to your older
    plurality (the scenario from §2).
  - Recency-plurality where a teammate's three-commit rebase-on-top beats a
    single older commit of yours.
  - `any`: one of your commits anywhere wins.
  - `first`: oldest author wins even if you wrote every later commit.
  - `tip`: author of newest commit wins.
  - Name disambiguation: same email with two different names across commits in
    range → the most-recent name wins.
- **Settings unit tests** (`src/core/settings.rs`):
  - Default strategy is `RecencyPlurality`.
  - All six strategy keys round-trip through `parse()`.
  - Unknown strategy value → default + warning.
- **Integration (YAML) tests** in `tests/manual/scenarios/{list,sync,prune}/`:
  - Repo with two authors and 10 commits, `daft list` under each strategy
    produces the expected Owner column values.
  - `daft sync` with `daft.ownership.strategy = tip` reproduces the current
    (pre-fix) behavior on a branch where a teammate wrote the tip (branch lands
    in "update only").
  - `daft sync` with default `recency-plurality` on the same repo lands that
    branch in "full sync" (the regression test for the user's original
    complaint).
- **Display/format** snapshot tests update to reflect `%an` rendering.
- **JSON output** round-trip test: `daft list --json` emits the new
  `{name, email}` object.

## Migration

- **Config:** purely additive; users who don't set `daft.ownership.strategy`
  silently get the new default. Users who prefer the old behavior can set
  `daft.ownership.strategy = tip`.
- **JSON output:** breaking. The `"owner"` field changes from `string | null` to
  `{name, email} | null`. Document in release notes. Consumers who read
  `jq '.worktrees[].owner'` and expect a bare email string will break. Given
  daft is pre-1.0 and the JSON surface is not advertised as stable, this is
  acceptable. A flag like `--json-owner-format=email` is not proposed; if it
  turns out to be needed we'll add it then.
- **YAML scenarios** in `tests/manual/scenarios/` that assert on Owner column
  values must be regenerated against the new default strategy.
- **Docs site** `docs/cli/daft-list.md` etc. — update the Owner column
  description (email → name) and link to the new config key doc.
- **Man pages:** regenerate after doc string changes (`mise run man:gen`).

## Scope boundaries

**In scope:**

- `OwnershipStrategy` enum with six variants.
- Config key `daft.ownership.strategy`, default `recency-plurality`.
- `BranchOwner { name, email, is_current_user }` data model replacing
  `owner_email` across `WorktreeInfo` and downstream consumers.
- Author-name display in Owner column across CLI, TUI, JSON.
- Migration of the three `get_author_email_for_ref` call sites to the new
  strategy-driven resolver.
- Full unit + YAML integration coverage per strategy.
- Docs + man page updates.

**Out of scope (future work):**

- `--ownership-strategy <x>` command-line override.
- Capping log range length (`-n 500` or similar) for monorepo performance.
- Strategies that weight by lines of code rather than commit count.
- Strategies that include `Co-authored-by:` trailer parsing.
- Gitoxide-native implementation (replacing the raw `git log` call with
  `gix::revision::Walk`).
- Persisting the resolved owner between runs as a cache.
- A per-branch "who declared this theirs" explicit-ownership mechanism (that's a
  separate design; we're explicitly staying in the commit-deduction space).
- Remote-informed signals (GitHub PR author, push identity).

## Open questions

None. All scope decisions are locked in:

- Window is `base..branch`, not "last N".
- Six strategies: `tip`, `any`, `first`, `plurality`, `majority`,
  `recency-plurality`.
- Default is `recency-plurality`.
- Configured via `daft.ownership.strategy` in git config.
- Owner column displays author name (`%an`), JSON carries both name and email as
  an object.
- Recency weight is `1/(k+1)` where `k=0` is the tip. Gentle decay, every commit
  counts.
- Ties in `plurality` / `recency-plurality` broken by most-recent-commit of the
  tied author.
- Unconfigured `user.email` preserves current "everything unowned" safe default.

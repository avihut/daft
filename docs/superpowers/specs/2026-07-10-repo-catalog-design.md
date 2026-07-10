# Repo Catalog & the Graph Pillar

> **Issue:** [#357](https://github.com/avihut/daft/issues/357) — Repo Catalog.
> Branch: `daft-357/feat/repo-catalog`.
>
> **Supersedes:** [#466](https://github.com/avihut/daft/issues/466) (docs-only
> Graph pillar). Absorbs the removed-repo log-lookup requirement raised on the
> [#421](https://github.com/avihut/daft/issues/421) thread.

## Summary

Daft's thesis is "parallelize development through isolation; coordinate across
the repo graph." This design lands the second half's foundation: a machine-local
**repo catalog** (identity, name, location, remote, default branch,
removed-state for every repo daft has touched), a committed **relations
manifest** (`relations:` in `daft.yml`, directed edges keyed by remote URL), and
cross-repo extensions to the existing command surface — `daft go <repo>`, fleet
flags on `list`/`fetch`/`prune`/`doctor`, `exec --repo/--all-repos/--related`,
`start --with-related`, `hooks jobs --repo` (including removed repos), and
`daft clone <name>`.

The catalog is daft's first cross-repo persistent store: one SQLite file under
the data dir, on the existing store spine, with its own migration lineage.
Everything ships in one PR together with the Graph docs pillar, per "docs and
features enter together."

## Goals & Non-goals

**Goals**

- A durable registry of repos: uuid (the `daft-id`), unique live name,
  project-root path, git common dir, remote URL (raw + normalized match key),
  default branch, created/updated/removed timestamps.
- Ambient maintenance: clone/init/adopt/eject register; any daft command run
  inside a repo lazily upserts it; `daft repo remove` tombstones the entry
  instead of deleting it. `daft repo add` exists only for repos daft never
  touched (and for renaming via `--name`).
- Cross-repo navigation: `daft go <repo>`, `daft go <repo> <branch>`,
  `daft go --repo <name>`, resolution from outside any git repository — with
  strict local-first precedence.
- A portable relations manifest and the first two relations consumers
  (`exec --related`, `start --with-related`).
- Fleet operations over the catalog with uniform `--repo <name>` / `--all-repos`
  flags.
- Removed repos stay addressable: `hooks jobs --repo <name>` reaches their
  retained logs; `daft clone <name>` restores them from the recorded remote.

**Non-goals**

- Migrating the trust/layout registry (`repos.json`) into the SQLite store.
  Explicitly reserved for its own PR (CLAUDE.md, Database & Storage); this
  design performs zero `TrustDatabase` writes from catalog code.
- Cross-repo `sync` / `merge` coordination (sequencing, partial failure,
  cancellation across repos) — future Graph work.
- Imperative relation editing (`daft repo link/unlink`); relations are declared
  in the committed manifest only.
- Cross-repo `carry` — uncommitted changes cannot move between unrelated
  histories.

## User surface

### `daft repo` catalog verbs

```text
daft repo add [<path>] [--name <name>]   Register explicitly / rename
daft repo list [--all] [--format …]      Show the catalog (--all: removed too)
daft repo info [<repo>] [--format …]     One entry + resolved relations
```

`add` is the loud path: catalog failures error, and an explicit `--name` that
another live repo holds is refused (`NameTaken`) rather than auto-suffixed.
`--name` on an already-registered repo renames it. `list`/`info` accept the
standard structured-emit flags; `info` resolves by catalog name, path, or uuid,
defaults to the current repo, and renders the repo's relations with each edge's
local resolution (or a `daft clone <url>` hint).

### `daft go` cross-repo grammar

```text
daft go <name>                 branch first; live catalog repo as fallback
daft go <repo> <branch>        open <branch> in <repo> (creating its worktree)
daft go --repo <name> [<br>]   explicit form (shadowed names, cross-repo -b)
daft go -b <branch> [base]     unchanged: repo-local creation
```

The bare two-positional form was an error before this change (positional 2
required `-b`), so assigning it `<repo> <branch>` is backward compatible.
`--repo` is long-only (`-r` is `--remote`). Bare `daft go <repo>` lands on the
repo's default-branch worktree. Outside any git repository, `go` resolves purely
against the catalog; a miss keeps the historical "Not inside a Git repository"
error. `daft go -` stays repo-local — and because a cross-repo hop records the
source worktree in the _target_ repo's previous-worktree state, `daft go -`
naturally hops back across repos.

### Relations manifest

Declared in `daft.yml` under a top-level `relations:` key (old daft versions
ignore unknown keys):

```yaml
relations:
  - url: git@github.com:org/api-client.git # required — the resolution key
    name: client # optional friendly label
    kind: consumer # optional, free-form
```

Edges are directed (A→B does not imply B→A) and keyed by **remote URL**, so a
committed manifest is portable across machines: resolution normalizes the URL
(scheme/user stripped, host lowercased, default port and one `.git` dropped;
local paths stay path-shaped) and matches it against the catalog's normalized
remotes, landing on wherever that repo is cloned locally. Both merge paths
(two-way overlay and the visitor three-way merge) handle the field — the
exhaustive-destructure compile guard forced that.

### exec and start

```text
daft exec --repo <name> [targets… | --all] -- CMD
daft exec --all-repos -- CMD
daft exec --related -- CMD
daft start <branch> --with-related
```

- `--repo` runs inside another repo (bare form targets its default-branch
  worktree; single-target interactive pass-through still applies).
- `--all-repos` fans out over every live repo's default-branch worktree.
- `--related` follows the **current branch**: the current repo's worktree plus,
  per manifest edge, that repo's worktree for the same branch — repos lacking it
  are skipped with a notice. Multi-repo targets render as `repo:branch` (`:` is
  illegal in refnames); `DAFT_BRANCH_NAME` stays raw.
- `start --with-related` creates the same branch here and in every related repo,
  each based on its own default branch. Resolution is all-upfront: a related
  repo that isn't cloned aborts before anything is created. Carry and `-x` stay
  in the current repo; per-repo failures are collected and reported, and the
  final cd is the current repo's new worktree.

### Fleet flags

`list`, `fetch`, and `prune` gain `--repo <name>` and `--all-repos` (long-only;
no clash with `list -a` = branches or `fetch/exec --all` = worktrees). Both work
from outside any repository. `list` fleet mode forces the blocking renderer;
`fetch --all-repos` implies `--all` within each repo; `prune` fleet mode uses
the sequential path with the **current repo last**, preserving its cwd-redirect
semantics. `doctor` gains `--all-repos` (per-repo Repository/Hooks categories)
plus an always-on **Catalog** category.

### hooks jobs and clone-by-name

`daft hooks jobs --repo <name|path|uuid>` resolves history through the catalog —
including removed repos — and implies all-worktrees. It applies to listing and
one-shot `logs` only (cancel/retry/`--follow` act on live jobs in the cwd repo).
`daft clone <name>` substitutes a bare word that matches a catalog entry (live
or removed) with its recorded remote URL; URL- and path-shaped inputs bypass the
catalog entirely.

### Completions

`daft go <Tab>` appends live catalog repos as a trailing group (branches always
shadow; outside a repo the catalog is the whole list), and position 2 completes
the target repo's branches (first positional passed via
`DAFT_COMPLETE_GO_FIRST`). A `repo-name` `__complete` arm serves `repo info` and
every `--repo` flag value in bash/zsh/fish. All catalog reads on the Tab path
are fail-fast: read-only open, 300 ms busy timeout, empty on any error, never
creates the file.

## Data model

One global database at `<data-dir>/catalog/catalog.db`. The dedicated `catalog/`
parent keeps the store's non-bypassable 0600/0700 permission invariants
(`tighten_perms` chmods the DB's parent) away from the shared data dir, which
also hosts centralized-layout worktrees. The catalog has its own migration
lineage (`src/store/migrations/catalog/NNN_*.sql`, `user_version` is per-file)
and shares the store `application_id`.

```sql
CREATE TABLE catalog_repos (
    uuid                  TEXT NOT NULL PRIMARY KEY,  -- daft-id (UUIDv7)
    name                  TEXT NOT NULL,
    path                  TEXT NOT NULL,              -- project root, canonical
    git_common_dir        TEXT NOT NULL,              -- what trust/hooks key on
    remote_url            TEXT,                       -- display / re-clone form
    remote_url_normalized TEXT,                       -- relations match key
    default_branch        TEXT,
    created_at            TEXT NOT NULL,
    updated_at            TEXT NOT NULL,
    removed_at            TEXT                        -- NULL = live
);
CREATE UNIQUE INDEX catalog_repos_live_name_idx
    ON catalog_repos(name) WHERE removed_at IS NULL;
CREATE UNIQUE INDEX catalog_repos_live_path_idx
    ON catalog_repos(path) WHERE removed_at IS NULL;
```

Semantics baked into the model:

- **Live wins.** Name lookups order live rows before removed ones; a name that
  was removed and later re-taken resolves to the live entry. Removed entries
  stay addressable by name, path, or uuid.
- **Tombstones, not deletes.** `removed_at` is set by `daft repo remove` (which
  registers the repo first if daft never cataloged it, so the path→uuid mapping
  is preserved for log lookup) and by `doctor --fix` for stale paths.
  Re-registration clears it (resurrect).
- **Retire-at-path.** Registering a new uuid at a path that hosts a live entry
  marks the old entry removed — a re-clone at the same path is a new identity
  taking over the name; the old uuid's logs remain reachable.
- **Names.** Implicit registration derives the name from the remote URL (falling
  back to the directory name) and resolves collisions by suffixing (`api`,
  `api-2`, …) with a printed notice; registration never clobbers a user-chosen
  name (uuid-keyed refresh preserves it). Explicit `--name` is strict:
  collisions error. The partial unique indexes enforce both invariants at the DB
  level.
- `path` and `git_common_dir` are both stored: the mapping between them is
  layout-dependent, and consumers need both (users navigate by path; trust,
  hooks, and `daft-id` key on the git common dir).

## Resolution & precedence

For `daft go <name>` the order is absolute:

| Priority | Meaning                                                     |
| -------- | ----------------------------------------------------------- |
| 1        | Anything resolvable in the current repo (worktree > local   |
|          | branch > remote branch) — exactly the pre-catalog behavior  |
| 2        | Live catalog repo (the `BranchNotFound` fallback; beats     |
|          | `daft.go.autoStart` — opening something that exists beats   |
|          | creating something new)                                     |
| 3        | Branch creation (`--start` forces it and skips the catalog) |

`--repo` bypasses local resolution entirely (the escape hatch for repos shadowed
by branch names). Catalog needles resolve live-name → uuid → canonical
path/git-common-dir → removed-name. Outside a git repository only the catalog
applies. The one-hop rule: after a cross-repo jump the fallback is disabled
inside the target repo, so resolution can never chain.

## Architecture

```
commands (go/exec/list/…, repo verbs, doctor)
   │
   ▼ call
src/catalog/        service layer: service (Catalog), normalize (pure),
   │                relations (pure), registration (ambient shell), fleet
   ▼ uses
src/store/          models/catalog_repo, repos/catalog_repos,
                    migrations/catalog/, shared connection/pool/migrate
```

- **Store spine reuse.** `migrate.rs` grew a `MigrationSet` (lineage + current
  version); `Pool::open_with(path, set)` and a single-connection
  `connection::open_read_only` are the only plumbing additions. `bring_up`
  remains the sole security gate (PRAGMAs, application-id check, perms) —
  nothing is duplicated or weakened.
- **Service layer, deliberately no port trait.** CLAUDE.md's DB-feature recipe
  calls for a port "if the feature has business logic" in the consuming
  subsystem. The catalog's business rules — URL/name normalization, suffixing,
  relation resolution — are pure functions over `CatalogRepoRow` slices,
  unit-tested with no database and no fakes; `Catalog` (open_rw/open_ro) is the
  single choke point and no command imports `rusqlite`. That covers what the
  ports rule protects. If the coordinator ever consumes the catalog, extract a
  `CatalogPort` into `coordinator/ports/` then — not speculatively now.
- **`repos.json` untouched.** Trust and layout stay where they are; catalog code
  performs zero `TrustDatabase` reads or writes (greppable review gate).
  Consolidation is the future trust-migration PR's job.
- **Ambient registration sites.** One call in `commands/clone.rs::run_clone`
  (covers all three layout paths), `init.rs`, `flow_adopt.rs`, `flow_eject.rs`;
  lazy `touch_current_repo()` (read-first, write only on drift) at the top of
  go/checkout, exec, list, fetch, prune — never on `__complete` or shell-init
  hot paths. `remove_repo.rs` calls `note_repo_removed` _before_ deleting the
  git dir (it reads `daft-id` and canonicalizes live paths), registering
  never-cataloged repos on their way out so removal always leaves an addressable
  tombstone. In-process unit tests suppress ambient writes unless
  `DAFT_DATA_DIR` is sandboxed, so command-level tests can't write the
  developer's real catalog.
- **Cross-repo mechanism = chdir-first.** `GitCommand` discovers the repo from
  the process cwd, and everything downstream — settings, layout resolution,
  trust lookups, hook config, `DAFT_CD_FILE`'s cd target — is cwd-derived.
  Entering the target repo's path before running the existing single-repo body
  is therefore the entire mechanism: hooks and trust operate on the right repo,
  the shell wrapper needs no changes, and `git-worktree-checkout` behavior stays
  byte-identical. Fleet flags are the same idea in a loop
  (`catalog::fleet::for_each_repo`), mirroring the pre-existing `-C <path>`
  global flag.
- **Trust policy for fan-outs.** The current repo keeps normal interactive
  behavior. Related repos are non-interactive: hooks run only when the repo is
  explicitly trusted (`Allow`); `Prompt`/`Deny` repos get `skip_hooks=all` plus
  a per-repo notice. A fan-out must never block on a dialoguer prompt.

## Error handling

| Situation                         | Behavior                                                      |
| --------------------------------- | ------------------------------------------------------------- |
| Unknown repo needle               | `repository 'x' not found in the catalog` + did-you-mean      |
|                                   | suggestions + `repo list` / `repo add` tips; the              |
|                                   | two-positional go form adds a note explaining the grammar     |
| Removed repo as a go/fleet target | Error with the restore hint: `` restore it with `daft clone   |
|                                   | <name>` ``                                                    |
| Live entry with a missing path    | Actionable error (re-add from the new location, or re-clone); |
|                                   | fleet sweeps skip it with a warning; doctor flags it and      |
|                                   | `--fix` tombstones it                                         |
| `--related` with no manifest      | `this repo declares no relations` + the minimal YAML shape    |
| `--with-related`, uncloned edge   | Fatal before anything is created, with the `daft clone <url>` |
|                                   | tip                                                           |
| Catalog unreadable / schema-newer | Loud on explicit paths ("upgrade daft" via `SchemaTooNew`);   |
|                                   | silent empty on completion/fallback paths                     |

Runtime hints always render through `daft_cmd()` so the executable is shown the
way the user invoked it.

## Testing

- **Unit:** migration lineages (per-lineage `SchemaTooNew`), catalog repo
  CRUD/resurrect/retire/live-wins plus the partial unique indexes, URL
  normalization matrix (scp/ssh/https/file/local-path, ports, case), name
  suffixing and validation, relation resolution, the full `daft go` grammar
  decode matrix (including error copy and the untouched `git-worktree-checkout`
  grammar), and the read-only open's never-creates contract. Service tests use
  `open_rw_at`/`open_ro_at` seams under tempdirs — no env mutation, no serial
  tests.
- **YAML scenarios** (the sandbox exports `DAFT_DATA_DIR`, so the catalog is
  isolated): `repo/catalog-*` (implicit registration, rename + name-taken,
  collision suffixing, remove tombstones, re-clone live-wins),
  `checkout/go-cross-repo*` (jump, repo+branch, slashed branches, `--repo`,
  autoStart precedence, `--start` override, local shadowing, outside-repo,
  `go -` hop-back), `exec/cross-repo`, `checkout-branch/start-with-related`,
  fleet scenarios for list/fetch/prune, `doctor/catalog-checks`,
  `hooks/jobs-removed-repo`, `clone/by-catalog-name`, and a completions
  assertion that repo names appear. Multi-repo topology uses the existing
  `repos: [{name, use_fixture: standard-remote}, …]` machinery.
- **Caveat discovered while testing:** the shared-bin build cache hashes HEAD
  plus _tracked_ `.rs` files, so a brand-new untracked source file does not
  invalidate it — `git add` new files before trusting `mise run test:manual`, or
  a stale binary runs. Worth a follow-up fix in `_shared_bin_lib.sh` (e.g.
  `git ls-files -co --exclude-standard`).
- No `test_shell_init.sh` changes were needed: `go` and `daft repo *` were
  already wrapper-wired, and the cross-repo cd rides the same `DAFT_CD_FILE`
  path.

## Open questions & future work

- **Trust/layout migration.** Fold `repos.json` into the store so per-repo state
  has one home (its own PR; the catalog's uuid+path bridge is the prerequisite
  it needed).
- **Cross-repo sync/merge.** Coordinated merges across the graph (service then
  client) need sequencing and partial-failure semantics — deep water,
  deliberately out.
- **Imperative relation verbs.** `daft repo link/unlink` writing the manifest;
  v1 keeps relations author-edited.
- **`DAFT_REPO_NAME` for exec.** Multi-repo runs could export the catalog name
  alongside `DAFT_BRANCH_NAME` for scripts.
- **Relation kinds.** `kind` is free-form; if patterns emerge (client, library,
  infra), tooling could interpret them (e.g. direction-aware `--related`
  filters).
- **Cancel/retry via `--repo`.** `hooks jobs --repo` covers listing and logs;
  extending cancel/retry is mechanical if ever needed (live jobs require a live
  repo, so the value is marginal).

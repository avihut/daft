# Repo Catalog Implementation Plan

> **Status:** COMPLETE — this plan is the retrospective record of the work as
> built on `daft-357/feat/repo-catalog`. All boxes are checked; phases map 1:1
> to the branch's commits.

**Goal:** Land the Graph pillar's foundation
([#357](https://github.com/avihut/daft/issues/357)) in one PR: a SQLite repo
catalog with ambient registration, `daft repo add/list/info`, cross-repo
`daft go`, a `relations:` manifest with `exec --related` /
`start --with-related`, fleet flags on list/fetch/prune/doctor,
`hooks jobs --repo` (incl. removed repos), clone-by-name, completions, and the
full Graph docs pillar.

**Spec:** `docs/superpowers/specs/2026-07-10-repo-catalog-design.md`

**Architecture:** New global DB (`<data-dir>/catalog/catalog.db`) on the
existing store spine with its own migration lineage; `src/catalog/` service
layer (no port trait — see spec); chdir-first cross-repo mechanism reusing each
command's single-repo body; `repos.json` untouched.

---

## Phase 0 — Store plumbing

_Commit: `feat(catalog): global repo catalog store and ambient registration`_

- [x] `MigrationSet` + `coordinator_set()`/`catalog_set()`/`run_set()` in
      `src/store/migrate.rs`; existing `run()` delegates unchanged.
- [x] `src/store/migrations/catalog/001_catalog.sql` — `catalog_repos` DDL with
      partial unique indexes on live name and live path.
- [x] `Pool::open_with(path, set)` (`src/store/pool.rs`) and
      `connection::open_read_only` (`src/store/connection.rs`); `bring_up` stays
      the single security gate.
- [x] `paths::catalog_db()` / `catalog_db_under()` / `catalog_db_probe()`
      (`src/store/paths.rs`) — dedicated `catalog/` parent so `tighten_perms`
      never chmods the shared data dir.
- [x] `src/store/models/catalog_repo.rs` (`CatalogRepoRow`) and
      `src/store/repos/catalog_repos.rs` (literal SQL via a `concat!` macro —
      the repo layer bans `format!`): insert, uuid-keyed `update_registration`
      (resurrects, never touches `name`), rename, `mark_removed`,
      `retire_live_at_path`, live-wins lookups, lists.
- [x] Unit tests: per-lineage `SchemaTooNew`, catalog tables, CRUD, resurrect,
      retire-at-path, index enforcement, perms, symlink escape.

## Phase 1 — Catalog domain + ambient registration

_Same commit as Phase 0._

- [x] `src/catalog/service.rs` — `Catalog` (`open_rw`/`open_ro` + `_at` test
      seams), `register` with auto-suffix outcomes, `resolve` precedence,
      `rename` (`NameTaken`), `mark_removed`, `refresh_default_branch`,
      `not_found` with did-you-mean suggestions.
- [x] `src/catalog/normalize.rs` — pure `normalize_url`, `suffixed_name`,
      `validate_catalog_name`, `derive_default_name`,
      `looks_like_remote_source`.
- [x] `src/catalog/registration.rs` — `gather_facts`, best-effort
      `register_repo` (suffix notice), silent read-first `touch_current_repo`,
      `note_repo_removed` (pre-delete tombstoning), unit-test ambient-write
      guard.
- [x] Wiring: `commands/clone.rs` (one call covering all layout paths),
      `init.rs`, `flow_adopt.rs`, `flow_eject.rs`,
      `core/worktree/remove_repo.rs`; lazy touch in
      checkout/exec/list/fetch/prune; `lib.rs` `pub mod catalog;`.
- [x] `.github/labeler.yml` — `area:catalog` glob for the new top-level module.

## Phase 2 — `daft repo` catalog verbs

_Commit: `feat(repo): add, info, and list verbs for the repo catalog`_

- [x] `src/commands/repo/{add,list,info}.rs` (argv `skip(3)` pattern;
      `add --name` renames, refuses collisions; structured emit on list/info).
- [x] Registration surfaces: `repo/mod.rs` dispatch + help,
      `suggest.rs::DAFT_REPO_SUBCOMMANDS`, `docs.rs` categories (both),
      `xtask/src/main.rs` COMMANDS + `get_command_for_name`, completions in
      bash/zsh/fish/fig, `repo-name` `__complete` arm, generated man +
      `docs/cli` pages.
- [x] Scenarios `tests/manual/scenarios/repo/catalog-*.yml`: basic (implicit
      clone registration, info by name/no-arg, suggestions), add-rename +
      name-taken, collision auto-suffix, remove tombstones, re-clone live-wins.

## Phase 3 — Cross-repo `daft go`

_Commit: `feat(go): cross-repo navigation through the repo catalog`_

- [x] `GoArgs` rework in `src/commands/checkout.rs`: optional first positional
      (`required_unless_present = "repo"`), ungated second positional, long-only
      `--repo`; `git-worktree-checkout` `Args` untouched.
- [x] `decode_go_grammar` + `GoRouting`/`CrossTarget`; twelve-case parse matrix
      unit tests including error copy.
- [x] `run_with_args` split into routing + `run_in_repo`; `go_to_repo`
      chdir-first flow; catalog fallback inside the `BranchNotFound` arm (beats
      autoStart, `--start` forces creation, one-hop rule); outside-repo
      catalog-only resolution; default-branch landing with best-effort
      write-back; `previous::save` records cross-repo hops so `daft go -` hops
      back.
- [x] Completions: catalog group appended in `complete_daft_go` (whole list
      outside a repo), `("daft-go", 2)` branch completion via
      `DAFT_COMPLETE_GO_FIRST` wired in bash/zsh/fish.
- [x] Scenarios: `checkout/go-cross-repo.yml`, `go-cross-repo-outside.yml`,
      `go-cross-repo-previous.yml`; full checkout/checkout-branch/completions
      suites re-run green.

## Phase 4 — Relations manifest + exec/start

_Commit:
`feat(graph): relations manifest, exec --repo/--all-repos/--related, start --with-related`_

- [x] `src/catalog/relations.rs` — `RelationEntry` (url/name/kind), pure
      `resolve_relations`, worktree-anchored `current_repo_relations`.
- [x] `relations:` field on `YamlConfig` (`src/hooks/yaml_config.rs`); both
      merge paths in `src/hooks/config_merge.rs` (two-way + `merge3`) handle it,
      with the full-config regression tests extended.
- [x] `exec` flags (`src/commands/exec.rs`): `--repo` (chdir + bare form targets
      the default-branch worktree), `--all-repos`, `--related` (current-branch
      set with skip notices); flat `ResolvedTarget` list into the unchanged
      pipeline; `ResolvedTarget.display` (`repo:branch`) with `label()` in both
      renderers; `DAFT_BRANCH_NAME` stays raw.
- [x] `start --with-related` (`checkout.rs`): `run_create_branch_core`
      extraction, upfront fatal resolution, per-repo creation from each repo's
      default-branch worktree (`find_representative_worktree`), non-interactive
      trust policy (hooks only on `Allow`), collected failures, final cd =
      current repo's new worktree.
- [x] Scenarios: `exec/cross-repo.yml`,
      `checkout-branch/start-with-related.yml`.

## Phase 5 — Fleet flags, jobs `--repo`, clone-by-name

_Commit:
`feat(fleet): catalog-aware list/fetch/prune/doctor, hooks jobs --repo, clone by name`_

- [x] `src/catalog/fleet.rs` — `for_each_repo` (headers, skip warnings,
      collected failures, optional current-repo-last ordering).
- [x] `list --repo/--all-repos` (blocking renderer forced),
      `fetch --repo/--all-repos` (fleet implies `--all`; bare `--repo` targets
      the default branch), `prune --repo/--all-repos` (sequential, current repo
      last), `doctor --all-repos` + always-on Catalog category
      (`src/doctor/catalog_checks.rs`: stale paths, daft-id drift, duplicate
      names; `--fix` + dry-run).
- [x] `hooks jobs --repo <name|path|uuid>` (`src/commands/hooks/jobs.rs`):
      catalog resolution incl. removed repos, implies all-worktrees, guarded to
      listing + one-shot logs.
- [x] Clone-by-name (`src/commands/clone.rs::resolve_clone_source`) with
      `looks_like_remote_source` bypass for URL/path inputs; `repo info` renders
      resolved relations.
- [x] `--repo` value completion (catalog names) across bash/zsh/fish via
      `command_has_repo_flag`.
- [x] Scenarios: `list/fleet-repos.yml`, `fetch/fleet-repos.yml`,
      `prune/fleet-repos.yml`, `doctor/catalog-checks.yml`,
      `hooks/jobs-removed-repo.yml`, `clone/by-catalog-name.yml`; full manual
      suite green (668 scenarios / 2652 steps).

## Phase 6 — Graph docs pillar + artifacts

- [x] This spec + plan pair under `docs/superpowers/`.
- [x] `docs/graph/{index,concepts,repo-catalog,coordinated-changes}.md`.
- [x] VitePress wiring: Graph nav entry + sidebar block + CLI sidebar entries;
      landing-page feature block and tagline; `why-daft.md` issue links → pillar
      links; comparison section; `_redirects` retarget;
      `hooks/yaml-reference.md` `relations:` section; two recipes; `SKILL.md`
      Graph section; `test-plans/repo-catalog.md`.
- [x] Generated surfaces current: `mise run man:gen`,
      `mise run     docs:cli:gen`; `mise run ci` parity.

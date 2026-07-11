---
branch: daft-357/feat/repo-catalog
---

# Repo Catalog & Graph Pillar

## Catalog basics

- [ ] Fresh clone registers automatically: `daft repo list` shows it with the
      right name, worktree count, path, and remote (one line per repo)
- [ ] From inside a repo, `daft repo list` marks it with a cyan `>`
- [ ] `daft repo list --sizes` in a terminal shimmer-loads the Size cells and
      settles into values plus a TOTAL row; piped output prints them directly
- [ ] `daft repo info` (no arg, inside the repo) shows the entry
- [ ] Second clone of the same remote elsewhere auto-suffixes (`x`, `x-2`) with
      a notice
- [ ] `daft repo add --name` renames; renaming to a taken name errors with a tip
- [ ] `daft repo list --format json` emits well-formed structured output
      including `worktrees` (and `size_bytes` with `--sizes`)

## Cross-repo go

- [ ] `daft go <repo>` from another repo lands in the target's default-branch
      worktree (shell cd works through the wrapper)
- [ ] `daft go <repo> <branch>` creates that branch's worktree over there
- [ ] `daft go -` afterwards hops back to the source worktree across repos
- [ ] A local branch named like a repo shadows it; `--repo` reaches the repo
- [ ] With `daft.go.autoStart=true`, a repo-name match opens the repo instead of
      creating a branch; `--start` still forces creation
- [ ] Outside any git repo: `daft go <repo>` works; unknown name keeps the "Not
      inside a Git repository" error
- [ ] Tab completion: repo names appear after branches; `daft go <repo> <Tab>`
      completes the target's branches

## Relations & coordinated changes

- [ ] `relations:` in daft.yml resolves in `daft repo info` (cloned → path;
      uncloned → `daft clone <url>` hint)
- [ ] `daft start <branch> --with-related` creates the branch in every related
      repo; missing clone aborts before creating anything
- [ ] Hooks in an untrusted related repo are skipped with a notice
- [ ] `daft exec --related` runs across the current branch's worktrees and skips
      repos lacking it (notice printed)

## Fleet commands

- [ ] `daft list --all-repos` from outside any repo shows per-repo sections
- [ ] `daft update --all-repos` and `daft prune --all-repos` sweep cleanly
      (prune leaves cwd semantics intact)
- [ ] `daft exec --all-repos -- <cmd>` runs in every default-branch worktree
- [ ] `daft doctor` shows the Catalog category; after deleting a repo behind
      daft's back, doctor warns and `--fix` tombstones the entry

## Removal & restore

- [ ] `daft repo remove` marks the entry removed (`repo list --all`)
- [ ] `daft hooks jobs --repo <name>` still lists the removed repo's history
- [ ] `daft clone <name>` restores the repo from its recorded remote

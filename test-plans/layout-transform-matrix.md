---
branch: feat/progressive-adoption
---

# Layout Transform — Full Scenario Matrix

## Layouts Under Test

| ID  | Layout            | Template                                         | Bare | Structure                                                  |
| --- | ----------------- | ------------------------------------------------ | ---- | ---------------------------------------------------------- |
| C   | contained         | `{{ repo_path }}/{{ branch }}`                   | Yes  | `repo/.git` (bare), `repo/main/`, `repo/develop/`          |
| CC  | contained-classic | `{{ repo_path }}/{{ branch \| repo }}`           | No   | `repo/main/.git`, `repo/develop/`                          |
| CF  | contained-flat    | `{{ repo_path }}/{{ branch \| sanitize }}`       | Yes  | `repo/.git` (bare), `repo/main/`, `repo/feature-test/`     |
| S   | sibling           | `{{ repo }}.{{ branch \| sanitize }}`            | No   | `repo/.git`, `repo.develop/`                               |
| N   | nested            | `{{ repo }}/.worktrees/{{ branch \| sanitize }}` | No   | `repo/.git`, `repo/.worktrees/develop/`                    |
| Z   | centralized       | `{{ daft_data_dir }}/worktrees/...`              | No   | `repo/.git`, `~/.local/share/daft/worktrees/repo/develop/` |

## 1. Pairwise Transform Matrix (30 scenarios)

Each scenario:

1. Clone with source layout
2. Checkout a second branch (develop) to have a linked worktree
3. Transform to target layout
4. Validate:
   - `.git` location matches target expectation
   - `core.bare` matches target expectation
   - Default branch worktree exists at expected path with correct branch
   - Non-default worktree exists at expected path with correct branch
   - `git status` is clean in all worktrees (no index corruption)
   - Working files (README.md) are present in all worktrees

### Matrix

| #   | From → To | Bare change     | .git moves                 | Default branch handling                      |
| --- | --------- | --------------- | -------------------------- | -------------------------------------------- |
| 1   | C → CC    | bare→nonbare    | repo/.git → repo/main/.git | stays in subdir                              |
| 2   | C → CF    | bare→bare       | stays                      | worktrees rename (slashes→dashes)            |
| 3   | C → S     | bare→nonbare    | stays                      | collapse into root                           |
| 4   | C → N     | bare→nonbare    | stays                      | collapse into root, worktrees→.worktrees/    |
| 5   | C → Z     | bare→nonbare    | stays                      | collapse into root, worktrees→XDG            |
| 6   | CC → C    | nonbare→bare    | repo/main/.git → repo/.git | stays in subdir                              |
| 7   | CC → CF   | nonbare→bare    | repo/main/.git → repo/.git | stays in subdir                              |
| 8   | CC → S    | nonbare→nonbare | repo/main/.git → repo/.git | collapse into root                           |
| 9   | CC → N    | nonbare→nonbare | repo/main/.git → repo/.git | collapse into root                           |
| 10  | CC → Z    | nonbare→nonbare | repo/main/.git → repo/.git | collapse into root                           |
| 11  | CF → C    | bare→bare       | stays                      | worktrees rename (dashes→slashes)            |
| 12  | CF → CC   | bare→nonbare    | repo/.git → repo/main/.git | stays in subdir                              |
| 13  | CF → S    | bare→nonbare    | stays                      | collapse into root                           |
| 14  | CF → N    | bare→nonbare    | stays                      | collapse into root                           |
| 15  | CF → Z    | bare→nonbare    | stays                      | collapse into root                           |
| 16  | S → C     | nonbare→bare    | stays                      | nest from root                               |
| 17  | S → CC    | nonbare→nonbare | repo/.git → repo/main/.git | nest from root                               |
| 18  | S → CF    | nonbare→bare    | stays                      | nest from root                               |
| 19  | S → N     | nonbare→nonbare | stays                      | worktrees→.worktrees/                        |
| 20  | S → Z     | nonbare→nonbare | stays                      | worktrees→XDG                                |
| 21  | N → C     | nonbare→bare    | stays                      | nest from root, worktrees out of .worktrees/ |
| 22  | N → CC    | nonbare→nonbare | repo/.git → repo/main/.git | nest from root                               |
| 23  | N → CF    | nonbare→bare    | stays                      | nest from root                               |
| 24  | N → S     | nonbare→nonbare | stays                      | worktrees out of .worktrees/                 |
| 25  | N → Z     | nonbare→nonbare | stays                      | worktrees→XDG                                |
| 26  | Z → C     | nonbare→bare    | stays                      | nest from root, worktrees from XDG           |
| 27  | Z → CC    | nonbare→nonbare | repo/.git → repo/main/.git | nest from root                               |
| 28  | Z → CF    | nonbare→bare    | stays                      | nest from root                               |
| 29  | Z → S     | nonbare→nonbare | stays                      | worktrees from XDG                           |
| 30  | Z → N     | nonbare→nonbare | stays                      | worktrees from XDG to .worktrees/            |

### Validation per scenario

For EACH of the 30 scenarios, verify:

- [ ] Exit code 0
- [ ] `.git` is at the expected location (dir for bare, dir for non-bare, file
      for linked worktrees)
- [ ] `core.bare` has the expected value
- [ ] Default branch worktree exists at expected path
- [ ] Default branch `git branch --show-current` returns correct branch
- [ ] Non-default worktree (develop) exists at expected path
- [ ] Non-default worktree `git branch --show-current` returns correct branch
- [ ] `git status --porcelain` is empty in default branch worktree
- [ ] `git status --porcelain` is empty in non-default worktree
- [ ] Working files (README.md) present in both worktrees

## 2. Same-Layout No-Op (6 scenarios)

Each of the 6 layouts → itself. Verify:

- [ ] Exit code 0
- [ ] No files moved
- [ ] All worktrees remain at original paths
- [ ] `git status` clean

## 3. Dirty State Handling (4 scenarios)

### 3a. Dirty default branch — abort

- Clone sibling, modify README.md (don't commit), transform to contained
- [ ] Exit code non-zero
- [ ] Error message mentions uncommitted changes
- [ ] No files moved, repo unchanged

### 3b. Dirty default branch — --force preserves changes

- Clone sibling, modify README.md (don't commit), transform to contained --force
- [ ] Exit code 0
- [ ] Transform completes
- [ ] Modified README.md content preserved in new worktree location
- [ ] `git diff` shows the same modification after transform

### 3c. Dirty linked worktree — --force preserves changes

- Clone contained, checkout develop, modify file in develop, transform to
  sibling --force
- [ ] Exit code 0
- [ ] Modified file preserved in relocated develop worktree

### 3d. Untracked files preserved

- Clone contained, add untracked file in worktree, transform to sibling --force
- [ ] Untracked file present at new location after transform

## 4. Non-Conforming Worktrees (4 scenarios)

### 4a. Non-conforming worktree skipped by default

- Clone contained, checkout develop normally, create experiment at custom path
  via `--at ~/custom/path`
- Transform to sibling
- [ ] develop moves to sibling position
- [ ] experiment stays at custom path (not relocated)
- [ ] Output mentions skipped worktree

### 4b. --include relocates specific non-conforming worktree

- Same setup as 4a
- Transform to sibling --include experiment
- [ ] Both develop and experiment move to sibling positions

### 4c. --include-all relocates all non-conforming worktrees

- Clone contained, create two worktrees at custom paths
- Transform to sibling --include-all
- [ ] All worktrees move to sibling positions

### 4d. Non-conforming worktree shown in dry-run

- Same setup as 4a
- Transform to sibling --dry-run
- [ ] Plan output mentions experiment as non-conforming
- [ ] Suggests --include

## 5. Dry Run (2 scenarios)

### 5a. Dry run shows plan without changes

- Clone contained, checkout develop
- Transform to sibling --dry-run
- [ ] Output contains "Transform plan"
- [ ] Output contains operation descriptions
- [ ] `.git` still at contained position (no changes made)
- [ ] Worktrees unchanged

### 5b. Dry run for no-op

- Clone sibling, transform to sibling --dry-run
- [ ] Output indicates no changes needed

## 6. Edge Cases (4 scenarios)

### 6a. Single worktree only (no linked worktrees)

- Clone contained (only default branch)
- Transform to sibling
- [ ] Default branch collapses to root
- [ ] Clean state

### 6b. Many worktrees (5+)

- Clone contained, create 4 additional worktrees
- Transform to sibling
- [ ] All 5 worktrees at correct sibling positions

### 6c. Branch with slashes (feature/auth)

- Clone contained, checkout feature/test-feature
- Transform to contained-flat
- [ ] feature/test-feature worktree at repo/feature-test-feature (sanitized)

### 6d. Round trip (A→B→A clean)

- Clone contained, checkout develop
- Transform to sibling
- Transform back to contained
- [ ] Structure matches original
- [ ] `git status` clean in all worktrees
- [ ] No stale .gitignore or artifacts

## Total: 50 scenarios

| Category                 | Count  |
| ------------------------ | ------ |
| Pairwise transforms      | 30     |
| Same-layout no-ops       | 6      |
| Dirty state              | 4      |
| Non-conforming worktrees | 4      |
| Dry run                  | 2      |
| Edge cases               | 4      |
| **Total**                | **50** |

# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

## [1.0.30](https://github.com/avihut/daft/compare/v1.0.29...v1.0.30) - 2026-02-21

### Bug Fixes

- *(ci)* prevent test workflow from running twice on release PRs
- *(bench)* missing symlink, jq overflow, and worktree cleanup ([#247](https://github.com/avihut/daft/pull/247))
- *(bench)* add target/release to PATH so benchmarks find daft ([#246](https://github.com/avihut/daft/pull/246))
- use \$SHELL -i for exec commands to support aliases and shell functions ([#242](https://github.com/avihut/daft/pull/242))

### Features

- *(hooks)* add job descriptions, skip descriptions, and OS/arch conditions ([#248](https://github.com/avihut/daft/pull/248))
- *(hooks)* add manual hook execution with `hooks run` command ([#245](https://github.com/avihut/daft/pull/245))

### Miscellaneous

- add benchmark suite with dashboard ([#244](https://github.com/avihut/daft/pull/244))

### Refactoring

- separate core logic from UI layer ([#249](https://github.com/avihut/daft/pull/249))

## [1.0.29](https://github.com/avihut/daft/compare/v1.0.28...v1.0.29) - 2026-02-19

### Bug Fixes

- prevent prune from deleting worktrees with uncommitted changes or untracked files ([#240](https://github.com/avihut/daft/pull/240))
- grow hook output window dynamically instead of pre-allocating blank lines ([#237](https://github.com/avihut/daft/pull/237))
- check remote tracking branch when validating merge status for deletion ([#236](https://github.com/avihut/daft/pull/236))
- doctor --fix now creates missing shortcuts and --dry-run shows concrete actions ([#232](https://github.com/avihut/daft/pull/232))
- cd to safe directory when branch-delete removes current worktree ([#234](https://github.com/avihut/daft/pull/234))
- run post-clone hook before worktree-post-create during clone ([#233](https://github.com/avihut/daft/pull/233))
- carry changes from base branch worktree instead of current worktree ([#231](https://github.com/avihut/daft/pull/231))

### Features

- accept worktree paths as arguments in branch-delete ([#239](https://github.com/avihut/daft/pull/239))
- add -x/--exec option to run commands in worktree after setup ([#238](https://github.com/avihut/daft/pull/238))

### Miscellaneous

- removed plans
- removed unused WARP.md
- reorganize mise tasks into file-based structure with : hierarchy ([#235](https://github.com/avihut/daft/pull/235))
- simplified daft hooks

## [1.0.28](https://github.com/avihut/daft/compare/v1.0.27...v1.0.28) - 2026-02-18

### Bug Fixes

- skip pre-push hooks for structural push operations ([#228](https://github.com/avihut/daft/pull/228))

### Features

- add Linux distribution pipeline with shell installer ([#229](https://github.com/avihut/daft/pull/229))

## [1.0.27](https://github.com/avihut/daft/compare/v1.0.26...v1.0.27) - 2026-02-18

### Bug Fixes

- enable CD after worktree-branch-delete removes current worktree ([#224](https://github.com/avihut/daft/pull/224))
- render hook job output after heading, not before ([#223](https://github.com/avihut/daft/pull/223))

### Features

- *(doctor)* improve output format, fix bugs, and add --dry-run ([#222](https://github.com/avihut/daft/pull/222))

## [1.0.26](https://github.com/avihut/daft/compare/v1.0.25...v1.0.26) - 2026-02-17

### Bug Fixes

- use temp file for shell wrapper CD communication ([#219](https://github.com/avihut/daft/pull/219))

### Features

- add hook execution progress display with spinners ([#217](https://github.com/avihut/daft/pull/217))

## [1.0.25](https://github.com/avihut/daft/compare/v1.0.24...v1.0.25) - 2026-02-16

### Bug Fixes

- *(fetch)* check target worktree for dirty state instead of current worktree ([#213](https://github.com/avihut/daft/pull/213))
- *(docs)* align changelog rendering for new release-plz format ([#210](https://github.com/avihut/daft/pull/210))

### Features

- *(output)* add colored output to CliOutput and fetch/prune commands ([#215](https://github.com/avihut/daft/pull/215))
- *(shortcuts)* make default-branch shortcuts shell-only functions ([#214](https://github.com/avihut/daft/pull/214))
- add git-worktree-branch-delete command ([#211](https://github.com/avihut/daft/pull/211))

## [1.0.24](https://github.com/avihut/daft/compare/v1.0.23...v1.0.24) - 2026-02-15

### Bug Fixes

- prevent hook commands from hanging on long-running tasks ([#205](https://github.com/avihut/daft/pull/205))

### Refactoring

- absorb checkout-branch-from-default into --from-default flag ([#206](https://github.com/avihut/daft/pull/206))

### Testing

- add gitoxide integration test coverage and move matrix to Rust xtask ([#208](https://github.com/avihut/daft/pull/208))

## [1.0.23](https://github.com/avihut/daft/compare/v1.0.22...v1.0.23) - 2026-02-14

### Miscellaneous

- add Prettier formatting with Bun for docs and YAML
- remove unused install.sh script ([#192](https://github.com/avihut/daft/pull/192))
- improve daft.yml hooks for full worktree automation ([#190](https://github.com/avihut/daft/pull/190))


### Miscellaneous

- Filter CI, deps, docs, and style commits from changelog
- Add Prettier formatting with Bun for docs and YAML
- Remove unused install.sh script (#192)
- Improve daft.yml hooks for full worktree automation (#190)
## [1.0.22] - 2026-02-07


### Bug Fixes

- **clone**: Fall back to git CLI for remote ops when no local repo exists (#184)
## [1.0.21] - 2026-02-07


### Features

- Add lefthook git hooks for local code quality checks (#183)
- Migrate from just to mise task runner (#182)
- **hooks**: Add YAML hooks system (#181)
- Add `daft doctor` diagnostic command (#169)
- **update-check**: Throttle new version notification to once per 24 hours (#168)
- **shell-init**: Add daft() shell wrapper for cd into worktrees (#165)
## [1.0.20] - 2026-02-04


### Features

- Add experimental gitoxide backend for git operations (#161)


### Refactoring

- **completions**: Replace Fig spec string concatenation with serde_json (#159)
## [1.0.19] - 2026-02-03


### Bug Fixes

- Correctly detect bare repo root to suppress carry warning


### Features

- **fetch**: Emit git-fetch-like output with real-time progress (#158)
- Add --no-cd flag to worktree-creating commands
- **prune**: Add streaming per-branch output in git-remote-prune style
- Handle pruning when inside the worktree being removed (#148)
- Add background update check with new version notifications (#143)
## [1.0.18] - 2026-02-02


### Bug Fixes

- Include shell completions in shell-init and add Fig/Amazon Q support (#142)
- Skip uncommitted changes check when not in a work tree (#141)
- Display full branch path for worktrees with slashes (#139)
- Clean up empty parent directories after pruning worktrees (#137)
## [1.0.17] - 2026-02-01


### Bug Fixes

- Handle '+' marker in git branch -vv for linked worktree branches (#136)


### Features

- Rename worktree hooks with deprecation support (#131)
## [1.0.16] - 2026-01-31


### Bug Fixes

- Show error message and suggestions for unknown subcommands (#121)
- Add retry logic for release PR detection in man page update (#117)


### Features

- Migrate Homebrew tap from avihut/homebrew-daft to avihut/homebrew-tap (#126)


### Refactoring

- Unify git-daft and daft command routing (#123)
## [1.0.15] - 2026-01-28


### Bug Fixes

- Sync man page versions with release workflow (#115)
## [1.0.14] - 2026-01-28


### Bug Fixes

- Install man pages before pkgshare.install in Homebrew formula (#113)
## [1.0.13] - 2026-01-28


### Bug Fixes

- Include man pages in binary distribution archives (#111)
## [1.0.12] - 2026-01-28


### Bug Fixes

- Skip man page verification on release PRs
- Try disabling git_only for xtask
- Use package-specific tag format for xtask
- Disable release-plz for all packages except daft
- Exclude xtask from release-plz processing
- Resolve YAML syntax error in release workflow
- Correct git-worktree-clone syntax in test-homebrew workflow
- Move man page generation to install block in Homebrew formula


### Refactoring

- Migrate man page generation to cargo xtask (#109)
## [1.0.11] - 2026-01-28


### Bug Fixes

- Simplify man page installation in Homebrew formula
- Ensure test-homebrew tests the correct release version
- Add man page installation to Homebrew formula
- Prevent duplicate release creation in workflow
- Make pager dependency Unix-only for Windows compatibility
- Resolve hooks trust mechanism issues (#101)


### Features

- Add daft release-notes command (#100)
- Improve help system with git-like format and dynamic generation (#98)
## [1.0.7] - 2026-01-24


### Bug Fixes

- Remove conflicting prerelease tags before changelog generation
## [1.0.4] - 2026-01-24


### Features

- Add one-time shell integration hint for new users (#81)
## [1.0.2] - 2026-01-24


### Bug Fixes

- Enable CD behavior for all shortcut styles (#80)
## [1.0.1] - 2026-01-24


### Bug Fixes

- Resolve v1.0.0 release issues (#78)
## [1.0.0] - 2026-01-24


### Bug Fixes

- Checkout existing branch no longer results in detached HEAD (#58)


### Features

- Support cloning empty repositories (#74)
- Add git-worktree-flow-adopt and git-worktree-flow-eject commands (#73)
- Add multi-remote workflow support (#71)
- Add multi-style command shortcuts system (#4) (#69)
- Add git-worktree-fetch command to update worktree branches (#66)
- Add man page generation using clap_mangen (#63)
- **hooks**: Add flexible project-managed hooks system (#62)
- **init**: Respect git config init.defaultBranch setting (#61)
- Add git config-based configuration system (#60)
- **clone**: Add -b/--branch option to specify checkout branch (#59)
- Git-like terminal logging (#55)
- Add IO abstraction layer for future TUI support (#54)
- Worktree-checkout to branch with existing worktree just CDs to that worktree (#51)
- Shell integration for automatic cd into new worktrees (#46)
- Carry uncommitted (dirty) state between worktrees (#44)


### Miscellaneous

- Add VHS demo recording tape
- Migrate from Makefile to justfile (#50)
- Remove legacy shell scripts (#48)
## [0.3.2] - 2026-01-11


### Bug Fixes

- Correct heredoc syntax in workflow YAML files
## [0.3.1] - 2026-01-11


### Bug Fixes

- Use gh CLI for git-cliff installation


### Features

- Integrate git-cliff for automated changelog generation (#38)
## [0.3.0] - 2026-01-10


### Features

- Setup develop branch with canary releases (v0.2.0)
## [0.2.2] - 2026-01-10


### Bug Fixes

- Exclude prerelease tags when finding latest version (#37)
## [0.2.1] - 2026-01-10


### Bug Fixes

- Add GitHub release creation to canary and beta workflows (#36)
## [0.1.28] - 2026-01-10


### Miscellaneous

- Add developer-tools keyword to Cargo.toml (#35)
- Use wheatley bot for Homebrew formula commits [skip ci]
## [0.1.27] - 2026-01-10


### Features

- Migrate Homebrew formula to separate tap repository
## [0.1.26] - 2026-01-10


### Miscellaneous

- Test release workflow
## [0.1.22] - 2026-01-10


### Miscellaneous

- Trigger v0.1.22 release
## [0.1.21] - 2026-01-10


### Feature

- Root Level `.git` Dir (#2)


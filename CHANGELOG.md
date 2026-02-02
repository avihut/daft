# Changelog

All notable changes to this project will be documented in this file.

## [1.0.18](https://github.com/avihut/daft/compare/v1.0.17...v1.0.18) - 2026-02-02

### Bug Fixes

- include shell completions in shell-init and add Fig/Amazon Q support ([#142](https://github.com/avihut/daft/pull/142))
- skip uncommitted changes check when not in a work tree ([#141](https://github.com/avihut/daft/pull/141))
- display full branch path for worktrees with slashes ([#139](https://github.com/avihut/daft/pull/139))
- clean up empty parent directories after pruning worktrees ([#137](https://github.com/avihut/daft/pull/137))

## [1.0.17](https://github.com/avihut/daft/compare/v1.0.16...v1.0.17) - 2026-02-01

### Bug Fixes

- handle '+' marker in git branch -vv for linked worktree branches

### Features

- rename worktree hooks with deprecation support ([#131](https://github.com/avihut/daft/pull/131))

## [1.0.16](https://github.com/avihut/daft/compare/v1.0.15...v1.0.16) - 2026-01-31

### Bug Fixes

- show error message and suggestions for unknown subcommands ([#121](https://github.com/avihut/daft/pull/121))
- add retry logic for release PR detection in man page update ([#117](https://github.com/avihut/daft/pull/117))

### Features

- migrate Homebrew tap from avihut/homebrew-daft to avihut/homebrew-tap ([#126](https://github.com/avihut/daft/pull/126))

### Refactoring

- unify git-daft and daft command routing ([#123](https://github.com/avihut/daft/pull/123))

## [1.0.15](https://github.com/avihut/daft/compare/v1.0.14...v1.0.15) - 2026-01-28

### Bug Fixes

- sync man page versions with release workflow ([#115](https://github.com/avihut/daft/pull/115))

## [1.0.14](https://github.com/avihut/daft/compare/v1.0.13...v1.0.14) - 2026-01-28

### Bug Fixes

- install man pages before pkgshare.install in Homebrew formula ([#113](https://github.com/avihut/daft/pull/113))

## [1.0.13](https://github.com/avihut/daft/compare/v1.0.12...v1.0.13) - 2026-01-28

### Bug Fixes

- include man pages in binary distribution archives ([#111](https://github.com/avihut/daft/pull/111))

## [1.0.12](https://github.com/avihut/daft/compare/v1.0.11...v1.0.12) - 2026-01-28

### Bug Fixes

- skip man page verification on release PRs
- try disabling git_only for xtask
- use package-specific tag format for xtask
- disable release-plz for all packages except daft
- exclude xtask from release-plz processing
- resolve YAML syntax error in release workflow
- correct git-worktree-clone syntax in test-homebrew workflow
- move man page generation to install block in Homebrew formula

### Documentation

- dedupe changelog entries for v1.0.11

### Refactoring

- migrate man page generation to cargo xtask ([#109](https://github.com/avihut/daft/pull/109))

## [1.0.11](https://github.com/avihut/daft/compare/v1.0.10...v1.0.11) - 2026-01-28

### Bug Fixes

- simplify man page installation in Homebrew formula
- prevent duplicate release creation in workflow

### CI/CD

- remove [skip ci] from formula commit to trigger tap workflow

## [1.0.10](https://github.com/avihut/daft/compare/v1.0.9...v1.0.10) - 2026-01-28

### Bug Fixes

- ensure test-homebrew tests the correct release version

### CI/CD

- trigger test-homebrew via repository_dispatch from tap
- auto-label release PRs with release-plz

## [1.0.9](https://github.com/avihut/daft/compare/v1.0.8...v1.0.9) - 2026-01-28

### Bug Fixes

- add man page installation to Homebrew formula

## [1.0.8](https://github.com/avihut/daft/compare/v1.0.7...v1.0.8) - 2026-01-28

### Bug Fixes

- make pager dependency Unix-only for Windows compatibility
- resolve hooks trust mechanism issues ([#101](https://github.com/avihut/daft/pull/101))

### CI/CD

- migrate from release-please to release-plz ([#99](https://github.com/avihut/daft/pull/99))
- remove redundant push trigger from test workflow ([#93](https://github.com/avihut/daft/pull/93))
- *(deps)* bump extractions/setup-just in the github-actions group ([#92](https://github.com/avihut/daft/pull/92))
- simplify release workflow with release-please ([#91](https://github.com/avihut/daft/pull/91))

### Features

- add daft release-notes command ([#100](https://github.com/avihut/daft/pull/100))
- improve help system with git-like format and dynamic generation ([#98](https://github.com/avihut/daft/pull/98))

## [1.0.7] - 2026-01-24


### Bug Fixes

- Remove conflicting prerelease tags before changelog generation
## [1.0.6] - 2026-01-24


### Documentation

- Add missing v1.0.0 changelog entry
## [1.0.5] - 2026-01-24


### Documentation

- Add adopt/eject documentation to README
## [1.0.4] - 2026-01-24


### Features

- Add one-time shell integration hint for new users (#81)
## [1.0.3] - 2026-01-24


### CI/CD

- Add post-install caveats to Homebrew formula
## [1.0.2] - 2026-01-24


### Bug Fixes

- Enable CD behavior for all shortcut styles (#80)
## [1.0.1] - 2026-01-24


### Bug Fixes

- Resolve v1.0.0 release issues (#78)

## [1.0.0] - 2026-01-24

First stable release of daft - a comprehensive Git extensions toolkit that enhances developer workflows with powerful worktree management.

### Features

- Shell integration for automatic cd into new worktrees (#46)
- Carry uncommitted (dirty) state between worktrees (#44)
- Worktree-checkout to branch with existing worktree just CDs to that worktree (#51)
- Add IO abstraction layer for future TUI support (#54)
- Git-like terminal logging (#55)
- **clone**: Add -b/--branch option to specify checkout branch (#59)
- Add git config-based configuration system (#60)
- **init**: Respect git config init.defaultBranch setting (#61)
- **hooks**: Add flexible project-managed hooks system (#62)
- Add man page generation using clap_mangen (#63)
- Add git-worktree-fetch command to update worktree branches (#66)
- Add multi-style command shortcuts system (#69)
- Add multi-remote workflow support (#71)
- Add git-worktree-flow-adopt and git-worktree-flow-eject commands (#73)
- Support cloning empty repositories (#74)

### Bug Fixes

- Checkout existing branch no longer results in detached HEAD (#58)

### CI/CD

- Add Homebrew integration tests (#70)

### Documentation

- Add badges to README
- Rewrite README intro with user-focused content
- Add before/after workflow diagram to README
- Condense README from 796 to 166 lines
- Add SECURITY.md and CODE_OF_CONDUCT.md
- Add demo GIF to README
- Add docs badge linking to avihu.dev/daft

### Miscellaneous

- Migrate from Makefile to justfile (#50)
- Remove legacy shell scripts (#48)

## [0.3.3] - 2026-01-17


### CI/CD

- **deps**: Bump the github-actions group with 4 updates (#39)
## [0.3.2] - 2026-01-11


### Bug Fixes

- Correct heredoc syntax in workflow YAML files
## [0.3.1] - 2026-01-11


### Bug Fixes

- Use gh CLI for git-cliff installation


### Features

- Integrate git-cliff for automated changelog generation (#38)
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


### CI/CD

- Add [skip ci] to homebrew formula commit


### Miscellaneous

- Test release workflow
## [0.1.24] - 2026-01-10


### CI/CD

- Use macos-15 runners, remove tag push trigger
## [0.1.23] - 2026-01-10


### CI/CD

- Allow-dirty for custom workflow modifications
- Add workflow_dispatch to release, trigger from bump-version
## [0.1.22] - 2026-01-10


### CI/CD

- Add [skip ci] to prevent workflow loop, remove push trigger from tests


### Miscellaneous

- Trigger v0.1.22 release
## [0.1.21] - 2026-01-10


### CI/CD

- **deps**: Bump the github-actions group across 1 directory with 3 updates (#31)
- **deps**: Bump actions/upload-artifact in the github-actions group (#29)
- **deps**: Bump the github-actions group with 2 updates (#26)


### Dependencies

- **deps**: Bump the cargo-dependencies group with 2 updates (#27)


### Feature

- Root Level `.git` Dir (#2)


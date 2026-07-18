# Changelog

All notable changes to this project will be documented in this file.

## [1.22.0](https://github.com/avihut/daft/compare/v1.21.1...v1.22.0) - 2026-07-18


### Features

- **repo**: Daft repo link/unlink — imperative relation editing (#724)


### Testing

- Finish integration-suite state isolation — completions sandbox + wider tripwire (#730)
## [1.21.1](https://github.com/avihut/daft/compare/v1.21.0...v1.21.1) - 2026-07-17


### Bug Fixes

- **output**: Defer hook-path warnings while a live region owns stderr (#721)
## [1.21.0](https://github.com/avihut/daft/compare/v1.20.0...v1.21.0) - 2026-07-17


### Features

- Native agent-skill install and doctor freshness check (#707)


### Miscellaneous

- Bump MSRV 1.94 → 1.95 (#713)
## [1.20.0](https://github.com/avihut/daft/compare/v1.19.0...v1.20.0) - 2026-07-15


### Bug Fixes

- Atomic, lock-serialized trust/repo registry writes (#703)


### Features

- **list**: Fast, shared, stale-then-refresh size walk for --columns +size (#706)
- **sync**: Dynamic resource governor for parallel pre-push hooks (#702)
- **exec**: Render multi-worktree runs on the plan-then-execute rail (#704)


### Testing

- Real-state tripwire + fix repo-catalog/state test-isolation leaks (#698)
## [1.19.0](https://github.com/avihut/daft/compare/v1.18.1...v1.19.0) - 2026-07-12


### Features

- Repo catalog and the Graph pillar (#696)
- **output**: Plan-then-execute rail timeline for worktree lifecycle commands (#688)
## [1.18.1](https://github.com/avihut/daft/compare/v1.18.0...v1.18.1) - 2026-07-10


### Bug Fixes

- **sync**: Graceful two-stage Ctrl+C cancellation; harden alias capture against job-control stops (#672)
- **push**: Skip pre-push hook on ref-only upstream pushes (#686)
- **deps**: Bump crossbeam-epoch to 0.9.20 (RUSTSEC-2026-0204) (#687)
- Detect multi-commit squash merges in the gone-branch merge verifier (#673)
- **list**: Collect only the fields the live view renders or sorts by (#675)
- Decouple daft_dev_build from release-tag proximity (#670)
## [1.18.0](https://github.com/avihut/daft/compare/v1.17.0...v1.18.0) - 2026-07-05


### Features

- **hooks**: Contextual per-command notices when untrusted repos skip hooks, with trust + replay suggestions (#654)
## [1.17.0](https://github.com/avihut/daft/compare/v1.16.0...v1.17.0) - 2026-06-20


### Bug Fixes

- Provenance-gated visitor daft-file propagation (#629)


### Features

- **push**: Honor and report pre-push hooks at every daft push site (#630)
## [1.16.0](https://github.com/avihut/daft/compare/v1.15.2...v1.16.0) - 2026-06-02


### Features

- DAG-aware --skip-hooks flag for worktree hooks (replaces --no-hooks) (#617)
## [1.15.2](https://github.com/avihut/daft/compare/v1.15.1...v1.15.2) - 2026-06-01


### Bug Fixes

- **ci**: Ensure release build toolchain meets MSRV on every runner (#614)
## [1.15.1](https://github.com/avihut/daft/compare/v1.15.0...v1.15.1) - 2026-06-01


### Bug Fixes

- **ci**: Avoid broken pipe in MSRV staleness version fetch (#607)


### Miscellaneous

- Bump MSRV 1.93 → 1.94 (#610)
## [1.15.0](https://github.com/avihut/daft/compare/v1.14.5...v1.15.0) - 2026-06-01


### Bug Fixes

- **list**: Honor daft.remote for default-branch resolution in blocking list path (#598)


### Features

- Visitor configuration — adopt daft without committing daft.yml (#605)
- **test-runner**: Redesign the live progress region with a braille completion-map (#580)
- **test-runner**: Opt-in RAM-disk sandbox for the YAML test suite (#574)


### Miscellaneous

- Harden fresh-clone mise bootstrap (#602)


### Performance

- **test-runner**: Skip dead template snapshot in non-interactive runs (#593)
- Collapse redundant gix::discover() per command (#592)
- **test-runner**: Parallelize fixture-cache priming (#578)
- **test-runner**: Share daft binary across worktrees (#576)
- **test-runner**: Share fixture remotes across scenarios (#575)
- **test-runner**: Exec daft directly when no shell features are needed (#570)


### Refactoring

- **test-runner**: Gate startup background work on DAFT_TESTING (#573)


### Testing

- **test-runner**: Dedupe inline repo specs into fixtures, fix parallel-prime collision (#594)
## [1.14.5](https://github.com/avihut/daft/compare/v1.14.4...v1.14.5) - 2026-05-27


### Bug Fixes

- **sync**: Refresh remote ahead/behind after Update task (#568)
- **deps**: Bump shlex constraint to 2 and prevent grouped major-bump drift (#569)


### Performance

- **test-runner**: Interleave scenarios across workers to flatten the parallel tail (#564)
- **test-runner**: Use APFS clonefile / Linux FICLONE for sandbox template snapshots (#562)
## [1.14.4](https://github.com/avihut/daft/compare/v1.14.3...v1.14.4) - 2026-05-26


### Bug Fixes

- **hooks**: Support background jobs depending on foreground jobs (#558)


### Features

- **test-runner**: Default to automatic; add --interactive (-i) for step-through (#557)
- **test-runner**: Test runner output improvements (#518) (#551)
## [1.14.3](https://github.com/avihut/daft/compare/v1.14.2...v1.14.3) - 2026-05-22


### Bug Fixes

- **ci**: Allow squash-merge ' (#nnn)' suffix in tag-merged-release regex (#549)
## [1.14.2](https://github.com/avihut/daft/compare/v1.14.1...v1.14.2) - 2026-05-22


### Bug Fixes

- **ci**: Allow cargo-release to run from release-pr branch in workflow (#544)
- **ci**: Install Node 24 in docs deploy to satisfy wrangler 4 (#541)


### Miscellaneous

- Ignore .understand-anything/ knowledge graph artifacts
## [1.14.1](https://github.com/avihut/daft/compare/v1.14.0...v1.14.1) - 2026-05-21


### Features

- **exec**: Add --show-output to dump captured output for successful worktrees (#530)
## [1.14.0](https://github.com/avihut/daft/compare/v1.13.0...v1.14.0) - 2026-05-18


### Features

- Add top-level -C <path> flag (git -C semantics) (#523)


### Miscellaneous

- **release-plz**: Give term-styles its own tag namespace (#528)


### Performance

- **test-runner**: Parallelize YAML scenario execution (#515)


### Refactoring

- **test-runner**: Introduce CommandExecutor port at runner/daft seam (#521)
## [1.13.0](https://github.com/avihut/daft/compare/v1.12.0...v1.13.0) - 2026-05-17


### Bug Fixes

- **tui**: Keep size column TOTAL summary intact when wider than data cells (#507)
- **tui**: Truncate worktree names in shared picker to keep state column visible (#506)
- **tui**: Always render selected columns; stop dropping by terminal width (#497)


### Features

- **coordinator**: Redesign internals — persistence, IPC, structured logs (#476) (#508)


### Miscellaneous

- Enforce dependency license allowlist via cargo-deny (#499)
- Reduce test.yml flake — pin mise tools, skip cargo-cooldown on CI (#498)
## [1.12.0](https://github.com/avihut/daft/compare/v1.11.1...v1.12.0) - 2026-05-09


### Bug Fixes

- Prevent panic on Shift+Tab in shared manage editor (#479)
- **hooks**: Stop yaml_executor unit tests from leaking into ~/.local/state/daft (#480)


### Features

- Redesign merge with rebase styles and PR-style hook gates (#471)


### Miscellaneous

- Rust ecosystem alignment — edition 2024, MSRV policy, forbid unsafe, dual-license (#481)
## [1.11.1](https://github.com/avihut/daft/compare/v1.11.0...v1.11.1) - 2026-05-02


### Bug Fixes

- **coordinator**: Honor needs: between background jobs (#454) (#463)
- **list**: Render empty-state hint when no worktrees, plus repo remove copy refresh (#462)
- **hooks**: Honor user git-config in commands that fire hooks (#461)
- **completions**: Offer path completions and out-of-repo path remove (#459)
- **clone**: Reject --no-checkout for non-bare layouts and cd into root on success (#458)
## [1.11.0](https://github.com/avihut/daft/compare/v1.10.3...v1.11.0) - 2026-05-02


### Features

- Add `daft repo remove` and `daft repo` subcommand category (#448)
## [1.10.3](https://github.com/avihut/daft/compare/v1.10.2...v1.10.3) - 2026-05-02


### Bug Fixes

- Do not create default-branch worktree for multi-branch bare clones (#453)
- **ci**: Scope claude-pr-review concurrency to the job (#440)
## [1.10.2](https://github.com/avihut/daft/compare/v1.10.1...v1.10.2) - 2026-05-01


### Bug Fixes

- **tui**: Stop default-branch owner shimmer in prune/sync (#430)


### Miscellaneous

- **deps**: Add 7-day cooldown across cargo, bun, mise, dependabot, and CI (#431)
## [1.10.1](https://github.com/avihut/daft/compare/v1.10.0...v1.10.1) - 2026-04-28


### Bug Fixes

- **hooks**: Gate coordinator IPC behind cfg(unix) so Windows builds (#426)


### Miscellaneous

- **hooks**: Warm up dev build via background mise dev job (#425)
## [1.10.0](https://github.com/avihut/daft/compare/v1.9.1...v1.10.0) - 2026-04-28


### Bug Fixes

- **exec**: Clarify failed-run output dump format (#422)


### Features

- **hooks**: Background hook jobs + universal logging + daft hooks jobs CLI (#411)
## [1.9.1](https://github.com/avihut/daft/compare/v1.9.0...v1.9.1) - 2026-04-27


### Bug Fixes

- **exec**: Isolate alias capture from rc-file stdout (#418)
- **shell-init,completions**: Skip update check and trust prune for shell-eval'd commands (#416)
## [1.9.0](https://github.com/avihut/daft/compare/v1.8.0...v1.9.0) - 2026-04-27


### Bug Fixes

- **exec**: Resolve user shell aliases and functions (#414)
- **test**: Guarantee manual-test sandbox cleanup via Drop (#413)


### Features

- Shared infrastructure for live list population (#402) (#410)
## [1.8.0](https://github.com/avihut/daft/compare/v1.7.2...v1.8.0) - 2026-04-25


### Bug Fixes

- **ci**: Skip cargo package verify in release-plz (#405)
- **exec**: Center header, fix stuck timer, brighten command preview (#404)
- **shell-init**: Resolve daft binary live, drop eager path cache (#403)
- **tests**: Tolerate per-test cleanup races in integration suite (#401)


### Features

- **exec**: Run commands across multiple worktrees (#381)
- Multi-format emit support (--format, --template) (#377)


### Miscellaneous

- Commit Cargo.lock and bump gix to 0.82 (#382)


### Revert

- Drop publish_no_verify in release-plz.toml (#406)
## [1.7.2](https://github.com/avihut/daft/compare/v1.7.1...v1.7.2) - 2026-04-21


### Bug Fixes

- **ownership**: Configurable strategy-driven branch ownership detection (#376)
- **sync**: Exclude rebase base branch from push (#375)
- **update-check**: Drop git-style "hint:" prefix from notification (#373)
- **hooks**: Skip prompt when trust level unchanged (#371)
## [1.7.1](https://github.com/avihut/daft/compare/v1.7.0...v1.7.1) - 2026-04-20


### Bug Fixes

- **output**: Keep outer spinner alive across hook boundary (#367)


### Miscellaneous

- Fix clippy warnings new in Rust 1.95 (#368)
## [1.7.0](https://github.com/avihut/daft/compare/v1.6.5...v1.7.0) - 2026-04-15


### Features

- **completions**: Rich grouped completions with configurable columns (#365)
## [1.6.5](https://github.com/avihut/daft/compare/v1.6.4...v1.6.5) - 2026-04-11


### Bug Fixes

- **ci**: Verify remote/ clone directly in test-homebrew (#363)
## [1.6.4](https://github.com/avihut/daft/compare/v1.6.3...v1.6.4) - 2026-04-11


### Bug Fixes

- **ci**: Run test-homebrew clone from a clean workspace subdir (#360)


### Miscellaneous

- Add --env flag to sandbox help command (#362)
## [1.6.3](https://github.com/avihut/daft/compare/v1.6.2...v1.6.3) - 2026-04-11


### Bug Fixes

- Add daft-go/daft-start to all packaging symlink lists (#358)
## [1.6.2](https://github.com/avihut/daft/compare/v1.6.1...v1.6.2) - 2026-04-11


### Bug Fixes

- Shared status/sync/manage work from contained layout root (#353)
## [1.6.1](https://github.com/avihut/daft/compare/v1.6.0...v1.6.1) - 2026-04-09


### Bug Fixes

- Shared status column alignment and default subcommand (#352)
- Collect uncollected shared files from manage TUI via Enter/Space (#351)
- Exclude Size column from default responsive column set (#350)
## [1.6.0](https://github.com/avihut/daft/compare/v1.5.1...v1.6.0) - 2026-03-31


### Bug Fixes

- Prevent auto-upstream tracking on new local-first branches (#342)
- Allow --remote delete when local branch does not exist (#341)
- **ci**: Update homebrew test for sibling layout and new commands


### Features

- Shared file manage TUI with inline editor (#344)
- Interactive sync for declared-but-uncollected shared files (#343)
- Add layout prompt to init command (#338)
- Local-first remote sync (#337)
## [1.5.1](https://github.com/avihut/daft/compare/v1.5.0...v1.5.1) - 2026-03-26


### Bug Fixes

- Gate unix symlink import with cfg(unix) to fix Windows build
## [1.5.0](https://github.com/avihut/daft/compare/v1.4.0...v1.5.0) - 2026-03-26


### Bug Fixes

- Move shared-env.sh out of gitignored .mise/ to fix release-plz
- Prevent fork bomb from recursive background task spawning (#332)


### Features

- Worktree shared files — centralize untracked config across worktrees (#328) (#331)
- Move hooks — identity-tracked hook replay on worktree move (#326)
## [1.4.0](https://github.com/avihut/daft/compare/v1.3.1...v1.4.0) - 2026-03-25


### Features

- Repeatable -b flag for multi-branch clone (#324)
## [1.3.1](https://github.com/avihut/daft/compare/v1.3.0...v1.3.1) - 2026-03-24


### Bug Fixes

- Gate unix-only MetadataExt behind cfg(unix) for Windows build (#321)
## [1.3.0](https://github.com/avihut/daft/compare/v1.2.1...v1.3.0) - 2026-03-24


### Features

- Progressive adoption layout system (#320)
- Add --columns hash and gitoxide commit metadata fast path (#319)
- Add --sort flag to list, prune, and sync commands (#317)
- Add optional size column with summary footer (#315)
## [1.2.1](https://github.com/avihut/daft/compare/v1.2.0...v1.2.1) - 2026-03-17


### Bug Fixes

- Navigate to detached HEAD worktrees and clean rebase lock files (#313)
## [1.2.0](https://github.com/avihut/daft/compare/v1.1.1...v1.2.0) - 2026-03-17


### Bug Fixes

- **sandbox**: Reset ZDOTDIR in setup-test to prevent shell contamination
- Add missing completions for top-level flags (#311)
- Align default branch symbol and color in sync and prune with list (#308)


### Features

- Branch ownership, local branch sync, and --include flag (#312)
- Add --columns flag for list, sync, and prune commands (#310)
## [1.1.1](https://github.com/avihut/daft/compare/v1.1.0...v1.1.1) - 2026-03-15


### Bug Fixes

- Rename git-sync to git-worktree-sync to avoid Homebrew conflicts (#305)
- Build.rs rerun-if-changed watches wrong paths in worktrees (#303)
## [1.1.0](https://github.com/avihut/daft/compare/v1.0.37...v1.1.0) - 2026-03-15


### Bug Fixes

- **sync**: Prevent push after rebase conflict (#299)


### Features

- Manual testing framework (#301)


### Miscellaneous

- Convert integration tests to YAML manual test framework (#302)
## [1.0.37](https://github.com/avihut/daft/compare/v1.0.36...v1.0.37) - 2026-03-13


### Bug Fixes

- **sync**: Show dirty status instead of pruned for unremovable worktrees (#298)
- **sync**: Refresh remote tracking info after push in TUI output (#297)
- Prevent brew link failures with Homebrew-aware symlink management (#294)
## [1.0.36](https://github.com/avihut/daft/compare/v1.0.35...v1.0.36) - 2026-03-12


### Bug Fixes

- **sync**: Treat non-fast-forwardable branches as warnings (#291)
- Simplify hooks trust level change output (#289)
- Use resolved remote URL for trust fingerprint during clone (#286)
- Sync symlink lists across all packaging configs


### Features

- **sync**: Add --push and --force-with-lease flags (#293)
- **sync**: Add --autostash flag for rebase phase (#292)
- **sync,prune**: Hook lifecycle in TUI with verbose job sub-rows (#290)
- **sync,prune**: Parallel DAG executor with inline ratatui TUI (#285)
- Allow force-removal of default branch worktree (#284)


### Miscellaneous

- Compartmentalized sandbox config directory (#288)
## [1.0.35](https://github.com/avihut/daft/compare/v1.0.34...v1.0.35) - 2026-03-09


### Bug Fixes

- Dev version format leaking to release builds (#282)


### Miscellaneous

- Replace custom commit-msg regex with cocogitto
## [1.0.34](https://github.com/avihut/daft/compare/v1.0.33...v1.0.34) - 2026-03-08


### Bug Fixes

- **completions**: Remote branches not matching when typing prefix
- **ci**: Pin release-plz to 0.3.156 to fix release PR creation
- **ci**: Add version to xtask path dependency for cargo package compatibility
- **hooks**: Trust fingerprinting, stale entry protection, and prune improvements (#278)
- **list**: Use orange for default branch indicator and tighten first column (#272)


### Features

- **list**: Enhance list command with stat modes, branch flags, and config (#280)
- Add spinner feedback for long-running operations (#277)


### Refactoring

- Hide completions command from help and shell completions (#270)
## [1.0.33](https://github.com/avihut/daft/compare/v1.0.32...v1.0.33) - 2026-02-24


### Bug Fixes

- **list**: Respect terminal width and work from repo root (#268)
## [1.0.32](https://github.com/avihut/daft/compare/v1.0.31...v1.0.32) - 2026-02-24


### Bug Fixes

- **docs**: Build issue and missing docs
- **release-notes**: Add PR links to older changelog entries
- **release-notes**: Improve link rendering and skip empty sections (#260)
- **hooks**: Resolve hooks not executing on first run after upgrade (#259)


### Features

- **list**: Enrich daft list with status indicators, branch age, and relative paths (#266)
- **list**: Add daft list / git worktree-list command (#264)
- **sync**: Add daft sync / git sync command (#263)
- Add `daft rename` / `git worktree-rename` command (#256)
- **go**: Add `daft go -` to toggle between previous worktrees (#262)
- **update**: Rename daft fetch to daft update with refspec support (#261)
- **go**: Auto-start worktrees and improved branch-not-found errors (#257)


### Refactoring

- **cli**: Separate daft go/start from git-worktree-checkout (#265)
## [1.0.31](https://github.com/avihut/daft/compare/v1.0.30...v1.0.31) - 2026-02-23


### Bug Fixes

- **gitoxide**: Detect gone upstream branches in branch_list_verbose (#252)


### Features

- **hooks**: OS-variant run map for polymorphic platform targeting (#254)
- Consolidate worktree-branch-delete into worktree-branch -d/-D (#253)
- Add daft verb aliases and consolidate checkout-branch into checkout -b (#250)
## [1.0.30](https://github.com/avihut/daft/compare/v1.0.29...v1.0.30) - 2026-02-22


### Bug Fixes

- **ci**: Prevent test workflow from running twice on release PRs
- **bench**: Missing symlink, jq overflow, and worktree cleanup (#247)
- **bench**: Add target/release to PATH so benchmarks find daft (#246)
- Use \$SHELL -i for exec commands to support aliases and shell functions (#242)


### Features

- **hooks**: Add job descriptions, skip descriptions, and OS/arch conditions (#248)
- **hooks**: Add manual hook execution with `hooks run` command (#245)


### Miscellaneous

- Add benchmark suite with dashboard (#244)


### Refactoring

- Separate core logic from UI layer (#249)
## [1.0.29](https://github.com/avihut/daft/compare/v1.0.28...v1.0.29) - 2026-02-19


### Bug Fixes

- Prevent prune from deleting worktrees with uncommitted changes or untracked files (#240)
- Grow hook output window dynamically instead of pre-allocating blank lines (#237)
- Check remote tracking branch when validating merge status for deletion (#236)
- Doctor --fix now creates missing shortcuts and --dry-run shows concrete actions (#232)
- Cd to safe directory when branch-delete removes current worktree (#234)
- Run post-clone hook before worktree-post-create during clone (#233)
- Carry changes from base branch worktree instead of current worktree (#231)


### Features

- Accept worktree paths as arguments in branch-delete (#239)
- Add -x/--exec option to run commands in worktree after setup (#238)


### Miscellaneous

- Removed plans
- Removed unused WARP.md
- Reorganize mise tasks into file-based structure with : hierarchy (#235)
- Simplified daft hooks
## [1.0.28](https://github.com/avihut/daft/compare/v1.0.27...v1.0.28) - 2026-02-18


### Bug Fixes

- Skip pre-push hooks for structural push operations (#228)


### Features

- Add Linux distribution pipeline with shell installer (#229)
## [1.0.27](https://github.com/avihut/daft/compare/v1.0.26...v1.0.27) - 2026-02-18


### Bug Fixes

- Enable CD after worktree-branch-delete removes current worktree (#224)
- Render hook job output after heading, not before (#223)


### Features

- **doctor**: Improve output format, fix bugs, and add --dry-run (#222)
## [1.0.26](https://github.com/avihut/daft/compare/v1.0.25...v1.0.26) - 2026-02-17


### Bug Fixes

- Use temp file for shell wrapper CD communication (#219)


### Features

- Add hook execution progress display with spinners (#217)
## [1.0.25](https://github.com/avihut/daft/compare/v1.0.24...v1.0.25) - 2026-02-16


### Bug Fixes

- **fetch**: Check target worktree for dirty state instead of current worktree (#213)
- **docs**: Align changelog rendering for new release-plz format (#210)


### Features

- **output**: Add colored output to CliOutput and fetch/prune commands (#215)
- **shortcuts**: Make default-branch shortcuts shell-only functions (#214)
- Add git-worktree-branch-delete command (#211)
## [1.0.24](https://github.com/avihut/daft/compare/v1.0.23...v1.0.24) - 2026-02-15


### Bug Fixes

- Prevent hook commands from hanging on long-running tasks (#205)


### Refactoring

- Absorb checkout-branch-from-default into --from-default flag (#206)


### Testing

- Add gitoxide integration test coverage and move matrix to Rust xtask (#208)
## [1.0.23](https://github.com/avihut/daft/compare/v1.0.22...v1.0.23) - 2026-02-14


### Miscellaneous

- Add Prettier formatting with Bun for docs and YAML
- Remove unused install.sh script (#192)
- Improve daft.yml hooks for full worktree automation (#190)
## [1.0.22](https://github.com/avihut/daft/compare/v1.0.21...v1.0.22) - 2026-02-07


### Bug Fixes

- **clone**: Fall back to git CLI for remote ops when no local repo exists (#184)
## [1.0.21](https://github.com/avihut/daft/compare/v1.0.20...v1.0.21) - 2026-02-07


### Features

- Add lefthook git hooks for local code quality checks (#183)
- Migrate from just to mise task runner (#182)
- **hooks**: Add YAML hooks system (#181)
- Add `daft doctor` diagnostic command (#169)
- **update-check**: Throttle new version notification to once per 24 hours (#168)
- **shell-init**: Add daft() shell wrapper for cd into worktrees (#165)
## [1.0.20](https://github.com/avihut/daft/compare/v1.0.19...v1.0.20) - 2026-02-04


### Features

- Add experimental gitoxide backend for git operations (#161)


### Refactoring

- **completions**: Replace Fig spec string concatenation with serde_json (#159)
## [1.0.19](https://github.com/avihut/daft/compare/v1.0.18...v1.0.19) - 2026-02-03


### Bug Fixes

- Correctly detect bare repo root to suppress carry warning


### Features

- **fetch**: Emit git-fetch-like output with real-time progress (#158)
- Add --no-cd flag to worktree-creating commands
- **prune**: Add streaming per-branch output in git-remote-prune style
- Handle pruning when inside the worktree being removed (#148)
- Add background update check with new version notifications (#143)
## [1.0.18](https://github.com/avihut/daft/compare/v1.0.17...v1.0.18) - 2026-02-02


### Bug Fixes

- Include shell completions in shell-init and add Fig/Amazon Q support (#142)
- Skip uncommitted changes check when not in a work tree (#141)
- Display full branch path for worktrees with slashes (#139)
- Clean up empty parent directories after pruning worktrees (#137)
## [1.0.17](https://github.com/avihut/daft/compare/v1.0.16...v1.0.17) - 2026-02-01


### Bug Fixes

- Handle '+' marker in git branch -vv for linked worktree branches (#136)


### Features

- Rename worktree hooks with deprecation support (#131)
## [1.0.16](https://github.com/avihut/daft/compare/v1.0.15...v1.0.16) - 2026-01-31


### Bug Fixes

- Show error message and suggestions for unknown subcommands (#121)
- Add retry logic for release PR detection in man page update (#117)


### Features

- Migrate Homebrew tap from avihut/homebrew-daft to avihut/homebrew-tap (#126)


### Refactoring

- Unify git-daft and daft command routing (#123)
## [1.0.15](https://github.com/avihut/daft/compare/v1.0.14...v1.0.15) - 2026-01-28


### Bug Fixes

- Sync man page versions with release workflow (#115)
## [1.0.14](https://github.com/avihut/daft/compare/v1.0.13...v1.0.14) - 2026-01-28


### Bug Fixes

- Install man pages before pkgshare.install in Homebrew formula (#113)
## [1.0.13](https://github.com/avihut/daft/compare/v1.0.12...v1.0.13) - 2026-01-28


### Bug Fixes

- Include man pages in binary distribution archives (#111)
## [1.0.12](https://github.com/avihut/daft/compare/v1.0.11...v1.0.12) - 2026-01-28


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
## [1.0.11](https://github.com/avihut/daft/compare/v1.0.7...v1.0.11) - 2026-01-28


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
## [1.0.7](https://github.com/avihut/daft/compare/v1.0.6...v1.0.7) - 2026-01-24


### Bug Fixes

- Remove conflicting prerelease tags before changelog generation
## [1.0.4](https://github.com/avihut/daft/compare/v1.0.3...v1.0.4) - 2026-01-24


### Features

- Add one-time shell integration hint for new users (#81)
## [1.0.2](https://github.com/avihut/daft/compare/v1.0.1...v1.0.2) - 2026-01-24


### Bug Fixes

- Enable CD behavior for all shortcut styles (#80)
## [1.0.1](https://github.com/avihut/daft/compare/v1.0.0...v1.0.1) - 2026-01-24


### Bug Fixes

- Resolve v1.0.0 release issues (#78)
## [1.0.0](https://github.com/avihut/daft/compare/v0.3.3...v1.0.0) - 2026-01-24


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
## [0.3.2](https://github.com/avihut/daft/compare/v0.3.1...v0.3.2) - 2026-01-11


### Bug Fixes

- Correct heredoc syntax in workflow YAML files
## [0.3.1](https://github.com/avihut/daft/compare/v0.3.0...v0.3.1) - 2026-01-11


### Bug Fixes

- Use gh CLI for git-cliff installation


### Features

- Integrate git-cliff for automated changelog generation (#38)
## [0.3.0](https://github.com/avihut/daft/compare/v0.2.2...v0.3.0) - 2026-01-10


### Features

- Setup develop branch with canary releases (v0.2.0)
## [0.2.2](https://github.com/avihut/daft/compare/v0.2.1...v0.2.2) - 2026-01-10


### Bug Fixes

- Exclude prerelease tags when finding latest version (#37)
## [0.2.1](https://github.com/avihut/daft/compare/v0.1.28...v0.2.1) - 2026-01-10


### Bug Fixes

- Add GitHub release creation to canary and beta workflows (#36)
## [0.1.28](https://github.com/avihut/daft/compare/v0.1.27...v0.1.28) - 2026-01-10


### Miscellaneous

- Add developer-tools keyword to Cargo.toml (#35)
- Use wheatley bot for Homebrew formula commits [skip ci]
## [0.1.27](https://github.com/avihut/daft/compare/v0.1.26...v0.1.27) - 2026-01-10


### Features

- Migrate Homebrew formula to separate tap repository
## [0.1.26](https://github.com/avihut/daft/compare/v0.1.25...v0.1.26) - 2026-01-10


### Miscellaneous

- Test release workflow
## [0.1.22](https://github.com/avihut/daft/compare/v0.1.21...v0.1.22) - 2026-01-10


### Miscellaneous

- Trigger v0.1.22 release
## [0.1.21](https://github.com/avihut/daft/compare/v0.1.20...v0.1.21) - 2026-01-10


### Feature

- Root Level `.git` Dir (#2)


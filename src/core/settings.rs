//! Git config-based settings for daft.
//!
//! This module provides user-configurable options via `git config`.
//! Settings are loaded from git's layered config system (local → global)
//! with built-in defaults as fallback.
//!
//! # Config Keys
//!
//! | Key | Default | Description |
//! |-----|---------|-------------|
//! | `daft.autocd` | `true` | CD into new worktrees (shell wrapper behavior) |
//! | `daft.checkout.push` | `false` | Push new branches to remote |
//! | `daft.checkout.fetch` | `false` | Fetch from remote before creating worktrees |
//! | `daft.checkout.upstream` | `true` | Set upstream tracking |
//! | `daft.checkout.pushVerify` | `auto` | When the auto-upstream push runs the repo's pre-push hook (`auto`, `always` or `never`) |
//! | `daft.remote` | `"origin"` | Default remote name |
//! | `daft.checkoutBranch.carry` | `true` | Default carry for checkout-branch |
//! | `daft.checkout.carry` | `false` | Default carry for checkout |
//! | `daft.go.autoStart` | `false` | Auto-create worktree when branch not found in go |
//! | `daft.prune.cdTarget` | `root` | Where to cd after pruning current worktree (`root` or `default-branch`) |
//! | `daft.list.stat` | `summary` | Default statistics mode for list command (`summary` or `lines`) |
//! | `daft.list.sort` | `branch` | Default sort order for list command |
//! | `daft.sync.sort` | `branch` | Default sort order for sync command |
//! | `daft.prune.sort` | `branch` | Default sort order for prune command |
//! | `daft.updateCheck` | `true` | Enable/disable new version notifications |
//! | `daft.branchDelete.remote` | `false` | Delete remote branch when removing |
//! | `daft.ownership.strategy` | `recency-plurality` | Branch ownership detection strategy (`tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`) |
//! | `daft.sync.pushTimeout` | `30m` | Wall-clock budget per push (git + pre-push hook); `off` disables |
//! | `daft.governor.mode` | `auto` | Sync push resource governor (`auto` or `off`) |
//! | `daft.governor.jobs` | `auto` | Cap on concurrent hook-bearing pushes (`auto` = max(2, cores/4), or a number) |
//! | `daft.governor.memoryReserve` | `auto` | Memory headroom the governor keeps free (`auto` = max(10% RAM, 2G), a size like `2G`, or `NN%`) |
//!
//! # Hooks Config Keys
//!
//! | Key | Default | Description |
//! |-----|---------|-------------|
//! | `daft.hooks.enabled` | `true` | Master switch for all hooks |
//! | `daft.hooks.defaultTrust` | `deny` | Default trust level for unknown repos |
//! | `daft.hooks.timeout` | `300` | Timeout for hook execution in seconds |
//! | `daft.hooks.output.quiet` | `false` | Suppress hook stdout/stderr |
//! | `daft.hooks.output.timerDelay` | `5` | Seconds before showing elapsed timer |
//! | `daft.hooks.output.tailLines` | `6` | Rolling output tail lines per job (0 = none) |
//! | `daft.hooks.output.verbose` | `false` | Show skipped jobs and their reasons |
//! | `daft.hooks.<hookName>.enabled` | `true` | Enable/disable specific hook |
//! | `daft.hooks.<hookName>.failMode` | varies | Behavior on hook failure (abort/warn) |
//!
//! # Example
//!
//! ```bash
//! # Disable auto-cd globally
//! git config --global daft.autocd false
//!
//! # Use a different remote for this repository
//! git config daft.remote upstream
//!
//! # Disable hooks globally
//! git config --global daft.hooks.enabled false
//!
//! # Make post-create hooks abort on failure
//! git config daft.hooks.postCreate.failMode abort
//! ```

use crate::core::worktree::list::Stat;
use crate::git::GitCommand;
use crate::hooks::{FailMode, HookConfig, HookType, HooksConfig, TrustLevel};
use anyhow::Result;
use std::path::PathBuf;

/// Where to cd after pruning the user's current worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneCdTarget {
    /// CD to the project root directory.
    Root,
    /// CD to the default branch worktree directory.
    DefaultBranch,
}

impl PruneCdTarget {
    /// Parse a string value into a PruneCdTarget.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "root" => Some(Self::Root),
            "default-branch" => Some(Self::DefaultBranch),
            _ => None,
        }
    }
}

/// When the automatic upstream push consults the repo's `pre-push` hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushVerify {
    /// Run the hook only when the push introduces commits the target remote
    /// does not already have; skip it for ref-only pushes (#679).
    Auto,
    /// Always run the hook, even for ref-only pushes.
    Always,
    /// Never run the hook on the automatic upstream push.
    Never,
}

impl PushVerify {
    /// Parse a string value into a PushVerify.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

/// Whether the sync push resource governor is active (#678).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorMode {
    /// Govern hook-bearing pushes: cap their concurrency (and throttle on
    /// memory pressure once the dynamic stages are active).
    Auto,
    /// Never construct the governor; pushes run uncapped.
    Off,
}

impl GovernorMode {
    /// Parse a string value into a GovernorMode.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Concurrency cap for hook-bearing push tasks during `daft sync --push`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorJobs {
    /// `max(2, cores/4)` — heavy pre-push hooks are internally parallel,
    /// so a few concurrent runs already saturate the machine while
    /// multiplying peak memory use (#678).
    Auto,
    /// A fixed cap (at least 1).
    Fixed(usize),
}

impl GovernorJobs {
    /// Parse a string value: `auto` or a positive integer.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        value
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .map(Self::Fixed)
    }

    /// The effective cap on a machine with `cores` logical cores.
    pub fn effective(self, cores: usize) -> usize {
        match self {
            Self::Auto => (cores / 4).max(2),
            Self::Fixed(n) => n.max(1),
        }
    }
}

/// Memory headroom the governor keeps free before admitting another
/// hook-bearing push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryReserve {
    /// `max(10% of RAM, 2 GiB)`.
    Auto,
    /// A fixed number of bytes.
    Bytes(u64),
    /// A percentage of total RAM (1–99).
    Percent(u8),
}

impl MemoryReserve {
    /// Parse `auto`, `NN%`, or a size in bytes with an optional
    /// case-insensitive `K`/`M`/`G` suffix (binary units).
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("auto") {
            return Some(Self::Auto);
        }
        if let Some(percent) = value.strip_suffix('%') {
            let p: u8 = percent.trim().parse().ok()?;
            return (1..=99).contains(&p).then_some(Self::Percent(p));
        }
        let lower = value.to_ascii_lowercase();
        let (digits, multiplier) = if let Some(d) = lower.strip_suffix('g') {
            (d, 1u64 << 30)
        } else if let Some(d) = lower.strip_suffix('m') {
            (d, 1u64 << 20)
        } else if let Some(d) = lower.strip_suffix('k') {
            (d, 1u64 << 10)
        } else {
            (lower.as_str(), 1)
        };
        let n: u64 = digits.trim().parse().ok()?;
        n.checked_mul(multiplier)
            .filter(|b| *b > 0)
            .map(Self::Bytes)
    }

    /// Resolve to bytes on a machine with `total_ram` bytes of memory.
    pub fn resolve(self, total_ram: u64) -> u64 {
        const TWO_GIB: u64 = 2 << 30;
        match self {
            Self::Auto => (total_ram / 10).max(TWO_GIB),
            Self::Bytes(bytes) => bytes,
            Self::Percent(percent) => total_ram * u64::from(percent) / 100,
        }
    }
}

/// Parse `daft.sync.pushTimeout`: `off`/`0` disables (outer `Some(None)`);
/// otherwise a positive duration with an optional case-insensitive
/// `s`/`m`/`h` suffix (bare numbers are seconds). Outer `None` = unparseable
/// (caller warns and keeps the default).
pub fn parse_push_timeout(value: &str) -> Option<Option<std::time::Duration>> {
    let value = value.trim().to_ascii_lowercase();
    if value == "off" || value == "0" {
        return Some(None);
    }
    let (digits, unit_secs) = if let Some(d) = value.strip_suffix('h') {
        (d, 3_600)
    } else if let Some(d) = value.strip_suffix('m') {
        (d, 60)
    } else if let Some(d) = value.strip_suffix('s') {
        (d, 1)
    } else {
        (value.as_str(), 1)
    };
    let n: u64 = digits.trim().parse().ok()?;
    (n > 0).then(|| Some(std::time::Duration::from_secs(n * unit_secs)))
}

/// Default values for settings.
pub mod defaults {
    use super::{GovernorJobs, GovernorMode, MemoryReserve, PruneCdTarget, PushVerify};
    use crate::core::worktree::list::Stat;

    /// Default value for sync.pushTimeout setting (30 minutes).
    pub const SYNC_PUSH_TIMEOUT: Option<std::time::Duration> =
        Some(std::time::Duration::from_secs(30 * 60));

    /// Default value for governor.mode setting.
    pub const GOVERNOR_MODE: GovernorMode = GovernorMode::Auto;

    /// Default value for governor.jobs setting.
    pub const GOVERNOR_JOBS: GovernorJobs = GovernorJobs::Auto;

    /// Default value for governor.memoryReserve setting.
    pub const GOVERNOR_MEMORY_RESERVE: MemoryReserve = MemoryReserve::Auto;

    /// Default value for autocd setting.
    pub const AUTOCD: bool = true;

    /// Default value for checkout.push setting.
    pub const CHECKOUT_PUSH: bool = false;

    /// Default value for checkout.fetch setting.
    pub const CHECKOUT_FETCH: bool = false;

    /// Default value for checkout.upstream setting.
    pub const CHECKOUT_UPSTREAM: bool = true;

    /// Default value for checkout.pushVerify setting.
    pub const CHECKOUT_PUSH_VERIFY: PushVerify = PushVerify::Auto;

    /// Default value for remote setting.
    pub const REMOTE: &str = "origin";

    /// Default value for checkoutBranch.carry setting.
    pub const CHECKOUT_BRANCH_CARRY: bool = true;

    /// Default value for checkout.carry setting.
    pub const CHECKOUT_CARRY: bool = false;

    /// Default value for update.args setting.
    pub const UPDATE_ARGS: &str = "--ff-only";

    /// Default value for multiRemote.enabled setting.
    pub const MULTI_REMOTE_ENABLED: bool = false;

    /// Default value for multiRemote.defaultRemote setting.
    pub const MULTI_REMOTE_DEFAULT_REMOTE: &str = "origin";

    /// Default value for prune.cdTarget setting.
    pub const PRUNE_CD_TARGET: PruneCdTarget = PruneCdTarget::Root;

    /// Default value for experimental.gitoxide setting.
    pub const USE_GITOXIDE: bool = false;

    /// Default value for go.autoStart setting.
    pub const GO_AUTO_START: bool = false;

    /// Default value for go.fetchOnMiss setting.
    pub const GO_FETCH_ON_MISS: bool = true;

    /// Default value for list.stat setting.
    pub const LIST_STAT: Stat = Stat::Summary;

    /// Default value for sync.stat setting.
    pub const SYNC_STAT: Stat = Stat::Summary;

    /// Default value for prune.stat setting.
    pub const PRUNE_STAT: Stat = Stat::Summary;

    /// Default value for branchDelete.remote setting.
    pub const BRANCH_DELETE_REMOTE: bool = false;

    /// Default value for ownership.strategy setting.
    pub const OWNERSHIP_STRATEGY: crate::core::ownership::OwnershipStrategy =
        crate::core::ownership::OwnershipStrategy::RecencyPlurality;

    /// Default value for merge.style setting.
    pub const MERGE_STYLE: crate::core::worktree::merge::MergeStyle =
        crate::core::worktree::merge::MergeStyle::Merge;

    /// Default value for merge.cleanup setting.
    pub const MERGE_CLEANUP: crate::core::worktree::merge::CleanupKind =
        crate::core::worktree::merge::CleanupKind::Keep;

    /// Default value for merge.commit setting.
    pub const MERGE_COMMIT: bool = true;

    /// Default value for merge.signoff setting.
    pub const MERGE_SIGNOFF: bool = false;

    /// Default value for merge.verifySignatures setting.
    pub const MERGE_VERIFY_SIGNATURES: bool = false;

    /// Default value for merge.allowUnrelatedHistories setting.
    pub const MERGE_ALLOW_UNRELATED_HISTORIES: bool = false;

    /// Default value for merge.adoptTargetOnDemand setting.
    pub const MERGE_ADOPT_TARGET_ON_DEMAND: crate::core::worktree::merge::AdoptPreset =
        crate::core::worktree::merge::AdoptPreset::Prompt;

    /// Default value for merge.requireCleanTarget setting.
    pub const MERGE_REQUIRE_CLEAN_TARGET: bool = true;
}

/// Git config keys for daft settings.
pub mod keys {
    /// Config key for autocd setting.
    pub const AUTOCD: &str = "daft.autocd";

    /// Config key for checkout.push setting.
    pub const CHECKOUT_PUSH: &str = "daft.checkout.push";

    /// Config key for checkout.fetch setting.
    pub const CHECKOUT_FETCH: &str = "daft.checkout.fetch";

    /// Config key for checkout.upstream setting.
    pub const CHECKOUT_UPSTREAM: &str = "daft.checkout.upstream";

    /// Config key for checkout.pushVerify setting.
    pub const CHECKOUT_PUSH_VERIFY: &str = "daft.checkout.pushVerify";

    /// Config key for remote setting.
    pub const REMOTE: &str = "daft.remote";

    /// Config key for checkoutBranch.carry setting.
    pub const CHECKOUT_BRANCH_CARRY: &str = "daft.checkoutBranch.carry";

    /// Config key for checkout.carry setting.
    pub const CHECKOUT_CARRY: &str = "daft.checkout.carry";

    /// Config key for update.args setting.
    pub const UPDATE_ARGS: &str = "daft.update.args";

    /// Deprecated config key for update.args (migration fallback).
    pub const FETCH_ARGS_DEPRECATED: &str = "daft.fetch.args";

    /// Multi-remote config keys.
    pub mod multi_remote {
        /// Config key for multiRemote.enabled setting.
        pub const ENABLED: &str = "daft.multiRemote.enabled";

        /// Config key for multiRemote.defaultRemote setting.
        pub const DEFAULT_REMOTE: &str = "daft.multiRemote.defaultRemote";
    }

    /// Config key for prune.cdTarget setting.
    pub const PRUNE_CD_TARGET: &str = "daft.prune.cdTarget";

    /// Config key for updateCheck setting.
    pub const UPDATE_CHECK: &str = "daft.updateCheck";

    /// Config key for go.autoStart setting.
    pub const GO_AUTO_START: &str = "daft.go.autoStart";

    /// Config key for go.fetchOnMiss setting.
    pub const GO_FETCH_ON_MISS: &str = "daft.go.fetchOnMiss";

    /// Config key for list.stat setting.
    pub const LIST_STAT: &str = "daft.list.stat";

    /// Config key for sync.stat setting.
    pub const SYNC_STAT: &str = "daft.sync.stat";

    /// Config key for prune.stat setting.
    pub const PRUNE_STAT: &str = "daft.prune.stat";

    /// Config key for list.columns setting.
    pub const LIST_COLUMNS: &str = "daft.list.columns";

    /// Config key for sync.columns setting.
    pub const SYNC_COLUMNS: &str = "daft.sync.columns";

    /// Config key for prune.columns setting.
    pub const PRUNE_COLUMNS: &str = "daft.prune.columns";

    /// Config key for list.sort setting.
    pub const LIST_SORT: &str = "daft.list.sort";

    /// Config key for sync.sort setting.
    pub const SYNC_SORT: &str = "daft.sync.sort";

    /// Config key for prune.sort setting.
    pub const PRUNE_SORT: &str = "daft.prune.sort";

    /// Config key for branchDelete.remote setting.
    pub const BRANCH_DELETE_REMOTE: &str = "daft.branchDelete.remote";

    /// Config key for ownership.strategy setting.
    pub const OWNERSHIP_STRATEGY: &str = "daft.ownership.strategy";

    /// Config key for sync.pushTimeout setting.
    pub const SYNC_PUSH_TIMEOUT: &str = "daft.sync.pushTimeout";

    /// Config key for governor.mode setting.
    pub const GOVERNOR_MODE: &str = "daft.governor.mode";

    /// Config key for governor.jobs setting.
    pub const GOVERNOR_JOBS: &str = "daft.governor.jobs";

    /// Config key for governor.memoryReserve setting.
    pub const GOVERNOR_MEMORY_RESERVE: &str = "daft.governor.memoryReserve";

    /// Config key for merge.style setting.
    pub const MERGE_STYLE: &str = "daft.merge.style";

    /// Config key for merge.cleanup setting.
    pub const MERGE_CLEANUP: &str = "daft.merge.cleanup";

    /// Config key for merge.commit setting.
    pub const MERGE_COMMIT: &str = "daft.merge.commit";

    /// Config key for merge.edit setting.
    pub const MERGE_EDIT: &str = "daft.merge.edit";

    /// Config key for merge.signoff setting.
    pub const MERGE_SIGNOFF: &str = "daft.merge.signoff";

    /// Config key for merge.gpgSign setting.
    pub const MERGE_GPG_SIGN: &str = "daft.merge.gpgSign";

    /// Config key for merge.verifySignatures setting.
    pub const MERGE_VERIFY_SIGNATURES: &str = "daft.merge.verifySignatures";

    /// Config key for merge.allowUnrelatedHistories setting.
    pub const MERGE_ALLOW_UNRELATED_HISTORIES: &str = "daft.merge.allowUnrelatedHistories";

    /// Config key for merge.strategy setting.
    pub const MERGE_STRATEGY: &str = "daft.merge.strategy";

    /// Config key for merge.strategyOption setting (comma-separated list).
    pub const MERGE_STRATEGY_OPTION: &str = "daft.merge.strategyOption";

    /// Config key for merge.adoptTargetOnDemand setting.
    pub const MERGE_ADOPT_TARGET_ON_DEMAND: &str = "daft.merge.adoptTargetOnDemand";

    /// Config key for merge.requireCleanTarget setting.
    pub const MERGE_REQUIRE_CLEAN_TARGET: &str = "daft.merge.requireCleanTarget";

    /// Experimental config keys.
    pub mod experimental {
        /// Config key for experimental.gitoxide setting.
        pub const GITOXIDE: &str = "daft.experimental.gitoxide";
    }

    /// Hooks config keys.
    pub mod hooks {
        /// Config key for hooks.enabled setting.
        pub const ENABLED: &str = "daft.hooks.enabled";

        /// Config key for hooks.defaultTrust setting.
        pub const DEFAULT_TRUST: &str = "daft.hooks.defaultTrust";

        /// Config key for hooks.userDirectory setting.
        pub const USER_DIRECTORY: &str = "daft.hooks.userDirectory";

        /// Config key for hooks.timeout setting.
        pub const TIMEOUT: &str = "daft.hooks.timeout";

        /// Config key for hooks.output.quiet setting.
        pub const OUTPUT_QUIET: &str = "daft.hooks.output.quiet";

        /// Config key for hooks.output.timerDelay setting.
        pub const OUTPUT_TIMER_DELAY: &str = "daft.hooks.output.timerDelay";

        /// Config key for hooks.output.tailLines setting.
        pub const OUTPUT_TAIL_LINES: &str = "daft.hooks.output.tailLines";

        /// Config key for hooks.output.verbose setting.
        pub const OUTPUT_VERBOSE: &str = "daft.hooks.output.verbose";

        /// Config key for hooks.trustPrune setting (auto-prune stale trust entries).
        pub const TRUST_PRUNE: &str = "daft.hooks.trustPrune";

        /// Generate a config key for a hook-specific setting.
        pub fn hook_key(hook_name: &str, setting: &str) -> String {
            format!("daft.hooks.{hook_name}.{setting}")
        }
    }

    /// Completions config keys.
    pub mod completions {
        /// Config key for completions.branches.columns setting.
        pub const BRANCHES_COLUMNS: &str = "daft.completions.branches.columns";
    }
}

/// User-configurable settings for daft commands.
///
/// Settings are loaded from git config with the following priority:
/// 1. Repository-local config (`git config daft.x`)
/// 2. Global config (`git config --global daft.x`)
/// 3. Built-in defaults
#[derive(Debug, Clone)]
pub struct DaftSettings {
    /// CD into new worktrees (shell wrapper behavior).
    pub autocd: bool,

    /// Push new branches to remote after creation.
    pub checkout_push: bool,

    /// Fetch from remote before creating worktrees.
    pub checkout_fetch: bool,

    /// Set upstream tracking for branches.
    pub checkout_upstream: bool,

    /// When the automatic upstream push runs the repo's pre-push hook.
    pub checkout_push_verify: PushVerify,

    /// Default remote name for operations.
    pub remote: String,

    /// Default carry setting for checkout-branch command.
    pub checkout_branch_carry: bool,

    /// Default carry setting for checkout command.
    pub checkout_carry: bool,

    /// Where to cd after pruning the user's current worktree.
    pub prune_cd_target: PruneCdTarget,

    /// Default arguments for git pull in update command (same-branch mode).
    pub update_args: String,

    /// Whether multi-remote mode is enabled.
    pub multi_remote_enabled: bool,

    /// Default remote for multi-remote mode.
    pub multi_remote_default: String,

    /// Use gitoxide for supported git operations.
    pub use_gitoxide: bool,

    /// Automatically create worktree when branch not found in go command.
    pub go_auto_start: bool,

    /// Whether `daft go` completion should run `git fetch` when the typed
    /// prefix has no local matches. Controlled by `daft.go.fetchOnMiss`.
    pub go_fetch_on_miss: bool,

    /// Default statistics mode for list command.
    pub list_stat: Stat,

    /// Default statistics mode for sync command.
    pub sync_stat: Stat,

    /// Default statistics mode for prune command.
    pub prune_stat: Stat,

    /// Column selection for list command (None = use defaults).
    pub list_columns: Option<String>,

    /// Column selection for sync command (None = use defaults).
    pub sync_columns: Option<String>,

    /// Column selection for prune command (None = use defaults).
    pub prune_columns: Option<String>,

    /// Sort specification for list command (None = default branch ascending).
    pub list_sort: Option<String>,

    /// Sort specification for sync command (None = default branch ascending).
    pub sync_sort: Option<String>,

    /// Sort specification for prune command (None = default branch ascending).
    pub prune_sort: Option<String>,

    /// Delete remote branch when removing a branch/worktree.
    pub branch_delete_remote: bool,

    /// Strategy for deducing branch ownership from the commit range
    /// `base..branch`. Set via `daft.ownership.strategy`.
    pub ownership_strategy: crate::core::ownership::OwnershipStrategy,

    /// Wall-clock budget for one sync push unit (git + pre-push hook);
    /// `None` = no timeout. Set via `daft.sync.pushTimeout` (#678).
    pub sync_push_timeout: Option<std::time::Duration>,

    /// Whether the sync push resource governor is active (#678). Set via
    /// `daft.governor.mode`.
    pub governor_mode: GovernorMode,

    /// Concurrency cap for hook-bearing push tasks. Set via
    /// `daft.governor.jobs`.
    pub governor_jobs: GovernorJobs,

    /// Memory headroom the governor keeps free. Set via
    /// `daft.governor.memoryReserve`.
    pub governor_memory_reserve: MemoryReserve,

    /// Selected merge style — replaces the legacy `merge_ff` + `merge_squash`
    /// combination. See [`MergeStyle`] for variants.
    pub merge_style: crate::core::worktree::merge::MergeStyle,
    /// Selected post-merge cleanup outcome. See [`CleanupKind`] for variants.
    pub merge_cleanup: crate::core::worktree::merge::CleanupKind,

    /// Default commit behavior for merge command. Set via `daft.merge.commit`.
    pub merge_commit: bool,

    /// Default edit behavior for merge command. `None` = let git decide based
    /// on TTY. Set via `daft.merge.edit`.
    pub merge_edit: Option<bool>,

    /// Default signoff behavior for merge command. Set via `daft.merge.signoff`.
    pub merge_signoff: bool,

    /// Default GPG signing key for merge command. `None` = unset;
    /// `Some("")` = default key; `Some("KEYID")` = specific key. Set via
    /// `daft.merge.gpgSign` (values `true`/`false`/`<keyid>`).
    pub merge_gpg_sign: Option<String>,

    /// Default verify-signatures behavior for merge command. Set via
    /// `daft.merge.verifySignatures`.
    pub merge_verify_signatures: bool,

    /// Default allow-unrelated-histories behavior for merge command. Set via
    /// `daft.merge.allowUnrelatedHistories`.
    pub merge_allow_unrelated_histories: bool,

    /// Default merge strategy for merge command. Set via `daft.merge.strategy`.
    pub merge_strategy: Option<String>,

    /// Default strategy options for merge command. Comma-separated in config;
    /// stored here as a `Vec<String>`. Set via `daft.merge.strategyOption`.
    pub merge_strategy_options: Vec<String>,

    /// Default adopt-target-on-demand behavior for merge command. Set via
    /// `daft.merge.adoptTargetOnDemand` (`prompt` | `yes` | `no`).
    pub merge_adopt_target_on_demand: crate::core::worktree::merge::AdoptPreset,

    /// Require the target worktree to be clean before starting a merge. Set
    /// via `daft.merge.requireCleanTarget`.
    pub merge_require_clean_target: bool,
}

impl Default for DaftSettings {
    fn default() -> Self {
        Self {
            autocd: defaults::AUTOCD,
            checkout_push: defaults::CHECKOUT_PUSH,
            checkout_fetch: defaults::CHECKOUT_FETCH,
            checkout_upstream: defaults::CHECKOUT_UPSTREAM,
            checkout_push_verify: defaults::CHECKOUT_PUSH_VERIFY,
            remote: defaults::REMOTE.to_string(),
            checkout_branch_carry: defaults::CHECKOUT_BRANCH_CARRY,
            checkout_carry: defaults::CHECKOUT_CARRY,
            prune_cd_target: defaults::PRUNE_CD_TARGET,
            update_args: defaults::UPDATE_ARGS.to_string(),
            multi_remote_enabled: defaults::MULTI_REMOTE_ENABLED,
            multi_remote_default: defaults::MULTI_REMOTE_DEFAULT_REMOTE.to_string(),
            use_gitoxide: defaults::USE_GITOXIDE,
            go_auto_start: defaults::GO_AUTO_START,
            go_fetch_on_miss: defaults::GO_FETCH_ON_MISS,
            list_stat: defaults::LIST_STAT,
            sync_stat: defaults::SYNC_STAT,
            prune_stat: defaults::PRUNE_STAT,
            list_columns: None,
            sync_columns: None,
            prune_columns: None,
            list_sort: None,
            sync_sort: None,
            prune_sort: None,
            branch_delete_remote: defaults::BRANCH_DELETE_REMOTE,
            ownership_strategy: defaults::OWNERSHIP_STRATEGY,
            sync_push_timeout: defaults::SYNC_PUSH_TIMEOUT,
            governor_mode: defaults::GOVERNOR_MODE,
            governor_jobs: defaults::GOVERNOR_JOBS,
            governor_memory_reserve: defaults::GOVERNOR_MEMORY_RESERVE,
            merge_style: defaults::MERGE_STYLE,
            merge_cleanup: defaults::MERGE_CLEANUP,
            merge_commit: defaults::MERGE_COMMIT,
            merge_edit: None,
            merge_signoff: defaults::MERGE_SIGNOFF,
            merge_gpg_sign: None,
            merge_verify_signatures: defaults::MERGE_VERIFY_SIGNATURES,
            merge_allow_unrelated_histories: defaults::MERGE_ALLOW_UNRELATED_HISTORIES,
            merge_strategy: None,
            merge_strategy_options: Vec::new(),
            merge_adopt_target_on_demand: defaults::MERGE_ADOPT_TARGET_ON_DEMAND,
            merge_require_clean_target: defaults::MERGE_REQUIRE_CLEAN_TARGET,
        }
    }
}

impl DaftSettings {
    /// Load settings from git config (local + global).
    ///
    /// This method reads from the current repository's config,
    /// falling back to global config and then to defaults.
    ///
    /// Use this in commands that run inside a git repository.
    pub fn load() -> Result<Self> {
        Self::load_with(&GitCommand::new(true))
    }

    /// Like [`DaftSettings::load`], but reuses a caller-provided [`GitCommand`]
    /// so the `gix::discover()` it performs is shared with the caller's other
    /// config reads (hooks-config load, command body) instead of each
    /// constructing a throwaway instance that re-discovers the repo. See #584.
    pub fn load_with(git: &GitCommand) -> Result<Self> {
        let mut settings = Self::default();

        if let Some(value) = git.config_get(keys::AUTOCD)? {
            settings.autocd = parse_bool(&value, defaults::AUTOCD);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_PUSH)? {
            settings.checkout_push = parse_bool(&value, defaults::CHECKOUT_PUSH);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_FETCH)? {
            settings.checkout_fetch = parse_bool(&value, defaults::CHECKOUT_FETCH);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_UPSTREAM)? {
            settings.checkout_upstream = parse_bool(&value, defaults::CHECKOUT_UPSTREAM);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_PUSH_VERIFY)?
            && !value.is_empty()
        {
            match PushVerify::parse(&value) {
                Some(verify) => settings.checkout_push_verify = verify,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::CHECKOUT_PUSH_VERIFY,
                    value
                ),
            }
        }

        if let Some(value) = git.config_get(keys::REMOTE)?
            && !value.is_empty()
        {
            settings.remote = value;
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_BRANCH_CARRY)? {
            settings.checkout_branch_carry = parse_bool(&value, defaults::CHECKOUT_BRANCH_CARRY);
        }

        if let Some(value) = git.config_get(keys::CHECKOUT_CARRY)? {
            settings.checkout_carry = parse_bool(&value, defaults::CHECKOUT_CARRY);
        }

        if let Some(value) = git.config_get(keys::PRUNE_CD_TARGET)?
            && let Some(target) = PruneCdTarget::parse(&value)
        {
            settings.prune_cd_target = target;
        }

        // Try new key first, fall back to deprecated key for migration
        let update_args_value = git.config_get(keys::UPDATE_ARGS)?;
        let update_args_value = update_args_value.or(git.config_get(keys::FETCH_ARGS_DEPRECATED)?);
        if let Some(value) = update_args_value
            && !value.is_empty()
        {
            settings.update_args = value;
        }

        if let Some(value) = git.config_get(keys::multi_remote::ENABLED)? {
            settings.multi_remote_enabled = parse_bool(&value, defaults::MULTI_REMOTE_ENABLED);
        }

        if let Some(value) = git.config_get(keys::multi_remote::DEFAULT_REMOTE)?
            && !value.is_empty()
        {
            settings.multi_remote_default = value;
        }

        if let Some(value) = git.config_get(keys::experimental::GITOXIDE)? {
            settings.use_gitoxide = parse_bool(&value, defaults::USE_GITOXIDE);
        }

        if let Some(value) = git.config_get(keys::GO_AUTO_START)? {
            settings.go_auto_start = parse_bool(&value, defaults::GO_AUTO_START);
        }

        if let Some(value) = git.config_get(keys::GO_FETCH_ON_MISS)? {
            settings.go_fetch_on_miss = parse_bool(&value, defaults::GO_FETCH_ON_MISS);
        }

        if let Some(value) = git.config_get(keys::LIST_STAT)?
            && let Some(stat) = Stat::parse(&value)
        {
            settings.list_stat = stat;
        }

        if let Some(value) = git.config_get(keys::SYNC_STAT)?
            && let Some(stat) = Stat::parse(&value)
        {
            settings.sync_stat = stat;
        }

        if let Some(value) = git.config_get(keys::PRUNE_STAT)?
            && let Some(stat) = Stat::parse(&value)
        {
            settings.prune_stat = stat;
        }

        if let Some(value) = git.config_get(keys::LIST_COLUMNS)?
            && !value.is_empty()
        {
            settings.list_columns = Some(value);
        }

        if let Some(value) = git.config_get(keys::SYNC_COLUMNS)?
            && !value.is_empty()
        {
            settings.sync_columns = Some(value);
        }

        if let Some(value) = git.config_get(keys::PRUNE_COLUMNS)?
            && !value.is_empty()
        {
            settings.prune_columns = Some(value);
        }

        if let Some(value) = git.config_get(keys::LIST_SORT)?
            && !value.is_empty()
        {
            settings.list_sort = Some(value);
        }

        if let Some(value) = git.config_get(keys::SYNC_SORT)?
            && !value.is_empty()
        {
            settings.sync_sort = Some(value);
        }

        if let Some(value) = git.config_get(keys::PRUNE_SORT)?
            && !value.is_empty()
        {
            settings.prune_sort = Some(value);
        }

        if let Some(value) = git.config_get(keys::BRANCH_DELETE_REMOTE)? {
            settings.branch_delete_remote = parse_bool(&value, defaults::BRANCH_DELETE_REMOTE);
        }

        if let Some(value) = git.config_get(keys::OWNERSHIP_STRATEGY)?
            && !value.is_empty()
        {
            match crate::core::ownership::OwnershipStrategy::parse(&value) {
                Some(strategy) => settings.ownership_strategy = strategy,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::OWNERSHIP_STRATEGY,
                    value
                ),
            }
        }

        if let Some(value) = git.config_get(keys::SYNC_PUSH_TIMEOUT)?
            && !value.is_empty()
        {
            match parse_push_timeout(&value) {
                Some(timeout) => settings.sync_push_timeout = timeout,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::SYNC_PUSH_TIMEOUT,
                    value
                ),
            }
        }

        if let Some(value) = git.config_get(keys::GOVERNOR_MODE)?
            && !value.is_empty()
        {
            match GovernorMode::parse(&value) {
                Some(mode) => settings.governor_mode = mode,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::GOVERNOR_MODE,
                    value
                ),
            }
        }

        if let Some(value) = git.config_get(keys::GOVERNOR_JOBS)?
            && !value.is_empty()
        {
            match GovernorJobs::parse(&value) {
                Some(jobs) => settings.governor_jobs = jobs,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::GOVERNOR_JOBS,
                    value
                ),
            }
        }

        if let Some(value) = git.config_get(keys::GOVERNOR_MEMORY_RESERVE)?
            && !value.is_empty()
        {
            match MemoryReserve::parse(&value) {
                Some(reserve) => settings.governor_memory_reserve = reserve,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::GOVERNOR_MEMORY_RESERVE,
                    value
                ),
            }
        }

        load_merge_settings(git, &mut settings)?;
        validate_merge_settings(settings.merge_commit, settings.merge_cleanup)?;

        Ok(settings)
    }

    /// Load settings from global git config only.
    ///
    /// This method only reads from global config, ignoring repository-local config.
    /// Use this for commands that run before a repository exists (e.g., clone, init).
    pub fn load_global() -> Result<Self> {
        let git = GitCommand::new(true);
        let mut settings = Self::default();

        if let Some(value) = git.config_get_global(keys::AUTOCD)? {
            settings.autocd = parse_bool(&value, defaults::AUTOCD);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_PUSH)? {
            settings.checkout_push = parse_bool(&value, defaults::CHECKOUT_PUSH);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_FETCH)? {
            settings.checkout_fetch = parse_bool(&value, defaults::CHECKOUT_FETCH);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_UPSTREAM)? {
            settings.checkout_upstream = parse_bool(&value, defaults::CHECKOUT_UPSTREAM);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_PUSH_VERIFY)?
            && !value.is_empty()
        {
            match PushVerify::parse(&value) {
                Some(verify) => settings.checkout_push_verify = verify,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::CHECKOUT_PUSH_VERIFY,
                    value
                ),
            }
        }

        if let Some(value) = git.config_get_global(keys::REMOTE)?
            && !value.is_empty()
        {
            settings.remote = value;
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_BRANCH_CARRY)? {
            settings.checkout_branch_carry = parse_bool(&value, defaults::CHECKOUT_BRANCH_CARRY);
        }

        if let Some(value) = git.config_get_global(keys::CHECKOUT_CARRY)? {
            settings.checkout_carry = parse_bool(&value, defaults::CHECKOUT_CARRY);
        }

        if let Some(value) = git.config_get_global(keys::PRUNE_CD_TARGET)?
            && let Some(target) = PruneCdTarget::parse(&value)
        {
            settings.prune_cd_target = target;
        }

        // Try new key first, fall back to deprecated key for migration
        let update_args_value = git.config_get_global(keys::UPDATE_ARGS)?;
        let update_args_value =
            update_args_value.or(git.config_get_global(keys::FETCH_ARGS_DEPRECATED)?);
        if let Some(value) = update_args_value
            && !value.is_empty()
        {
            settings.update_args = value;
        }

        if let Some(value) = git.config_get_global(keys::multi_remote::ENABLED)? {
            settings.multi_remote_enabled = parse_bool(&value, defaults::MULTI_REMOTE_ENABLED);
        }

        if let Some(value) = git.config_get_global(keys::multi_remote::DEFAULT_REMOTE)?
            && !value.is_empty()
        {
            settings.multi_remote_default = value;
        }

        if let Some(value) = git.config_get_global(keys::experimental::GITOXIDE)? {
            settings.use_gitoxide = parse_bool(&value, defaults::USE_GITOXIDE);
        }

        if let Some(value) = git.config_get_global(keys::GO_AUTO_START)? {
            settings.go_auto_start = parse_bool(&value, defaults::GO_AUTO_START);
        }

        if let Some(value) = git.config_get_global(keys::GO_FETCH_ON_MISS)? {
            settings.go_fetch_on_miss = parse_bool(&value, defaults::GO_FETCH_ON_MISS);
        }

        if let Some(value) = git.config_get_global(keys::LIST_STAT)?
            && let Some(stat) = Stat::parse(&value)
        {
            settings.list_stat = stat;
        }

        if let Some(value) = git.config_get_global(keys::SYNC_STAT)?
            && let Some(stat) = Stat::parse(&value)
        {
            settings.sync_stat = stat;
        }

        if let Some(value) = git.config_get_global(keys::PRUNE_STAT)?
            && let Some(stat) = Stat::parse(&value)
        {
            settings.prune_stat = stat;
        }

        if let Some(value) = git.config_get_global(keys::LIST_COLUMNS)?
            && !value.is_empty()
        {
            settings.list_columns = Some(value);
        }

        if let Some(value) = git.config_get_global(keys::SYNC_COLUMNS)?
            && !value.is_empty()
        {
            settings.sync_columns = Some(value);
        }

        if let Some(value) = git.config_get_global(keys::PRUNE_COLUMNS)?
            && !value.is_empty()
        {
            settings.prune_columns = Some(value);
        }

        if let Some(value) = git.config_get_global(keys::LIST_SORT)?
            && !value.is_empty()
        {
            settings.list_sort = Some(value);
        }

        if let Some(value) = git.config_get_global(keys::SYNC_SORT)?
            && !value.is_empty()
        {
            settings.sync_sort = Some(value);
        }

        if let Some(value) = git.config_get_global(keys::PRUNE_SORT)?
            && !value.is_empty()
        {
            settings.prune_sort = Some(value);
        }

        if let Some(value) = git.config_get_global(keys::BRANCH_DELETE_REMOTE)? {
            settings.branch_delete_remote = parse_bool(&value, defaults::BRANCH_DELETE_REMOTE);
        }

        if let Some(value) = git.config_get_global(keys::OWNERSHIP_STRATEGY)?
            && !value.is_empty()
        {
            match crate::core::ownership::OwnershipStrategy::parse(&value) {
                Some(strategy) => settings.ownership_strategy = strategy,
                None => eprintln!(
                    "daft: unknown value for {}: {:?} — using default",
                    keys::OWNERSHIP_STRATEGY,
                    value
                ),
            }
        }

        Ok(settings)
    }
}

/// Load all `daft.merge.*` keys from the given [`GitCommand`] into `settings`.
///
/// Extracted from [`DaftSettings::load`] for readability and to keep the load
/// path cohesive for Slice 13's fourteen merge keys. Invalid values for enum
/// keys (`ff`, `adoptTargetOnDemand`) silently fall back to the built-in
/// default, matching the existing pattern for `list.stat` etc.
fn load_merge_settings(git: &GitCommand, settings: &mut DaftSettings) -> Result<()> {
    use crate::core::worktree::merge::{AdoptPreset, CleanupKind, MergeStyle};

    if let Some(value) = git.config_get(keys::MERGE_STYLE)? {
        settings.merge_style = match value.as_str() {
            "merge" => MergeStyle::Merge,
            "squash" => MergeStyle::Squash,
            "rebase" => MergeStyle::Rebase,
            "rebase-merge" => MergeStyle::RebaseMerge,
            _ => defaults::MERGE_STYLE,
        };
    }
    if let Some(value) = git.config_get(keys::MERGE_CLEANUP)? {
        settings.merge_cleanup = match value.as_str() {
            "keep" => CleanupKind::Keep,
            "remove-branch" => CleanupKind::RemoveBranch,
            _ => defaults::MERGE_CLEANUP,
        };
    }

    if let Some(value) = git.config_get(keys::MERGE_COMMIT)? {
        settings.merge_commit = parse_bool(&value, defaults::MERGE_COMMIT);
    }

    if let Some(value) = git.config_get(keys::MERGE_EDIT)? {
        // `Some(bool)`: user expressed a preference either way.
        settings.merge_edit = Some(parse_bool(&value, true));
    }

    if let Some(value) = git.config_get(keys::MERGE_SIGNOFF)? {
        settings.merge_signoff = parse_bool(&value, defaults::MERGE_SIGNOFF);
    }

    if let Some(value) = git.config_get(keys::MERGE_GPG_SIGN)? {
        // Tri-state: "true" = default key, "false" = unset, anything else = KEYID.
        settings.merge_gpg_sign = match value.to_lowercase().as_str() {
            "true" | "yes" | "on" | "1" => Some(String::new()),
            "false" | "no" | "off" | "0" => None,
            _ => Some(value),
        };
    }

    if let Some(value) = git.config_get(keys::MERGE_VERIFY_SIGNATURES)? {
        settings.merge_verify_signatures = parse_bool(&value, defaults::MERGE_VERIFY_SIGNATURES);
    }

    if let Some(value) = git.config_get(keys::MERGE_ALLOW_UNRELATED_HISTORIES)? {
        settings.merge_allow_unrelated_histories =
            parse_bool(&value, defaults::MERGE_ALLOW_UNRELATED_HISTORIES);
    }

    if let Some(value) = git.config_get(keys::MERGE_STRATEGY)?
        && !value.is_empty()
    {
        settings.merge_strategy = Some(value);
    }

    if let Some(value) = git.config_get(keys::MERGE_STRATEGY_OPTION)? {
        // Comma-separated list; empty/whitespace entries dropped so configuring
        // a trailing comma doesn't inject an empty `-X` token at render time.
        settings.merge_strategy_options = value
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    if let Some(value) = git.config_get(keys::MERGE_ADOPT_TARGET_ON_DEMAND)? {
        settings.merge_adopt_target_on_demand = match value.as_str() {
            "prompt" => AdoptPreset::Prompt,
            "yes" => AdoptPreset::Yes,
            "no" => AdoptPreset::No,
            _ => defaults::MERGE_ADOPT_TARGET_ON_DEMAND,
        };
    }

    if let Some(value) = git.config_get(keys::MERGE_REQUIRE_CLEAN_TARGET)? {
        settings.merge_require_clean_target =
            parse_bool(&value, defaults::MERGE_REQUIRE_CLEAN_TARGET);
    }

    Ok(())
}

/// Validate that merge settings are internally consistent.
///
/// Returns an error if `daft.merge.commit = false` is combined with
/// `daft.merge.cleanup = remove-branch` — cleanup requires a committed merge.
pub(crate) fn validate_merge_settings(
    merge_commit: bool,
    cleanup: crate::core::worktree::merge::CleanupKind,
) -> Result<()> {
    use crate::core::worktree::merge::CleanupKind;
    if !merge_commit && cleanup == CleanupKind::RemoveBranch {
        anyhow::bail!(
            "daft.merge.commit = false is incompatible with \
             daft.merge.cleanup = remove-branch: \
             branch cleanup requires a committed merge to justify deletion"
        );
    }
    Ok(())
}

/// Parse a git config boolean value.
///
/// Git accepts various boolean representations:
/// - true: `true`, `yes`, `on`, `1`
/// - false: `false`, `no`, `off`, `0`
///
/// Returns the default value if parsing fails.
pub(crate) fn parse_bool(value: &str, default: bool) -> bool {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => true,
        "false" | "no" | "off" | "0" => false,
        _ => default,
    }
}

/// Configuration for hook output display.
#[derive(Debug, Clone)]
pub struct HookOutputConfig {
    /// Suppress hook stdout/stderr (only show spinner + result line).
    pub quiet: bool,
    /// Seconds before showing elapsed timer on spinners.
    pub timer_delay_secs: u32,
    /// Number of rolling output tail lines per job (0 = no tail).
    pub tail_lines: u32,
    /// Show verbose output including skipped jobs and their reasons.
    pub verbose: bool,
    /// When true, on job finish print a single compact row
    /// (`✓ name (dur)` / `✗ name (dur)`) and drop the inline output dump.
    /// Hooks leave this false; `daft exec` sets it true.
    pub compact_finalization: bool,
}

impl Default for HookOutputConfig {
    fn default() -> Self {
        Self {
            quiet: false,
            timer_delay_secs: 5,
            tail_lines: 6,
            verbose: false,
            compact_finalization: false,
        }
    }
}

impl HookOutputConfig {
    /// The configured output settings with the CLI's `-v` folded in — the
    /// flag opts into the full hook block on the rail (#651) on top of
    /// whatever `daft.hooks.output.verbose` says.
    pub fn with_cli_verbose(&self, cli_verbose: bool) -> Self {
        let mut config = self.clone();
        config.verbose |= cli_verbose;
        config
    }
}

/// Load hooks configuration from git config.
///
/// This loads hooks settings from the current repository's config,
/// falling back to global config and then to defaults.
pub fn load_hooks_config() -> Result<HooksConfig> {
    load_hooks_config_with(&GitCommand::new(true))
}

/// Like [`load_hooks_config`], but reuses a caller-provided [`GitCommand`] so
/// the `gix::discover()` it performs is shared with the caller's settings load
/// and command body rather than re-discovering the repo. See [`DaftSettings::load_with`].
pub fn load_hooks_config_with(git: &GitCommand) -> Result<HooksConfig> {
    let mut config = HooksConfig::default();

    // Load global hooks settings
    if let Some(value) = git.config_get(keys::hooks::ENABLED)? {
        config.enabled = parse_bool(&value, true);
    }

    if let Some(value) = git.config_get(keys::hooks::DEFAULT_TRUST)?
        && let Some(level) = TrustLevel::parse(&value)
    {
        config.default_trust = level;
    }

    if let Some(value) = git.config_get(keys::hooks::USER_DIRECTORY)?
        && !value.is_empty()
    {
        // Expand ~ to home directory
        let expanded = if let Some(stripped) = value.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped)
            } else {
                PathBuf::from(&value)
            }
        } else {
            PathBuf::from(&value)
        };
        config.user_directory = expanded;
    }

    if let Some(value) = git.config_get(keys::hooks::TIMEOUT)?
        && let Ok(timeout) = value.parse::<u32>()
    {
        config.timeout_seconds = timeout;
    }

    // Load output settings
    if let Some(value) = git.config_get(keys::hooks::OUTPUT_QUIET)? {
        config.output.quiet = parse_bool(&value, false);
    }
    if let Some(value) = git.config_get(keys::hooks::OUTPUT_TIMER_DELAY)?
        && let Ok(delay) = value.parse::<u32>()
    {
        config.output.timer_delay_secs = delay;
    }
    if let Some(value) = git.config_get(keys::hooks::OUTPUT_TAIL_LINES)?
        && let Ok(lines) = value.parse::<u32>()
    {
        config.output.tail_lines = lines;
    }
    if let Some(value) = git.config_get(keys::hooks::OUTPUT_VERBOSE)? {
        config.output.verbose = parse_bool(&value, false);
    }

    // Load per-hook settings
    for hook_type in HookType::all() {
        let hook_config = config.get_hook_config_mut(*hook_type);
        load_hook_type_config(git, *hook_type, hook_config)?;
    }

    Ok(config)
}

/// Load hooks configuration from global git config only.
///
/// Use this for commands that run before a repository exists (e.g., clone).
pub fn load_hooks_config_global() -> Result<HooksConfig> {
    let git = GitCommand::new(true);
    let mut config = HooksConfig::default();

    // Load global hooks settings
    if let Some(value) = git.config_get_global(keys::hooks::ENABLED)? {
        config.enabled = parse_bool(&value, true);
    }

    if let Some(value) = git.config_get_global(keys::hooks::DEFAULT_TRUST)?
        && let Some(level) = TrustLevel::parse(&value)
    {
        config.default_trust = level;
    }

    if let Some(value) = git.config_get_global(keys::hooks::USER_DIRECTORY)?
        && !value.is_empty()
    {
        let expanded = if let Some(stripped) = value.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(stripped)
            } else {
                PathBuf::from(&value)
            }
        } else {
            PathBuf::from(&value)
        };
        config.user_directory = expanded;
    }

    if let Some(value) = git.config_get_global(keys::hooks::TIMEOUT)?
        && let Ok(timeout) = value.parse::<u32>()
    {
        config.timeout_seconds = timeout;
    }

    // Load output settings
    if let Some(value) = git.config_get_global(keys::hooks::OUTPUT_QUIET)? {
        config.output.quiet = parse_bool(&value, false);
    }
    if let Some(value) = git.config_get_global(keys::hooks::OUTPUT_TIMER_DELAY)?
        && let Ok(delay) = value.parse::<u32>()
    {
        config.output.timer_delay_secs = delay;
    }
    if let Some(value) = git.config_get_global(keys::hooks::OUTPUT_TAIL_LINES)?
        && let Ok(lines) = value.parse::<u32>()
    {
        config.output.tail_lines = lines;
    }
    if let Some(value) = git.config_get_global(keys::hooks::OUTPUT_VERBOSE)? {
        config.output.verbose = parse_bool(&value, false);
    }

    // Load per-hook settings from global config
    for hook_type in HookType::all() {
        let hook_config = config.get_hook_config_mut(*hook_type);
        load_hook_type_config_global(&git, *hook_type, hook_config)?;
    }

    Ok(config)
}

/// Load configuration for a specific hook type.
///
/// Falls back to deprecated config keys if the new key is not found.
fn load_hook_type_config(
    git: &GitCommand,
    hook_type: HookType,
    hook_config: &mut HookConfig,
) -> Result<()> {
    let enabled_key = keys::hooks::hook_key(hook_type.config_key(), "enabled");
    let enabled_value = match (
        git.config_get(&enabled_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get(&keys::hooks::hook_key(dep, "enabled"))?,
        (None, None) => None,
    };
    if let Some(value) = enabled_value {
        hook_config.enabled = parse_bool(&value, true);
    }

    let fail_mode_key = keys::hooks::hook_key(hook_type.config_key(), "failMode");
    let fail_mode_value = match (
        git.config_get(&fail_mode_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get(&keys::hooks::hook_key(dep, "failMode"))?,
        (None, None) => None,
    };
    if let Some(value) = fail_mode_value
        && let Some(mode) = FailMode::parse(&value)
    {
        hook_config.fail_mode = mode;
    }

    Ok(())
}

/// Load configuration for a specific hook type from global config only.
///
/// Falls back to deprecated config keys if the new key is not found.
fn load_hook_type_config_global(
    git: &GitCommand,
    hook_type: HookType,
    hook_config: &mut HookConfig,
) -> Result<()> {
    let enabled_key = keys::hooks::hook_key(hook_type.config_key(), "enabled");
    let enabled_value = match (
        git.config_get_global(&enabled_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get_global(&keys::hooks::hook_key(dep, "enabled"))?,
        (None, None) => None,
    };
    if let Some(value) = enabled_value {
        hook_config.enabled = parse_bool(&value, true);
    }

    let fail_mode_key = keys::hooks::hook_key(hook_type.config_key(), "failMode");
    let fail_mode_value = match (
        git.config_get_global(&fail_mode_key)?,
        hook_type.deprecated_config_key(),
    ) {
        (Some(v), _) => Some(v),
        (None, Some(dep)) => git.config_get_global(&keys::hooks::hook_key(dep, "failMode"))?,
        (None, None) => None,
    };
    if let Some(value) = fail_mode_value
        && let Some(mode) = FailMode::parse(&value)
    {
        hook_config.fail_mode = mode;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = DaftSettings::default();
        assert!(settings.autocd);
        assert!(!settings.checkout_push);
        assert!(!settings.checkout_fetch);
        assert!(settings.checkout_upstream);
        assert_eq!(settings.remote, "origin");
        assert!(settings.checkout_branch_carry);
        assert!(!settings.checkout_carry);
        assert_eq!(settings.prune_cd_target, PruneCdTarget::Root);
        assert_eq!(settings.update_args, "--ff-only");
        assert!(!settings.multi_remote_enabled);
        assert_eq!(settings.multi_remote_default, "origin");
        assert!(!settings.use_gitoxide);
        assert!(!settings.go_auto_start);
        assert_eq!(settings.list_stat, Stat::Summary);
        assert!(!settings.branch_delete_remote);
        assert_eq!(
            settings.ownership_strategy,
            crate::core::ownership::OwnershipStrategy::RecencyPlurality
        );
        assert_eq!(settings.governor_mode, GovernorMode::Auto);
        assert_eq!(settings.governor_jobs, GovernorJobs::Auto);
        assert_eq!(settings.governor_memory_reserve, MemoryReserve::Auto);
    }

    #[test]
    fn governor_mode_parse() {
        assert_eq!(GovernorMode::parse("auto"), Some(GovernorMode::Auto));
        assert_eq!(GovernorMode::parse("AUTO"), Some(GovernorMode::Auto));
        assert_eq!(GovernorMode::parse("off"), Some(GovernorMode::Off));
        assert_eq!(GovernorMode::parse("Off"), Some(GovernorMode::Off));
        assert_eq!(GovernorMode::parse("on"), None);
        assert_eq!(GovernorMode::parse(""), None);
    }

    #[test]
    fn governor_jobs_parse() {
        assert_eq!(GovernorJobs::parse("auto"), Some(GovernorJobs::Auto));
        assert_eq!(GovernorJobs::parse(" Auto "), Some(GovernorJobs::Auto));
        assert_eq!(GovernorJobs::parse("1"), Some(GovernorJobs::Fixed(1)));
        assert_eq!(GovernorJobs::parse("16"), Some(GovernorJobs::Fixed(16)));
        assert_eq!(GovernorJobs::parse("0"), None);
        assert_eq!(GovernorJobs::parse("-2"), None);
        assert_eq!(GovernorJobs::parse("many"), None);
    }

    #[test]
    fn governor_jobs_effective_cap() {
        // Auto = max(2, cores/4).
        assert_eq!(GovernorJobs::Auto.effective(4), 2);
        assert_eq!(GovernorJobs::Auto.effective(10), 2);
        assert_eq!(GovernorJobs::Auto.effective(16), 4);
        assert_eq!(GovernorJobs::Auto.effective(32), 8);
        assert_eq!(GovernorJobs::Auto.effective(1), 2);
        assert_eq!(GovernorJobs::Fixed(6).effective(32), 6);
        assert_eq!(GovernorJobs::Fixed(0).effective(32), 1);
    }

    #[test]
    fn memory_reserve_parse() {
        assert_eq!(MemoryReserve::parse("auto"), Some(MemoryReserve::Auto));
        assert_eq!(
            MemoryReserve::parse("2G"),
            Some(MemoryReserve::Bytes(2 << 30))
        );
        assert_eq!(
            MemoryReserve::parse("512m"),
            Some(MemoryReserve::Bytes(512 << 20))
        );
        assert_eq!(
            MemoryReserve::parse("1024K"),
            Some(MemoryReserve::Bytes(1 << 20))
        );
        assert_eq!(
            MemoryReserve::parse("1048576"),
            Some(MemoryReserve::Bytes(1 << 20))
        );
        assert_eq!(
            MemoryReserve::parse("15%"),
            Some(MemoryReserve::Percent(15))
        );
        assert_eq!(MemoryReserve::parse("0%"), None);
        assert_eq!(MemoryReserve::parse("100%"), None);
        assert_eq!(MemoryReserve::parse("0"), None);
        assert_eq!(MemoryReserve::parse("2T"), None);
        assert_eq!(MemoryReserve::parse("lots"), None);
        // Multi-byte junk must not panic the suffix split.
        assert_eq!(MemoryReserve::parse("2é"), None);
    }

    #[test]
    fn push_timeout_parse() {
        use std::time::Duration;
        assert_eq!(parse_push_timeout("off"), Some(None));
        assert_eq!(parse_push_timeout("OFF"), Some(None));
        assert_eq!(parse_push_timeout("0"), Some(None));
        assert_eq!(
            parse_push_timeout("30m"),
            Some(Some(Duration::from_secs(1_800)))
        );
        assert_eq!(parse_push_timeout("2s"), Some(Some(Duration::from_secs(2))));
        assert_eq!(
            parse_push_timeout("1h"),
            Some(Some(Duration::from_secs(3_600)))
        );
        assert_eq!(
            parse_push_timeout("90"),
            Some(Some(Duration::from_secs(90)))
        );
        assert_eq!(parse_push_timeout("forever"), None);
        assert_eq!(parse_push_timeout("-5m"), None);
        assert_eq!(parse_push_timeout(""), None);
    }

    #[test]
    fn memory_reserve_resolve() {
        const GIB: u64 = 1 << 30;
        // Auto = max(10% RAM, 2 GiB): floor on small machines…
        assert_eq!(MemoryReserve::Auto.resolve(8 * GIB), 2 * GIB);
        // …and 10% on big ones.
        assert_eq!(MemoryReserve::Auto.resolve(64 * GIB), 64 * GIB / 10);
        assert_eq!(MemoryReserve::Bytes(GIB).resolve(64 * GIB), GIB);
        assert_eq!(MemoryReserve::Percent(25).resolve(64 * GIB), 16 * GIB);
    }

    #[test]
    fn test_prune_cd_target_parse() {
        assert_eq!(PruneCdTarget::parse("root"), Some(PruneCdTarget::Root));
        assert_eq!(PruneCdTarget::parse("Root"), Some(PruneCdTarget::Root));
        assert_eq!(PruneCdTarget::parse("ROOT"), Some(PruneCdTarget::Root));
        assert_eq!(
            PruneCdTarget::parse("default-branch"),
            Some(PruneCdTarget::DefaultBranch)
        );
        assert_eq!(
            PruneCdTarget::parse("Default-Branch"),
            Some(PruneCdTarget::DefaultBranch)
        );
        assert_eq!(PruneCdTarget::parse("invalid"), None);
        assert_eq!(PruneCdTarget::parse(""), None);
    }

    #[test]
    fn test_push_verify_parse() {
        assert_eq!(PushVerify::parse("auto"), Some(PushVerify::Auto));
        assert_eq!(PushVerify::parse("always"), Some(PushVerify::Always));
        assert_eq!(PushVerify::parse("never"), Some(PushVerify::Never));
        // Case-insensitive, like the sibling config-enum parsers.
        assert_eq!(PushVerify::parse("Auto"), Some(PushVerify::Auto));
        assert_eq!(PushVerify::parse("ALWAYS"), Some(PushVerify::Always));
        assert_eq!(PushVerify::parse("Never"), Some(PushVerify::Never));
        // Unknown and empty values are rejected (caller keeps the default).
        assert_eq!(PushVerify::parse("verify"), None);
        assert_eq!(PushVerify::parse(""), None);
    }

    #[test]
    fn test_parse_bool_true_variants() {
        assert!(parse_bool("true", false));
        assert!(parse_bool("True", false));
        assert!(parse_bool("TRUE", false));
        assert!(parse_bool("yes", false));
        assert!(parse_bool("Yes", false));
        assert!(parse_bool("YES", false));
        assert!(parse_bool("on", false));
        assert!(parse_bool("On", false));
        assert!(parse_bool("ON", false));
        assert!(parse_bool("1", false));
    }

    #[test]
    fn test_parse_bool_false_variants() {
        assert!(!parse_bool("false", true));
        assert!(!parse_bool("False", true));
        assert!(!parse_bool("FALSE", true));
        assert!(!parse_bool("no", true));
        assert!(!parse_bool("No", true));
        assert!(!parse_bool("NO", true));
        assert!(!parse_bool("off", true));
        assert!(!parse_bool("Off", true));
        assert!(!parse_bool("OFF", true));
        assert!(!parse_bool("0", true));
    }

    #[test]
    fn test_parse_bool_invalid_returns_default() {
        assert!(parse_bool("invalid", true));
        assert!(!parse_bool("invalid", false));
        assert!(parse_bool("", true));
        assert!(!parse_bool("", false));
        assert!(parse_bool("maybe", true));
        assert!(!parse_bool("maybe", false));
    }

    #[test]
    fn test_hook_output_config_defaults() {
        let config = HookOutputConfig::default();
        assert!(!config.quiet);
        assert_eq!(config.timer_delay_secs, 5);
        assert_eq!(config.tail_lines, 6);
    }

    #[test]
    fn test_default_column_settings() {
        let settings = DaftSettings::default();
        assert!(settings.list_columns.is_none());
        assert!(settings.sync_columns.is_none());
        assert!(settings.prune_columns.is_none());
    }

    #[test]
    fn default_settings_have_go_fetch_on_miss_true() {
        let settings = DaftSettings::default();
        assert!(
            settings.go_fetch_on_miss,
            "go.fetchOnMiss must default to true — the fetch-on-miss spinner \
             path is opt-out, not opt-in"
        );
    }

    #[test]
    fn default_ownership_strategy_is_recency_plurality() {
        let settings = DaftSettings::default();
        assert_eq!(
            settings.ownership_strategy,
            crate::core::ownership::OwnershipStrategy::RecencyPlurality
        );
    }

    #[test]
    fn defaults_for_merge() {
        let s = DaftSettings::default();
        assert_eq!(
            s.merge_style,
            crate::core::worktree::merge::MergeStyle::Merge
        );
        assert_eq!(
            s.merge_cleanup,
            crate::core::worktree::merge::CleanupKind::Keep
        );
        assert!(s.merge_commit);
        assert!(s.merge_require_clean_target);
        assert_eq!(
            s.merge_adopt_target_on_demand,
            crate::core::worktree::merge::AdoptPreset::Prompt
        );
        assert!(s.merge_strategy.is_none());
        assert!(s.merge_strategy_options.is_empty());
        assert!(s.merge_edit.is_none());
        assert!(s.merge_gpg_sign.is_none());
        assert!(!s.merge_signoff);
        assert!(!s.merge_verify_signatures);
        assert!(!s.merge_allow_unrelated_histories);
    }

    #[test]
    fn refuses_no_commit_with_remove_branch_cleanup() {
        use crate::core::worktree::merge::CleanupKind;
        let result = validate_merge_settings(false, CleanupKind::RemoveBranch);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("daft.merge.commit"));
        assert!(msg.contains("remove-branch"));
    }

    #[test]
    fn allows_compatible_merge_settings() {
        use crate::core::worktree::merge::CleanupKind;
        assert!(validate_merge_settings(true, CleanupKind::RemoveBranch).is_ok());
        assert!(validate_merge_settings(false, CleanupKind::Keep).is_ok());
        assert!(validate_merge_settings(true, CleanupKind::Keep).is_ok());
    }

    #[test]
    fn merge_style_default_is_merge() {
        let s = DaftSettings::default();
        assert_eq!(
            s.merge_style,
            crate::core::worktree::merge::MergeStyle::Merge
        );
    }

    #[test]
    fn merge_cleanup_default_is_keep() {
        let s = DaftSettings::default();
        assert_eq!(
            s.merge_cleanup,
            crate::core::worktree::merge::CleanupKind::Keep
        );
    }
}

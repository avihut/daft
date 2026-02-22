//! YAML configuration data structures for the hooks system.
//!
//! This module defines the serde-deserializable structs that represent
//! a `daft.yml` configuration file.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Known hook names that are recognized by the system.
pub const KNOWN_HOOK_NAMES: &[&str] = &[
    "post-clone",
    "worktree-pre-create",
    "worktree-post-create",
    "worktree-pre-remove",
    "worktree-post-remove",
];

/// Top-level YAML configuration.
///
/// The main `daft.yml` file maps to this struct. Hook definitions are
/// stored in the `hooks` map, keyed by hook name (e.g., "post-clone",
/// "pre-commit").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct YamlConfig {
    /// Minimum daft version required to use this config.
    pub min_version: Option<String>,

    /// Whether to use colored output.
    pub colors: Option<bool>,

    /// Whether to disable TTY detection.
    pub no_tty: Option<bool>,

    /// Shell RC file to source before running hooks.
    pub rc: Option<String>,

    /// Output settings (list of hook names to show output for, or false to suppress all).
    pub output: Option<OutputSetting>,

    /// List of additional config files to extend from.
    pub extends: Option<Vec<String>>,

    /// Directory for script files (default: ".daft").
    pub source_dir: Option<String>,

    /// Directory for local (gitignored) script files (default: ".daft-local").
    pub source_dir_local: Option<String>,

    /// Hook definitions, keyed by hook name.
    pub hooks: HashMap<String, HookDef>,
}

/// Output setting: either a list of hook names or false to suppress.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputSetting {
    /// Suppress all hook output.
    Disabled(bool),
    /// Show output only for these hooks.
    Hooks(Vec<String>),
}

/// Definition for a single hook type.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HookDef {
    /// Run jobs in parallel.
    pub parallel: Option<bool>,

    /// Run jobs sequentially, stop on first failure.
    pub piped: Option<bool>,

    /// Run jobs sequentially, continue on failure.
    pub follow: Option<bool>,

    /// Tags to exclude at hook level.
    pub exclude_tags: Option<Vec<String>>,

    /// Glob patterns to exclude at hook level.
    pub exclude: Option<Vec<String>>,

    /// Skip condition at hook level.
    pub skip: Option<SkipCondition>,

    /// Only condition at hook level.
    pub only: Option<OnlyCondition>,

    /// List of jobs to execute.
    pub jobs: Option<Vec<JobDef>>,

    /// Legacy alias for jobs (commands map).
    pub commands: Option<HashMap<String, CommandDef>>,
}

/// Target operating system for platform constraints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TargetOs {
    Macos,
    Linux,
    Windows,
}

impl TargetOs {
    /// Return the OS string as used by `std::env::consts::OS`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetOs::Macos => "macos",
            TargetOs::Linux => "linux",
            TargetOs::Windows => "windows",
        }
    }
}

/// Target CPU architecture for platform constraints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetArch {
    X86_64,
    Aarch64,
}

impl TargetArch {
    /// Return the arch string as used by `std::env::consts::ARCH`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetArch::X86_64 => "x86_64",
            TargetArch::Aarch64 => "aarch64",
        }
    }
}

/// A platform constraint that can be a single value or a list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformConstraint<T> {
    Single(T),
    List(Vec<T>),
}

impl<T> PlatformConstraint<T> {
    /// Return the values as a slice.
    pub fn as_slice(&self) -> &[T] {
        match self {
            PlatformConstraint::Single(v) => std::slice::from_ref(v),
            PlatformConstraint::List(v) => v,
        }
    }
}

/// A run command that can be a simple string or OS-keyed map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunCommand {
    /// Simple string command (runs on all platforms).
    Simple(String),
    /// OS-keyed map of commands.
    Platform(HashMap<TargetOs, PlatformRunCommand>),
}

/// A platform-specific run command (string or list).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformRunCommand {
    /// Single command string.
    Simple(String),
    /// List of commands joined with " && ".
    List(Vec<String>),
}

impl RunCommand {
    pub fn resolve_for_current_os(&self) -> Option<String> {
        match self {
            RunCommand::Simple(s) => Some(s.clone()),
            RunCommand::Platform(map) => {
                let current_os = Self::current_target_os()?;
                map.get(&current_os).map(|cmd| cmd.to_command_string())
            }
        }
    }

    pub fn is_platform(&self) -> bool {
        matches!(self, RunCommand::Platform(_))
    }

    pub fn current_target_os() -> Option<TargetOs> {
        match std::env::consts::OS {
            "macos" => Some(TargetOs::Macos),
            "linux" => Some(TargetOs::Linux),
            "windows" => Some(TargetOs::Windows),
            _ => None,
        }
    }
}

impl PlatformRunCommand {
    pub fn to_command_string(&self) -> String {
        match self {
            PlatformRunCommand::Simple(s) => s.clone(),
            PlatformRunCommand::List(cmds) => cmds.join(" && "),
        }
    }
}

/// A single job definition within a hook.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JobDef {
    /// Optional name for the job (used for merging and display).
    pub name: Option<String>,

    /// Human-readable description of what this job does.
    pub description: Option<String>,

    /// Shell command to run (simple string or OS-keyed map).
    pub run: Option<RunCommand>,

    /// Script file to run (relative to source_dir).
    pub script: Option<String>,

    /// Runner for script files (e.g., "bash", "python").
    pub runner: Option<String>,

    /// Arguments to pass to the script.
    pub args: Option<String>,

    /// Working directory (relative to worktree root).
    pub root: Option<String>,

    /// Tags for this job (for filtering).
    pub tags: Option<Vec<String>>,

    /// Skip condition.
    pub skip: Option<SkipCondition>,

    /// Only condition.
    pub only: Option<OnlyCondition>,

    /// Restrict job to specific CPU architectures.
    pub arch: Option<PlatformConstraint<TargetArch>>,

    /// Extra environment variables.
    pub env: Option<HashMap<String, String>>,

    /// Custom failure message.
    pub fail_text: Option<String>,

    /// Whether this job needs TTY/stdin (forces sequential).
    pub interactive: Option<bool>,

    /// Priority for execution ordering (lower runs first).
    pub priority: Option<i32>,

    /// Names of jobs that must complete before this job runs.
    pub needs: Option<Vec<String>>,

    /// Nested group of jobs.
    pub group: Option<GroupDef>,
}

/// Legacy command definition (alias for JobDef).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommandDef {
    pub run: Option<String>,
    pub script: Option<String>,
    pub runner: Option<String>,
    pub tags: Option<Vec<String>>,
    pub skip: Option<SkipCondition>,
    pub env: Option<HashMap<String, String>>,
}

impl CommandDef {
    /// Convert a legacy CommandDef to a JobDef.
    pub fn to_job_def(&self, name: &str) -> JobDef {
        JobDef {
            name: Some(name.to_string()),
            run: self.run.as_ref().map(|r| RunCommand::Simple(r.clone())),
            script: self.script.clone(),
            runner: self.runner.clone(),
            tags: self.tags.clone(),
            skip: self.skip.clone(),
            env: self.env.clone(),
            ..Default::default()
        }
    }
}

/// Skip condition: bool, string, or list of skip rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkipCondition {
    /// Always skip (true) or never skip (false).
    Bool(bool),
    /// Skip if this env var is set and truthy.
    EnvVar(String),
    /// List of skip rules (any match → skip).
    Rules(Vec<SkipRule>),
}

/// A single skip rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkipRule {
    /// Named condition: "merge" or "rebase".
    Named(String),
    /// Structured condition.
    Structured(SkipRuleStructured),
}

/// Structured skip rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkipRuleStructured {
    /// Skip if current ref matches this pattern.
    #[serde(rename = "ref")]
    pub ref_pattern: Option<String>,
    /// Skip if this env var is set and truthy.
    pub env: Option<String>,
    /// Skip if this command exits 0.
    pub run: Option<String>,
    /// Human-readable description of why this skip rule exists.
    pub desc: Option<String>,
}

/// Only condition: mirrors SkipCondition but with inverse semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OnlyCondition {
    /// Only run if true, never run if false.
    Bool(bool),
    /// Only run if this env var is set and truthy.
    EnvVar(String),
    /// List of only rules (all must match → run).
    Rules(Vec<OnlyRule>),
}

/// A single only rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OnlyRule {
    /// Named condition: "merge" or "rebase".
    Named(String),
    /// Structured condition.
    Structured(OnlyRuleStructured),
}

/// Structured only rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlyRuleStructured {
    /// Only run if current ref matches this pattern.
    #[serde(rename = "ref")]
    pub ref_pattern: Option<String>,
    /// Only run if this env var is set and truthy.
    pub env: Option<String>,
    /// Only run if this command exits 0.
    pub run: Option<String>,
    /// Human-readable description of why this only rule exists.
    pub desc: Option<String>,
}

/// A group of jobs that runs as a unit.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GroupDef {
    /// Run grouped jobs in parallel.
    pub parallel: Option<bool>,
    /// Run grouped jobs sequentially, stop on first failure.
    pub piped: Option<bool>,
    /// Nested jobs in this group.
    pub jobs: Option<Vec<JobDef>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: setup
        run: echo "hello"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.hooks.contains_key("worktree-post-create"));
        let hook = &config.hooks["worktree-post-create"];
        let jobs = hook.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name.as_deref(), Some("setup"));
        match &jobs[0].run {
            Some(RunCommand::Simple(s)) => assert_eq!(s, "echo \"hello\""),
            other => panic!("Expected Simple, got {other:?}"),
        }
    }

    #[test]
    fn test_empty_config() {
        let yaml = "";
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.hooks.is_empty());
        assert!(config.min_version.is_none());
    }

    #[test]
    fn test_full_config() {
        let yaml = r#"
min_version: "1.0.20"
colors: true
no_tty: false
source_dir: ".daft"
extends:
  - shared.yml
hooks:
  worktree-pre-create:
    parallel: true
    jobs:
      - name: lint
        run: cargo clippy
        tags:
          - lint
        priority: 1
      - name: format
        run: cargo fmt --check
        tags:
          - format
        priority: 2
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
        skip: CI
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.min_version.as_deref(), Some("1.0.20"));
        assert_eq!(config.colors, Some(true));
        assert_eq!(config.extends.as_ref().unwrap().len(), 1);

        let pre_create = &config.hooks["worktree-pre-create"];
        assert_eq!(pre_create.parallel, Some(true));
        let jobs = pre_create.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].priority, Some(1));
        assert_eq!(jobs[1].priority, Some(2));

        let post_create = &config.hooks["worktree-post-create"];
        let jobs = post_create.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 1);
        // skip: CI should parse as EnvVar
        match &jobs[0].skip {
            Some(SkipCondition::EnvVar(v)) => assert_eq!(v, "CI"),
            other => panic!("Expected EnvVar, got {other:?}"),
        }
    }

    #[test]
    fn test_skip_condition_bool() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
        skip: true
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        match &job.skip {
            Some(SkipCondition::Bool(true)) => {}
            other => panic!("Expected Bool(true), got {other:?}"),
        }
    }

    #[test]
    fn test_skip_condition_rules() {
        let yaml = r#"
hooks:
  worktree-post-create:
    skip:
      - merge
      - ref: "release/*"
      - env: SKIP_HOOKS
      - run: "test -f .skip-hooks"
    jobs:
      - name: test
        run: echo test
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = &config.hooks["worktree-post-create"];
        match &hook.skip {
            Some(SkipCondition::Rules(rules)) => {
                assert_eq!(rules.len(), 4);
                match &rules[0] {
                    SkipRule::Named(s) => assert_eq!(s, "merge"),
                    other => panic!("Expected Named, got {other:?}"),
                }
                match &rules[1] {
                    SkipRule::Structured(s) => {
                        assert_eq!(s.ref_pattern.as_deref(), Some("release/*"));
                    }
                    other => panic!("Expected Structured with ref, got {other:?}"),
                }
            }
            other => panic!("Expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_commands_legacy_alias() {
        let yaml = r#"
hooks:
  worktree-post-create:
    commands:
      lint:
        run: cargo clippy
      format:
        run: cargo fmt --check
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = &config.hooks["worktree-post-create"];
        let cmds = hook.commands.as_ref().unwrap();
        assert_eq!(cmds.len(), 2);
        assert!(cmds.contains_key("lint"));
        assert!(cmds.contains_key("format"));
    }

    #[test]
    fn test_group_def() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: checks
        group:
          parallel: true
          jobs:
            - name: lint
              run: cargo clippy
            - name: format
              run: cargo fmt --check
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        let group = job.group.as_ref().unwrap();
        assert_eq!(group.parallel, Some(true));
        assert_eq!(group.jobs.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_output_setting_disabled() {
        let yaml = r#"
output: false
hooks: {}
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.output {
            Some(OutputSetting::Disabled(false)) => {}
            other => panic!("Expected Disabled(false), got {other:?}"),
        }
    }

    #[test]
    fn test_output_setting_hooks_list() {
        let yaml = r#"
output:
  - worktree-post-create
  - post-clone
hooks: {}
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.output {
            Some(OutputSetting::Hooks(h)) => {
                assert_eq!(h.len(), 2);
                assert_eq!(h[0], "worktree-post-create");
            }
            other => panic!("Expected Hooks list, got {other:?}"),
        }
    }

    #[test]
    fn test_env_vars_on_job() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
        env:
          RUST_BACKTRACE: "1"
          MY_VAR: hello
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        let env = job.env.as_ref().unwrap();
        assert_eq!(env.get("RUST_BACKTRACE").unwrap(), "1");
        assert_eq!(env.get("MY_VAR").unwrap(), "hello");
    }

    #[test]
    fn test_command_def_to_job_def() {
        let cmd = CommandDef {
            run: Some("cargo test".to_string()),
            tags: Some(vec!["test".to_string()]),
            ..Default::default()
        };
        let job = cmd.to_job_def("my-test");
        assert_eq!(job.name.as_deref(), Some("my-test"));
        match &job.run {
            Some(RunCommand::Simple(s)) => assert_eq!(s, "cargo test"),
            other => panic!("Expected Simple, got {other:?}"),
        }
        assert!(job.needs.is_none());
    }

    #[test]
    fn test_needs_deserialize() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: install-npm
        run: npm install
      - name: npm-build
        run: npm run build
        needs: [install-npm]
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let jobs = config.hooks["worktree-post-create"].jobs.as_ref().unwrap();
        assert!(jobs[0].needs.is_none());
        assert_eq!(
            jobs[1].needs.as_deref().unwrap(),
            &["install-npm".to_string()]
        );
    }

    #[test]
    fn test_needs_absent() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        assert!(job.needs.is_none());
    }

    #[test]
    fn test_needs_empty() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
        needs: []
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        assert!(job.needs.as_ref().unwrap().is_empty());
    }

    #[test]
    fn test_job_description() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        description: Install Homebrew package manager
        run: echo "install brew"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        assert_eq!(
            job.description.as_deref(),
            Some("Install Homebrew package manager")
        );
    }

    #[test]
    fn test_skip_rule_desc() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        run: echo "install"
        skip:
          - run: "command -v brew"
            desc: Brew is already installed
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.skip {
            Some(SkipCondition::Rules(rules)) => match &rules[0] {
                SkipRule::Structured(s) => {
                    assert_eq!(s.desc.as_deref(), Some("Brew is already installed"));
                    assert_eq!(s.run.as_deref(), Some("command -v brew"));
                }
                other => panic!("Expected Structured, got {other:?}"),
            },
            other => panic!("Expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_only_rule_desc() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-deps
        run: npm install
        only:
          - run: "test -f package.json"
            desc: Only when package.json exists
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.only {
            Some(OnlyCondition::Rules(rules)) => match &rules[0] {
                OnlyRule::Structured(s) => {
                    assert_eq!(s.desc.as_deref(), Some("Only when package.json exists"));
                }
                other => panic!("Expected Structured, got {other:?}"),
            },
            other => panic!("Expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_arch_single() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: arm-setup
        arch: aarch64
        run: echo "arm"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.arch {
            Some(PlatformConstraint::Single(arch)) => assert_eq!(*arch, TargetArch::Aarch64),
            other => panic!("Expected Single(Aarch64), got {other:?}"),
        }
    }

    #[test]
    fn test_arch_list() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: multi-arch
        arch: [x86_64, aarch64]
        run: echo "multi"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.arch {
            Some(PlatformConstraint::List(arch_list)) => {
                assert_eq!(arch_list.len(), 2);
                assert_eq!(arch_list[0], TargetArch::X86_64);
                assert_eq!(arch_list[1], TargetArch::Aarch64);
            }
            other => panic!("Expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_run_simple_string() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: test
        run: echo hello
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.run {
            Some(RunCommand::Simple(s)) => assert_eq!(s, "echo hello"),
            other => panic!("Expected Simple, got {other:?}"),
        }
    }

    #[test]
    fn test_run_os_map() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.run {
            Some(RunCommand::Platform(map)) => {
                assert_eq!(map.len(), 2);
                match &map[&TargetOs::Macos] {
                    PlatformRunCommand::Simple(s) => assert_eq!(s, "brew install mise"),
                    other => panic!("Expected Simple, got {other:?}"),
                }
            }
            other => panic!("Expected Platform, got {other:?}"),
        }
    }

    #[test]
    fn test_run_os_map_single_os() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        run:
          macos: /bin/bash -c "$(curl -fsSL https://example.com)"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.run {
            Some(RunCommand::Platform(map)) => {
                assert_eq!(map.len(), 1);
                assert!(map.contains_key(&TargetOs::Macos));
            }
            other => panic!("Expected Platform, got {other:?}"),
        }
    }
}

use crate::{
    WorktreeConfig,
    core::{
        TimelineBridge,
        global_config::GlobalConfig,
        layout::{
            BuiltinLayout, Layout,
            resolver::{LayoutResolutionContext, LayoutSource, resolve_layout},
        },
        worktree::{checkout, checkout_branch, previous},
    },
    get_current_worktree_path, get_git_common_dir, get_project_root,
    git::{GitCommand, should_show_gitoxide_notice},
    hints::maybe_show_shell_hint,
    hooks::{HookExecutor, TrustDatabase, yaml_config_loader},
    is_git_repository,
    logging::init_logging,
    output::{
        CliOutput, Output, OutputConfig,
        timeline::{Timeline, TimelineMode},
    },
    settings::{DaftSettings, PushVerify},
    utils::*,
};
use anyhow::Result;
use clap::Parser;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Clone)]
#[command(name = "git-worktree-checkout")]
#[command(version = crate::VERSION)]
#[command(about = "Create a worktree for an existing branch, or a new branch with -b")]
#[command(long_about = r#"
Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

With -b, creates a new branch and a corresponding worktree in a single
operation. The new branch is based on the current branch, or on `<base-branch>`
if specified. After creating the branch locally, it is pushed to the remote
and upstream tracking is configured. The repo's pre-push hook runs only when
that push introduces new commits; a ref-only push of already-pushed commits
skips it (configurable via daft.checkout.pushVerify: auto, always, or never).

With --start (or -s), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically, as if 'daft start'
had been called. This can also be enabled permanently with the daft.go.autoStart
git config option.

Pass a pull/merge request instead of a branch to check it out into a
worktree: `pr:123`, `mr:45`, or a pasted PR/MR URL. daft resolves it through
the `gh`/`glab` CLI (which supply the auth — daft stores no tokens), creates
a worktree on the PR/MR's source branch, and configures it to pull from the
PR/MR head. Fork PRs work without adding a remote. The platform is detected
from the repository's remote; `pr:`/`mr:` are interchangeable aliases. Set
`daft.forge.platform` (github/gitlab) to disambiguate a mixed-remote repo.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.
Cannot be combined with -b/--create-branch.

This command can be run from anywhere within the repository. If a worktree
for the specified branch already exists, no new worktree is created; the
working directory is changed to the existing worktree instead.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(
        help = "Branch to check out (or create with -b), a pull/merge request (pr:123, mr:45, or a PR/MR URL), or '-' for the previous worktree",
        allow_hyphen_values = true
    )]
    branch_name: String,

    #[arg(
        help = "Branch to use as the base for the new branch (only with -b); defaults to the current branch"
    )]
    base_branch_name: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch instead of checking out an existing one"
    )]
    create_branch: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,

    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(short = '@', long, value_name = "PATH")]
    at: Option<PathBuf>,

    #[arg(long, help = "Skip all remote operations (no fetch, no push)")]
    local: bool,

    #[arg(
        long,
        help = "Skip the repo's pre-push hook on the automatic upstream push"
    )]
    no_verify: bool,

    /// Skip hooks this run. Repeatable / comma-separated.
    /// Selectors: `all`, a hook name (`worktree-post-create`, …),
    /// `tag:<tag>`, or a job name (plus its dependents). See daft-hooks(1).
    #[arg(
        long,
        value_name = "SELECTOR",
        value_delimiter = ',',
        help = "Skip hooks this run (all | <hook> | tag:<tag> | <job>); repeatable/comma-separated"
    )]
    skip_hooks: Vec<String>,
}

/// Daft-style args for `daft go`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to navigating worktrees, with tailored about text.
#[derive(Parser)]
#[command(name = "daft go")]
#[command(version = crate::VERSION)]
#[command(about = "Open a worktree for an existing branch, or create one with -b")]
#[command(long_about = r#"
Opens a worktree for an existing local or remote branch. The worktree is
placed at the project root level as a sibling to other worktrees, using the
branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If the branch exists both locally and on the remote, the local
branch is checked out and upstream tracking is configured.

If a worktree for the specified branch already exists, no new worktree is
created; the working directory is changed to the existing worktree instead.

Use '-' as the branch name to switch to the previous worktree, similar to
'cd -'. Repeated 'daft go -' toggles between the two most recent worktrees.

`daft go` also jumps across repositories through the repo catalog. A name
that matches no branch in the current repository falls back to the catalog
and opens that repository's default-branch worktree. Two arguments —
`daft go <repo> <branch>` — open a specific branch there, creating its
worktree if needed. `--repo <name>` addresses a repository explicitly (for
names shadowed by local branches), and outside any git repository `daft go`
resolves purely against the catalog. Anything resolvable in the current
repository always wins over a catalog match.

With -b, creates a new branch and worktree in a single operation. The new
branch is based on the current branch, or on `<base-branch>` if specified. It
is pushed to the remote and upstream tracking is configured; the pre-push hook
runs only when that push introduces new commits, skipping ref-only pushes
(configurable via daft.checkout.pushVerify: auto, always, or never). Prefer
'daft start' for creating new branches.

With -s (--start), if the specified branch does not exist locally or on the
remote, a new branch and worktree are created automatically. This can also
be enabled permanently with the daft.go.autoStart git config option.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct GoArgs {
    #[arg(
        help = "Branch (or catalog repo) to open; use '-' for previous worktree",
        allow_hyphen_values = true,
        required_unless_present = "repo"
    )]
    branch_name: Option<String>,

    #[arg(help = "Branch inside <repo> when two arguments are given; base branch with -b")]
    second: Option<String>,

    #[arg(
        long = "repo",
        value_name = "REPO",
        help = "Open a repository from the catalog (jump across repos)"
    )]
    repo: Option<String>,

    #[arg(
        short = 'b',
        long = "create-branch",
        help = "Create a new branch (prefer 'daft start' instead)"
    )]
    create_branch: bool,

    #[arg(
        short = 's',
        long = "start",
        help = "Create a new worktree if the branch does not exist"
    )]
    start: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(short = '@', long, value_name = "PATH")]
    at: Option<PathBuf>,

    #[arg(long, help = "Skip all remote operations (no fetch, no push)")]
    local: bool,

    #[arg(
        long,
        help = "Skip the repo's pre-push hook on the automatic upstream push"
    )]
    no_verify: bool,

    /// Skip hooks this run (only applies when `go` creates a worktree).
    /// Selectors: `all`, a hook name (`worktree-post-create`, …),
    /// `tag:<tag>`, or a job name (plus its dependents). See daft-hooks(1).
    #[arg(
        long,
        value_name = "SELECTOR",
        value_delimiter = ',',
        help = "Skip hooks when creating a worktree (all | <hook> | tag:<tag> | <job>); repeatable/comma-separated"
    )]
    skip_hooks: Vec<String>,
}

/// Daft-style args for `daft start`. Separate from `Args` so that `-h`/`--help`
/// shows only the flags relevant to creating new branches, without `-b` or `--start`.
#[derive(Parser)]
#[command(name = "daft start")]
#[command(version = crate::VERSION)]
#[command(about = "Create a new branch and worktree")]
#[command(long_about = r#"
Creates a new branch and a corresponding worktree in a single operation. The
worktree is placed at the project root level as a sibling to other worktrees,
using the branch name as the directory name.

The new branch is based on the current branch, or on `<base-branch>` if
specified. After creating the branch locally, it is pushed to the remote and
upstream tracking is configured (unless disabled via daft.checkout.push). The
repo's pre-push hook runs only when that push introduces new commits; a
ref-only push of already-pushed commits skips it (configurable via
daft.checkout.pushVerify: auto, always, or never).

`daft start` can also create the branch in another repository from the repo
catalog: `daft start <repo> <branch> [base]`, or explicitly with
`--repo <repo>`. Anything meaningful in the current repository wins over a
catalog match: with two names, an existing local branch keeps the local
`<branch> <base>` reading, and only otherwise does a live cataloged repo
select cross-repo creation. Three names are always `<repo> <branch> <base>`.
The resolved destination is announced before any work happens; without a
base the branch is based on the target repo's default branch, the target
repo's hooks run only if it is trusted, and the shell lands in the new
worktree there. Carry (`-c`) cannot cross repositories; `-x` runs in the
target worktree.

With --with-related, the same branch is also created in every repo the
primary repo's daft.yml `relations:` manifest points at — the entry point
for a coordinated cross-repo change (pair with `daft exec --related`). The
primary repo is the current one, or the named repo when combined with a
catalog target (the fan-out is rooted there). Each related repo bases the
branch on its own default branch; carry and -x stay in the primary repo;
hooks run in a related repo only when it is explicitly trusted. All related
repos must be cloned locally first, and the final working directory is the
primary repo's new worktree.

This command can be run from anywhere within the repository, or from
outside any repository when a catalog target is named.

Lifecycle hooks from .daft/hooks/ are executed if the repository is trusted.
See daft-hooks(1) for hook management.
"#)]
pub struct StartArgs {
    #[arg(
        value_name = "BRANCH_NAME",
        help = "Name for the new branch; or a cataloged repo to create it in, with `daft start <repo> <branch> [base]`"
    )]
    first: String,

    #[arg(
        value_name = "BASE_OR_BRANCH",
        help = "Base branch (defaults to the current branch); or the new branch inside `<repo>`"
    )]
    second: Option<String>,

    #[arg(
        value_name = "BASE",
        help = "Base branch inside `<repo>` (three-name form); must exist there"
    )]
    third: Option<String>,

    #[arg(
        long = "repo",
        value_name = "REPO",
        help = "Create the branch in a repository from the catalog (for repo names shadowed by local branches)"
    )]
    repo: Option<String>,

    #[arg(
        long = "with-related",
        help = "Also create the branch in every related repo (relations manifest), each based on its own default branch"
    )]
    with_related: bool,

    #[arg(
        short = 'c',
        long = "carry",
        help = "Apply uncommitted changes from the current worktree to the new one"
    )]
    carry: bool,

    #[arg(long, help = "Do not carry uncommitted changes")]
    no_carry: bool,

    #[arg(
        short = 'r',
        long = "remote",
        help = "Remote for worktree organization (multi-remote mode)"
    )]
    remote: Option<String>,

    #[arg(long, help = "Do not change directory to the new worktree")]
    no_cd: bool,

    #[arg(
        short = 'x',
        long = "exec",
        help = "Run a command in the worktree after setup completes (repeatable)"
    )]
    exec: Vec<String>,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    /// Place the worktree at a specific path instead of using the layout template.
    #[arg(short = '@', long, value_name = "PATH")]
    at: Option<PathBuf>,

    #[arg(long, help = "Skip all remote operations (no fetch, no push)")]
    local: bool,

    #[arg(
        long,
        help = "Skip the repo's pre-push hook on the automatic upstream push"
    )]
    no_verify: bool,

    /// Skip hooks this run. Repeatable / comma-separated.
    /// Selectors: `all`, a hook name (`worktree-post-create`, …),
    /// `tag:<tag>`, or a job name (plus its dependents). See daft-hooks(1).
    #[arg(
        long,
        value_name = "SELECTOR",
        value_delimiter = ',',
        help = "Skip hooks this run (all | <hook> | tag:<tag> | <job>); repeatable/comma-separated"
    )]
    skip_hooks: Vec<String>,
}

impl StartArgs {
    /// The internal `Args` for creating `branch` from `base` with this
    /// invocation's flags.
    fn to_create_args(&self, branch: String, base: Option<String>) -> Args {
        Args {
            branch_name: branch,
            base_branch_name: base,
            create_branch: true,
            start: false,
            carry: self.carry,
            no_carry: self.no_carry,
            remote: self.remote.clone(),
            no_cd: self.no_cd,
            exec: self.exec.clone(),
            quiet: self.quiet,
            verbose: self.verbose,
            at: self.at.clone(),
            local: self.local,
            no_verify: self.no_verify,
            skip_hooks: self.skip_hooks.clone(),
        }
    }
}

/// Entry point for `git-worktree-checkout`.
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-checkout"));
    run_with_args(args, GoRouting::local_only())
}

/// Entry point for `daft go`.
pub fn run_go() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-go");
    raw[0] = "daft go".to_string();
    let go_args = GoArgs::parse_from(raw);

    let (branch_name, base_branch_name, routing) = decode_go_grammar(
        go_args.branch_name,
        go_args.second,
        go_args.repo,
        go_args.create_branch,
    )?;
    let args = Args {
        branch_name,
        base_branch_name,
        create_branch: go_args.create_branch,
        start: go_args.start,
        carry: go_args.carry,
        no_carry: go_args.no_carry,
        remote: go_args.remote,
        no_cd: go_args.no_cd,
        exec: go_args.exec,
        quiet: go_args.quiet,
        verbose: go_args.verbose,
        at: go_args.at,
        local: go_args.local,
        no_verify: go_args.no_verify,
        skip_hooks: go_args.skip_hooks,
    };
    run_with_args(args, routing)
}

/// Cross-repo target decoded from `daft go`'s grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrossTarget {
    /// Catalog needle (name, path, or uuid).
    repo: String,
    /// Branch to open there; `None` = the repo's default branch.
    branch: Option<String>,
    /// True for the bare `daft go <repo> <branch>` form (drives error copy).
    positional: bool,
}

/// How a checkout-family invocation may interact with the repo catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoRouting {
    /// Explicit cross-repo target (`--repo` or the two-positional form).
    cross: Option<CrossTarget>,
    /// Whether a `BranchNotFound` may fall back to a catalog repo name
    /// (`daft go <name>` only — never `git-worktree-checkout` or `-b`).
    catalog_fallback: bool,
}

impl GoRouting {
    /// Plain repo-local behavior (`git-worktree-checkout`, `daft start`).
    fn local_only() -> Self {
        Self {
            cross: None,
            catalog_fallback: false,
        }
    }
}

/// Decode `daft go`'s argument grammar into the local `Args` fields
/// (`branch_name`, `base_branch_name`) plus the catalog routing.
///
/// The grammar, in precedence order:
///   * `--repo R [<branch>]`, `--repo R -b <branch> [base]` — explicit
///     cross-repo forms (disambiguates repos shadowed by branch names).
///   * `go -b <branch> [base]` — positional 2 keeps its base-branch
///     meaning; creation is always repo-local.
///   * `go <repo> <branch>` — bare two positionals had no legal meaning
///     before the catalog existed, so this form is cross-repo by
///     definition.
///   * `go <name>` — current-repo resolution first; the catalog is only a
///     `BranchNotFound` fallback.
fn decode_go_grammar(
    first: Option<String>,
    second: Option<String>,
    repo: Option<String>,
    create_branch: bool,
) -> Result<(String, Option<String>, GoRouting)> {
    if let Some(repo) = repo {
        if first.as_deref() == Some("-") {
            anyhow::bail!("Cannot use '-' with --repo");
        }
        if !create_branch && second.is_some() {
            anyhow::bail!(
                "--repo takes a single <branch>: got '{}' and '{}'",
                first.as_deref().unwrap_or(""),
                second.as_deref().unwrap_or("")
            );
        }
        if create_branch && first.is_none() {
            anyhow::bail!("-b/--create-branch with --repo requires a branch name");
        }
        let branch = first.clone();
        return Ok((
            first.unwrap_or_default(),
            if create_branch { second } else { None },
            GoRouting {
                cross: Some(CrossTarget {
                    repo,
                    branch,
                    positional: false,
                }),
                catalog_fallback: false,
            },
        ));
    }

    let first = first.expect("clap enforces the first positional unless --repo is present");

    if let Some(second) = second {
        if create_branch {
            // `daft go -b <branch> [base]` — unchanged local meaning.
            return Ok((first, Some(second), GoRouting::local_only()));
        }
        if first == "-" {
            anyhow::bail!("Cannot use '-' with a second argument");
        }
        return Ok((
            second.clone(),
            None,
            GoRouting {
                cross: Some(CrossTarget {
                    repo: first,
                    branch: Some(second),
                    positional: true,
                }),
                catalog_fallback: false,
            },
        ));
    }

    Ok((
        first,
        None,
        GoRouting {
            cross: None,
            catalog_fallback: !create_branch,
        },
    ))
}

/// Entry point for `daft start`.
pub fn run_start() -> Result<()> {
    let mut raw = crate::get_clap_args("daft-start");
    raw[0] = "daft start".to_string();
    let start_args = StartArgs::parse_from(raw);

    let routing = decode_start_grammar(
        start_args.first.clone(),
        start_args.second.clone(),
        start_args.third.clone(),
        start_args.repo.clone(),
        local_branch_exists,
        |name| {
            lookup_live_repo(name)
                .map(|row| std::path::Path::new(&row.path).is_dir())
                .unwrap_or(false)
        },
    )?;

    match routing {
        StartRouting::Local { branch, base } => {
            if start_args.with_related {
                let source_worktree = get_current_worktree_path().ok();
                return run_start_with_related(start_args, branch, base, source_worktree);
            }
            let args = start_args.to_create_args(branch, base);
            run_with_args(args, GoRouting::local_only())
        }
        StartRouting::LocalCollision {
            branch,
            second,
            also_live_repo,
        } => {
            let mut msg = format!("branch '{branch}' already exists in this repository");
            if also_live_repo {
                msg.push_str(&format!(
                    "\n  note: '{branch}' is also a cataloged repo — to create branch \
                     '{second}' there, use `{}`",
                    crate::daft_cmd(&format!("start --repo {branch} {second}"))
                ));
            } else {
                msg.push_str(&format!(
                    "\n  tip: `{}` opens its worktree",
                    crate::daft_cmd(&format!("go {branch}"))
                ));
            }
            Err(anyhow::anyhow!(msg))
        }
        StartRouting::Cross {
            repo,
            branch,
            base,
            guessed,
        } => run_start_cross(start_args, repo, branch, base, guessed),
    }
}

/// How a `daft start` invocation routes after grammar decode.
#[derive(Debug, Clone, PartialEq, Eq)]
enum StartRouting {
    /// `daft start <branch> [base]` in the current repository.
    Local {
        branch: String,
        base: Option<String>,
    },
    /// The two-name form preferred the local reading, but the branch already
    /// exists here — fail fast instead of guessing across repos.
    LocalCollision {
        branch: String,
        second: String,
        also_live_repo: bool,
    },
    /// Create the branch in another cataloged repository.
    Cross {
        repo: String,
        branch: String,
        base: Option<String>,
        guessed: bool,
    },
}

/// Decode `daft start`'s argument grammar.
///
/// The grammar, in precedence order:
///   * `start --repo R <branch> [base]` — explicit cross-repo creation
///     (disambiguates repos shadowed by branch names; script-safe).
///   * `start <repo> <branch> <base>` — three names have no local meaning
///     (local start takes at most two), so this form is cross-repo by
///     definition; the repo must resolve (hard error on a miss).
///   * `start <A> <B>` — the only guessed arity. Anything meaningful in the
///     current repository wins over a catalog match: an existing local
///     branch `A` keeps the local reading (failing fast as "already
///     exists"); otherwise a live catalog repo `A` means "create `B` in
///     `A`"; otherwise plain local `<branch> <base>`.
///   * `start <branch>` — always local; a lone repo name is never a start
///     target (there is no branch to create).
fn decode_start_grammar(
    first: String,
    second: Option<String>,
    third: Option<String>,
    repo: Option<String>,
    is_local_branch: impl Fn(&str) -> bool,
    is_live_repo: impl Fn(&str) -> bool,
) -> Result<StartRouting> {
    if let Some(repo) = repo {
        if let Some(third) = third {
            anyhow::bail!(
                "--repo already names the target repo: unexpected argument '{third}' \
                 (usage: `daft start --repo <repo> <branch> [base]`)"
            );
        }
        if first == "-" || second.as_deref() == Some("-") {
            anyhow::bail!("Cannot use '-' with --repo");
        }
        return Ok(StartRouting::Cross {
            repo,
            branch: first,
            base: second,
            guessed: false,
        });
    }

    if first == "-" || second.as_deref() == Some("-") || third.as_deref() == Some("-") {
        anyhow::bail!(
            "'-' is not a branch name here — `daft go -` switches to the previous worktree"
        );
    }

    let Some(second) = second else {
        return Ok(StartRouting::Local {
            branch: first,
            base: None,
        });
    };

    if let Some(third) = third {
        return Ok(StartRouting::Cross {
            repo: first,
            branch: second,
            base: Some(third),
            guessed: false,
        });
    }

    if is_local_branch(&first) {
        let also_live_repo = is_live_repo(&first);
        return Ok(StartRouting::LocalCollision {
            branch: first,
            second,
            also_live_repo,
        });
    }
    if is_live_repo(&first) {
        return Ok(StartRouting::Cross {
            repo: first,
            branch: second,
            base: None,
            guessed: true,
        });
    }
    Ok(StartRouting::Local {
        branch: first,
        base: Some(second),
    })
}

/// Silent probe for the two-name guess: does `refs/heads/<name>` exist in
/// the repository the cwd sits in? False outside any repository. Runs
/// through `git_command_at` so an inherited `GIT_DIR` (hook context) cannot
/// retarget the check. Also used by tab completion to mirror the guess.
pub(crate) fn local_branch_exists(name: &str) -> bool {
    let Ok(cwd) = get_current_directory() else {
        return false;
    };
    git_command_at(&cwd)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Create `branch` in another cataloged repository (`daft start <repo>
/// <branch> [base]` and the `--repo` spelling). The destination is
/// announced before any work so a wrong guess is visible immediately.
fn run_start_cross(
    mut start_args: StartArgs,
    repo_needle: String,
    branch: String,
    base: Option<String>,
    guessed: bool,
) -> Result<()> {
    init_logging(start_args.verbose);

    if start_args.carry {
        anyhow::bail!(
            "-c/--carry cannot cross repositories: uncommitted changes here cannot be \
             applied to a worktree in '{repo_needle}'"
        );
    }
    // Belt and braces for config-level carry too — a dirty tree never crosses.
    start_args.no_carry = true;

    let inside_repo = is_git_repository()?;
    if inside_repo {
        crate::catalog::touch_current_repo();
    }
    let original_dir = get_current_directory()?;
    // Capture the source worktree BEFORE the cross-repo chdir, so the target
    // repo's `daft go -` can hop back across repos (best-effort).
    let source_worktree = if inside_repo {
        get_current_worktree_path().ok()
    } else {
        None
    };

    let row = if guessed {
        // The decode probe saw this repo moments ago; a miss here is a race.
        lookup_live_repo(&repo_needle).ok_or_else(|| {
            anyhow::anyhow!("repository '{repo_needle}' is no longer in the catalog")
        })?
    } else {
        let note = start_args.repo.is_none().then_some(START_THREE_ARG_NOTE);
        resolve_cross_target(&repo_needle, note)?
    };

    let base = match base {
        Some(base) => base,
        None => repo_default_branch(&row).ok_or_else(|| {
            anyhow::anyhow!(
                "could not determine the default branch of '{}'; pass a base: `{}`",
                row.name,
                crate::daft_cmd(&format!("start {} {} <base>", row.name, branch))
            )
        })?,
    };

    // Announce the resolved destination before any work happens — a guessed
    // mutating target must be impossible to miss (`-q` opts out).
    let mut announce = CliOutput::new(OutputConfig::with_autocd(
        start_args.quiet,
        start_args.verbose,
        false,
    ));
    announce.result(&format!(
        "Creating branch '{}' in '{}' ({}) — based on '{}'",
        branch, row.name, row.path, base
    ));

    if start_args.with_related {
        // Root the fan-out in the target repo: enter a worktree there and run
        // the ordinary --with-related flow against ITS relations manifest.
        let repo_root = std::path::Path::new(&row.path);
        let Some(worktree) = crate::core::repo::find_representative_worktree(repo_root) else {
            anyhow::bail!("'{}' has no worktrees to base the new branch on", row.name);
        };
        change_directory(&worktree)?;
        let result = run_start_with_related(start_args, branch, Some(base), source_worktree);
        if result.is_err() {
            change_directory(&original_dir).ok();
        }
        return result;
    }

    let args = start_args.to_create_args(branch, Some(base));
    go_to_repo(&row, None, args, original_dir, source_worktree)
}

fn run_with_args(args: Args, routing: GoRouting) -> Result<()> {
    init_logging(args.verbose);

    let inside_repo = is_git_repository()?;
    if inside_repo {
        crate::catalog::touch_current_repo();
    }

    // Handle `daft go -` (previous worktree navigation) — repo-local by
    // definition (decode already rejected `-` in the cross-repo forms).
    if routing.cross.is_none() && args.branch_name == "-" {
        if !inside_repo {
            anyhow::bail!("Not inside a Git repository");
        }
        if args.create_branch {
            anyhow::bail!("Cannot use '-' with -b/--create-branch");
        }

        let settings = DaftSettings::load()?;
        let autocd = settings.autocd && !args.no_cd;
        let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
        let mut output = CliOutput::new(config);
        return run_go_previous(&mut output);
    }

    // Validate: base_branch_name only valid with -b
    if args.base_branch_name.is_some() && !args.create_branch {
        anyhow::bail!("<BASE_BRANCH_NAME> can only be used with -b/--create-branch");
    }

    let original_dir = get_current_directory()?;

    // Capture the source worktree BEFORE any cross-repo chdir, so the
    // target repo's `daft go -` can hop back across repos (best-effort).
    let source_worktree = if inside_repo {
        get_current_worktree_path().ok()
    } else {
        None
    };

    // Explicit cross-repo target (`--repo` or the two-positional form):
    // loud resolution, then run inside the target repo.
    if let Some(cross) = &routing.cross {
        let row = resolve_cross_target(&cross.repo, cross.positional.then_some(GO_TWO_ARG_NOTE))?;
        return go_to_repo(
            &row,
            cross.branch.clone(),
            args,
            original_dir,
            source_worktree,
        );
    }

    // Bare `daft go <name>` outside any git repo: the catalog is the only
    // possible meaning. On a miss, keep the exact historical error.
    if !inside_repo {
        if routing.catalog_fallback
            && !args.start
            && let Some(row) = lookup_live_repo(&args.branch_name)
        {
            return go_to_repo(&row, None, args, original_dir, source_worktree);
        }
        anyhow::bail!("Not inside a Git repository");
    }

    run_in_repo(
        args,
        routing.catalog_fallback,
        original_dir,
        source_worktree,
    )
}

/// Miss note for the bare `daft go <repo> <branch>` form.
const GO_TWO_ARG_NOTE: &str = "two arguments mean `daft go <repo> <branch>`; to create \
                     a branch use `daft go -b <branch> [base]` or `daft start`";

/// Miss note for the `daft start <repo> <branch> <base>` form.
const START_THREE_ARG_NOTE: &str = "three arguments mean `daft start <repo> <branch> [base]`; \
                     for a local branch use `daft start <branch> [base]`";

/// Resolve an explicit cross-repo needle against the catalog, loudly. A
/// `positional_note` is appended on a miss to explain the positional form
/// that routed here (`None` for the `--repo` spellings).
fn resolve_cross_target(
    needle: &str,
    positional_note: Option<&str>,
) -> Result<crate::store::CatalogRepoRow> {
    let Some(catalog) = crate::catalog::Catalog::open_ro()? else {
        anyhow::bail!(
            "the repo catalog is empty — clone a repo or run `{}` first",
            crate::daft_cmd("repo add")
        );
    };
    match catalog.resolve(needle)? {
        Some(row) if row.removed_at.is_none() => Ok(row),
        Some(row) => anyhow::bail!(
            "repository '{}' was removed from the catalog; restore it with `{}`",
            row.name,
            crate::daft_cmd(&format!("clone {}", row.name))
        ),
        None => {
            let err = catalog.not_found(needle);
            let mut msg = err.to_string();
            if let crate::catalog::CatalogError::NotFound { suggestions, .. } = &err
                && !suggestions.is_empty()
            {
                msg.push_str(&format!("\n  did you mean: {}", suggestions.join(", ")));
            }
            if let Some(note) = positional_note {
                anyhow::bail!("{msg}\n  note: {note}");
            }
            anyhow::bail!(
                "{msg}\n  tip: `{}` shows known repos; `{}` registers the current one",
                crate::daft_cmd("repo list"),
                crate::daft_cmd("repo add")
            );
        }
    }
}

/// Silent live-name lookup for the `BranchNotFound` fallback path.
fn lookup_live_repo(name: &str) -> Option<crate::store::CatalogRepoRow> {
    let catalog = crate::catalog::Catalog::open_ro().ok().flatten()?;
    catalog.resolve_live_name(name).ok().flatten()
}

/// A cataloged repo's default branch: the catalog's recorded value,
/// refreshed from the repo's local `origin/HEAD` when unknown (write-back
/// is best-effort).
fn repo_default_branch(row: &crate::store::CatalogRepoRow) -> Option<String> {
    if let Some(branch) = &row.default_branch {
        return Some(branch.clone());
    }
    let branch =
        crate::core::remote::local_default_branch(std::path::Path::new(&row.path), "origin")?;
    if let Ok(catalog) = crate::catalog::Catalog::open_rw() {
        let _ = catalog.refresh_default_branch(&row.uuid, &branch);
    }
    Some(branch)
}

/// The branch `daft go <repo>` lands on. Assumes cwd is already the target repo.
fn resolve_repo_default_branch(row: &crate::store::CatalogRepoRow) -> Result<String> {
    repo_default_branch(row).ok_or_else(|| {
        anyhow::anyhow!(
            "could not determine the default branch of '{}'; pass a branch: `{}`",
            row.name,
            crate::daft_cmd(&format!("go {} <branch>", row.name))
        )
    })
}

/// Enter `row`'s repository and open `branch` there (the repo's default
/// branch when `None`). Everything downstream — git discovery, settings,
/// layout, trust, hooks, `DAFT_CD_FILE` — is cwd-derived, so a chdir is
/// the entire cross-repo mechanism.
fn go_to_repo(
    row: &crate::store::CatalogRepoRow,
    explicit_branch: Option<String>,
    mut args: Args,
    original_dir: PathBuf,
    source_worktree: Option<PathBuf>,
) -> Result<()> {
    let path = std::path::Path::new(&row.path);
    if !path.is_dir() {
        anyhow::bail!(
            "catalog entry '{}' points at '{}', which no longer exists\n  \
             tip: if the repo moved, run `{}` from its new location; \
             if it's gone, `{}` re-clones it",
            row.name,
            row.path,
            crate::daft_cmd("repo add"),
            crate::daft_cmd(&format!("clone {}", row.name))
        );
    }
    change_directory(path)?;

    let result = (|| {
        if !args.create_branch {
            args.branch_name = match explicit_branch {
                Some(branch) => branch,
                None => resolve_repo_default_branch(row)?,
            };
        }
        // Catalog fallback stays off inside the target repo — one hop only.
        run_in_repo(args, false, original_dir.clone(), source_worktree)
    })();
    if result.is_err() {
        change_directory(&original_dir).ok();
    }
    result
}

/// Dispatch a checkout/create inside the repo the cwd currently sits in.
fn run_in_repo(
    args: Args,
    catalog_fallback: bool,
    original_dir: PathBuf,
    source_worktree: Option<PathBuf>,
) -> Result<()> {
    // Construct one `GitCommand` and load settings (and, in the run_checkout /
    // run_create_branch bodies, the hooks config) through it so a checkout
    // discovers the repo exactly once instead of per throwaway instance (#584).
    let git = GitCommand::new(args.quiet);
    let settings = DaftSettings::load_with(&git)?;
    let git = git.with_gitoxide(settings.use_gitoxide);

    let autocd = settings.autocd && !args.no_cd;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, autocd);
    let mut output = CliOutput::new(config);

    let result = if args.create_branch {
        run_create_branch(&args, &settings, &git, &mut output)
    } else {
        match run_checkout(&args, &settings, &git, &mut output) {
            Ok(already_existed) => {
                // --at is invalid when navigating to an existing worktree
                // (it only applies when creating a new one)
                if args.at.is_some() && already_existed {
                    change_directory(&original_dir).ok();
                    anyhow::bail!(
                        "--at cannot be used: worktree already exists for '{}'. \
                         Use 'daft go {}' without --at to navigate to it.",
                        args.branch_name,
                        args.branch_name
                    );
                }
                Ok(())
            }
            Err(checkout::CheckoutError::BranchNotFound {
                ref branch,
                ref remote,
                fetch_failed,
            }) => {
                // A live catalog repo beats creating a branch: `daft go api`
                // means "open the api repo" when no branch `api` exists.
                // `--start` forces branch creation instead.
                if catalog_fallback
                    && !args.start
                    && let Some(row) = lookup_live_repo(branch)
                    && std::path::Path::new(&row.path).is_dir()
                {
                    change_directory(&original_dir).ok();
                    output.result(&format!(
                        "Opening repository '{}' (use --start to create a branch named '{}')",
                        row.name, branch
                    ));
                    return go_to_repo(&row, None, args.clone(), original_dir, source_worktree);
                }

                let auto_start = args.start || settings.go_auto_start;
                if auto_start {
                    change_directory(&original_dir).ok();
                    output.result(&format!(
                        "Branch '{branch}' not found, creating new worktree..."
                    ));
                    run_create_branch(&args, &settings, &git, &mut output)
                } else {
                    change_directory(&original_dir).ok();
                    // --at with a non-existent branch requires --start or autoStart
                    if args.at.is_some() {
                        anyhow::bail!(
                            "--at requires --start (or daft.go.autoStart=true) \
                             when branch '{branch}' does not exist"
                        );
                    }
                    render_branch_not_found_error(branch, remote, fetch_failed, &settings);
                    std::process::exit(1);
                }
            }
            Err(checkout::CheckoutError::Other(e)) => Err(e),
        }
    };

    if let Err(e) = result {
        change_directory(&original_dir).ok();
        return Err(e);
    }

    // Save the source worktree as previous (best-effort, after success).
    // After a cross-repo hop the cwd — and therefore the git dir — is the
    // target repo's, so its previous-worktree file records the source
    // worktree from the other repo and `daft go -` hops back.
    if let Some(src) = source_worktree
        && let Ok(git_dir) = get_git_common_dir()
    {
        let _ = previous::save(&git_dir, &src);
    }

    Ok(())
}

/// Navigate to the previous worktree (`daft go -`).
fn run_go_previous(output: &mut dyn Output) -> Result<()> {
    let git_dir = get_git_common_dir()?;

    let previous_path = previous::load(&git_dir)?
        .ok_or_else(|| anyhow::anyhow!("No previous worktree to switch to"))?;

    if !previous_path.exists() {
        anyhow::bail!(
            "Previous worktree no longer exists: '{}'",
            previous_path.display()
        );
    }

    // Save current worktree as the new previous before switching
    if let Ok(current) = get_current_worktree_path() {
        let _ = previous::save(&git_dir, &current);
    }

    change_directory(&previous_path)?;

    // Try to get the branch name for display
    let branch_display =
        crate::get_current_branch().unwrap_or_else(|_| previous_path.display().to_string());
    output.result(&format!("Switched to worktree '{branch_display}'"));

    output.cd_path(&previous_path);
    maybe_show_shell_hint(output)?;

    Ok(())
}

/// Resolve the layout for checkout operations.
///
/// Loads the layout from the config chain: repo store > daft.yml > global config > detection > default.
/// Also checks if the resolved layout requires a bare repo and warns if the current repo
/// is not bare.
fn resolve_checkout_layout(
    git: &GitCommand,
    output: &mut dyn Output,
) -> (crate::core::layout::Layout, LayoutSource) {
    let global_config = GlobalConfig::load().unwrap_or_default();
    let git_dir = get_git_common_dir().ok();
    let trust_db = TrustDatabase::load().unwrap_or_default();

    // Load daft.yml layout field from the current worktree (best-effort)
    let yaml_layout: Option<String> = get_current_worktree_path()
        .ok()
        .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
        .and_then(|cfg| cfg.layout);

    let repo_store_layout = git_dir
        .as_ref()
        .and_then(|d| trust_db.get_layout(d).map(String::from));

    // Run detection when no explicit layout is set.
    let detection = if repo_store_layout.is_none() && yaml_layout.is_none() {
        git_dir
            .as_ref()
            .map(|d| crate::core::layout::detect::detect_layout(d, &global_config))
    } else {
        None
    };

    let (layout, source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: None, // checkout doesn't have --layout yet
        repo_store_layout: repo_store_layout.as_deref(),
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
        detection,
    });

    // Graceful degradation: warn if layout needs bare but repo is not bare.
    // Use config_get("core.bare") instead of rev_parse_is_bare_repository()
    // because the latter returns false from inside a linked worktree of a
    // bare repo — which is exactly where users run checkout from.
    let is_bare = git
        .config_get("core.bare")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_lowercase() == "true");
    if layout.needs_bare() && !is_bare {
        output.warning(&format!(
            "Layout '{}' works best with a bare repository. \
             Consider running `daft layout transform` to convert.",
            layout.name
        ));
    }

    (layout, source)
}

/// Decide whether to use the resolved layout as-is or to prompt the user.
///
/// Returns `(layout, should_persist)`:
/// - `should_persist` is true when the layout should be saved to the repo store.
fn interactive_layout_resolution(
    layout: &Layout,
    source: LayoutSource,
    output: &mut dyn Output,
) -> Result<(Layout, bool)> {
    let is_testing = std::env::var("DAFT_TESTING").is_ok();
    let is_interactive = std::io::stdin().is_terminal() && !is_testing;

    match source {
        // Explicitly configured — use as-is, never persist again.
        LayoutSource::Cli
        | LayoutSource::RepoStore
        | LayoutSource::YamlConfig
        | LayoutSource::GlobalConfig => Ok((layout.clone(), false)),

        // Detection found a match — ask the user to confirm (interactive only).
        LayoutSource::Detected => {
            if !is_interactive {
                // Non-interactive: use detected layout and persist it.
                return Ok((layout.clone(), true));
            }

            output.info(&format!("Detected layout: {}", layout.name));

            let confirmed =
                dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
                    .with_prompt("Use this layout?")
                    .default(true)
                    .interact()?;

            if confirmed {
                Ok((layout.clone(), true))
            } else {
                let picked = show_layout_picker(Some(layout))?;
                maybe_consolidate(&picked, output)?;
                Ok((picked, true))
            }
        }

        // Nothing was detected — check if this is a repo with linked worktrees.
        LayoutSource::Unresolved => {
            // Check for linked worktrees (Flow A vs Flow C).
            let git = GitCommand::new(true);
            let has_linked_worktrees = git
                .worktree_list_porcelain()
                .ok()
                .map(|porcelain| {
                    crate::core::layout::detect::parse_worktree_list(&porcelain)
                        .into_iter()
                        .any(|w| !w.is_main)
                })
                .unwrap_or(false);

            if !has_linked_worktrees {
                // Flow A: plain git clone — silently use default and persist.
                return Ok((layout.clone(), true));
            }

            // Flow C: worktrees exist in an unrecognized arrangement.
            if !is_interactive {
                // Non-interactive: use default layout, do not persist.
                return Ok((layout.clone(), false));
            }

            output.info("Found worktrees in unrecognized arrangement.");
            let picked = show_layout_picker(Some(layout))?;
            maybe_consolidate(&picked, output)?;
            Ok((picked, true))
        }
    }
}

/// Show a layout picker and return the selected layout.
fn show_layout_picker(preselect: Option<&Layout>) -> Result<Layout> {
    let global_config = GlobalConfig::load().unwrap_or_default();

    // Build list: builtins first, then custom layouts.
    let mut items: Vec<Layout> = BuiltinLayout::all().iter().map(|b| b.to_layout()).collect();
    items.extend(global_config.custom_layouts());

    // Format each item as "{name:<20}{template}"
    let display: Vec<String> = items
        .iter()
        .map(|l| format!("{:<20}{}", l.name, l.template))
        .collect();

    // Pre-select the provided layout, or fall back to index 0.
    let default_idx = preselect
        .and_then(|pre| items.iter().position(|l| l.name == pre.name))
        .unwrap_or(0);

    let selection = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Select a layout")
        .items(&display)
        .default(default_idx)
        .interact()?;

    Ok(items.remove(selection))
}

/// Ask whether to consolidate existing worktrees to the chosen layout.
fn maybe_consolidate(chosen_layout: &Layout, output: &mut dyn Output) -> Result<()> {
    if !std::io::stdin().is_terminal() || std::env::var("DAFT_TESTING").is_ok() {
        return Ok(());
    }

    let git = GitCommand::new(true);
    let porcelain = git.worktree_list_porcelain()?;
    let worktrees = crate::core::layout::detect::parse_worktree_list(&porcelain);
    let linked_count = worktrees.iter().filter(|wt| !wt.is_main).count();

    if linked_count == 0 {
        return Ok(());
    }

    let prompt = format!(
        "Consolidate {} existing worktree{} to match \"{}\" layout?",
        linked_count,
        if linked_count == 1 { "" } else { "s" },
        chosen_layout.name,
    );

    let consolidate = dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt(prompt)
        .default(false)
        .interact()?;

    if consolidate {
        output.info(&format!(
            "Run `daft layout transform {}` to consolidate.",
            chosen_layout.name,
        ));
    }

    Ok(())
}

/// Returns `Ok(already_existed)` — true if the worktree already existed
/// (navigation only, no creation).
/// A forge PR/MR checkout target resolved from a `pr:`/`mr:`/URL positional.
struct ResolvedForge {
    /// The PR/MR's source branch — the local branch daft creates or opens.
    branch_name: String,
    /// Everything core needs to fetch the ref and configure tracking.
    forge: checkout::ForgeCheckout,
    /// Rail header target, e.g. `PR #123`.
    header: String,
}

/// If the checkout positional is a forge PR/MR reference, resolve it (this is
/// the one networked step) and map it for core. `Ok(None)` for an ordinary
/// branch. Runs before anything interactive so a bad reference fails fast;
/// resolution/preflight failures are `CheckoutError::Other`, so the caller's
/// branch-not-found morph never reinterprets the rewritten source-branch name.
fn resolve_forge_target(
    args: &Args,
    settings: &DaftSettings,
    git: &GitCommand,
    project_root: &std::path::Path,
) -> Result<Option<ResolvedForge>, checkout::CheckoutError> {
    let Some(target) = crate::forge::ForgeTarget::parse(&args.branch_name) else {
        return Ok(None);
    };
    if args.local {
        return Err(anyhow::anyhow!(
            "checking out a pull/merge request requires the network; drop --local"
        )
        .into());
    }

    let started = std::time::Instant::now();
    let resolved = crate::forge::resolve(
        &target,
        git,
        project_root,
        &settings.remote,
        &crate::forge::ForgeConfig::load(git),
    )?;
    crate::forge::preflight_fork_collision(git, &resolved.info)?;
    let elapsed = started.elapsed();

    // Write-through to the forge-PR cache: we hold this PR's fresh metadata,
    // so `daft list --columns +pr` and pr: completion learn it immediately.
    // Best-effort; never delays or fails the checkout.
    crate::commands::forge_cache::persist_resolved(&resolved.info);

    let info = resolved.info;
    // The head lives at a base-repo ref, fetched into a local remote-tracking
    // ref named for the platform's convention. For a fork PR/MR that ref is
    // the only way to the source branch; for a same-repo one it is the
    // fallback core uses when the source branch is gone from the base repo
    // (deleted after a merge/close).
    let head_refs = checkout::ForgeForkRefs {
        head_ref: info.head_ref(),
        local_ref: format!(
            "refs/remotes/{}/{}/{}",
            resolved.base_remote,
            info.kind.tag(),
            info.number
        ),
    };
    let (fork, head_fallback) = if info.is_cross_repo {
        (Some(head_refs), None)
    } else {
        (None, Some(head_refs))
    };

    Ok(Some(ResolvedForge {
        header: info.display(),
        branch_name: info.source_branch.clone(),
        forge: checkout::ForgeCheckout {
            remote: resolved.base_remote,
            fork,
            head_fallback,
            display: info.display(),
            title: info.title.clone(),
            state_note: info.state_note(),
            resolve_elapsed: elapsed,
        },
    }))
}

fn run_checkout(
    args: &Args,
    settings: &DaftSettings,
    git: &GitCommand,
    output: &mut dyn Output,
) -> Result<bool, checkout::CheckoutError> {
    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let project_root = get_project_root()?;

    // Resolve a forge PR/MR target (pr:/mr:/URL) up front — it rewrites the
    // branch name, forces the fetch, and drives the rail header.
    let forge = resolve_forge_target(args, settings, git, &project_root)?;
    let is_forge = forge.is_some();
    let effective_branch = forge
        .as_ref()
        .map_or(args.branch_name.as_str(), |f| f.branch_name.as_str())
        .to_string();
    let header_target = forge
        .as_ref()
        .map_or(args.branch_name.as_str(), |f| f.header.as_str())
        .to_string();
    let forge_checkout = forge.map(|f| f.forge);

    let (resolved_layout, source) = resolve_checkout_layout(git, output);
    let (layout, should_persist) = interactive_layout_resolution(&resolved_layout, source, output)?;

    if should_persist && let Ok(git_dir) = get_git_common_dir() {
        let _ = TrustDatabase::update(|db| {
            db.set_layout(&git_dir, layout.name.clone());
            Ok(())
        });
    }

    let params = checkout::CheckoutParams {
        branch_name: effective_branch,
        carry: args.carry,
        no_carry: args.no_carry,
        remote: args.remote.clone(),
        remote_name: wt_config.remote_name.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_carry: settings.checkout_carry,
        checkout_upstream: settings.checkout_upstream,
        // Forge targets always fetch (the PR/MR ref must be materialized);
        // --local is rejected earlier for them.
        checkout_fetch: if is_forge {
            true
        } else if args.local {
            false
        } else {
            settings.checkout_fetch
        },
        layout: Some(layout),
        at_path: args.at.clone(),
        // The morph (branch missing → run_create_branch) must leave no rail
        // behind: hold the plan until the branch is known to exist, so the
        // fetch runs under the planning face and a not-found dissolves the
        // face tracelessly instead of closing a Failed receipt before
        // start's rail opens. Forge targets never morph (their misses are
        // Other, not BranchNotFound), so they don't defer.
        defer_plan_until_branch_known: !is_forge && (args.start || settings.go_auto_start),
        forge: forge_checkout,
    };

    let hooks_config = crate::core::settings::load_hooks_config_with(git)?;
    let hook_output_config = hooks_config.output.with_cli_verbose(output.is_verbose());
    let executor = HookExecutor::new(hooks_config)?.with_job_filter(
        crate::hooks::yaml_executor::JobFilter::skipping(&args.skip_hooks),
    );

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    // Plan-execute rail timeline (#651). The rail opens immediately with a
    // planning face; the core replaces it with the committed plan. The
    // early-exit paths (existing worktree, `go -`, fetch-off branch not
    // found) never commit — the face collapses without a trace and today's
    // single-line output renders.
    let mut timeline = Timeline::new(
        TimelineMode::auto(output.is_quiet()),
        output.is_verbose(),
        format!("Opening {header_target}"),
    );

    timeline.open_planning("Resolving branch");
    let checkout_result = {
        let mut bridge = TimelineBridge::new(output, &mut timeline, executor, hook_output_config);
        checkout::execute(&params, git, &project_root, &mut bridge)
    };
    timeline.abandon_planning();
    let result = match checkout_result {
        Ok(result) => result,
        Err(e) => {
            // With a committed plan (fetch-on branch not found, step
            // failures) this closes the rail into a Failed receipt; a
            // resolve-phase error left no region behind and this no-ops.
            timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
            return Err(e);
        }
    };

    if timeline.region_live() {
        timeline.finish(&format!("Ready in {}", timeline.elapsed_display()));
    }
    // On the rail, the header + footer are the record; Plain/Hidden (and the
    // no-rail early exits) keep the result line byte-identical to before —
    // and so does a redirected stdout, which never saw the rail.
    if !timeline.replaces_stdout_record() || result.already_existed {
        render_checkout_result(&result, output);
    }

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(result.already_existed)
}

fn run_create_branch(
    args: &Args,
    settings: &DaftSettings,
    git: &GitCommand,
    output: &mut dyn Output,
) -> Result<()> {
    let result = run_create_branch_core(args, settings, git, output)?;

    // Run exec commands (after hooks, before cd_path)
    let exec_result = crate::exec::run_exec_commands(&args.exec, output);

    output.cd_path(&result.cd_target);
    maybe_show_shell_hint(output)?;

    // Propagate exec error after cd_path is written
    exec_result?;

    Ok(())
}

/// The create-branch machinery without the terminal tail (exec commands,
/// cd redirect, shell hint) — reusable per-repo by `--with-related`.
fn run_create_branch_core(
    args: &Args,
    settings: &DaftSettings,
    git: &GitCommand,
    output: &mut dyn Output,
) -> Result<checkout_branch::CheckoutBranchResult> {
    // A forge PR/MR reference names an existing PR, not a new branch to create.
    // This is the single choke point for the create family (`daft start`,
    // `checkout -b`, `go -b`, `--with-related`).
    if crate::forge::ForgeTarget::parse(&args.branch_name).is_some() {
        anyhow::bail!(
            "'{}' is a pull/merge request reference, not a new branch name.\n  \
             tip: `{}` checks it out into a worktree.",
            args.branch_name,
            crate::daft_cmd(&format!("go {}", args.branch_name)),
        );
    }
    if let Some(base) = &args.base_branch_name
        && crate::forge::ForgeTarget::parse(base).is_some()
    {
        anyhow::bail!(
            "basing a new branch on a pull/merge request isn't supported yet.\n  \
             tip: check out `{base}` first with `{}`.",
            crate::daft_cmd(&format!("go {base}")),
        );
    }

    let wt_config = WorktreeConfig {
        remote_name: settings.remote.clone(),
        quiet: output.is_quiet(),
    };
    let project_root = get_project_root()?;

    let (resolved_layout, source) = resolve_checkout_layout(git, output);
    let (layout, should_persist) = interactive_layout_resolution(&resolved_layout, source, output)?;

    if should_persist && let Ok(git_dir) = get_git_common_dir() {
        let _ = TrustDatabase::update(|db| {
            db.set_layout(&git_dir, layout.name.clone());
            Ok(())
        });
    }

    let params = checkout_branch::CheckoutBranchParams {
        new_branch_name: args.branch_name.clone(),
        base_branch_name: args.base_branch_name.clone(),
        carry: args.carry,
        no_carry: args.no_carry,
        remote: args.remote.clone(),
        remote_name: wt_config.remote_name.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_branch_carry: settings.checkout_branch_carry,
        checkout_push: if args.local {
            false
        } else {
            settings.checkout_push
        },
        no_verify: args.no_verify,
        push_verify: settings.checkout_push_verify,
        checkout_fetch: if args.local {
            false
        } else {
            settings.checkout_fetch
        },
        layout: Some(layout),
        at_path: args.at.clone(),
    };

    let hooks_config = crate::core::settings::load_hooks_config_with(git)?;
    let hook_output_config = hooks_config.output.with_cli_verbose(output.is_verbose());
    let executor = HookExecutor::new(hooks_config)?.with_job_filter(
        crate::hooks::yaml_executor::JobFilter::skipping(&args.skip_hooks),
    );

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    // Plan-execute rail timeline (#651).
    let mut timeline = Timeline::new(
        TimelineMode::auto(output.is_quiet()),
        output.is_verbose(),
        format!("Starting {}", args.branch_name),
    );
    let interactive = timeline.is_interactive();

    // Presenter for the pre-push hook run on the auto-upstream push. On the
    // rail it embeds under the active Push row; off the rail, keep the legacy
    // #599 behavior (presenter only when a hook could fire, spinner otherwise).
    // Whether the hook ACTUALLY runs is decided in core (push_if_enabled) after
    // the post-fetch ref-only probe (#679); this gate is a conservative upper
    // bound, so `PushVerify::Never` (hook never consulted) is excluded and a
    // skipped hook simply never fires the presenter.
    let push_hook_may_render = params.checkout_push
        && !params.no_verify
        && params.push_verify != PushVerify::Never
        && git.pre_push_hook_exists(&project_root);
    let push_presenter: Option<Arc<dyn crate::executor::presenter::JobPresenter>> = if interactive {
        Some(
            crate::executor::cli_presenter::CliPresenter::embedded_for_stage(
                &hook_output_config,
                timeline.handle(),
                crate::core::stage::StageId::Push,
            ),
        )
    } else if push_hook_may_render {
        Some(crate::executor::cli_presenter::CliPresenter::auto(
            &hook_output_config,
        ))
    } else {
        None
    };

    // The rail opens immediately with a planning face; the plan commits
    // milliseconds later (start's resolution is local — the fetch and push
    // are planned rows). The pre-push hook embeds under the active Push row
    // (#686's silent-gap concern is covered by the rail itself); core's
    // pause_spinner/resume_spinner bracketing in push_if_enabled stays for
    // the legacy CommandBridge commands.
    timeline.open_planning("Resolving base branch");
    let checkout_result = {
        let mut bridge =
            TimelineBridge::new(output, &mut timeline, executor, hook_output_config.clone());
        checkout_branch::execute(
            &params,
            git,
            &project_root,
            push_presenter.as_ref(),
            &mut bridge,
        )
    };
    timeline.abandon_planning();
    let result = match checkout_result {
        Ok(result) => result,
        Err(e) => {
            timeline.abort(&format!("Failed after {}", timeline.elapsed_display()));
            return Err(e);
        }
    };

    if timeline.region_live() {
        timeline.finish(&format!("Ready in {}", timeline.elapsed_display()));
    }
    if !timeline.replaces_stdout_record() {
        render_create_result(&result, output);
    }

    Ok(result)
}

/// `daft start <branch> --with-related`: create `branch` in the primary
/// repo (the cwd's — `run_start_cross` roots a catalog target by entering
/// it first), then in every repo the primary's relations manifest points
/// at. Resolution is all-upfront (a missing clone aborts before anything is
/// created); per-repo creation failures are collected and reported, not
/// cascaded. The final cd target is the primary repo's new worktree.
/// `source_worktree` is the caller's pre-hop worktree for `daft go -`.
fn run_start_with_related(
    start_args: StartArgs,
    branch: String,
    base: Option<String>,
    source_worktree: Option<PathBuf>,
) -> Result<()> {
    init_logging(start_args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }
    crate::catalog::touch_current_repo();

    // Resolve every relation before creating anything.
    let resolved = crate::catalog::relations::current_repo_resolved_relations()?;
    if resolved.is_empty() {
        anyhow::bail!(
            "this repo declares no relations — add a `relations:` section to daft.yml \
             (each entry: `- url: <remote-url>`)"
        );
    }
    for relation in &resolved {
        let Some(row) = &relation.repo else {
            anyhow::bail!(
                "related repo '{}' is not cloned locally\n  tip: `{}`, then re-run — \
                 --with-related creates the branch everywhere, so every related repo \
                 must exist first",
                relation.entry.label(),
                crate::daft_cmd(&format!("clone {}", relation.entry.url))
            );
        };
        if !std::path::Path::new(&row.path).is_dir() {
            anyhow::bail!(
                "related repo '{}' points at '{}', which no longer exists\n  tip: `{}`",
                row.name,
                row.path,
                crate::daft_cmd(&format!("clone {}", row.name))
            );
        }
    }

    let args = start_args.to_create_args(branch, base);

    let original_dir = get_current_directory()?;

    // 1) Current repo first — its failure aborts the whole fan-out.
    let git = GitCommand::new(args.quiet);
    let settings = DaftSettings::load_with(&git)?;
    let git = git.with_gitoxide(settings.use_gitoxide);
    let autocd = settings.autocd && !args.no_cd;
    let mut output = CliOutput::new(OutputConfig::with_autocd(args.quiet, args.verbose, autocd));

    let current_result = match run_create_branch_core(&args, &settings, &git, &mut output) {
        Ok(result) => result,
        Err(e) => {
            change_directory(&original_dir).ok();
            return Err(e);
        }
    };

    // 2) Every related repo, collecting failures instead of cascading.
    let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
    for relation in &resolved {
        let row = relation.repo.as_ref().expect("verified upfront");
        output.result(&format!(
            "Creating '{}' in '{}'…",
            args.branch_name, row.name
        ));
        if let Err(e) = create_branch_in_related_repo(row, &args, &mut output) {
            failures.push((row.name.clone(), e));
        }
    }

    // 3) Settle in the current repo's new worktree.
    change_directory(&current_result.cd_target)?;
    if let Some(src) = source_worktree
        && let Ok(git_dir) = get_git_common_dir()
    {
        let _ = previous::save(&git_dir, &src);
    }

    // -x runs only in the current repo (documented).
    let exec_result = crate::exec::run_exec_commands(&args.exec, &mut output);
    output.cd_path(&current_result.cd_target);
    maybe_show_shell_hint(&mut output)?;
    exec_result?;

    if !failures.is_empty() {
        for (name, e) in &failures {
            output.warning(&format!("'{name}': {e}"));
        }
        anyhow::bail!(
            "branch '{}' created here, but creation failed in {} related repo(s)",
            args.branch_name,
            failures.len()
        );
    }
    Ok(())
}

/// Create `args.branch_name` in a related repo, based on that repo's own
/// default branch (recorded in the catalog, else origin/HEAD) regardless of
/// which worktree happens to be checked out there. Never carry, never run
/// `-x`, and run hooks only when the repo is explicitly trusted (`Allow`) — a
/// fan-out must not block on interactive trust prompts.
fn create_branch_in_related_repo(
    row: &crate::store::CatalogRepoRow,
    args: &Args,
    output: &mut dyn Output,
) -> Result<()> {
    let repo_root = std::path::Path::new(&row.path);
    let Some(worktree) = crate::core::repo::find_representative_worktree(repo_root) else {
        anyhow::bail!("'{}' has no worktrees to base the new branch on", row.name);
    };
    let restore = get_current_directory()?;
    change_directory(&worktree)?;

    let result = (|| {
        let git = GitCommand::new(args.quiet);
        let settings = DaftSettings::load_with(&git)?;
        let git = git.with_gitoxide(settings.use_gitoxide);

        let mut repo_args = args.clone();
        // Base the new branch on the related repo's own default branch (its
        // recorded catalog default, else origin/HEAD) — NOT whatever branch
        // find_representative_worktree happened to enter, which is wrong when
        // the default branch has no checkout. An explicit base only applies to
        // the current repo; fall back to the entered worktree's branch (None)
        // only when the default is unknown.
        repo_args.base_branch_name = crate::catalog::effective_default_branch(row);
        // Carry and -x never cross repos.
        repo_args.carry = false;
        repo_args.no_carry = true;
        repo_args.exec = Vec::new();

        let git_dir = get_git_common_dir()?;
        let trusted = TrustDatabase::load()
            .map(|db| db.get_trust_level(&git_dir) == crate::hooks::TrustLevel::Allow)
            .unwrap_or(false);
        if !trusted && !repo_args.skip_hooks.iter().any(|s| s == "all") {
            repo_args.skip_hooks.push("all".to_string());
            output.notice(&format!(
                "hooks skipped in '{}' (repo not trusted; run `{}` there)",
                row.name,
                crate::daft_cmd("hooks trust")
            ));
        }

        run_create_branch_core(&repo_args, &settings, &git, output).map(|_| ())
    })();

    change_directory(&restore).ok();
    result
}

fn render_branch_not_found_error(
    branch: &str,
    remote: &str,
    fetch_failed: bool,
    settings: &DaftSettings,
) {
    // Section 1: Diagnosis
    if fetch_failed {
        eprintln!(
            "error: Branch '{branch}' not found -- could not reach remote '{remote}' to check"
        );
    } else {
        eprintln!(
            "error: Branch '{branch}' not found -- it does not exist locally or on remote '{remote}'"
        );
    }

    // Section 2: Start suggestion (skip if fetch failed since start would also likely fail)
    if !fetch_failed {
        eprintln!();
        eprintln!("  tip: Use `daft go --start {branch}` or `daft start {branch}` to create it");
    }

    // Section 3: Fuzzy matches
    let git = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
    let all_branches = checkout::collect_branch_names(&git, remote);
    let suggestions = crate::suggest::find_similar(branch, &all_branches, 5);
    if !suggestions.is_empty() {
        eprintln!();
        if suggestions.len() == 1 {
            eprintln!("  Did you mean this?");
        } else {
            eprintln!("  Did you mean one of these?");
        }
        for s in &suggestions {
            eprintln!("    {s}");
        }
    }
}

fn render_checkout_result(result: &checkout::CheckoutResult, output: &mut dyn Output) {
    if result.already_existed {
        output.result(&format!(
            "Switched to existing worktree '{}'",
            result.branch_name
        ));
    } else {
        output.result(&format!("Prepared worktree '{}'", result.branch_name));
    }
}

fn render_create_result(result: &checkout_branch::CheckoutBranchResult, output: &mut dyn Output) {
    output.result(&format!(
        "Created worktree '{}' from '{}'",
        result.new_branch_name, result.base_branch
    ));
}

#[cfg(test)]
mod skip_hooks_parse_tests {
    use super::*;

    #[test]
    fn flag_after_positional() {
        let a = Args::parse_from(["git-worktree-checkout", "feat/x", "--skip-hooks", "all"]);
        assert_eq!(a.branch_name, "feat/x");
        assert_eq!(a.skip_hooks, vec!["all".to_string()]);
    }

    #[test]
    fn flag_before_positional() {
        let a = Args::parse_from(["git-worktree-checkout", "--skip-hooks", "all", "feat/x"]);
        assert_eq!(a.branch_name, "feat/x");
        assert_eq!(a.skip_hooks, vec!["all".to_string()]);
    }

    #[test]
    fn comma_split() {
        let a = Args::parse_from([
            "git-worktree-checkout",
            "feat/x",
            "--skip-hooks",
            "all,tag:heavy",
        ]);
        assert_eq!(
            a.skip_hooks,
            vec!["all".to_string(), "tag:heavy".to_string()]
        );
    }
}

#[cfg(test)]
mod start_grammar_tests {
    use super::*;

    /// Parse a `daft start` argv and decode it against fake catalogs.
    fn decode(argv: &[&str], live_repos: &[&str], local_branches: &[&str]) -> Result<StartRouting> {
        let mut full = vec!["daft start"];
        full.extend_from_slice(argv);
        let a = StartArgs::try_parse_from(full).expect("argv should parse");
        decode_start_grammar(
            a.first,
            a.second,
            a.third,
            a.repo,
            |name| local_branches.contains(&name),
            |name| live_repos.contains(&name),
        )
    }

    #[test]
    fn single_positional_is_local() {
        let routing = decode(&["feat/x"], &[], &[]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Local {
                branch: "feat/x".into(),
                base: None
            }
        );
    }

    #[test]
    fn single_positional_stays_local_even_for_a_live_repo() {
        // A lone repo name can never be a start target — there is no branch.
        let routing = decode(&["api"], &["api"], &[]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Local {
                branch: "api".into(),
                base: None
            }
        );
    }

    #[test]
    fn two_positionals_guess_cross_for_a_live_repo() {
        let routing = decode(&["api", "feat/x"], &["api"], &[]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Cross {
                repo: "api".into(),
                branch: "feat/x".into(),
                base: None,
                guessed: true,
            }
        );
    }

    #[test]
    fn two_positionals_stay_local_when_first_is_not_a_repo() {
        let routing = decode(&["feat/x", "develop"], &["api"], &[]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Local {
                branch: "feat/x".into(),
                base: Some("develop".into())
            }
        );
    }

    #[test]
    fn local_branch_beats_catalog_match() {
        // Local meaning always wins over the catalog: fail fast, never
        // silently retarget another repo.
        let routing = decode(&["api", "feat/x"], &["api"], &["api"]).unwrap();
        assert_eq!(
            routing,
            StartRouting::LocalCollision {
                branch: "api".into(),
                second: "feat/x".into(),
                also_live_repo: true,
            }
        );
    }

    #[test]
    fn local_branch_collision_without_a_repo_match() {
        let routing = decode(&["api", "feat/x"], &[], &["api"]).unwrap();
        assert_eq!(
            routing,
            StartRouting::LocalCollision {
                branch: "api".into(),
                second: "feat/x".into(),
                also_live_repo: false,
            }
        );
    }

    #[test]
    fn three_positionals_are_cross_without_consulting_probes() {
        let routing = decode(&["api", "feat/x", "main"], &[], &[]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Cross {
                repo: "api".into(),
                branch: "feat/x".into(),
                base: Some("main".into()),
                guessed: false,
            }
        );
    }

    #[test]
    fn repo_flag_forces_cross() {
        // Even a name shadowed by a local branch stays cross with --repo.
        let routing = decode(&["--repo", "api", "feat/x"], &[], &["api"]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Cross {
                repo: "api".into(),
                branch: "feat/x".into(),
                base: None,
                guessed: false,
            }
        );
    }

    #[test]
    fn repo_flag_with_base() {
        let routing = decode(&["--repo", "api", "feat/x", "main"], &[], &[]).unwrap();
        assert_eq!(
            routing,
            StartRouting::Cross {
                repo: "api".into(),
                branch: "feat/x".into(),
                base: Some("main".into()),
                guessed: false,
            }
        );
    }

    #[test]
    fn repo_flag_rejects_a_third_positional() {
        let err = decode(&["--repo", "api", "feat/x", "main", "extra"], &[], &[]).unwrap_err();
        assert!(
            err.to_string()
                .contains("--repo already names the target repo")
        );
    }

    #[test]
    fn dash_is_rejected_everywhere() {
        assert!(decode(&["-"], &[], &[]).is_err());
        assert!(decode(&["api", "-"], &["api"], &[]).is_err());
        assert!(decode(&["feat/x", "main", "-"], &[], &[]).is_err());
        assert!(decode(&["--repo", "api", "-"], &[], &[]).is_err());
    }

    #[test]
    fn repo_flag_requires_a_branch_positional() {
        assert!(StartArgs::try_parse_from(["daft start", "--repo", "api"]).is_err());
    }
}

#[cfg(test)]
mod go_grammar_tests {
    use super::*;

    /// Parse a `daft go` argv and decode it.
    fn decode(argv: &[&str]) -> Result<(String, Option<String>, GoRouting)> {
        let mut full = vec!["daft go"];
        full.extend_from_slice(argv);
        let go = GoArgs::parse_from(full);
        decode_go_grammar(go.branch_name, go.second, go.repo, go.create_branch)
    }

    fn cross(routing: &GoRouting) -> &CrossTarget {
        routing
            .cross
            .as_ref()
            .expect("expected a cross-repo target")
    }

    #[test]
    fn single_positional_is_local_with_fallback() {
        let (branch, base, routing) = decode(&["feat/x"]).unwrap();
        assert_eq!(branch, "feat/x");
        assert_eq!(base, None);
        assert!(routing.cross.is_none());
        assert!(routing.catalog_fallback);
    }

    #[test]
    fn dash_stays_local() {
        let (branch, _, routing) = decode(&["-"]).unwrap();
        assert_eq!(branch, "-");
        assert!(routing.cross.is_none());
    }

    #[test]
    fn two_positionals_mean_repo_and_branch() {
        let (branch, base, routing) = decode(&["api", "feat/x"]).unwrap();
        assert_eq!(branch, "feat/x", "args carry the target branch");
        assert_eq!(base, None);
        let c = cross(&routing);
        assert_eq!(c.repo, "api");
        assert_eq!(c.branch.as_deref(), Some("feat/x"));
        assert!(c.positional);
        assert!(!routing.catalog_fallback);
    }

    #[test]
    fn create_branch_keeps_base_meaning_for_second_positional() {
        let (branch, base, routing) = decode(&["-b", "feat/x", "main"]).unwrap();
        assert_eq!(branch, "feat/x");
        assert_eq!(base.as_deref(), Some("main"));
        assert!(routing.cross.is_none());
        assert!(!routing.catalog_fallback, "-b never consults the catalog");
    }

    #[test]
    fn repo_flag_alone_targets_default_branch() {
        let (_, base, routing) = decode(&["--repo", "api"]).unwrap();
        assert_eq!(base, None);
        let c = cross(&routing);
        assert_eq!(c.repo, "api");
        assert_eq!(c.branch, None);
        assert!(!c.positional);
    }

    #[test]
    fn repo_flag_with_branch() {
        let (branch, _, routing) = decode(&["--repo", "api", "feat/x"]).unwrap();
        assert_eq!(branch, "feat/x");
        assert_eq!(cross(&routing).branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn repo_flag_with_create_and_base() {
        let (branch, base, routing) = decode(&["--repo", "api", "-b", "feat/x", "main"]).unwrap();
        assert_eq!(branch, "feat/x");
        assert_eq!(base.as_deref(), Some("main"));
        assert_eq!(cross(&routing).repo, "api");
    }

    #[test]
    fn repo_flag_rejects_two_positionals() {
        let err = decode(&["--repo", "api", "a", "b"]).unwrap_err();
        assert!(err.to_string().contains("--repo takes a single <branch>"));
    }

    #[test]
    fn repo_flag_rejects_dash() {
        let err = decode(&["--repo", "api", "-"]).unwrap_err();
        assert!(err.to_string().contains("Cannot use '-' with --repo"));
    }

    #[test]
    fn repo_flag_with_create_requires_branch_name() {
        let err = decode(&["--repo", "api", "-b"]).unwrap_err();
        assert!(
            err.to_string().contains("requires a branch name"),
            "got: {err}"
        );
    }

    #[test]
    fn dash_with_second_positional_is_rejected() {
        let err = decode(&["-", "feat/x"]).unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot use '-' with a second argument")
        );
    }

    #[test]
    fn checkout_binary_grammar_is_untouched() {
        // git-worktree-checkout still gates positional 2 on -b.
        let parse = Args::try_parse_from(["git-worktree-checkout", "a", "b"]);
        assert!(
            parse.is_ok(),
            "clap-level parse of two positionals stays OK (validated later)"
        );
        let a = parse.unwrap();
        assert_eq!(a.branch_name, "a");
        assert_eq!(a.base_branch_name.as_deref(), Some("b"));
    }
}

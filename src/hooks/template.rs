//! Template substitution for hook commands.
//!
//! Replaces template variables like `{worktree_path}`, `{branch}`, etc.
//! in hook command strings with actual values from the execution context.

use super::environment::HookContext;

/// Substitute template variables in a command string.
///
/// Supported templates:
/// - `{worktree_path}` — target worktree path
/// - `{worktree_branch}` — target branch name
/// - `{worktree_root}` — project root directory
/// - `{worktree_slug}` — sanitized worktree name, safe for docker/DNS use
/// - `{branch}` — alias for `{worktree_branch}`
/// - `{commit}` — pinned commit OID (branchless sandbox worktrees only)
/// - `{job_name}` — name of the current job (if provided)
/// - `{source_worktree}` — source worktree path
/// - `{git_dir}` — path to .git directory
/// - `{remote}` — remote name (usually "origin")
/// - `{base_branch}` — base branch name (if set)
/// - `{repository_url}` — repository URL (if set)
/// - `{default_branch}` — default branch name (if set)
/// - `{merge_source_path}` — the merge source's worktree path (merge hooks
///   with exactly one worktree-backed source)
/// - `{merge_target_path}` — the merge target's worktree path (merge hooks
///   with a worktree-backed target)
///
/// The merge templates substitute only when the corresponding
/// `DAFT_MERGE_*_PATH` env var is present AND non-empty; otherwise the
/// placeholder is left intact so callers with fail-closed semantics (job
/// `root:`) can detect and refuse it rather than running somewhere wrong.
///
/// For git stages, `{1}`..`{9}` expand to the arguments git passed the hook
/// and `{0}` to all of them — see [`substitute_positionals`].
///
/// `{changed_files}` is NOT handled here: it expands to a per-job filtered
/// file list, so the job adapter substitutes it after glob filtering (see
/// [`crate::hooks::changed_files::CHANGED_FILES_TEMPLATE`]).
pub fn substitute(command: &str, ctx: &HookContext, job_name: Option<&str>) -> String {
    let mut result = command.to_string();

    // Positionals first: a stage argument is a path or a ref that must not be
    // re-scanned for `{…}` placeholders once substituted.
    if !ctx.stage_argv.is_empty() {
        result = substitute_positionals(&result, &ctx.stage_argv);
    }

    result = result.replace("{worktree_path}", &ctx.worktree_path.to_string_lossy());
    result = result.replace("{worktree_branch}", &ctx.branch_name);
    result = result.replace("{worktree_root}", &ctx.project_root.to_string_lossy());
    // Compute the slug lazily only when referenced — cheap either way, but
    // avoids a filesystem-path allocation for the common no-slug command.
    if result.contains("{worktree_slug}") {
        result = result.replace("{worktree_slug}", &worktree_slug(ctx));
    }
    result = result.replace("{branch}", &ctx.branch_name);
    result = result.replace("{source_worktree}", &ctx.source_worktree.to_string_lossy());
    result = result.replace("{git_dir}", &ctx.git_dir.to_string_lossy());
    result = result.replace("{remote}", &ctx.remote);

    if let Some(name) = job_name {
        result = result.replace("{job_name}", name);
    }

    if let Some(ref base) = ctx.base_branch {
        result = result.replace("{base_branch}", base);
    }

    if let Some(ref commit) = ctx.commit {
        result = result.replace("{commit}", commit);
    }

    if let Some(ref url) = ctx.repository_url {
        result = result.replace("{repository_url}", url);
    }

    if let Some(ref branch) = ctx.default_branch {
        result = result.replace("{default_branch}", branch);
    }

    // Merge-context paths, sourced from the extra env the merge command
    // threads in. Empty means "no such worktree" and leaves the placeholder
    // for the caller's fail-closed handling.
    for (template, env_key) in [
        ("{merge_source_path}", "DAFT_MERGE_SOURCE_PATH"),
        ("{merge_target_path}", "DAFT_MERGE_TARGET_PATH"),
    ] {
        if let Some(value) = ctx.extra_env.get(env_key).filter(|v| !v.is_empty()) {
            result = result.replace(template, value);
        }
    }

    // Move-specific templates
    let old_path = ctx
        .old_worktree_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let old_branch = ctx.old_branch_name.as_deref().unwrap_or_default();
    result = result.replace("{old_worktree_path}", &old_path);
    result = result.replace("{old_branch}", old_branch);

    result
}

/// Expand `{1}`..`{9}` to git's hook arguments and `{0}` to all of them.
///
/// The numbering is git's, and deliberately so: a `commit-msg` job written
/// against any other hook manager reads `$1` as the message file, and `{1}`
/// is the same thing said in this schema's syntax. `{0}` is the whole argv,
/// matching `"$@"`.
///
/// Values are shell-quoted, because a command string is handed to `sh -c`
/// and a message-file path can contain spaces. An index git did not supply
/// expands empty — the same thing the shell does with an unset `$2`, and the
/// reason a `prepare-commit-msg` job can reference `{2}` without guarding it.
///
/// Single left-to-right pass, so a substituted value containing `{1}` is
/// never rescanned.
fn substitute_positionals(command: &str, argv: &[String]) -> String {
    let quote = |s: &String| crate::utils::quote_argv(std::slice::from_ref(s));
    let mut out = String::with_capacity(command.len());
    let mut rest = command;

    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        // `{` then digits then `}`, with nothing else in between.
        match after.find('}') {
            Some(close) if close > 0 && after[..close].chars().all(|c| c.is_ascii_digit()) => {
                let index: usize = after[..close].parse().unwrap_or(usize::MAX);
                if index == 0 {
                    out.push_str(&crate::utils::quote_argv(argv));
                } else if let Some(value) = argv.get(index - 1) {
                    out.push_str(&quote(value));
                }
                rest = &after[close + 1..];
            }
            _ => {
                // Not a positional — emit the brace and keep scanning after
                // it so `{worktree_path}` survives for the caller.
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Sanitized slug for the worktree, safe to embed in docker compose project
/// names, DB schema names, DNS labels, and temp-dir names.
///
/// Thin context adapter over [`crate::core::slug::worktree_slug_from`] — the
/// algorithm lives there so the `{worktree_slug}` template variable and the
/// derived env values (`daft env`) can never disagree about a worktree's
/// identity.
pub fn worktree_slug(ctx: &HookContext) -> String {
    crate::core::slug::worktree_slug_from(&ctx.worktree_path, &ctx.project_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;

    fn make_ctx() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
        .with_base_branch("main")
    }

    fn stage_ctx(stage: crate::hooks::git_stage::GitStage, argv: &[&str]) -> HookContext {
        HookContext::new(
            HookType::Git(stage),
            "__hook",
            "/project",
            "/project/.git",
            "origin",
            "/project/feature",
            "/project/feature",
            "feature",
        )
        .with_stage_payload(argv.iter().map(|s| s.to_string()).collect(), None::<String>)
    }

    #[test]
    fn positionals_carry_gits_own_numbering() {
        use crate::hooks::git_stage::GitStage;
        // `{1}` is `$1` — the message file every other hook manager's
        // commit-msg job already reads.
        let ctx = stage_ctx(GitStage::CommitMsg, &[".git/COMMIT_EDITMSG"]);
        assert_eq!(
            substitute("check {1}", &ctx, None),
            "check .git/COMMIT_EDITMSG"
        );
    }

    #[test]
    fn zero_expands_to_the_whole_argv() {
        use crate::hooks::git_stage::GitStage;
        let ctx = stage_ctx(GitStage::PostCheckout, &["abc", "def", "1"]);
        assert_eq!(substitute("log {0}", &ctx, None), "log abc def 1");
    }

    #[test]
    fn positionals_are_shell_quoted() {
        use crate::hooks::git_stage::GitStage;
        // The expansion lands inside a string handed to `sh -c`, and a
        // worktree path with a space is ordinary.
        let ctx = stage_ctx(GitStage::CommitMsg, &["/my worktree/.git/MSG"]);
        let out = substitute("wc -l {1}", &ctx, None);
        assert_eq!(out, "wc -l '/my worktree/.git/MSG'");
    }

    #[test]
    fn a_missing_positional_expands_empty_like_an_unset_shell_arg() {
        use crate::hooks::git_stage::GitStage;
        // git omits the source for a plain commit, so a job may reference
        // `{2}` unconditionally — as it would `$2`.
        let ctx = stage_ctx(GitStage::PrepareCommitMsg, &["/tmp/MSG"]);
        assert_eq!(substitute("f {1} {2}", &ctx, None), "f /tmp/MSG ");
    }

    #[test]
    fn named_templates_survive_beside_positionals() {
        use crate::hooks::git_stage::GitStage;
        let ctx = stage_ctx(GitStage::CommitMsg, &["/tmp/MSG"]);
        assert_eq!(
            substitute("cd {worktree_path} && lint {1} {branch}", &ctx, None),
            "cd /project/feature && lint /tmp/MSG feature"
        );
    }

    #[test]
    fn a_substituted_value_is_not_rescanned() {
        use crate::hooks::git_stage::GitStage;
        // A ref or path that itself contains `{1}` must come out literally,
        // not re-expand into the first argument again.
        let ctx = stage_ctx(GitStage::CommitMsg, &["lit{1}eral", "second"]);
        assert_eq!(substitute("f {1}", &ctx, None), "f 'lit{1}eral'");
    }

    #[test]
    fn lifecycle_hooks_leave_brace_digits_alone() {
        // No stage payload means no positional pass: a lifecycle command
        // containing `{1}` (a regex quantifier, say) is untouched.
        let ctx = make_ctx();
        assert_eq!(
            substitute("grep -E 'a{1,3}' {branch}", &ctx, None),
            "grep -E 'a{1,3}' feature/new"
        );
    }

    #[test]
    fn test_basic_substitution() {
        let ctx = make_ctx();
        let result = substitute("echo {branch}", &ctx, None);
        assert_eq!(result, "echo feature/new");
    }

    #[test]
    fn test_multiple_templates() {
        let ctx = make_ctx();
        let result = substitute("cd {worktree_path} && git checkout {branch}", &ctx, None);
        assert_eq!(
            result,
            "cd /project/feature/new && git checkout feature/new"
        );
    }

    #[test]
    fn test_job_name() {
        let ctx = make_ctx();
        let result = substitute("echo {job_name}", &ctx, Some("lint"));
        assert_eq!(result, "echo lint");
    }

    #[test]
    fn test_no_templates() {
        let ctx = make_ctx();
        let result = substitute("echo hello world", &ctx, None);
        assert_eq!(result, "echo hello world");
    }

    #[test]
    fn test_worktree_root() {
        let ctx = make_ctx();
        let result = substitute("ls {worktree_root}", &ctx, None);
        assert_eq!(result, "ls /project");
    }

    #[test]
    fn test_base_branch() {
        let ctx = make_ctx();
        let result = substitute("git diff {base_branch}", &ctx, None);
        assert_eq!(result, "git diff main");
    }

    /// `{commit}` substitutes only on the branchless sandbox path; `{branch}`
    /// there is the empty string by contract.
    #[test]
    fn test_commit_substitution_on_the_sandbox_path() {
        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/v1",
            "",
        )
        .with_commit("abc123d");
        assert_eq!(
            substitute("build {commit} on '{branch}'", &ctx, None),
            "build abc123d on ''"
        );
        // Without a commit the placeholder is left alone, like {base_branch}.
        assert_eq!(
            substitute("echo {commit}", &make_ctx(), None),
            "echo {commit}"
        );
    }

    #[test]
    fn merge_path_templates_substitute_from_extra_env() {
        let extra: std::collections::BTreeMap<String, String> = [
            ("DAFT_MERGE_SOURCE_PATH".to_string(), "/p/feat".to_string()),
            ("DAFT_MERGE_TARGET_PATH".to_string(), "/p/main".to_string()),
        ]
        .into();
        let ctx = make_ctx().with_extra_env(extra);
        assert_eq!(
            substitute(
                "test {merge_source_path} into {merge_target_path}",
                &ctx,
                None
            ),
            "test /p/feat into /p/main"
        );
    }

    #[test]
    fn merge_path_templates_stay_intact_when_absent_or_empty() {
        // Absent: no merge context at all.
        assert_eq!(
            substitute("cd {merge_source_path}", &make_ctx(), None),
            "cd {merge_source_path}"
        );
        // Present but empty (no worktree / multi-source): also left intact —
        // callers with fail-closed semantics detect the placeholder.
        let extra: std::collections::BTreeMap<String, String> =
            [("DAFT_MERGE_SOURCE_PATH".to_string(), String::new())].into();
        let ctx = make_ctx().with_extra_env(extra);
        assert_eq!(
            substitute("cd {merge_source_path}", &ctx, None),
            "cd {merge_source_path}"
        );
    }

    #[test]
    fn test_worktree_slug_nested_relative_path() {
        // make_ctx: worktree /project/feature/new under root /project.
        let ctx = make_ctx();
        assert_eq!(worktree_slug(&ctx), "feature-new");
    }

    #[test]
    fn test_worktree_slug_template_substitution() {
        let ctx = make_ctx();
        assert_eq!(
            substitute("api-{worktree_slug}", &ctx, None),
            "api-feature-new"
        );
    }

    #[test]
    fn test_worktree_slug_outside_root_uses_basename() {
        let ctx = HookContext::new(
            HookType::PostCreate,
            "checkout",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/elsewhere/My-WT",
            "feature/x",
        );
        assert_eq!(worktree_slug(&ctx), "my-wt");
    }

    #[test]
    fn test_old_template_vars_during_move() {
        use std::path::PathBuf;
        let ctx = HookContext {
            hook_type: HookType::PostCreate,
            command: "rename".to_string(),
            project_root: PathBuf::from("/project"),
            git_dir: PathBuf::from("/project/.git"),
            remote: "origin".to_string(),
            source_worktree: PathBuf::from("/project/old-wt"),
            worktree_path: PathBuf::from("/project/new-wt"),
            branch_name: "feat/new".to_string(),
            commit: None,
            is_new_branch: false,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
            is_move: true,
            old_worktree_path: Some(PathBuf::from("/project/old-wt")),
            old_branch_name: Some("feat/old".to_string()),
            changed_attributes: None,
            extra_env: std::collections::BTreeMap::new(),
            state_dir: None,
            task_name: None,
            stage_argv: Vec::new(),
            stage_stdin: None,
        };
        let result = substitute(
            "from {old_worktree_path} to {worktree_path} branch {old_branch}",
            &ctx,
            None,
        );
        assert_eq!(
            result,
            "from /project/old-wt to /project/new-wt branch feat/old"
        );
    }

    #[test]
    fn test_old_template_vars_empty_when_not_move() {
        use std::path::PathBuf;
        let ctx = HookContext {
            hook_type: HookType::PostCreate,
            command: "checkout".to_string(),
            project_root: PathBuf::from("/project"),
            git_dir: PathBuf::from("/project/.git"),
            remote: "origin".to_string(),
            source_worktree: PathBuf::from("/project/src"),
            worktree_path: PathBuf::from("/project/wt"),
            branch_name: "feat/x".to_string(),
            commit: None,
            is_new_branch: true,
            base_branch: None,
            repository_url: None,
            default_branch: None,
            removal_reason: None,
            is_move: false,
            old_worktree_path: None,
            old_branch_name: None,
            changed_attributes: None,
            extra_env: std::collections::BTreeMap::new(),
            state_dir: None,
            task_name: None,
            stage_argv: Vec::new(),
            stage_stdin: None,
        };
        let result = substitute("old={old_worktree_path} branch={old_branch}", &ctx, None);
        assert_eq!(result, "old= branch=");
    }
}

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
/// - `{branch}` — alias for `{worktree_branch}`
/// - `{job_name}` — name of the current job (if provided)
/// - `{source_worktree}` — source worktree path
/// - `{git_dir}` — path to .git directory
/// - `{remote}` — remote name (usually "origin")
/// - `{0}` — all hook arguments space-joined
/// - `{1}`, `{2}`, ... — individual positional hook arguments
///
/// File templates (`{staged_files}`, etc.) are handled separately in Phase 4.
pub fn substitute(
    command: &str,
    ctx: &HookContext,
    job_name: Option<&str>,
    hook_args: &[String],
) -> String {
    let mut result = command.to_string();

    result = result.replace("{worktree_path}", &ctx.worktree_path.to_string_lossy());
    result = result.replace("{worktree_branch}", &ctx.branch_name);
    result = result.replace("{worktree_root}", &ctx.project_root.to_string_lossy());
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

    if let Some(ref url) = ctx.repository_url {
        result = result.replace("{repository_url}", url);
    }

    if let Some(ref branch) = ctx.default_branch {
        result = result.replace("{default_branch}", branch);
    }

    // Hook argument templates: {0} = all args, {1}, {2}, etc.
    if result.contains("{0}") {
        let all_args = hook_args.join(" ");
        result = result.replace("{0}", &all_args);
    }

    // Replace individual positional args {1}, {2}, etc.
    for (i, arg) in hook_args.iter().enumerate() {
        let placeholder = format!("{{{}}}", i + 1);
        result = result.replace(&placeholder, arg);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;

    fn make_ctx() -> HookContext {
        HookContext::new(
            HookType::PostCreate,
            "checkout-branch",
            "/project",
            "/project/.git",
            "origin",
            "/project/main",
            "/project/feature/new",
            "feature/new",
        )
        .with_base_branch("main")
    }

    #[test]
    fn test_basic_substitution() {
        let ctx = make_ctx();
        let result = substitute("echo {branch}", &ctx, None, &[]);
        assert_eq!(result, "echo feature/new");
    }

    #[test]
    fn test_multiple_templates() {
        let ctx = make_ctx();
        let result = substitute(
            "cd {worktree_path} && git checkout {branch}",
            &ctx,
            None,
            &[],
        );
        assert_eq!(
            result,
            "cd /project/feature/new && git checkout feature/new"
        );
    }

    #[test]
    fn test_job_name() {
        let ctx = make_ctx();
        let result = substitute("echo {job_name}", &ctx, Some("lint"), &[]);
        assert_eq!(result, "echo lint");
    }

    #[test]
    fn test_no_templates() {
        let ctx = make_ctx();
        let result = substitute("echo hello world", &ctx, None, &[]);
        assert_eq!(result, "echo hello world");
    }

    #[test]
    fn test_worktree_root() {
        let ctx = make_ctx();
        let result = substitute("ls {worktree_root}", &ctx, None, &[]);
        assert_eq!(result, "ls /project");
    }

    #[test]
    fn test_base_branch() {
        let ctx = make_ctx();
        let result = substitute("git diff {base_branch}", &ctx, None, &[]);
        assert_eq!(result, "git diff main");
    }

    #[test]
    fn test_hook_args_all() {
        let ctx = make_ctx();
        let args = vec!["arg1".to_string(), "arg2".to_string()];
        let result = substitute("echo {0}", &ctx, None, &args);
        assert_eq!(result, "echo arg1 arg2");
    }

    #[test]
    fn test_hook_args_positional() {
        let ctx = make_ctx();
        let args = vec!["foo".to_string(), "bar".to_string(), "baz".to_string()];
        let result = substitute("echo {1} {3}", &ctx, None, &args);
        assert_eq!(result, "echo foo baz");
    }

    #[test]
    fn test_hook_args_empty() {
        let ctx = make_ctx();
        let result = substitute("echo {0}", &ctx, None, &[]);
        assert_eq!(result, "echo ");
    }
}

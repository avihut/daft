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
/// - `{base_branch}` — base branch name (if set)
/// - `{repository_url}` — repository URL (if set)
/// - `{default_branch}` — default branch name (if set)
pub fn substitute(command: &str, ctx: &HookContext, job_name: Option<&str>) -> String {
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

    result
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
}

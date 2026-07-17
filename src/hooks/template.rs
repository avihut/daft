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

    if let Some(ref url) = ctx.repository_url {
        result = result.replace("{repository_url}", url);
    }

    if let Some(ref branch) = ctx.default_branch {
        result = result.replace("{default_branch}", branch);
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

/// Sanitized slug for the worktree, safe to embed in docker compose project
/// names, DB schema names, DNS labels, and temp-dir names.
///
/// The raw name is the worktree path relative to the project root when the
/// worktree lives under it (so a nested worktree like `feature/new` slugs to
/// `feature-new`), otherwise the final path component. It is then lowercased,
/// every run of non-`[a-z0-9]` characters collapses to a single `-`, leading
/// and trailing `-` are trimmed, and the result is capped at 63 characters
/// (the DNS-label limit). An empty result falls back to `"worktree"`.
pub fn worktree_slug(ctx: &HookContext) -> String {
    let raw = ctx
        .worktree_path
        .strip_prefix(&ctx.project_root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            ctx.worktree_path
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    slugify(&raw)
}

/// Lowercase, collapse non-alphanumeric runs to single `-`, trim `-`, cap at
/// 63 chars. Empty input (or input that reduces to nothing) yields
/// `"worktree"`.
fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            // Collapse any run of separators/other chars into one dash.
            // Leading separators are suppressed (out is still empty).
            out.push('-');
            prev_dash = true;
        }
    }
    // Cap at the DNS-label length, then trim any trailing dash the cap or the
    // collapse may have left.
    out.truncate(63);
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "worktree".to_string()
    } else {
        trimmed.to_string()
    }
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
    fn test_slugify_cases() {
        assert_eq!(slugify("Feature/New"), "feature-new");
        assert_eq!(slugify("API_Server 2"), "api-server-2");
        assert_eq!(slugify("feat/ABC-123"), "feat-abc-123");
        // Pure-separator and empty inputs fall back.
        assert_eq!(slugify("---"), "worktree");
        assert_eq!(slugify(""), "worktree");
        assert_eq!(slugify("!!!@@@"), "worktree");
        // Unicode reduces to the fallback (no ascii-alphanumerics).
        assert_eq!(slugify("日本語"), "worktree");
        // 63-char DNS-label cap.
        assert_eq!(slugify(&"a".repeat(100)).len(), 63);
        // A trailing dash left by the cap is trimmed.
        let capped = format!("{}-tail", "a".repeat(62));
        assert_eq!(slugify(&capped), "a".repeat(62));
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
        };
        let result = substitute("old={old_worktree_path} branch={old_branch}", &ctx, None);
        assert_eq!(result, "old= branch=");
    }
}

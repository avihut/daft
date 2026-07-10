//! Pure normalization rules for catalog names and remote URLs.
//!
//! `normalize_url` produces the *match key* used to resolve relations-
//! manifest entries against catalog rows. Both sides of every comparison go
//! through this function, so the exact canonical form matters less than
//! applying it consistently: `git@github.com:Org/Api.git`,
//! `ssh://git@github.com/Org/Api`, and `https://github.com/Org/Api.git`
//! all collapse to `github.com/Org/Api`.
//!
//! Local filesystem remotes (integration fixtures, `file://` URLs) stay
//! path-shaped so path-based remotes match across clones of the same
//! fixture.

/// Normalize a remote URL into its match-key form.
///
/// Rules: scheme and user stripped, host lowercased (path case preserved),
/// default port for the scheme dropped, one trailing `.git` stripped,
/// trailing slashes stripped. Non-URL inputs (local paths) only get the
/// trailing-slash and `.git` treatment.
pub fn normalize_url(raw: &str) -> String {
    let s = raw.trim().trim_end_matches('/');

    if let Some(rest) = s.strip_prefix("ssh://") {
        return host_path(rest, Some("22"));
    }
    if let Some(rest) = s.strip_prefix("git+ssh://") {
        return host_path(rest, Some("22"));
    }
    if let Some(rest) = s.strip_prefix("git://") {
        return host_path(rest, Some("9418"));
    }
    if let Some(rest) = s.strip_prefix("http://") {
        return host_path(rest, Some("80"));
    }
    if let Some(rest) = s.strip_prefix("https://") {
        return host_path(rest, Some("443"));
    }
    if let Some(rest) = s.strip_prefix("file://") {
        return strip_git_suffix(rest).to_string();
    }
    if let Some((user_host, path)) = scp_like_parts(s) {
        let host = user_host.rsplit('@').next().unwrap_or(user_host);
        let path = strip_git_suffix(path.trim_start_matches('/'));
        return format!("{}/{}", host.to_lowercase(), path);
    }
    // Local path (absolute, relative, or ~) — keep as-is minus suffixes.
    strip_git_suffix(s).to_string()
}

/// `[user@]host[:port]/path` → `host[:port]/path` with the scheme's default
/// port dropped and the host lowercased.
fn host_path(rest: &str, default_port: Option<&str>) -> String {
    let (host_part, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => (rest, ""),
    };
    let host_port = host_part.rsplit('@').next().unwrap_or(host_part);
    let (host, port) = match host_port.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => (h, Some(p)),
        _ => (host_port, None),
    };
    let mut out = host.to_lowercase();
    if let Some(p) = port
        && default_port != Some(p)
    {
        out.push(':');
        out.push_str(p);
    }
    if !path.is_empty() {
        out.push('/');
        out.push_str(strip_git_suffix(path));
    }
    out
}

/// Split an scp-like remote (`[user@]host:path`) into its two halves.
/// Returns `None` for anything that can't be scp shorthand: URLs with a
/// scheme, strings without a colon, or colons that appear after the first
/// path separator (`/dir/weird:name` is a local path, not a host).
fn scp_like_parts(s: &str) -> Option<(&str, &str)> {
    if s.contains("://") {
        return None;
    }
    let (before, after) = s.split_once(':')?;
    if before.is_empty() || before.contains('/') {
        return None;
    }
    Some((before, after))
}

fn strip_git_suffix(s: &str) -> &str {
    let s = s.trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s)
}

/// Find a free name by suffixing `-2`, `-3`, … to `base`. `taken` reports
/// whether a candidate is already claimed by a live catalog entry.
pub fn suffixed_name(base: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let mut n: u32 = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Validate an explicit user-supplied catalog name. Stricter than the
/// implicit-derivation path: the name must already be in canonical form
/// (rather than silently rewriting what the user typed) and must not start
/// with `-` (would parse as a flag in `daft go <name>`).
pub fn validate_catalog_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name is empty".to_string());
    }
    if name.starts_with('-') {
        return Err("name cannot start with '-'".to_string());
    }
    if name.contains("..") {
        return Err("name cannot contain '..'".to_string());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.'))
    {
        return Err(format!(
            "name may only contain letters, digits, '-', '_' and '.' (found {bad:?})"
        ));
    }
    if name.len() > 255 {
        return Err("name is too long (max 255 characters)".to_string());
    }
    Ok(())
}

/// Derive a default catalog name for implicit registration: prefer the
/// remote URL's repo component, fall back to the project directory's name,
/// finally a literal `"repo"`. Always yields a valid catalog name.
pub fn derive_default_name(remote_url: Option<&str>, project_root: &std::path::Path) -> String {
    if let Some(url) = remote_url
        && let Ok(name) = crate::core::repo::extract_repo_name(url)
        && validate_catalog_name(&name).is_ok()
    {
        return name;
    }
    if let Some(base) = project_root.file_name().and_then(|s| s.to_str()) {
        let base = strip_git_suffix(base);
        let cleaned: String = base
            .chars()
            .filter(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.'))
            .collect();
        let cleaned = cleaned.trim_matches('.').trim_start_matches('-');
        if !cleaned.is_empty() && validate_catalog_name(cleaned).is_ok() {
            return cleaned.to_string();
        }
    }
    "repo".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn scp_https_and_ssh_forms_collapse_to_one_key() {
        let expected = "github.com/Org/Api";
        assert_eq!(normalize_url("git@github.com:Org/Api.git"), expected);
        assert_eq!(normalize_url("https://github.com/Org/Api.git"), expected);
        assert_eq!(normalize_url("https://github.com/Org/Api"), expected);
        assert_eq!(normalize_url("ssh://git@github.com/Org/Api.git"), expected);
        assert_eq!(normalize_url("ssh://git@github.com:22/Org/Api"), expected);
        assert_eq!(normalize_url("git://github.com/Org/Api.git"), expected);
    }

    #[test]
    fn host_lowercases_but_path_case_is_preserved() {
        assert_eq!(
            normalize_url("git@GitHub.COM:Org/Api.git"),
            "github.com/Org/Api"
        );
    }

    #[test]
    fn non_default_port_is_kept() {
        assert_eq!(
            normalize_url("ssh://git@example.com:2222/org/repo.git"),
            "example.com:2222/org/repo"
        );
        assert_eq!(
            normalize_url("https://example.com:8443/org/repo"),
            "example.com:8443/org/repo"
        );
    }

    #[test]
    fn local_paths_stay_path_shaped() {
        assert_eq!(normalize_url("/remotes/test-repo"), "/remotes/test-repo");
        assert_eq!(
            normalize_url("/remotes/test-repo.git/"),
            "/remotes/test-repo"
        );
        assert_eq!(
            normalize_url("file:///remotes/test-repo.git"),
            "/remotes/test-repo"
        );
        // Same fixture referenced with and without `.git` matches.
        assert_eq!(
            normalize_url("/remotes/fixture.git"),
            normalize_url("/remotes/fixture")
        );
    }

    #[test]
    fn colon_after_slash_is_a_path_not_scp() {
        assert_eq!(normalize_url("/dir/weird:name"), "/dir/weird:name");
        assert_eq!(normalize_url("./rel/weird:name"), "./rel/weird:name");
    }

    #[test]
    fn suffixed_name_walks_until_free() {
        let taken = ["api", "api-2"];
        let got = suffixed_name("api", |n| taken.contains(&n));
        assert_eq!(got, "api-3");
        assert_eq!(suffixed_name("solo", |_| false), "solo");
    }

    #[test]
    fn validate_rejects_flags_traversal_and_junk() {
        assert!(validate_catalog_name("api").is_ok());
        assert!(validate_catalog_name("api-v2.rs").is_ok());
        assert!(validate_catalog_name("").is_err());
        assert!(validate_catalog_name("-api").is_err());
        assert!(validate_catalog_name("a/b").is_err());
        assert!(validate_catalog_name("a..b").is_err());
        assert!(validate_catalog_name("has space").is_err());
    }

    #[test]
    fn derive_prefers_url_then_dirname() {
        assert_eq!(
            derive_default_name(Some("git@github.com:org/api.git"), Path::new("/w/whatever")),
            "api"
        );
        assert_eq!(
            derive_default_name(None, Path::new("/w/my-service")),
            "my-service"
        );
        assert_eq!(derive_default_name(None, Path::new("/w/repo.git")), "repo");
    }
}

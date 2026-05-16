//! Strip secret-shaped environment variables before persisting them.
//!
//! Hook jobs run with the full process environment; persisting it verbatim
//! into the store would leak whatever credentials the user happens to have
//! exported (GH_TOKEN, AWS_SECRET_ACCESS_KEY, …) into a long-lived file.
//! `scrub` filters by suffix on the variable *name* — substantive enough to
//! catch the common cases, conservative enough not to drop benign vars.
//!
//! All matching is case-insensitive. Future tightening (allowlist mode,
//! configurable patterns) can layer on top of this; the conservative suffix
//! filter is the safe default.

use std::collections::HashMap;

/// Suffix patterns matched case-insensitively against env var names.
const SECRET_SUFFIXES: &[&str] = &["_TOKEN", "_SECRET", "_KEY", "_PASSWORD"];

/// Return a copy of `env` with secret-shaped entries removed.
pub fn scrub(env: &HashMap<String, String>) -> HashMap<String, String> {
    env.iter()
        .filter(|(k, _)| !looks_like_secret(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn looks_like_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_SUFFIXES.iter().any(|s| upper.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn removes_token_secret_key_password_suffixes() {
        let input = env_of(&[
            ("GH_TOKEN", "abc"),
            ("AWS_SECRET_ACCESS_KEY", "def"),
            ("API_KEY", "ghi"),
            ("DB_PASSWORD", "jkl"),
            ("PATH", "/usr/bin"),
            ("HOME", "/home/x"),
        ]);
        let out = scrub(&input);
        assert!(!out.contains_key("GH_TOKEN"));
        assert!(!out.contains_key("AWS_SECRET_ACCESS_KEY"));
        assert!(!out.contains_key("API_KEY"));
        assert!(!out.contains_key("DB_PASSWORD"));
        assert_eq!(out.get("PATH"), Some(&"/usr/bin".to_string()));
        assert_eq!(out.get("HOME"), Some(&"/home/x".to_string()));
    }

    #[test]
    fn match_is_case_insensitive() {
        let input = env_of(&[("gh_token", "x"), ("gh_Token", "y"), ("My_Password", "z")]);
        let out = scrub(&input);
        assert!(out.is_empty(), "all three should be scrubbed, got {out:?}");
    }

    #[test]
    fn does_not_match_substrings() {
        // _TOKENISH does not end in _TOKEN; keep it.
        let input = env_of(&[("FOO_TOKENISH", "x"), ("TOKEN_OF_HONOR", "y")]);
        let out = scrub(&input);
        assert_eq!(out.len(), 2, "neither value is a suffix match, got {out:?}");
    }

    #[test]
    fn empty_env_returns_empty() {
        assert!(scrub(&HashMap::new()).is_empty());
    }
}

//! The value-address grammar: `[repo:]VAR[@worktree]`.
//!
//! A derived value's full coordinate is `(repo, worktree, var)`. The address
//! syntax spells the two optional qualifiers with distinct separators —
//! colon-prefix for the repo (precedent: `daft go pr:123`), at-suffix for the
//! worktree (precedent: ssh's `user@host`, git's `branch@{...}`) — so roles
//! are explicit and a typo can never be silently absorbed into the wrong
//! slot the way positional guessing would.
//!
//! `--repo` / `--worktree` flags remain the canonical spellings per the
//! repo-aware command grammar; the address form is sugar and is mutually
//! exclusive with them.

/// A parsed value address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address<'a> {
    /// Repo qualifier (`backend:VAR`) — `None` = current repo.
    pub repo: Option<&'a str>,
    /// The variable name.
    pub var: &'a str,
    /// Worktree qualifier (`VAR@wt`) — `None` = current worktree (or, when
    /// `repo` names another repo, the worktree matching the current slug).
    pub worktree: Option<&'a str>,
}

/// Why an address failed to parse — each variant names its own fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    EmptyVar,
    EmptyRepo,
    EmptyWorktree,
    /// More than one `:` — `a:b:VAR` has no reading.
    ExtraColon,
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressError::EmptyVar => write!(f, "the address names no variable"),
            AddressError::EmptyRepo => {
                write!(
                    f,
                    "empty repo qualifier before ':'; write `repo:VAR` or drop the colon"
                )
            }
            AddressError::EmptyWorktree => {
                write!(
                    f,
                    "empty worktree qualifier after '@'; write `VAR@worktree` or drop the '@'"
                )
            }
            AddressError::ExtraColon => {
                write!(
                    f,
                    "more than one ':'; the address grammar is `[repo:]VAR[@worktree]`"
                )
            }
        }
    }
}

/// Parse `[repo:]VAR[@worktree]`.
///
/// Both qualifiers split on the **first** occurrence of their separator: a
/// var name can legally contain neither `:` nor `@` (`[A-Z_][A-Z0-9_]*`), so
/// the first occurrence is structural — and anything after the first `@`
/// belongs to the worktree name, which may itself contain `@` (it gets
/// slugified before matching anyway).
pub fn parse_address(raw: &str) -> Result<Address<'_>, AddressError> {
    let raw = raw.trim();
    let (head, worktree) = match raw.split_once('@') {
        Some((head, wt)) => {
            if wt.is_empty() {
                return Err(AddressError::EmptyWorktree);
            }
            (head, Some(wt))
        }
        None => (raw, None),
    };
    let (repo, var) = match head.split_once(':') {
        Some((repo, rest)) => {
            if repo.is_empty() {
                return Err(AddressError::EmptyRepo);
            }
            if rest.contains(':') {
                return Err(AddressError::ExtraColon);
            }
            (Some(repo), rest)
        }
        None => (None, head),
    };
    if var.is_empty() {
        return Err(AddressError::EmptyVar);
    }
    Ok(Address {
        repo,
        var,
        worktree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_var() {
        assert_eq!(
            parse_address("WEBAPP_PORT"),
            Ok(Address {
                repo: None,
                var: "WEBAPP_PORT",
                worktree: None
            })
        );
    }

    #[test]
    fn repo_qualified() {
        assert_eq!(
            parse_address("backend:API_PORT"),
            Ok(Address {
                repo: Some("backend"),
                var: "API_PORT",
                worktree: None
            })
        );
    }

    #[test]
    fn worktree_qualified() {
        assert_eq!(
            parse_address("API_PORT@other-wt"),
            Ok(Address {
                repo: None,
                var: "API_PORT",
                worktree: Some("other-wt")
            })
        );
    }

    #[test]
    fn fully_qualified() {
        assert_eq!(
            parse_address("backend:API_PORT@feat-x"),
            Ok(Address {
                repo: Some("backend"),
                var: "API_PORT",
                worktree: Some("feat-x")
            })
        );
    }

    #[test]
    fn first_at_is_structural_rest_belongs_to_the_worktree() {
        assert_eq!(
            parse_address("API_PORT@release@2"),
            Ok(Address {
                repo: None,
                var: "API_PORT",
                worktree: Some("release@2")
            })
        );
    }

    #[test]
    fn errors_name_their_fix() {
        assert_eq!(parse_address(""), Err(AddressError::EmptyVar));
        assert_eq!(parse_address(":X"), Err(AddressError::EmptyRepo));
        assert_eq!(parse_address("X@"), Err(AddressError::EmptyWorktree));
        assert_eq!(parse_address("a:b:X"), Err(AddressError::ExtraColon));
        assert_eq!(parse_address("repo:@wt"), Err(AddressError::EmptyVar));
    }
}

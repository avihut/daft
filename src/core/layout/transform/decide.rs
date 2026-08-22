//! The flag / terminal / `-y` matrix behind `daft layout transform`'s three
//! prompts, as pure functions.
//!
//! `is_tty` is the caller's observation of stdin at the moment of the
//! decision (`false` under `DAFT_TESTING` and `--dry-run`, which must never
//! block on a human). The asymmetry between the three is the whole point:
//!
//! - `-y` answers a **yes/no** (the cross-volume copy confirmation) and takes
//!   a **default** (the sha-derived directory name for a detached main working
//!   tree). Both are things daft can reasonably decide for the user once told
//!   to.
//! - `-y` never picks a **pivot**: which worktree becomes the repository root
//!   is a choice with no safe default, so without `--pivot` it is blocked even
//!   when `-y` was passed.
//!
//! Mirrors `crate::core::worktree::merge::decide_adopt` in shape.

/// What to do about the root role of a bare repository whose choice is
/// ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PivotDecision {
    /// Nothing to decide here: not ambiguous, or `--pivot` was given (the
    /// engine validates it).
    Settled,
    /// Interactive and undecided: show the picker.
    Ask,
    /// Undecided and no way to ask: report `--pivot <branch>`.
    Blocked,
}

/// What to do about the directory name of a detached main working tree that
/// the target layout nests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirnameDecision {
    /// Not detached, or the target layout does not nest the main working tree.
    Settled,
    /// Use this name (from `--as`, or the derived default under `-y`).
    Use(String),
    /// Interactive: prompt, pre-filled with this default.
    Ask(String),
    /// No name, no way to ask: report `--as <dir>` (and `-y` for the default).
    Blocked(String),
}

/// What to do about a consequence that needs a yes (a copy across volumes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmDecision {
    /// Nothing to confirm, or `-y` was passed.
    Accept,
    /// Interactive: ask.
    Ask,
    /// Non-interactive without `-y`: report `-y`.
    Blocked,
}

/// Decide the pivot question. `ambiguous` is the engine's verdict (bare →
/// non-bare, default branch has no worktree, more than one candidate);
/// `flag` is `--pivot`.
pub fn decide_pivot(flag: Option<&str>, ambiguous: bool, is_tty: bool, yes: bool) -> PivotDecision {
    if flag.is_some() || !ambiguous {
        return PivotDecision::Settled;
    }
    // `-y` suppresses prompts; it does not answer this one.
    if is_tty && !yes {
        PivotDecision::Ask
    } else {
        PivotDecision::Blocked
    }
}

/// Decide the directory-name question. `detached_nesting` is the engine's
/// verdict (the main working tree is detached and the target layout nests
/// it); `flag` is `--as`; `derived` is the sha-derived default.
pub fn decide_dirname(
    flag: Option<&str>,
    detached_nesting: bool,
    derived: &str,
    is_tty: bool,
    yes: bool,
) -> DirnameDecision {
    if !detached_nesting {
        return DirnameDecision::Settled;
    }
    if let Some(name) = flag {
        return DirnameDecision::Use(name.to_string());
    }
    // `-y` takes the default — checked before the terminal, so a `-y` run on
    // a TTY does not prompt either.
    if yes {
        return DirnameDecision::Use(derived.to_string());
    }
    if is_tty {
        DirnameDecision::Ask(derived.to_string())
    } else {
        DirnameDecision::Blocked(derived.to_string())
    }
}

/// Decide a consequence confirmation. `needed` is whether the plan has
/// anything to confirm (a copy across volumes).
pub fn decide_copy_confirm(needed: bool, is_tty: bool, yes: bool) -> ConfirmDecision {
    if !needed || yes {
        ConfirmDecision::Accept
    } else if is_tty {
        ConfirmDecision::Ask
    } else {
        ConfirmDecision::Blocked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pivot_matrix() {
        // flag wins, on or off a terminal, with or without -y
        assert_eq!(
            decide_pivot(Some("b"), true, false, false),
            PivotDecision::Settled
        );
        assert_eq!(
            decide_pivot(Some("b"), true, true, true),
            PivotDecision::Settled
        );
        // not ambiguous: nothing to decide
        assert_eq!(
            decide_pivot(None, false, true, false),
            PivotDecision::Settled
        );
        assert_eq!(
            decide_pivot(None, false, false, false),
            PivotDecision::Settled
        );
        // ambiguous on a terminal: ask
        assert_eq!(decide_pivot(None, true, true, false), PivotDecision::Ask);
        // the discriminating row: -y must NOT pick a pivot
        assert_eq!(decide_pivot(None, true, true, true), PivotDecision::Blocked);
        // no terminal: blocked either way
        assert_eq!(
            decide_pivot(None, true, false, false),
            PivotDecision::Blocked
        );
        assert_eq!(
            decide_pivot(None, true, false, true),
            PivotDecision::Blocked
        );
    }

    #[test]
    fn dirname_matrix() {
        let d = "779c1ab3f8e2";
        assert_eq!(
            decide_dirname(Some("spike"), true, d, true, false),
            DirnameDecision::Use("spike".into())
        );
        assert_eq!(
            decide_dirname(Some("spike"), true, d, false, true),
            DirnameDecision::Use("spike".into())
        );
        // the twin of the pivot row: -y DOES take the --as default
        assert_eq!(
            decide_dirname(None, true, d, true, true),
            DirnameDecision::Use(d.into())
        );
        assert_eq!(
            decide_dirname(None, true, d, false, true),
            DirnameDecision::Use(d.into())
        );
        assert_eq!(
            decide_dirname(None, true, d, true, false),
            DirnameDecision::Ask(d.into())
        );
        assert_eq!(
            decide_dirname(None, true, d, false, false),
            DirnameDecision::Blocked(d.into())
        );
        // not detached / not nesting: settled regardless
        assert_eq!(
            decide_dirname(None, false, d, false, false),
            DirnameDecision::Settled
        );
        assert_eq!(
            decide_dirname(Some("x"), false, d, false, false),
            DirnameDecision::Settled
        );
    }

    #[test]
    fn copy_confirm_matrix() {
        assert_eq!(
            decide_copy_confirm(false, false, false),
            ConfirmDecision::Accept
        );
        assert_eq!(
            decide_copy_confirm(true, false, true),
            ConfirmDecision::Accept
        );
        assert_eq!(
            decide_copy_confirm(true, true, true),
            ConfirmDecision::Accept
        );
        assert_eq!(decide_copy_confirm(true, true, false), ConfirmDecision::Ask);
        assert_eq!(
            decide_copy_confirm(true, false, false),
            ConfirmDecision::Blocked
        );
    }
}

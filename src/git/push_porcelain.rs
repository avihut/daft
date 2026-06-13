//! Pure parser for `git push --porcelain` stdout.
//!
//! Porcelain ref-status lines have the machine-stable shape
//! `<flag>\t<from>:<to>\t<summary>`, bracketed by `To <url>` and `Done`.
//! Other content interleaves on stdout — a pre-push hook's own stdout, and
//! `--set-upstream`'s "branch '…' set up to track …" notice — so parsing keys
//! strictly on the flag + tab structure and ignores everything else.
//!
//! `--quiet` suppresses the ref-status lines entirely (only `Done` remains),
//! which would make every push look like a hook-gated abort. `run_push` never
//! passes `--quiet` for this reason; quietness is a display concern handled by
//! the capture/tee layer above.

/// Status flag of a single pushed ref, per the `git push --porcelain` format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefStatusFlag {
    /// ` ` — successfully pushed fast-forward.
    FastForward,
    /// `+` — successful forced update.
    Forced,
    /// `*` — successfully pushed new ref.
    New,
    /// `-` — successfully deleted ref.
    Deleted,
    /// `=` — ref was already up to date and did not need pushing.
    UpToDate,
    /// `!` — ref was rejected or failed to push.
    Rejected,
}

impl RefStatusFlag {
    fn from_char(c: char) -> Option<Self> {
        match c {
            ' ' => Some(Self::FastForward),
            '+' => Some(Self::Forced),
            '*' => Some(Self::New),
            '-' => Some(Self::Deleted),
            '=' => Some(Self::UpToDate),
            '!' => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// One parsed `<flag>\t<from>:<to>\t<summary>` line.
#[derive(Debug, Clone)]
pub struct RefStatus {
    pub flag: RefStatusFlag,
    /// The `<from>:<to>` refspec as printed (deletes have an empty `<from>`).
    pub refspec: String,
    /// Human summary: `[up to date]`, `[new branch]`, `<old>..<new>`, …
    pub summary: String,
}

/// Parsed report of a `git push --porcelain` run.
#[derive(Debug, Clone, Default)]
pub struct PushReport {
    pub refs: Vec<RefStatus>,
}

impl PushReport {
    /// Every pushed ref was already up to date (and at least one ref line was
    /// seen). Replaces the locale-fragile `"Everything up-to-date"` stderr grep.
    pub fn all_up_to_date(&self) -> bool {
        !self.refs.is_empty() && self.refs.iter().all(|r| r.flag == RefStatusFlag::UpToDate)
    }

    /// Whether any ref-status line was emitted at all. A failed push with NO
    /// ref lines never reached ref negotiation — the signature of a local
    /// gate (pre-push hook) refusal or a transport failure.
    pub fn has_ref_lines(&self) -> bool {
        !self.refs.is_empty()
    }
}

/// Parse the stdout of a `git push --porcelain` invocation.
pub fn parse_push_report(stdout: &str) -> PushReport {
    let refs = stdout
        .lines()
        .filter_map(|line| {
            let mut chars = line.chars();
            let flag = RefStatusFlag::from_char(chars.next()?)?;
            let rest = chars.as_str().strip_prefix('\t')?;
            let (refspec, summary) = rest.split_once('\t')?;
            // Real refspecs are `<from>:<to>`; deletes print `:refs/...`.
            // The colon requirement filters stray tab-bearing hook output.
            if !refspec.contains(':') {
                return None;
            }
            Some(RefStatus {
                flag,
                refspec: refspec.to_string(),
                summary: summary.to_string(),
            })
        })
        .collect();

    PushReport { refs }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures captured from git 2.x in scratch repos (issue #599, step E0).

    #[test]
    fn fast_forward_update() {
        let out = "To /tmp/remote.git\n \trefs/heads/feature:refs/heads/feature\ta59a2e2..f33e772\nDone\n";
        let report = parse_push_report(out);
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].flag, RefStatusFlag::FastForward);
        assert_eq!(
            report.refs[0].refspec,
            "refs/heads/feature:refs/heads/feature"
        );
        assert!(!report.all_up_to_date());
        assert!(report.has_ref_lines());
    }

    #[test]
    fn up_to_date() {
        let out =
            "To /tmp/remote.git\n=\trefs/heads/feature:refs/heads/feature\t[up to date]\nDone\n";
        let report = parse_push_report(out);
        assert!(report.all_up_to_date());
    }

    #[test]
    fn new_branch_with_set_upstream_notice() {
        // --set-upstream interleaves a non-porcelain notice on stdout.
        let out = "To /tmp/remote.git\n*\trefs/heads/feature:refs/heads/feature\t[new branch]\nbranch 'feature' set up to track 'origin/feature'.\nDone\n";
        let report = parse_push_report(out);
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].flag, RefStatusFlag::New);
        assert!(!report.all_up_to_date());
    }

    #[test]
    fn delete_has_empty_from() {
        let out = "To /tmp/remote.git\n-\t:refs/heads/feature\t[deleted]\nDone\n";
        let report = parse_push_report(out);
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].flag, RefStatusFlag::Deleted);
        assert_eq!(report.refs[0].refspec, ":refs/heads/feature");
    }

    #[test]
    fn forced_update_with_hook_stdout_noise() {
        // A passing pre-push hook's stdout interleaves before the report.
        let out = "hook stdout line\nTo /tmp/remote.git\n+\trefs/heads/feature:refs/heads/feature\t82ec994...851c641 (forced update)\nDone\n";
        let report = parse_push_report(out);
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].flag, RefStatusFlag::Forced);
    }

    #[test]
    fn rejected_non_fast_forward() {
        let out = "To /tmp/remote.git\n!\trefs/heads/feature:refs/heads/feature\t[rejected] (non-fast-forward)\nDone\n";
        let report = parse_push_report(out);
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].flag, RefStatusFlag::Rejected);
        assert!(!report.all_up_to_date());
    }

    #[test]
    fn hook_rejection_yields_no_ref_lines() {
        // A failing pre-push hook aborts before ref negotiation: stdout holds
        // only the hook's own noise — no To/ref/Done lines at all.
        let report = parse_push_report("HOOK STDOUT NOISE\n");
        assert!(!report.has_ref_lines());
        assert!(!report.all_up_to_date());
    }

    #[test]
    fn empty_output() {
        let report = parse_push_report("");
        assert!(!report.has_ref_lines());
        assert!(!report.all_up_to_date());
    }

    #[test]
    fn hook_noise_with_tabs_is_not_a_ref_line() {
        // Tab-bearing hook output without the flag+refspec shape is ignored.
        let out = "x\tnot-a-refspec\twhatever\n \tno-colon-here\tsummary\n";
        let report = parse_push_report(out);
        assert!(!report.has_ref_lines());
    }
}

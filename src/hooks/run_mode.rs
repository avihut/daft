//! `--hooks <mode>`: how a command's hook phase executes for one run.
//!
//! The flag answers *how the phase runs*; `--skip-hooks` answers *which jobs
//! run*. Keeping those axes separate is deliberate — `--skip-hooks` selects
//! over an open, repo-specific vocabulary (hook names, `tag:<tag>`, and every
//! job name in the repo's `daft.yml`), which no closed enum can hold. The one
//! place they meet is [`HookMode::Off`], which lowers to the `all` selector
//! rather than introducing a second way to disable hooks.

use clap::ValueEnum;

/// How this run's hook jobs execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum HookMode {
    /// Honor each job's own `background:` declaration — the default.
    #[default]
    Auto,
    /// Run every job inline and wait for the whole phase to finish.
    ///
    /// A promoted job's failure then counts against the hook outcome, which
    /// for `worktree-post-create` aborts the command (#765). Promoted jobs
    /// keep the standard job timeout: it already applies on the detached
    /// path, so a job that dies at five minutes must not start succeeding
    /// just because someone is watching it.
    Foreground,
    /// Dispatch every job to the coordinator and return without waiting.
    ///
    /// This is the ordinary `background: true` bargain applied to the whole
    /// phase: the command reports success once the worktree is usable, and
    /// the work is followed through `daft hooks jobs`. It applies to the
    /// post-* phases only — a gate phase
    /// ([`HookType::gates_the_operation_after_it`]) keeps running inline,
    /// because detaching it would not delay the gate but remove it.
    ///
    /// [`HookType::gates_the_operation_after_it`]: crate::hooks::HookType::gates_the_operation_after_it
    Background,
    /// Skip the hook phase entirely — equivalent to `--skip-hooks all`.
    Off,
}

impl HookMode {
    /// Whether background jobs are promoted to inline execution.
    pub fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }

    /// Whether every job in `hook_type`'s phase should be detached.
    ///
    /// Takes the hook type rather than answering in the abstract: the mode
    /// deliberately does nothing to a gate phase, so the caller cannot make
    /// this decision correctly without it.
    pub fn detaches_all_jobs(self, hook_type: crate::hooks::HookType) -> bool {
        self == Self::Background && !hook_type.gates_the_operation_after_it()
    }

    /// The job filter this mode implies, folded together with any explicit
    /// `--skip-hooks` selectors.
    ///
    /// `Off` appends the `all` selector rather than short-circuiting
    /// elsewhere, so every "don't run hooks" path — this flag, an explicit
    /// `--skip-hooks all`, and the untrusted-repo fallback — converges on one
    /// mechanism and renders the same attributed skips.
    pub fn job_filter(self, skip_hooks: &[String]) -> crate::hooks::yaml_executor::JobFilter {
        if self == Self::Off {
            let mut selectors = skip_hooks.to_vec();
            selectors.push("all".to_string());
            return crate::hooks::yaml_executor::JobFilter::skipping(&selectors);
        }
        crate::hooks::yaml_executor::JobFilter::skipping(skip_hooks)
    }

    /// How this run asked for "no hooks", spelled the way the user typed it.
    /// Microcopy that names a flag the user did not pass reads as a bug
    /// report about someone else's invocation.
    pub fn skip_all_label(self) -> &'static str {
        if self == Self::Off {
            "--hooks off"
        } else {
            "--skip-hooks all"
        }
    }

    /// The value set, for the settings registry's `variants()` convention and
    /// for shell-completion generation. Declared next to `parse`-adjacent
    /// code so a new variant cannot be spelled out a second time elsewhere.
    pub fn variants() -> &'static [&'static str] {
        &["auto", "foreground", "background", "off"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_the_default_and_promotes_nothing() {
        assert_eq!(HookMode::default(), HookMode::Auto);
        assert!(!HookMode::Auto.is_foreground());
        assert!(HookMode::Foreground.is_foreground());
        assert!(!HookMode::Off.is_foreground());
    }

    #[test]
    fn auto_passes_skip_selectors_through_untouched() {
        let filter = HookMode::Auto.job_filter(&["clippy".to_string()]);
        assert!(!filter.skip.all, "auto must not skip everything");
        assert!(filter.skip.names.contains(&"clippy".to_string()));
    }

    /// `off` is sugar for `--skip-hooks all`, not a parallel mechanism.
    #[test]
    fn off_lowers_to_the_all_selector() {
        let filter = HookMode::Off.job_filter(&[]);
        assert!(filter.skip.all);
    }

    /// Composing the two flags keeps both contributions: `off` still skips
    /// everything, and an explicit selector is not dropped on the way.
    #[test]
    fn off_preserves_explicit_selectors_alongside_all() {
        let filter = HookMode::Off.job_filter(&["tag:slow".to_string()]);
        assert!(filter.skip.all);
        assert!(filter.skip.tags.contains(&"slow".to_string()));
    }

    /// `background` detaches an ordinary post-* phase — the whole point.
    #[test]
    fn background_detaches_the_post_phases() {
        for hook in [
            crate::hooks::HookType::PostCreate,
            crate::hooks::HookType::PostClone,
            crate::hooks::HookType::PostMerge,
            crate::hooks::HookType::PostRemove,
        ] {
            assert!(
                HookMode::Background.detaches_all_jobs(hook),
                "{hook:?} runs after the operation it accompanies, so it detaches"
            );
        }
    }

    /// ...and never a gate. Detaching one of these does not defer the gate,
    /// it removes it: `pre-create` is awaited before `git worktree add`,
    /// `pre-merge` before the merge lands, and `pre-remove` runs inside the
    /// directory that is about to be deleted.
    #[test]
    fn background_leaves_gate_phases_inline() {
        for hook in [
            crate::hooks::HookType::PreCreate,
            crate::hooks::HookType::PreRemove,
            crate::hooks::HookType::PreMerge,
        ] {
            assert!(
                !HookMode::Background.detaches_all_jobs(hook),
                "{hook:?} gates the operation that follows it and must stay inline"
            );
        }
    }

    /// No other mode detaches anything, gate or not — `auto` honors the
    /// declarations and `foreground` is the opposite request.
    #[test]
    fn only_background_detaches() {
        for mode in [HookMode::Auto, HookMode::Foreground, HookMode::Off] {
            assert!(!mode.detaches_all_jobs(crate::hooks::HookType::PostCreate));
        }
    }

    /// The completion/registry list must not drift from the enum. Adding a
    /// variant without listing it here fails right where the omission is.
    #[test]
    fn variants_match_the_enum() {
        let declared: Vec<String> = HookMode::value_variants()
            .iter()
            .map(|v| {
                v.to_possible_value()
                    .expect("every HookMode variant is selectable")
                    .get_name()
                    .to_string()
            })
            .collect();
        assert_eq!(declared, HookMode::variants());
    }
}

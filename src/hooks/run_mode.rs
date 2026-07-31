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
    /// the work is followed through `daft hooks jobs`.
    ///
    /// It applies to a phase only when detaching it changes *when* the work
    /// happens and nothing else — daft must be finished with both the work
    /// and the directory when the hook returns, and the phase must not
    /// declare an ordering the coordinator cannot express. See
    /// [`detaches_all_jobs`](Self::detaches_all_jobs) for the two guards and
    /// why each one is not negotiable. A phase that fails either keeps
    /// running inline and says so.
    Background,
    /// Skip the hook phase entirely — equivalent to `--skip-hooks all`.
    Off,
}

impl HookMode {
    /// Whether background jobs are promoted to inline execution.
    pub fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }

    /// Whether every job in this phase should be detached.
    ///
    /// Takes the phase's context rather than answering in the abstract,
    /// because the mode deliberately declines in two situations the caller
    /// cannot evaluate for it:
    ///
    /// - `hook_type` still has daft work after it
    ///   ([`HookType::precedes_more_daft_work`]) — detaching would race or
    ///   delete that work rather than defer the hook.
    /// - `exec_mode` is anything but
    ///   [`Parallel`](crate::executor::ExecutionMode::Parallel). The
    ///   coordinator's scheduler is built from `(name, needs)` pairs alone
    ///   and has no representation for a phase-level ordering, so detaching a
    ///   `parallel: false`, `piped:` or `follow:` hook would silently
    ///   reinterpret the config as "run these concurrently" — and for
    ///   `piped:` would drop the stdout→stdin wiring while still reporting
    ///   success. Synthesizing `needs:` edges is not a fix either: a
    ///   dependency that did not succeed *skips* its dependents, which turns
    ///   `parallel: false`'s continue-on-failure into stop-on-failure.
    ///
    /// [`HookType::precedes_more_daft_work`]: crate::hooks::HookType::precedes_more_daft_work
    pub fn detaches_all_jobs(
        self,
        hook_type: crate::hooks::HookType,
        exec_mode: crate::executor::ExecutionMode,
    ) -> bool {
        self == Self::Background
            && !hook_type.precedes_more_daft_work()
            && exec_mode == crate::executor::ExecutionMode::Parallel
    }

    /// Why `background` declined to detach this phase, as user-facing
    /// microcopy — or `None` when it did detach (or was never asked to).
    ///
    /// Silently doing nothing is the failure mode this exists to prevent: a
    /// user who passes `--hooks background` and waits through the whole phase
    /// has no way to tell a config with no background-eligible work from a
    /// flag that quietly did not apply.
    pub fn detach_declined_reason(
        self,
        hook_type: crate::hooks::HookType,
        exec_mode: crate::executor::ExecutionMode,
    ) -> Option<String> {
        if self != Self::Background || self.detaches_all_jobs(hook_type, exec_mode) {
            return None;
        }
        if hook_type.precedes_more_daft_work() {
            return Some(format!(
                "--hooks background does not apply to {hook_type}: daft acts on its result \
                 as soon as it returns. Running it inline."
            ));
        }
        Some(format!(
            "--hooks background does not apply to {hook_type}: it declares an execution \
             order ({}) that background jobs cannot preserve. Running it inline.",
            match exec_mode {
                crate::executor::ExecutionMode::Sequential => "parallel: false / follow:",
                crate::executor::ExecutionMode::Piped => "piped:",
                crate::executor::ExecutionMode::Parallel => "parallel",
            }
        ))
    }

    /// Lower `off` into the `all` skip selector, in place.
    ///
    /// `off` is sugar for `--skip-hooks all`, and several commands gate on
    /// the raw selector list *before* any executor exists — clone's
    /// `--trust-hooks` conflict and `--install` trust implication, adopt's
    /// post-adopt short-circuit, checkout's untrusted-repo notice. Lowering
    /// at entry is what lets every one of them keep reading the single
    /// mechanism they already read. Idempotent, so a command that also
    /// received an explicit `--skip-hooks all` does not grow a duplicate.
    pub fn lower_off_into(self, skip_hooks: &mut Vec<String>) {
        if self == Self::Off && !skip_hooks.iter().any(|s| s == "all") {
            skip_hooks.push("all".to_string());
        }
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

    /// One-line gloss per value, for completion engines that show a
    /// description beside each suggestion (Fig/Amazon Q). Lives beside
    /// [`variants`](Self::variants) so the two cannot drift apart.
    pub fn describe(variant: &str) -> &'static str {
        match variant {
            "auto" => "Honor each job's own `background:` declaration",
            "foreground" => "Run every job inline and wait for the phase",
            "background" => "Dispatch every job and return without waiting",
            "off" => "Skip the hook phase (same as --skip-hooks all)",
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The execution mode a hook gets when it declares no ordering — the
    /// only one `background` detaches under.
    const PARALLEL: crate::executor::ExecutionMode = crate::executor::ExecutionMode::Parallel;

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

    /// `background` detaches a phase daft is finished with — the whole point.
    #[test]
    fn background_detaches_the_phases_daft_is_done_with() {
        for hook in [
            crate::hooks::HookType::PostCreate,
            crate::hooks::HookType::PostMerge,
            crate::hooks::HookType::PostRemove,
        ] {
            assert!(
                HookMode::Background.detaches_all_jobs(hook, PARALLEL),
                "{hook:?} runs after the operation it accompanies, so it detaches"
            );
        }
    }

    /// ...and never a phase daft still acts on. Detaching one of these does
    /// not defer it, it breaks it: `pre-create` is awaited before
    /// `git worktree add`, `pre-merge` before the merge lands, `pre-remove`
    /// runs inside the directory about to be deleted, and `post-clone` is
    /// followed by the per-branch `worktree-post-create` phase that consumes
    /// its repo-root setup.
    #[test]
    fn background_leaves_phases_with_follow_on_work_inline() {
        for hook in [
            crate::hooks::HookType::PreCreate,
            crate::hooks::HookType::PreRemove,
            crate::hooks::HookType::PreMerge,
            crate::hooks::HookType::PostClone,
        ] {
            assert!(
                !HookMode::Background.detaches_all_jobs(hook, PARALLEL),
                "{hook:?} still has daft work after it and must stay inline"
            );
        }
    }

    /// A phase that declares an execution order keeps it. The coordinator
    /// schedules `needs:` edges and nothing else, so detaching a
    /// `parallel: false` or `piped:` hook would silently reinterpret it as
    /// "run these concurrently".
    #[test]
    fn background_leaves_ordered_phases_inline() {
        for exec_mode in [
            crate::executor::ExecutionMode::Sequential,
            crate::executor::ExecutionMode::Piped,
        ] {
            assert!(
                !HookMode::Background
                    .detaches_all_jobs(crate::hooks::HookType::PostCreate, exec_mode),
                "{exec_mode:?} declares an order background jobs cannot preserve"
            );
        }
    }

    /// Declining is announced. A mode that silently did nothing would be
    /// indistinguishable from a config with no background-eligible work, and
    /// the reason names which guard fired.
    #[test]
    fn declining_to_detach_explains_which_guard_fired() {
        let ordered = HookMode::Background
            .detach_declined_reason(
                crate::hooks::HookType::PostCreate,
                crate::executor::ExecutionMode::Piped,
            )
            .expect("a piped phase declines");
        assert!(ordered.contains("piped:"), "{ordered}");

        let follow_on = HookMode::Background
            .detach_declined_reason(crate::hooks::HookType::PreCreate, PARALLEL)
            .expect("a phase with follow-on work declines");
        assert!(follow_on.contains("as soon as it returns"), "{follow_on}");

        assert!(
            HookMode::Background
                .detach_declined_reason(crate::hooks::HookType::PostCreate, PARALLEL)
                .is_none(),
            "a phase that does detach has nothing to explain"
        );
        for mode in [HookMode::Auto, HookMode::Foreground, HookMode::Off] {
            assert!(
                mode.detach_declined_reason(crate::hooks::HookType::PreCreate, PARALLEL)
                    .is_none(),
                "{mode:?} never asked to detach, so it is not declined"
            );
        }
    }

    /// No other mode detaches anything — `auto` honors the declarations and
    /// `foreground` is the opposite request.
    #[test]
    fn only_background_detaches() {
        for mode in [HookMode::Auto, HookMode::Foreground, HookMode::Off] {
            assert!(!mode.detaches_all_jobs(crate::hooks::HookType::PostCreate, PARALLEL));
        }
    }

    /// `off` lowering is idempotent and leaves the other modes alone — the
    /// three commands that call it do so at funnels a single run can reach
    /// more than once.
    #[test]
    fn lowering_off_is_idempotent_and_mode_specific() {
        let mut selectors = vec!["clippy".to_string()];
        HookMode::Off.lower_off_into(&mut selectors);
        HookMode::Off.lower_off_into(&mut selectors);
        assert_eq!(selectors, vec!["clippy".to_string(), "all".to_string()]);

        for mode in [HookMode::Auto, HookMode::Foreground, HookMode::Background] {
            let mut untouched = vec!["clippy".to_string()];
            mode.lower_off_into(&mut untouched);
            assert_eq!(untouched, vec!["clippy".to_string()], "{mode:?}");
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

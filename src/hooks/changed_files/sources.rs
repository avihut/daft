//! The file lists a hook fire can offer, and which one a job gets by default.
//!
//! A lifecycle hook has at most one notion of "the files this operation
//! touched", so a single provider sufficed. A git stage has several that are
//! all meaningful at once — what is staged, what a push would send, the whole
//! tree — and a job says which it means by the placeholder it writes.
//!
//! Every source is constructed up front and resolved lazily, so a hook with
//! four sources declared and one used still runs one `git` subprocess.

use super::ChangedFilesProvider;
use crate::hooks::HookType;
use crate::hooks::environment::HookContext;
use crate::hooks::git_stage::GitStage;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A named file list a job can ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SourceKind {
    /// What the hook's own operation touched: the merge range for merge
    /// hooks, or the hook-level `files:` command when one is declared.
    Operation,
    /// What is staged for the commit in progress.
    Staged,
    /// What a push would send.
    Pushed,
    /// Every tracked file.
    AllTracked,
}

impl SourceKind {
    /// The template placeholder that names this source explicitly.
    pub fn placeholder(self) -> &'static str {
        match self {
            SourceKind::Operation => "{files}",
            SourceKind::Staged => "{staged_files}",
            SourceKind::Pushed => "{push_files}",
            SourceKind::AllTracked => "{all_files}",
        }
    }

    /// How a job that referenced this source reads in an error message.
    pub fn describe(self) -> &'static str {
        match self {
            SourceKind::Operation => "the hook's own file list",
            SourceKind::Staged => "the staged files",
            SourceKind::Pushed => "the files being pushed",
            SourceKind::AllTracked => "every tracked file",
        }
    }

    /// Every kind, for placeholder scanning.
    pub fn all() -> &'static [SourceKind] {
        &[
            SourceKind::Operation,
            SourceKind::Staged,
            SourceKind::Pushed,
            SourceKind::AllTracked,
        ]
    }
}

/// The file sources available to one hook fire.
#[derive(Debug, Default)]
pub struct FileSources {
    providers: BTreeMap<SourceKind, ChangedFilesProvider>,
    /// Which kind a bare `{files}` / `{changed_files}` / `glob:` resolves to.
    /// `None` when the hook has no natural file list, which makes a
    /// file-aware job there a loud configuration error rather than a silent
    /// empty run.
    default: Option<SourceKind>,
}

impl FileSources {
    /// Build the sources for a hook fire.
    ///
    /// `hook_files` is the hook-level `files:` command, which — when present —
    /// *replaces* whatever the hook would otherwise have offered as its
    /// operation source. Declaring it is an explicit statement about what the
    /// hook is gating, and silently unioning it with git's answer would make
    /// the declaration mean less than it says.
    pub fn for_hook(
        ctx: &HookContext,
        worktree: &Path,
        hook_files: Option<&str>,
        index_file: Option<PathBuf>,
    ) -> Self {
        let mut providers = BTreeMap::new();

        if let Some(command) = hook_files {
            providers.insert(
                SourceKind::Operation,
                ChangedFilesProvider::from_command(command, worktree),
            );
        } else if let Some(p) = ChangedFilesProvider::for_hook(ctx, worktree) {
            providers.insert(SourceKind::Operation, p);
        }

        // The git-backed sources exist for every git stage, whether or not
        // the stage's default names them: a `post-checkout` job may perfectly
        // well want `{staged_files}`, and answering "no such source" for a
        // question git can answer would be an invented limitation.
        if let HookType::Git(stage) = ctx.hook_type {
            providers.insert(
                SourceKind::Staged,
                ChangedFilesProvider::staged(worktree, index_file),
            );
            providers.insert(
                SourceKind::AllTracked,
                ChangedFilesProvider::all_tracked(worktree),
            );
            if stage == GitStage::PrePush
                && let Some(p) = push_provider(ctx, worktree)
            {
                providers.insert(SourceKind::Pushed, p);
            }
        }

        let default = default_kind(ctx, hook_files.is_some(), &providers);
        Self { providers, default }
    }

    /// A sources set over one already-known list, used as the operation
    /// source. The test seam, and how `daft hooks run` supplies a list it
    /// resolved itself.
    pub fn preresolved(files: Vec<String>) -> Self {
        Self {
            providers: [(
                SourceKind::Operation,
                ChangedFilesProvider::preresolved(files),
            )]
            .into_iter()
            .collect(),
            default: Some(SourceKind::Operation),
        }
    }

    /// The provider for an explicitly named source.
    pub fn get(&self, kind: SourceKind) -> Option<&ChangedFilesProvider> {
        self.providers.get(&kind)
    }

    /// Which source a bare file reference resolves to.
    pub fn default_kind(&self) -> Option<SourceKind> {
        self.default
    }

    /// The provider a bare file reference resolves to.
    pub fn default_provider(&self) -> Option<&ChangedFilesProvider> {
        self.default.and_then(|k| self.get(k))
    }

    /// Whether any source is available at all.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// The source a bare `{files}` / `glob:` means, per hook.
///
/// A hook-level `files:` always wins — it is the most specific statement
/// available. Otherwise each git stage names the list it is *about*: the
/// commit family is about what is staged, `pre-push` about what is being
/// pushed. Stages with no natural answer get `None` rather than a guess;
/// `{all_files}` there is a choice a job makes explicitly, because "lint
/// everything on every checkout" must not be something a config falls into.
fn default_kind(
    ctx: &HookContext,
    has_hook_files: bool,
    providers: &BTreeMap<SourceKind, ChangedFilesProvider>,
) -> Option<SourceKind> {
    if has_hook_files {
        return Some(SourceKind::Operation);
    }
    let HookType::Git(stage) = ctx.hook_type else {
        // Lifecycle hooks keep their existing behaviour exactly: the merge
        // range when there is one, nothing otherwise.
        return providers
            .contains_key(&SourceKind::Operation)
            .then_some(SourceKind::Operation);
    };
    match stage {
        GitStage::PreCommit
        | GitStage::PreMergeCommit
        | GitStage::PrepareCommitMsg
        | GitStage::CommitMsg
        | GitStage::ApplypatchMsg
        | GitStage::PreApplypatch => Some(SourceKind::Staged),
        GitStage::PrePush => providers
            .contains_key(&SourceKind::Pushed)
            .then_some(SourceKind::Pushed),
        _ => None,
    }
}

/// The `pre-push` file source, read back from the refs the dispatcher parsed
/// out of git's stdin. Returns `None` when the push carries nothing with
/// content (a delete-only push), which makes every file-aware job skip —
/// correct, since a delete has no files to check.
fn push_provider(ctx: &HookContext, worktree: &Path) -> Option<ChangedFilesProvider> {
    let refs = crate::hooks::git_stage::parse_push_refs(ctx.stage_stdin.as_deref()?);
    let remote = ctx.stage_argv.first().map_or("origin", String::as_str);
    ChangedFilesProvider::push_range(worktree, remote, &refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(hook_type: HookType) -> HookContext {
        HookContext::new(
            hook_type, "__hook", "/p", "/p/.git", "origin", "/p/feat", "/p/feat", "feat",
        )
    }

    #[test]
    fn placeholders_are_distinct_and_stable() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in SourceKind::all() {
            assert!(seen.insert(kind.placeholder()), "{kind:?} duplicates");
        }
        assert_eq!(SourceKind::Staged.placeholder(), "{staged_files}");
        assert_eq!(SourceKind::Pushed.placeholder(), "{push_files}");
        assert_eq!(SourceKind::AllTracked.placeholder(), "{all_files}");
        assert_eq!(SourceKind::Operation.placeholder(), "{files}");
    }

    #[test]
    fn the_commit_family_defaults_to_staged() {
        for stage in [
            GitStage::PreCommit,
            GitStage::PreMergeCommit,
            GitStage::CommitMsg,
            GitStage::PrepareCommitMsg,
            GitStage::ApplypatchMsg,
            GitStage::PreApplypatch,
        ] {
            let sources =
                FileSources::for_hook(&ctx(HookType::Git(stage)), Path::new("/p/feat"), None, None);
            assert_eq!(sources.default_kind(), Some(SourceKind::Staged), "{stage}");
        }
    }

    #[test]
    fn a_stage_with_no_natural_list_defaults_to_nothing() {
        // Not `{all_files}`: "lint the entire tree on every checkout" is a
        // choice, not something a config should fall into by omission.
        for stage in [
            GitStage::PostCheckout,
            GitStage::PostCommit,
            GitStage::PostMerge,
            GitStage::PreRebase,
        ] {
            let sources =
                FileSources::for_hook(&ctx(HookType::Git(stage)), Path::new("/p/feat"), None, None);
            assert_eq!(sources.default_kind(), None, "{stage}");
            // …but the named sources are still reachable on request.
            assert!(sources.get(SourceKind::Staged).is_some(), "{stage}");
            assert!(sources.get(SourceKind::AllTracked).is_some(), "{stage}");
        }
    }

    #[test]
    fn a_hook_level_files_command_replaces_the_operation_source() {
        let sources = FileSources::for_hook(
            &ctx(HookType::Git(GitStage::PreCommit)),
            Path::new("/p/feat"),
            Some("echo a.rs"),
            None,
        );
        // The explicit declaration wins over the stage default — it is the
        // most specific statement about what this hook gates.
        assert_eq!(sources.default_kind(), Some(SourceKind::Operation));
        assert!(sources.get(SourceKind::Operation).is_some());
        // The git-backed sources remain addressable by name.
        assert!(sources.get(SourceKind::Staged).is_some());
    }

    #[test]
    fn lifecycle_hooks_keep_their_single_source() {
        let sources =
            FileSources::for_hook(&ctx(HookType::PostCreate), Path::new("/p/feat"), None, None);
        assert_eq!(sources.default_kind(), None);
        assert!(sources.is_empty());
        // No stage means no git-backed sources: a lifecycle hook asking for
        // `{staged_files}` is a config error, not an empty list.
        assert!(sources.get(SourceKind::Staged).is_none());
    }

    #[test]
    fn a_delete_only_push_offers_no_pushed_source() {
        let ctx = ctx(HookType::Git(GitStage::PrePush)).with_stage_payload(
            vec!["origin".into(), "git@example.com:x/y.git".into()],
            Some("(delete) 0000000000000000000000000000000000000000 refs/heads/gone abc123"),
        );
        let sources = FileSources::for_hook(&ctx, Path::new("/p/feat"), None, None);
        // Nothing with content is being sent, so file-aware jobs skip rather
        // than run against an empty list they cannot distinguish from an
        // error.
        assert!(sources.get(SourceKind::Pushed).is_none());
        assert_eq!(sources.default_kind(), None);
    }

    #[test]
    fn a_push_with_content_offers_the_pushed_source_as_default() {
        let ctx = ctx(HookType::Git(GitStage::PrePush)).with_stage_payload(
            vec!["origin".into(), "git@example.com:x/y.git".into()],
            Some("refs/heads/f aaa111 refs/heads/f bbb222"),
        );
        let sources = FileSources::for_hook(&ctx, Path::new("/p/feat"), None, None);
        assert_eq!(sources.default_kind(), Some(SourceKind::Pushed));
    }

    #[test]
    fn preresolved_is_the_operation_source() {
        let sources = FileSources::preresolved(vec!["a.rs".into()]);
        assert_eq!(sources.default_kind(), Some(SourceKind::Operation));
        assert_eq!(
            sources.default_provider().unwrap().files().unwrap(),
            &["a.rs".to_string()][..]
        );
    }
}

//! The scripts daft writes into the hooks directory so git will call it.
//!
//! A shim is deliberately the smallest thing that can work: git needs an
//! executable file at a fixed name, and everything else — which stages are
//! configured, what they run, whether the repository is trusted — is decided
//! by daft when it is invoked. Nothing about the config is baked in, so
//! editing `daft.yml` never requires reinstalling.

use super::GitStage;
use std::path::Path;

/// Marker on line 2 of every shim daft writes.
///
/// Ownership is proven by content, not by presence: `uninstall` removes only
/// files carrying this line, and `install` refuses to overwrite a file that
/// does not. A hook daft did not write is somebody's work, and the version
/// number is what lets a future format change be recognised rather than
/// mistaken for a foreign script.
const MARKER_PREFIX: &str = "# daft:shim:v1 stage=";

/// Render the shim for `stage`, pointing at `binary`.
///
/// Three properties the script must have, in order of how badly they fail:
///
/// 1. **A missing binary must not brick the repository.** If daft has been
///    uninstalled or moved off `PATH`, the shim warns and exits 0. The
///    alternative — a `pre-commit` that cannot run and therefore refuses
///    every commit — turns "I removed a tool" into "I cannot commit", and the
///    fix is not discoverable from the error.
/// 2. **The guard is checked here, not only in daft.** A shim that re-enters
///    its own stage exits before spawning anything, which is both faster and
///    the only place the recursion can be cut without a process.
/// 3. **`exec` rather than a call.** The stage's exit code, its stdin, and any
///    signal it receives are git's business; replacing the process means none
///    of them are mediated.
pub fn render(stage: GitStage, binary: &Path) -> String {
    let name = stage.git_hook_filename();
    format!(
        r#"#!/bin/sh
{MARKER_PREFIX}{name}
# Installed by daft. Edit daft.yml, not this file — reinstalling rewrites it.
# Remove with: daft hooks uninstall

[ "${{DAFT_STAGE_GUARD:-}}" = "{name}" ] && exit 0

DAFT="{binary}"
[ -x "$DAFT" ] || DAFT="$(command -v daft 2>/dev/null)"
if [ -z "$DAFT" ]; then
  echo "daft: not found; skipping the {name} hook" >&2
  exit 0
fi

exec "$DAFT" __hook {name} "$@"
"#,
        binary = binary.display(),
    )
}

/// The stage a shim declares, or `None` for a script daft did not write.
///
/// Reads the marker rather than pattern-matching the body, so a shim someone
/// edited by hand is still recognised as daft's — and so a foreign hook that
/// happens to mention daft is not.
pub fn declared_stage(contents: &str) -> Option<GitStage> {
    contents
        .lines()
        .take(5)
        .find_map(|line| line.trim().strip_prefix(MARKER_PREFIX))
        .and_then(|name| GitStage::from_git_filename(name.trim()))
}

/// Whether `contents` is a shim daft wrote.
pub fn is_daft_shim(contents: &str) -> bool {
    declared_stage(contents).is_some()
}

/// Whether `contents` looks like a hook another manager installed.
///
/// Used only to decide whether backing the file up is safe and expected
/// versus something to report and stop for. A recognised manager's shim is
/// regenerable from its own config, so moving it aside loses nothing; an
/// unrecognised script might be the only copy of somebody's work.
///
/// git-lfs is in this list for a different reason than the managers: daft
/// chains `git lfs <stage>` itself, so its hook file is genuinely superseded
/// rather than merely displaced.
pub fn recognized_foreign_manager(contents: &str) -> Option<&'static str> {
    const SIGNATURES: &[(&str, &str)] = &[
        ("lefthook", "lefthook"),
        ("husky.sh", "husky"),
        ("_/husky.sh", "husky"),
        ("pre-commit.com", "pre-commit"),
        ("INSTALL_PYTHON", "pre-commit"),
        ("git lfs", "git-lfs"),
        ("git-lfs", "git-lfs"),
        ("overcommit", "overcommit"),
    ];
    let head: String = contents.lines().take(40).collect::<Vec<_>>().join("\n");
    SIGNATURES
        .iter()
        .find(|(needle, _)| head.contains(needle))
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn shim(stage: GitStage) -> String {
        render(stage, &PathBuf::from("/usr/local/bin/daft"))
    }

    #[test]
    fn a_shim_declares_its_own_stage() {
        for &stage in GitStage::all() {
            let rendered = shim(stage);
            assert_eq!(declared_stage(&rendered), Some(stage), "{stage}");
            assert!(is_daft_shim(&rendered), "{stage}");
        }
    }

    #[test]
    fn the_marker_is_near_the_top_so_recognition_is_cheap() {
        // `declared_stage` reads five lines; the shebang plus the marker must
        // fit inside that even if the header grows a little.
        let rendered = shim(GitStage::PreCommit);
        let marker_line = rendered
            .lines()
            .position(|l| l.contains(MARKER_PREFIX))
            .unwrap();
        assert!(marker_line < 5, "marker on line {marker_line}");
    }

    #[test]
    fn rendering_is_deterministic_so_reinstall_can_compare_content() {
        assert_eq!(shim(GitStage::PrePush), shim(GitStage::PrePush));
    }

    #[test]
    fn the_shim_execs_daft_with_its_stage_and_arguments() {
        let rendered = shim(GitStage::CommitMsg);
        assert!(
            rendered.contains(r#"exec "$DAFT" __hook commit-msg "$@""#),
            "{rendered}"
        );
    }

    #[test]
    fn git_post_merge_uses_gits_filename_not_the_yaml_key() {
        let rendered = shim(GitStage::PostMerge);
        assert!(rendered.contains("__hook post-merge "), "{rendered}");
        assert!(!rendered.contains("git-post-merge"), "{rendered}");
    }

    #[test]
    fn the_shim_stands_down_for_its_own_stage_only() {
        let rendered = shim(GitStage::PreCommit);
        assert!(
            rendered.contains(r#"[ "${DAFT_STAGE_GUARD:-}" = "pre-commit" ] && exit 0"#),
            "{rendered}"
        );
    }

    #[test]
    fn a_missing_binary_exits_zero_rather_than_blocking_the_operation() {
        // "I uninstalled daft" must not become "I cannot commit".
        let rendered = shim(GitStage::PreCommit);
        assert!(rendered.contains("command -v daft"), "{rendered}");
        assert!(rendered.contains("exit 0"), "{rendered}");
        assert!(!rendered.contains("exit 1"), "{rendered}");
    }

    #[test]
    fn foreign_scripts_are_not_mistaken_for_shims() {
        assert_eq!(
            declared_stage("#!/bin/sh\nexec lefthook run pre-commit"),
            None
        );
        assert_eq!(declared_stage(""), None);
        // A hook that merely mentions daft is not daft's.
        assert_eq!(
            declared_stage("#!/bin/sh\n# we used to use daft here\nexit 0"),
            None
        );
        // A marker naming a stage daft does not manage is not a shim either.
        assert_eq!(
            declared_stage("#!/bin/sh\n# daft:shim:v1 stage=pre-receive\n"),
            None
        );
    }

    #[test]
    fn a_hand_edited_shim_is_still_recognised_as_daft_s() {
        // Ownership is the marker, not the body — otherwise `uninstall` would
        // abandon a shim someone tweaked.
        let edited = format!(
            "#!/bin/sh\n{MARKER_PREFIX}pre-commit\necho 'my own note'\nexec daft __hook pre-commit\n"
        );
        assert_eq!(declared_stage(&edited), Some(GitStage::PreCommit));
    }

    #[test]
    fn known_managers_are_recognised_for_safe_backup() {
        assert_eq!(
            recognized_foreign_manager("#!/bin/sh\nexec lefthook run pre-commit \"$@\""),
            Some("lefthook")
        );
        assert_eq!(
            recognized_foreign_manager("#!/bin/sh\n. \"$(dirname -- \"$0\")/_/husky.sh\""),
            Some("husky")
        );
        assert_eq!(
            recognized_foreign_manager("#!/bin/sh\ncommand -v git-lfs >/dev/null"),
            Some("git-lfs")
        );
        // Somebody's own script is not, so install stops and reports rather
        // than moving what might be the only copy.
        assert_eq!(
            recognized_foreign_manager("#!/bin/bash\nmake check || exit 1"),
            None
        );
    }
}

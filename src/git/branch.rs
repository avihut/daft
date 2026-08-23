use super::GitCommand;
use super::oxide;
use anyhow::{Context, Result};
use std::process::Command;

/// What a local branch's configuration says it tracks, and whether the
/// tracked ref is still here — the typed answer to what `git branch -vv`
/// renders as `[<upstream>]` / `[<upstream>: gone]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchTracking {
    /// Short local branch name.
    pub branch: String,
    /// `branch.<name>.remote` as configured: a remote name, a URL, or the
    /// local pseudo-remote `.`.
    pub remote: String,
    /// `branch.<name>.merge` exactly as configured — `refs/heads/<x>` for an
    /// ordinary upstream, `refs/pull/<n>/head` and friends for a forge
    /// checkout — so unlike `remote` it is a ref name, not a remote. `None`
    /// when only the remote is configured. A value without a `refs/` prefix
    /// is kept as is and never mapped: git resolves no upstream from it
    /// either ("not stored as a remote-tracking branch").
    pub merge: Option<String>,
    /// The local ref git resolves the upstream to, and whether it is here.
    pub upstream: UpstreamRef,
}

impl BranchTracking {
    /// The upstream was there and is not any more — what `git branch -vv`
    /// renders as `[<upstream>: gone]`. Never true for an unmapped upstream:
    /// unknown is not gone.
    pub fn gone(&self) -> bool {
        matches!(self.upstream, UpstreamRef::Gone(_))
    }

    /// The local ref the upstream resolves to, when the configuration maps to
    /// one.
    pub fn tracking_ref(&self) -> Option<&str> {
        match &self.upstream {
            UpstreamRef::Unmapped => None,
            UpstreamRef::Present(tracking) | UpstreamRef::Gone(tracking) => Some(tracking),
        }
    }
}

/// Where a branch's configured upstream lands locally — `merge` mapped through
/// the remote's fetch refspecs (`refs/remotes/<remote>/<x>` for the usual
/// `+refs/heads/*:refs/remotes/<remote>/*`), or `merge` itself for the `.`
/// pseudo-remote.
///
/// `Gone` carries the ref it looked for, so "gone" can only ever be said of a
/// ref that is known: the shape makes a gone-but-unknown upstream
/// unrepresentable, which matters because prune deletes on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpstreamRef {
    /// The configuration maps to no local ref — a URL remote, a narrow refspec
    /// that does not cover the branch, a `refs/pull/*` upstream, a bare
    /// `merge` value — which is exactly where git shows no upstream at all.
    Unmapped,
    /// The tracking ref, present.
    Present(String),
    /// The tracking ref, absent: `git fetch --prune` took it, or the local
    /// branch it tracked was deleted.
    Gone(String),
}

impl GitCommand {
    pub fn branch_rename(&self, old_name: &str, new_name: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["branch", "-m", old_name, new_name])
            .output()
            .context("Failed to execute git branch -m command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git branch rename failed: {}", stderr);
        }

        Ok(())
    }

    pub fn branch_delete(&self, branch: &str, force: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["branch"]);

        if force {
            cmd.arg("-D");
        } else {
            cmd.arg("-d");
        }

        cmd.arg(branch);

        let output = cmd
            .output()
            .context("Failed to execute git branch command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git branch delete failed: {}", stderr);
        }

        Ok(())
    }

    /// What every local branch with a `branch.<name>.remote` entry tracks, in
    /// ref order — see [`BranchTracking`]. One in-process read of the refs
    /// and the resolved config; the `gone` verdict comes from git's own
    /// upstream resolution (refspec mapping), not from a hand-built
    /// `refs/remotes/<remote>/<branch>` guess.
    pub fn branch_tracking(&self) -> Result<Vec<BranchTracking>> {
        oxide::branch_tracking(&self.gix_repo()?)
    }

    /// Checkout a branch in the current working directory.
    pub fn checkout(&self, branch: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["checkout"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(branch);

        let output = cmd
            .output()
            .context("Failed to execute git checkout command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git checkout failed: {}", stderr);
        }

        Ok(())
    }
}

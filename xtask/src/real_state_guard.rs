//! `xtask real-state-guard` — the test-isolation tripwire (#697).
//!
//! Snapshots the *real* daft state a test run must never touch, then verifies
//! it is unchanged after the suite. It exists because the daft test harnesses
//! sandbox state via `DAFT_{CONFIG,DATA,STATE}_DIR`, but that isolation
//! silently evaporates if the binary under test is not a `daft_dev_build` (a
//! release/tagged build, or a system `daft` on `PATH`): the overrides compile
//! out and every catalog / registry / job write lands in the developer's real
//! dirs. #696's catalog leak (822 `/tmp` repos in the real `catalog.db`) is the
//! incident this guards against; #666 (`repos.json`) and #478/#669 (state
//! `jobs/`) are the same class on the other two surfaces.
//!
//! The guard resolves the real dirs itself via `dirs` — it never reads
//! `DAFT_*_DIR`, so it always targets the real surface regardless of the
//! ambient env — and never *creates* anything it inspects. A drift test pins
//! its resolution to daft's own `daft_{config,data,state}_dir()` (overrides
//! unset) so the two cannot diverge.
//!
//! Coverage is *targeted*, not a whole-XDG-dir walk: on macOS the config and
//! data dirs are the same `~/Library/Application Support/daft` that also hosts
//! centralized-layout worktrees, so a wholesale walk would be slow and trip on
//! unrelated edits. Instead:
//!   * `<data>/daft/catalog/`     — content-hash every file (the DB triplet).
//!   * `<config>/daft/repos.json` — content-hash the repo/trust registry.
//!   * `<state>/daft/` + `jobs/`  — a compact entry-set digest (count + a hash
//!     of the sorted child names). These dirs are litter-prone — the real
//!     `jobs/` can hold tens of thousands of orphaned dirs (#669) — so we store
//!     a digest, not every name, and never hash child *contents* (job
//!     DBs/sockets churn at runtime; a leaked repo always lands under a
//!     brand-new UUID name, which the digest catches).
//!   * `~/.claude/skills/daft-worktree-workflow/SKILL.md` — content-hash of
//!     the real user-global agent skill. Unlike the other surfaces this one
//!     has no `DAFT_*_DIR` override at all: `daft skill install` and
//!     `daft doctor --fix` resolve it from HOME, so any test exercising them
//!     must pass `--dir` or an inline `HOME=` override — this hash catches
//!     the one that forgets (#664).

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Mode {
    /// Record the current real-state fingerprint into <FILE>.
    Snapshot,
    /// Recompute the fingerprint and fail if it differs from <FILE>.
    Verify,
}

/// Run the guard in `mode`, reading/writing the fingerprint at `file`.
pub fn run(mode: Mode, file: &Path) -> Result<()> {
    let now = capture().context("capturing real daft-state fingerprint")?;
    match mode {
        Mode::Snapshot => {
            let yaml = serde_yaml::to_string(&now).context("serializing fingerprint")?;
            std::fs::write(file, yaml)
                .with_context(|| format!("writing fingerprint to {}", file.display()))?;
            Ok(())
        }
        Mode::Verify => {
            let prev_yaml = std::fs::read_to_string(file)
                .with_context(|| format!("reading fingerprint from {}", file.display()))?;
            let prev: Snapshot = serde_yaml::from_str(&prev_yaml)
                .with_context(|| format!("parsing fingerprint from {}", file.display()))?;
            let diffs = prev.diff(&now);
            if diffs.is_empty() {
                return Ok(());
            }
            bail!("{}", tripwire_message(&diffs));
        }
    }
}

/// Fingerprint of the daft-owned artifacts on the three real XDG surfaces.
/// Absent files/dirs are represented as empty/`None`/`!exists` so a run that
/// *creates* one (the leak we guard against) shows up as a diff.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Snapshot {
    /// `filename -> content hash` for every file directly in
    /// `<data>/daft/catalog/`. Empty when the catalog dir does not exist.
    catalog: BTreeMap<String, String>,
    /// Content hash of `<config>/daft/repos.json`, or `None` when absent.
    repos_json: Option<String>,
    /// Entry-set digest of `<state>/daft/`.
    state_top: DirDigest,
    /// Entry-set digest of `<state>/daft/jobs/`.
    state_jobs: DirDigest,
    /// Content hash of the real user-global agent skill
    /// (`~/.claude/skills/daft-worktree-workflow/SKILL.md`), or `None` when
    /// not installed. `serde(default)` keeps older fingerprint files
    /// readable.
    #[serde(default)]
    claude_skill: Option<String>,
}

/// Compact digest of a directory's immediate entry set: whether it exists, how
/// many children it has, and a hash of their sorted names. Storing a digest
/// rather than every name keeps the fingerprint tiny even when `jobs/` holds
/// tens of thousands of entries.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DirDigest {
    /// `false` when the directory does not exist — a valid state that becomes
    /// a diff the moment a run creates it.
    exists: bool,
    count: usize,
    /// FNV of the sorted, newline-joined child names (`""` when absent).
    names_hash: String,
}

impl Snapshot {
    /// Human-readable list of the surfaces that changed between `self`
    /// (recorded) and `now` (recomputed). Empty ⇒ nothing leaked.
    fn diff(&self, now: &Snapshot) -> Vec<String> {
        let mut out = Vec::new();
        if self.catalog != now.catalog {
            out.push("data:   the repo catalog under <data>/daft/catalog/ changed".to_string());
        }
        if self.repos_json != now.repos_json {
            out.push("config: <config>/daft/repos.json changed".to_string());
        }
        if self.state_top != now.state_top {
            out.push(format!(
                "state:  entries under <state>/daft/ changed ({} → {})",
                self.state_top.count, now.state_top.count
            ));
        }
        if self.state_jobs != now.state_jobs {
            out.push(format!(
                "state:  entries under <state>/daft/jobs/ changed ({} → {})",
                self.state_jobs.count, now.state_jobs.count
            ));
        }
        if self.claude_skill != now.claude_skill {
            out.push(
                "home:   ~/.claude/skills/daft-worktree-workflow/SKILL.md changed".to_string(),
            );
        }
        out
    }
}

/// Capture the current fingerprint of the real surfaces.
fn capture() -> Result<Snapshot> {
    Ok(Snapshot {
        catalog: hash_dir_files(&real_data_dir()?.join("catalog"))?,
        repos_json: hash_file_opt(&real_config_dir()?.join("repos.json"))?,
        state_top: dir_digest(&real_state_dir())?,
        state_jobs: dir_digest(&real_state_dir().join("jobs"))?,
        claude_skill: hash_file_opt(&real_claude_skill_file()?)?,
    })
}

/// Build the failure message, appending the concrete real paths so the reader
/// knows exactly which files to inspect.
fn tripwire_message(diffs: &[String]) -> String {
    let mut msg = String::from(
        "TRIPWIRE: the real daft state changed during this run — the test suite leaked \
         into your real config/state/data dirs.\n\n",
    );
    for d in diffs {
        msg.push_str("  • ");
        msg.push_str(d);
        msg.push('\n');
    }
    msg.push_str(
        "\nFor the config/data/state surfaces this almost always means the binary under \
         test is not a daft_dev_build — a release/tagged build, or a system `daft` on PATH \
         — so DAFT_*_DIR was ignored and writes hit the real dirs; rebuild the dev binary \
         and re-run (#697). A changed agent-skill file instead means a test ran \
         `daft skill install` or `daft doctor --fix` without `--dir` or an inline `HOME=` \
         override (#664).\n\n\
         Real paths on this machine:\n",
    );
    let show = |label: &str, p: Result<PathBuf>| match p {
        Ok(p) => format!("  {label}: {}\n", p.display()),
        Err(_) => format!("  {label}: <unresolved>\n"),
    };
    msg.push_str(&show("config", real_config_dir()));
    msg.push_str(&show("data  ", real_data_dir()));
    msg.push_str(&show("state ", Ok(real_state_dir())));
    msg.push_str(&show("skill ", real_claude_skill_file()));
    msg
}

// --- real-dir resolution (override-independent; pinned to daft by a test) ---

/// `<config>/daft` — the real config dir, ignoring `DAFT_CONFIG_DIR`. Mirrors
/// the fallback branch of `daft::daft_config_dir`.
fn real_config_dir() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("resolving OS config dir")?
        .join("daft"))
}

/// `<data>/daft` — the real data dir, ignoring `DAFT_DATA_DIR`.
fn real_data_dir() -> Result<PathBuf> {
    Ok(dirs::data_dir()
        .context("resolving OS data dir")?
        .join("daft"))
}

/// The real user-global agent-skill file. Mirrors
/// `daft::skill::user_skills_root()` + `skill_file_path()`; there is no env
/// override to ignore — the skill path is always HOME-derived.
fn real_claude_skill_file() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("resolving home dir")?
        .join(".claude")
        .join("skills")
        .join("daft-worktree-workflow")
        .join("SKILL.md"))
}

/// `<state>/daft` — the real state dir, ignoring `DAFT_STATE_DIR`. Mirrors
/// `daft::daft_state_dir`'s macOS fallback (`dirs::state_dir()` is `None` on
/// macOS → `~/.local/state`).
fn real_state_dir() -> PathBuf {
    dirs::state_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not determine home directory")
                .join(".local")
                .join("state")
        })
        .join("daft")
}

// --- fingerprint primitives ---

/// `filename -> content hash` for every regular file directly under `dir`.
/// A missing `dir` is a valid state (empty map) — the catalog dir does not
/// exist until daft first writes it.
fn hash_dir_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in rd {
        let entry = entry?;
        if entry.path().is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            map.insert(name, hash_file(&entry.path())?);
        }
    }
    Ok(map)
}

/// Content hash of `path`, or `None` when it does not exist.
fn hash_file_opt(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(hex(fnv1a64(&bytes)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("hashing {}", path.display())),
    }
}

/// Content hash of an existing file.
fn hash_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("hashing {}", path.display()))?;
    Ok(hex(fnv1a64(&bytes)))
}

/// Compact entry-set digest of `dir` (see [`DirDigest`]). A missing dir yields
/// `DirDigest::default()` (`exists: false`).
fn dir_digest(dir: &Path) -> Result<DirDigest> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(DirDigest::default()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in rd {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(DirDigest {
        exists: true,
        count: names.len(),
        names_hash: hex(fnv1a64(names.join("\n").as_bytes())),
    })
}

/// FNV-1a-64 over `bytes`. A dependency-free, deterministic content digest —
/// snapshot and verify run the same xtask binary, so cross-version stability
/// is irrelevant; all we need is "did these bytes change?".
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn hex(h: u64) -> String {
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard's own real-dir resolution must track daft's resolvers exactly
    /// — otherwise the tripwire watches different files than daft writes and is
    /// worthless. daft's resolvers honor `DAFT_*_DIR` in dev builds, so a
    /// surface can only be compared when *its* override is unset. We assert
    /// per-surface (rather than skipping wholesale) so `data` and `state` — the
    /// surfaces #697 cares about — are still checked even in a dev environment
    /// that sets `DAFT_CONFIG_DIR` (the `.git/.daft-sandbox`); CI's clean env
    /// exercises all three. No env mutation, so it stays race-free under
    /// `cargo test`'s parallelism.
    #[test]
    fn guard_dirs_match_daft_resolvers() {
        if std::env::var_os("DAFT_CONFIG_DIR").is_none() {
            assert_eq!(real_config_dir().unwrap(), daft::daft_config_dir().unwrap());
        }
        if std::env::var_os("DAFT_DATA_DIR").is_none() {
            assert_eq!(real_data_dir().unwrap(), daft::daft_data_dir().unwrap());
        }
        if std::env::var_os("DAFT_STATE_DIR").is_none() {
            assert_eq!(real_state_dir(), daft::daft_state_dir().unwrap());
        }
        // The skill path has no env override, so this pin is unconditional.
        assert_eq!(
            real_claude_skill_file().unwrap(),
            daft::skill::skill_file_path(&daft::skill::user_skills_root().unwrap())
        );
    }

    #[test]
    fn absent_surfaces_read_as_empty_not_error() {
        // Surfaces that don't exist on a fresh machine must round-trip as
        // "nothing there" rather than erroring — otherwise a clean machine
        // reads as a leak.
        let missing = PathBuf::from("/definitely/not/a/real/daft/dir/zzz");
        assert!(hash_dir_files(&missing.join("catalog")).unwrap().is_empty());
        assert!(hash_file_opt(&missing.join("repos.json"))
            .unwrap()
            .is_none());
        assert!(!dir_digest(&missing).unwrap().exists);
    }

    #[test]
    fn dir_digest_changes_when_a_child_appears() {
        // The core state-leak signal: a new entry (e.g. a leaked jobs/<uuid>/)
        // must move the digest.
        let dir = tempfile::tempdir().unwrap();
        let before = dir_digest(dir.path()).unwrap();
        assert!(before.exists && before.count == 0);
        std::fs::create_dir(dir.path().join("019d-some-repo-uuid")).unwrap();
        let after = dir_digest(dir.path()).unwrap();
        assert_ne!(before, after);
        assert_eq!(after.count, 1);
    }

    #[test]
    fn diff_flags_each_surface_independently() {
        let base = Snapshot::default();

        let mut catalog_changed = Snapshot::default();
        catalog_changed
            .catalog
            .insert("catalog.db".to_string(), "deadbeef".to_string());
        assert_eq!(base.diff(&catalog_changed).len(), 1);
        assert!(base.diff(&catalog_changed)[0].contains("catalog"));

        let repos_changed = Snapshot {
            repos_json: Some("abc".to_string()),
            ..Snapshot::default()
        };
        assert!(base.diff(&repos_changed)[0].contains("repos.json"));

        let jobs_changed = Snapshot {
            state_jobs: DirDigest {
                exists: true,
                count: 1,
                names_hash: "x".to_string(),
            },
            ..Snapshot::default()
        };
        assert!(base.diff(&jobs_changed)[0].contains("jobs"));

        let skill_changed = Snapshot {
            claude_skill: Some("abc".to_string()),
            ..Snapshot::default()
        };
        assert!(base.diff(&skill_changed)[0].contains(".claude/skills"));

        // Identical snapshots ⇒ clean.
        assert!(base.diff(&Snapshot::default()).is_empty());
    }

    #[test]
    fn snapshot_round_trips_through_yaml() {
        let mut snap = Snapshot::default();
        snap.catalog
            .insert("catalog.db".to_string(), "1234".to_string());
        snap.state_top = DirDigest {
            exists: true,
            count: 3,
            names_hash: "abc".to_string(),
        };
        let yaml = serde_yaml::to_string(&snap).unwrap();
        let back: Snapshot = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(snap, back);
    }
}

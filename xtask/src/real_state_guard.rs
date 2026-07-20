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
//!   * `<config>/daft/`           — content-hash every top-level file except
//!     the volatile stamps in [`VOLATILE_CONFIG_FILES`] (#667 widened this
//!     from `repos.json` alone, so `config.toml` and any *unexpected* new
//!     file — the real leak signature — are covered too). Two deliberate
//!     blind spots: subdirectories are skipped, because on macOS this dir
//!     also hosts centralized-layout worktrees and walking them would be slow
//!     and noisy; and a leak into a future `<config>/daft/<subdir>/` would go
//!     unseen until this scan learns to descend into known-daft subdirs.
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

/// Config-dir files that daft's own *background* work rewrites, independently
/// of whatever the test suite is doing: the update-check/trust-prune/log-clean
/// daemons (`__check-update` & co., which `setsid` away and land seconds after
/// their parent exits) and the one-shot hint ledger.
///
/// They are excluded from the fingerprint because a change to them cannot be
/// attributed to the run under guard — the developer running `daft go` in
/// another terminal, or a git hook invoking daft, writes them just as readily.
/// Watching them made a clean suite fail with "the test suite leaked into your
/// real config/state/data dirs", and a tripwire that cries wolf gets disabled.
/// The residual risk (a genuine leak of exactly these files going unseen) is
/// small: any binary leaking them also leaks `repos.json` / the catalog / the
/// state dir, which are watched, and the `__dirs` preflight already rejects a
/// binary that ignores `DAFT_*_DIR` before the suite starts.
const VOLATILE_CONFIG_FILES: &[&str] = &[
    "update-check.json",
    "update-notification.json",
    "trust-prune.json",
    "log-clean.json",
    "hints.json",
];

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
            let (fatal, advisory) = partition_fatality(&diffs, is_ci());
            if !fatal.is_empty() {
                // A change on an attributable surface (catalog / config / skill,
                // or any state surface in CI) — fail, folding in the concurrent
                // state churn for context.
                bail!("{}", tripwire_message(&fatal, &advisory));
            }
            // Only the machine-global state surfaces changed, outside CI:
            // unattributable concurrent-daft churn. Warn, don't fail.
            eprintln!("{}", concurrency_note(&advisory));
            Ok(())
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
    /// `filename -> content hash` for every file directly in `<config>/daft/`
    /// except [`VOLATILE_CONFIG_FILES`] (`repos.json`, `config.toml`, and
    /// anything unexpected). Empty when the config dir does not exist.
    /// Deliberately file-only: on macOS this dir can also host
    /// centralized-layout worktrees, which are subdirectories.
    #[serde(default)]
    config_files: BTreeMap<String, String>,
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

/// Which real surface a diff belongs to. Drives fatality (see
/// [`Surface::is_concurrency_exposed`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Surface {
    Catalog,
    ConfigFiles,
    StateTop,
    StateJobs,
    ClaudeSkill,
}

impl Surface {
    /// `<state>/daft/` and its `jobs/` subtree are shared by *every* daft
    /// process on the machine: a sibling worktree's test suite, a `daft`
    /// command in another terminal, or the background `__clean-logs`/coordinator
    /// all create and remove `jobs/<uuid>/` entries there, independently of the
    /// guarded run. An entry-set change is therefore unattributable — the same
    /// property that made the config daemon-stamps ([`VOLATILE_CONFIG_FILES`])
    /// unwatchable. Outside CI (where the runner is not the only daft) these
    /// surfaces are advisory, not fatal.
    ///
    /// Catalog / config / skill are NOT exposed: a honoring binary (guaranteed
    /// by the `__dirs` preflight for exec suites, and by cfg(test) construction
    /// for unit tests) sandboxes its catalog too, so concurrent activity cannot
    /// churn the real one — only a genuine leak can. They stay fatal everywhere.
    fn is_concurrency_exposed(self) -> bool {
        matches!(self, Surface::StateTop | Surface::StateJobs)
    }
}

impl Snapshot {
    /// The surfaces that changed between `self` (recorded) and `now`
    /// (recomputed), each tagged with its [`Surface`] so the caller can decide
    /// fatality. Empty ⇒ nothing changed.
    fn diff(&self, now: &Snapshot) -> Vec<(Surface, String)> {
        let mut out = Vec::new();
        if self.catalog != now.catalog {
            out.push((
                Surface::Catalog,
                "data:   the repo catalog under <data>/daft/catalog/ changed".to_string(),
            ));
        }
        if self.config_files != now.config_files {
            out.push((
                Surface::ConfigFiles,
                format!(
                    "config: files under <config>/daft/ changed ({})",
                    changed_keys(&self.config_files, &now.config_files).join(", ")
                ),
            ));
        }
        if self.state_top != now.state_top {
            out.push((
                Surface::StateTop,
                format!(
                    "state:  entries under <state>/daft/ changed ({} → {})",
                    self.state_top.count, now.state_top.count
                ),
            ));
        }
        if self.state_jobs != now.state_jobs {
            out.push((
                Surface::StateJobs,
                format!(
                    "state:  entries under <state>/daft/jobs/ changed ({} → {})",
                    self.state_jobs.count, now.state_jobs.count
                ),
            ));
        }
        if self.claude_skill != now.claude_skill {
            out.push((
                Surface::ClaudeSkill,
                "home:   ~/.claude/skills/daft-worktree-workflow/SKILL.md changed".to_string(),
            ));
        }
        out
    }
}

/// CI-environment variables, mirroring daft's `trust_prune::is_ci_environment`
/// (which is `pub(crate)` and so unreachable from xtask). Kept in lockstep by
/// [`tests::ci_var_list_matches_daft`] — a CI daft treats as CI but this guard
/// does not would keep the concurrency-exposed state surfaces fatal there,
/// reintroducing the false positive in that CI.
const CI_ENV_VARS: &[&str] = &[
    "CI",
    "GITHUB_ACTIONS",
    "JENKINS_URL",
    "TRAVIS",
    "CIRCLECI",
    "GITLAB_CI",
    "BUILDKITE",
    "TF_BUILD",
];

/// Whether the guard is running under CI, where the runner is the only daft on
/// the machine so the state surfaces are reliable (no concurrent churn).
fn is_ci() -> bool {
    CI_ENV_VARS.iter().any(|v| std::env::var_os(v).is_some())
}

/// Split diffs into `(fatal, advisory)`. A diff is advisory only when it is on a
/// concurrency-exposed surface AND we are not in CI — otherwise it is fatal.
/// Pure, so it is unit-tested without touching the env or filesystem.
fn partition_fatality(diffs: &[(Surface, String)], ci: bool) -> (Vec<String>, Vec<String>) {
    let mut fatal = Vec::new();
    let mut advisory = Vec::new();
    for (surface, msg) in diffs {
        if surface.is_concurrency_exposed() && !ci {
            advisory.push(msg.clone());
        } else {
            fatal.push(msg.clone());
        }
    }
    (fatal, advisory)
}

/// Capture the current fingerprint of the real surfaces.
fn capture() -> Result<Snapshot> {
    Ok(Snapshot {
        catalog: hash_dir_files(&real_data_dir()?.join("catalog"), &[])?,
        config_files: hash_dir_files(&real_config_dir()?, VOLATILE_CONFIG_FILES)?,
        state_top: dir_digest(&real_state_dir())?,
        state_jobs: dir_digest(&real_state_dir().join("jobs"))?,
        claude_skill: hash_file_opt(&real_claude_skill_file()?)?,
    })
}

/// Names of files added, removed, or rewritten between two filename→hash
/// maps, so the tripwire names *which* config files moved.
fn changed_keys(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Vec<String> {
    let mut names = Vec::new();
    for k in a.keys().chain(b.keys()) {
        if a.get(k) != b.get(k) && !names.contains(k) {
            names.push(k.clone());
        }
    }
    names
}

/// Build the failure message, appending the concrete real paths so the reader
/// knows exactly which files to inspect. `fatal` are the attributable surfaces
/// that failed the run; `advisory` is any concurrent state churn, folded in for
/// context so the reader isn't puzzled by a partial picture.
fn tripwire_message(fatal: &[String], advisory: &[String]) -> String {
    let mut msg = String::from(
        "TRIPWIRE: the real daft state changed during this run — the test suite leaked \
         into your real config/state/data dirs.\n\n",
    );
    for d in fatal {
        msg.push_str("  • ");
        msg.push_str(d);
        msg.push('\n');
    }
    if !advisory.is_empty() {
        msg.push_str(
            "\nAlso changed (machine-global state dir — usually concurrent daft activity, \
             not this run):\n",
        );
        for d in advisory {
            msg.push_str("  • ");
            msg.push_str(d);
            msg.push('\n');
        }
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

/// Non-fatal note for when *only* the concurrency-exposed state surfaces changed
/// outside CI: the shared `<state>/daft/` dir moved, but that is unattributable
/// to the guarded run (another daft process churns it just as readily), so the
/// run passes with a heads-up rather than a spurious failure (#742).
fn concurrency_note(advisory: &[String]) -> String {
    let mut msg = String::from(
        "NOTE: the real daft state dir changed during this run, but only on the \
         machine-global state surface — not the catalog, config, or agent skill. This is \
         expected when another daft process runs concurrently: a sibling worktree's test \
         suite, a `daft` command in another terminal, or the background log-clean / \
         coordinator, all of which create and remove jobs/<uuid>/ entries in the shared \
         state dir. Not attributable to this run, so not failing it — in CI, where the \
         runner is the only daft, this stays fatal (#742).\n\n",
    );
    for d in advisory {
        msg.push_str("  • ");
        msg.push_str(d);
        msg.push('\n');
    }
    msg.push_str(&format!(
        "\nReal state dir: {}\n",
        real_state_dir().display()
    ));
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

/// `filename -> content hash` for every regular file directly under `dir`,
/// skipping any name in `skip`. A missing `dir` is a valid state (empty map) —
/// the catalog dir does not exist until daft first writes it.
///
/// Files that vanish between the listing and the read are skipped rather than
/// propagated as an error: both scanned dirs receive `NamedTempFile`s that
/// exist only for the moment between write and atomic rename (`repos.json` via
/// `hooks::trust::save_to`, the catalog DB's journal churn), and a snapshot
/// that aborts on that race takes the whole suite down with it — a guard that
/// fails the run it is supposed to be watching over.
fn hash_dir_files(dir: &Path, skip: &[&str]) -> Result<BTreeMap<String, String>> {
    let mut map = BTreeMap::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in rd {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if skip.contains(&name.as_str()) {
            continue;
        }
        // No let-chain: xtask is edition 2021.
        if entry.path().is_file() {
            if let Some(hash) = hash_file_opt(&entry.path())? {
                map.insert(name, hash);
            }
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
        // The skill root honors DAFT_SKILLS_DIR in dev builds (same as the
        // dirs above), so only pin it when *its* override is unset — otherwise
        // a dev shell that sets it (shared-env.sh) would trip this assert.
        if std::env::var_os(daft::skill::SKILLS_DIR_ENV).is_none() {
            assert_eq!(
                real_claude_skill_file().unwrap(),
                daft::skill::skill_file_path(&daft::skill::user_skills_root().unwrap())
            );
        }
    }

    #[test]
    fn absent_surfaces_read_as_empty_not_error() {
        // Surfaces that don't exist on a fresh machine must round-trip as
        // "nothing there" rather than erroring — otherwise a clean machine
        // reads as a leak.
        let missing = PathBuf::from("/definitely/not/a/real/daft/dir/zzz");
        assert!(hash_dir_files(&missing.join("catalog"), &[])
            .unwrap()
            .is_empty());
        assert!(hash_dir_files(&missing, VOLATILE_CONFIG_FILES)
            .unwrap()
            .is_empty());
        assert!(hash_file_opt(&missing.join("SKILL.md")).unwrap().is_none());
        assert!(!dir_digest(&missing).unwrap().exists);
    }

    /// The daemon stamps must not enter the fingerprint: `__check-update` &
    /// co. rewrite them from a *detached* process on the developer's own
    /// unrelated `daft` invocations, so watching them turned "someone used
    /// daft in another terminal" into a fatal "the suite leaked real state".
    #[test]
    fn volatile_daemon_stamps_are_excluded_from_config_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("repos.json"), "{}").unwrap();
        for name in VOLATILE_CONFIG_FILES {
            std::fs::write(dir.path().join(name), "{}").unwrap();
        }
        let snap = hash_dir_files(dir.path(), VOLATILE_CONFIG_FILES).unwrap();
        assert_eq!(snap.keys().collect::<Vec<_>>(), vec!["repos.json"]);

        // A daemon rewriting a stamp mid-run must not move the fingerprint…
        std::fs::write(dir.path().join("update-check.json"), "{\"v\":2}").unwrap();
        assert_eq!(
            hash_dir_files(dir.path(), VOLATILE_CONFIG_FILES).unwrap(),
            snap
        );
        // …while a genuinely unexpected file still does.
        std::fs::write(dir.path().join("leaked-by-a-test.json"), "x").unwrap();
        assert_ne!(
            hash_dir_files(dir.path(), VOLATILE_CONFIG_FILES).unwrap(),
            snap
        );
    }

    /// `repos.json` is rewritten via `NamedTempFile` + rename *in this dir*, so
    /// an entry can be listed by `read_dir` and gone by the time we read it.
    /// That must be a skip, not an error — an erroring snapshot aborts the run
    /// the guard is supposed to be watching over.
    ///
    /// Drives the real interleaving (a deleter thread against a live scan)
    /// rather than deleting up front, which would just make the entry absent
    /// from the listing and prove nothing.
    #[test]
    fn vanishing_file_is_skipped_not_fatal() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("repos.json"), "{}").unwrap();

        const CHURN_FILES: usize = 128;
        let stop = Arc::new(AtomicBool::new(false));
        let churn_dir = dir.path().to_path_buf();
        let churn_stop = Arc::clone(&stop);
        // Mimics save_to's temp-file churn. Batched (create all, then unlink
        // all) rather than create-then-unlink one at a time, so a scan that
        // lists a full batch routinely finds it gone by the time it reads —
        // the interleaving that made this fatal in the first place.
        let churn = std::thread::spawn(move || {
            while !churn_stop.load(Ordering::Relaxed) {
                for i in 0..CHURN_FILES {
                    let _ = std::fs::write(churn_dir.join(format!(".tmp{i}")), "half-written");
                }
                for i in 0..CHURN_FILES {
                    let _ = std::fs::remove_file(churn_dir.join(format!(".tmp{i}")));
                }
            }
        });

        for _ in 0..500 {
            let snap = hash_dir_files(dir.path(), &[])
                .expect("a file vanishing mid-scan must not fail the snapshot");
            // The durable file is always present regardless of the churn.
            assert!(snap.contains_key("repos.json"));
        }

        stop.store(true, Ordering::Relaxed);
        churn.join().unwrap();
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
        let catalog_msgs = base.diff(&catalog_changed);
        assert_eq!(catalog_msgs.len(), 1);
        assert_eq!(catalog_msgs[0].0, Surface::Catalog);
        assert!(catalog_msgs[0].1.contains("catalog"));

        // #667: a config write beyond repos.json (the update-check stamp) must
        // trip the config surface — hashing only repos.json missed it — and
        // the message must name the file.
        let mut config_changed = Snapshot::default();
        config_changed
            .config_files
            .insert("update-check.json".to_string(), "abc".to_string());
        let config_msgs = base.diff(&config_changed);
        assert_eq!(config_msgs.len(), 1);
        assert_eq!(config_msgs[0].0, Surface::ConfigFiles);
        assert!(config_msgs[0].1.contains("config"));
        assert!(config_msgs[0].1.contains("update-check.json"));

        let mut repos_changed = Snapshot::default();
        repos_changed
            .config_files
            .insert("repos.json".to_string(), "abc".to_string());
        assert!(base.diff(&repos_changed)[0].1.contains("repos.json"));

        let jobs_changed = Snapshot {
            state_jobs: DirDigest {
                exists: true,
                count: 1,
                names_hash: "x".to_string(),
            },
            ..Snapshot::default()
        };
        let jobs_msgs = base.diff(&jobs_changed);
        assert_eq!(jobs_msgs[0].0, Surface::StateJobs);
        assert!(jobs_msgs[0].1.contains("jobs"));

        let skill_changed = Snapshot {
            claude_skill: Some("abc".to_string()),
            ..Snapshot::default()
        };
        let skill_msgs = base.diff(&skill_changed);
        assert_eq!(skill_msgs[0].0, Surface::ClaudeSkill);
        assert!(skill_msgs[0].1.contains(".claude/skills"));

        // Identical snapshots ⇒ clean.
        assert!(base.diff(&Snapshot::default()).is_empty());
    }

    /// #742: the observed false positive — only `<state>/daft/jobs/` changed,
    /// the signature of a concurrent daft process creating `jobs/<uuid>/` dirs
    /// in the shared state dir. Outside CI it must be advisory (warn, not fail);
    /// in CI, where the runner is the only daft, it stays fatal.
    #[test]
    fn state_jobs_churn_is_advisory_outside_ci_but_fatal_in_ci() {
        let base = Snapshot::default();
        let jobs_changed = Snapshot {
            state_jobs: DirDigest {
                exists: true,
                count: 11,
                names_hash: "x".to_string(),
            },
            ..Snapshot::default()
        };
        let diffs = base.diff(&jobs_changed);
        assert_eq!(diffs.len(), 1);
        assert!(diffs[0].0.is_concurrency_exposed());

        let (fatal, advisory) = partition_fatality(&diffs, false);
        assert!(fatal.is_empty(), "state churn must not be fatal outside CI");
        assert_eq!(advisory.len(), 1);

        let (fatal_ci, advisory_ci) = partition_fatality(&diffs, true);
        assert_eq!(fatal_ci.len(), 1, "state churn stays fatal in CI");
        assert!(advisory_ci.is_empty());
    }

    /// The top-level `<state>/daft/` set (coordinator sockets/pids) is equally
    /// concurrency-exposed — another session's coordinator spawn/exit churns it.
    #[test]
    fn state_top_churn_is_advisory_outside_ci() {
        let base = Snapshot::default();
        let top_changed = Snapshot {
            state_top: DirDigest {
                exists: true,
                count: 3,
                names_hash: "y".to_string(),
            },
            ..Snapshot::default()
        };
        let (fatal, advisory) = partition_fatality(&base.diff(&top_changed), false);
        assert!(fatal.is_empty());
        assert_eq!(advisory.len(), 1);
    }

    /// Catalog / config / skill are attributable (a honoring binary sandboxes
    /// them), so a change is a genuine leak and must fail regardless of CI —
    /// otherwise this change would blind the guard to the #696 / #666 / #664
    /// leaks it exists to catch.
    #[test]
    fn attributable_surface_leaks_are_fatal_even_outside_ci() {
        let base = Snapshot::default();

        let mut catalog = Snapshot::default();
        catalog
            .catalog
            .insert("catalog.db".to_string(), "d".to_string());
        let mut config = Snapshot::default();
        config
            .config_files
            .insert("repos.json".to_string(), "d".to_string());
        let skill = Snapshot {
            claude_skill: Some("d".to_string()),
            ..Snapshot::default()
        };

        for leaked in [catalog, config, skill] {
            let (fatal, advisory) = partition_fatality(&base.diff(&leaked), false);
            assert_eq!(
                fatal.len(),
                1,
                "attributable-surface leak must be fatal outside CI"
            );
            assert!(advisory.is_empty());
        }
    }

    /// A genuine catalog leak beside concurrent state churn must still fail
    /// outside CI — the fatal catalog surface dominates; the state advisory
    /// cannot rescue it.
    #[test]
    fn real_leak_beside_concurrent_state_churn_still_fails_outside_ci() {
        let base = Snapshot::default();
        let mut mixed = Snapshot::default();
        mixed
            .catalog
            .insert("catalog.db".to_string(), "d".to_string());
        mixed.state_jobs = DirDigest {
            exists: true,
            count: 1,
            names_hash: "z".to_string(),
        };
        let (fatal, advisory) = partition_fatality(&base.diff(&mixed), false);
        assert_eq!(fatal.len(), 1);
        assert_eq!(advisory.len(), 1);
    }

    /// `is_ci()`'s var list mirrors daft's `trust_prune::is_ci_environment`
    /// (which is `pub(crate)`, so we can't compare directly). A drift — daft
    /// learning a CI var this guard doesn't — would keep the state surfaces
    /// fatal in that CI, reintroducing the false positive there. Pin the set so
    /// the drift is a failing test, not a silent regression.
    #[test]
    fn ci_var_list_matches_daft() {
        assert_eq!(
            CI_ENV_VARS,
            &[
                "CI",
                "GITHUB_ACTIONS",
                "JENKINS_URL",
                "TRAVIS",
                "CIRCLECI",
                "GITLAB_CI",
                "BUILDKITE",
                "TF_BUILD",
            ]
        );
    }

    #[test]
    fn snapshot_round_trips_through_yaml() {
        let mut snap = Snapshot::default();
        snap.catalog
            .insert("catalog.db".to_string(), "1234".to_string());
        snap.config_files
            .insert("update-check.json".to_string(), "5678".to_string());
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

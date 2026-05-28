//! Run-wide cache of pre-generated fixture remotes.
//!
//! Without the cache, `repo_gen::generate_repo` runs once per scenario per
//! `use_fixture:` reference. The full suite references ~400 fixtures but
//! only ~50 distinct `(use_fixture, name)` tuples — every shared fixture
//! is rebuilt several times. The cache generates each unique tuple once
//! at run start, then per-scenario provisioning becomes a `cow_copy::copy_dir`
//! from the cache into the sandbox's `remotes/` directory (O(1) on
//! APFS / reflink-capable Linux, byte-copy fallback elsewhere).
//!
//! Cache lifetime is per-run: the root is registered with the runner's
//! cleanup registry and reclaimed on natural end and SIGINT alike. A
//! persistent cross-run mode (with hash-based invalidation) would amortise
//! the prime further but is out of scope for #513.
//!
//! Cache key is `(use_fixture, name)`, not just `use_fixture`: the fixture
//! YAML embeds `{{NAME}}` placeholders that get substituted with the
//! scenario's chosen repo name, and those substitutions appear in branch
//! file contents — so two scenarios referencing the same fixture with
//! different `name:` values produce different bare-repo contents.

use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::cow_copy;
use super::repo_gen;
use super::resolve_fixture_spec;
use super::schema;

/// Identifier for a single cached fixture: `(use_fixture, name)`.
///
/// Owned `String`s rather than borrows so the cache can survive past the
/// scenario-parse phase that produced the keys.
pub(crate) type FixtureKey = (String, String);

/// Walk the raw YAML of each scenario file in `scenario_files` and collect
/// the unique set of `(use_fixture, name)` tuples referenced anywhere in
/// the suite.
///
/// Inline `RepoEntry::Inline` specs are ignored — they have no fixture to
/// share. Scenario files that fail to parse contribute nothing to the
/// index but do not abort the walk: a per-scenario parse error will
/// surface again (with proper context) when the worker tries to load that
/// scenario through the normal path. We don't want a single broken YAML
/// file to take down the cache prime for the other 580 scenarios.
pub(crate) fn collect_fixture_keys(scenario_files: &[PathBuf]) -> BTreeSet<FixtureKey> {
    let mut keys = BTreeSet::new();
    for path in scenario_files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(raw) = serde_yaml::from_str::<schema::RawScenario>(&content) else {
            continue;
        };
        for entry in raw.repos {
            if let schema::RepoEntry::Fixture(fr) = entry {
                keys.insert((fr.use_fixture, fr.name));
            }
        }
    }
    keys
}

/// Cache of pre-built bare remotes keyed by `(use_fixture, name)`.
///
/// Built once per run by [`FixtureCache::prime`]. Workers call
/// [`FixtureCache::clone_into`] to materialise a per-scenario copy in the
/// scenario's sandbox. The cache root is owned by the runner's cleanup
/// registry, not the struct — registration happens before prime so a
/// partial prime still gets cleaned up via the existing SIGINT path.
pub(crate) struct FixtureCache {
    /// `(use_fixture, name) → absolute path of the prebuilt bare repo`.
    paths: HashMap<FixtureKey, PathBuf>,
}

impl FixtureCache {
    /// Generate each `(use_fixture, name)` tuple in `keys` into a bare repo
    /// under `<root>/<use_fixture>/<name>/`. The two-level layout keeps
    /// multiple fixtures isolated without prefix collisions on the inner
    /// directory name.
    pub fn prime(keys: &BTreeSet<FixtureKey>, fixtures_dir: &Path, root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)
            .with_context(|| format!("creating fixture cache root: {}", root.display()))?;

        let mut paths = HashMap::with_capacity(keys.len());
        for (use_fixture, name) in keys {
            let parent = root.join(use_fixture);
            std::fs::create_dir_all(&parent)
                .with_context(|| format!("creating fixture cache subdir: {}", parent.display()))?;
            let spec = resolve_fixture_spec(fixtures_dir, use_fixture, name.clone())?;
            let bare_path = repo_gen::generate_repo(&spec, &parent).with_context(|| {
                format!(
                    "priming fixture '{}' for repo '{}' under {}",
                    use_fixture,
                    name,
                    parent.display()
                )
            })?;
            paths.insert((use_fixture.clone(), name.clone()), bare_path);
        }
        Ok(Self { paths })
    }

    /// Materialise a per-scenario copy of the cached fixture at `key` into
    /// `dst`. `dst` must not already exist; its parent must.
    pub fn clone_into(&self, key: &FixtureKey, dst: &Path) -> Result<()> {
        let src = self.paths.get(key).with_context(|| {
            format!(
                "fixture cache miss for ('{}', '{}') — this is a programming \
                 error: every fixture-derived RepoSpec encountered by a worker \
                 must have been indexed by collect_fixture_keys at run start",
                key.0, key.1
            )
        })?;
        cow_copy::copy_dir(src, dst).with_context(|| {
            format!(
                "cloning fixture '{}' for repo '{}' from cache to {}",
                key.0,
                key.1,
                dst.display()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    fn fixtures_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tests/manual/fixtures/repos")
    }

    fn write_scenario(dir: &Path, file: &str, body: &str) -> std::path::PathBuf {
        let p = dir.join(file);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn collect_finds_unique_fixture_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_scenario(
            tmp.path(),
            "a.yml",
            r#"
name: a
repos:
  - { name: origin, use_fixture: standard-remote }
steps:
  - name: noop
    run: "true"
"#,
        );
        let keys = collect_fixture_keys(&[a]);
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&("standard-remote".into(), "origin".into())));
    }

    #[test]
    fn collect_dedupes_identical_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let body = r#"
name: x
repos:
  - { name: origin, use_fixture: standard-remote }
steps:
  - name: noop
    run: "true"
"#;
        let a = write_scenario(tmp.path(), "a.yml", body);
        let b = write_scenario(tmp.path(), "b.yml", body);
        let keys = collect_fixture_keys(&[a, b]);
        assert_eq!(
            keys.len(),
            1,
            "identical (fixture, name) tuples should collapse to one cache key",
        );
    }

    #[test]
    fn collect_distinguishes_different_names() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_scenario(
            tmp.path(),
            "a.yml",
            r#"
name: a
repos:
  - { name: alpha, use_fixture: standard-remote }
steps:
  - name: noop
    run: "true"
"#,
        );
        let b = write_scenario(
            tmp.path(),
            "b.yml",
            r#"
name: b
repos:
  - { name: beta, use_fixture: standard-remote }
steps:
  - name: noop
    run: "true"
"#,
        );
        let keys = collect_fixture_keys(&[a, b]);
        // Different `name:` values mean different `{{NAME}}` substitutions,
        // so different bare-repo contents — must be separate cache entries.
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn collect_skips_inline_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_scenario(
            tmp.path(),
            "a.yml",
            r#"
name: a
repos:
  - name: inline-repo
    branches:
      - name: main
        files: [{ path: x, content: y }]
        commits: [{ message: init }]
steps:
  - name: noop
    run: "true"
"#,
        );
        let keys = collect_fixture_keys(&[a]);
        assert!(keys.is_empty(), "inline repos should not enter the cache");
    }

    #[test]
    fn collect_tolerates_unparseable_files() {
        let tmp = tempfile::tempdir().unwrap();
        let broken = write_scenario(tmp.path(), "broken.yml", "this: is: invalid: yaml:::");
        let ok = write_scenario(
            tmp.path(),
            "ok.yml",
            r#"
name: ok
repos:
  - { name: origin, use_fixture: standard-remote }
steps:
  - name: noop
    run: "true"
"#,
        );
        let keys = collect_fixture_keys(&[broken, ok]);
        assert_eq!(
            keys.len(),
            1,
            "broken scenario should be skipped, valid one should contribute its key",
        );
    }

    #[test]
    fn prime_creates_one_repo_per_key() {
        let cache_root = tempfile::tempdir().unwrap();
        let root_path = cache_root.path().to_path_buf();
        let mut keys = BTreeSet::new();
        keys.insert(("standard-remote".to_string(), "alpha".to_string()));
        keys.insert(("standard-remote".to_string(), "beta".to_string()));

        let cache = FixtureCache::prime(&keys, &fixtures_dir(), root_path.clone()).unwrap();

        assert_eq!(cache.paths.len(), 2);
        assert!(root_path.join("standard-remote/alpha").is_dir());
        assert!(root_path.join("standard-remote/beta").is_dir());
        // Each bare repo carries its own HEAD — a quick proof that
        // generate_repo ran independently for each key rather than
        // sharing state across them.
        assert!(root_path.join("standard-remote/alpha/HEAD").is_file());
        assert!(root_path.join("standard-remote/beta/HEAD").is_file());
    }

    #[test]
    fn clone_into_produces_equivalent_history() {
        let cache_root = tempfile::tempdir().unwrap();
        let mut keys = BTreeSet::new();
        keys.insert(("standard-remote".to_string(), "origin".to_string()));
        let cache =
            FixtureCache::prime(&keys, &fixtures_dir(), cache_root.path().to_path_buf()).unwrap();

        // Clone the cached fixture into a fresh destination ...
        let dst_parent = tempfile::tempdir().unwrap();
        let dst = dst_parent.path().join("origin");
        cache
            .clone_into(&("standard-remote".into(), "origin".into()), &dst)
            .unwrap();

        // ... and build a from-scratch reference from the same spec.
        let ref_parent = tempfile::tempdir().unwrap();
        let spec =
            resolve_fixture_spec(&fixtures_dir(), "standard-remote", "origin".into()).unwrap();
        let ref_bare = repo_gen::generate_repo(&spec, ref_parent.path()).unwrap();

        // Equality on commit message + branch shape is the right check —
        // `generate_repo` writes the current wall-clock into committer/author
        // metadata, so SHAs and pack bytes differ between invocations even
        // when the content is identical. What must match is the user-visible
        // history: every commit message present in the cached copy must also
        // be present in the reference, and the branch tips must align.
        let subjects = |path: &Path| -> BTreeSet<String> {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["log", "--all", "--format=%s"])
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            subjects(&dst),
            subjects(&ref_bare),
            "clone-from-cache must reproduce the same commit subjects as a fresh generate_repo",
        );

        let branches = |path: &Path| -> BTreeSet<String> {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            branches(&dst),
            branches(&ref_bare),
            "clone-from-cache must reproduce the same branch set as a fresh generate_repo",
        );
    }
}

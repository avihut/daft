//! Bounded, parallel directory-size walker shared by `daft list` and
//! `daft repo list`.
//!
//! Both commands need the on-disk size of one or more directory trees (each a
//! worktree or a cataloged repo). The walk is a pure `readdir` + `lstat`
//! syscall storm (`sys >> user`); on APFS/SSD it parallelises ~2x up to ~4-5
//! concurrent walkers and then plateaus, whether the concurrency comes from
//! many roots (cross-tree) or one deep root (intra-tree).
//!
//! Every walk runs through ONE bounded budget of `jobs` worker threads
//! (default [`resolve_jobs`] → `available_parallelism()`), fed by a shared work
//! queue of *directories* drawn from ALL roots at once. That single shared
//! budget is what makes the concurrency correct: total disk-metadata
//! concurrency never exceeds `jobs`, so N roots can't each spin up a full pool
//! and oversubscribe the device (the failure mode of the old
//! thread-per-worktree + single-threaded-walk design).
//!
//! Semantics match the historical single-threaded `compute_directory_size`
//! byte-for-byte: `symlink_metadata` per entry, recurse into real dirs, sum
//! file `len()`, dedup hard links by `(dev, ino)` when `nlink > 1`, skip
//! unreadable entries, never follow symlinks. Traversal order differs (a work
//! queue, not a strict DFS) but the sum does not.
//!
//! Built on the same `available_parallelism()` + `std::thread::scope`
//! bounded-worker idiom as [`crate::core::worktree::sync_dag`]; no new
//! dependency, no `unsafe`.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Condvar, Mutex};

/// Concurrency used when `available_parallelism()` can't be determined
/// (mirrors `sync_dag`/`executor::runner`).
const FALLBACK_JOBS: usize = 4;

/// Environment override for the walk's concurrency budget — highest precedence,
/// above `daft.list.sizeConcurrency` config and the `available_parallelism()`
/// default. Read here rather than in the settings loader so `DaftSettings`
/// stays a pure git-config projection.
const JOBS_ENV: &str = "DAFT_SIZE_WALK_JOBS";

/// Resolve the walk concurrency budget: `DAFT_SIZE_WALK_JOBS` env var, then the
/// caller-supplied config value (`daft.list.sizeConcurrency`), then
/// `available_parallelism()`. Always at least 1.
pub fn resolve_jobs(config: Option<usize>) -> usize {
    resolve_jobs_from(std::env::var(JOBS_ENV).ok().as_deref(), config)
}

/// Pure core of [`resolve_jobs`], with the env value passed in so precedence is
/// testable without touching (racy, shared-process) real environment state.
fn resolve_jobs_from(env: Option<&str>, config: Option<usize>) -> usize {
    env.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .or(config.filter(|&n| n >= 1))
        .unwrap_or_else(default_jobs)
        .max(1)
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(FALLBACK_JOBS)
}

/// Blocking walk: return the size of each root, indexed the same as `roots`.
/// Each entry is `Some(bytes)` (a missing/unreadable root reports `Some(0)`,
/// matching the historical walk). For the two blocking call sites.
pub fn walk_all(roots: &[PathBuf], cancel: Option<&AtomicBool>, jobs: usize) -> Vec<Option<u64>> {
    let mut out = vec![None; roots.len()];
    walk_streaming(roots, cancel, jobs, |idx, size| out[idx] = size);
    out
}

/// Streaming walk: invoke `on_complete(root_index, Some(bytes))` as each root's
/// total is finalised, so live tables can patch cells the moment a walk
/// finishes. `on_complete` runs single-threaded on the caller (never needs to
/// be `Sync`). For the two live call sites. Honors `cancel` cooperatively
/// between directories.
pub fn walk_streaming(
    roots: &[PathBuf],
    cancel: Option<&AtomicBool>,
    jobs: usize,
    mut on_complete: impl FnMut(usize, Option<u64>),
) {
    if roots.is_empty() {
        return;
    }
    let n = roots.len();
    let shared = Shared {
        queue: Mutex::new(Queue {
            jobs: roots
                .iter()
                .enumerate()
                .map(|(root, dir)| Job {
                    root,
                    dir: dir.clone(),
                })
                .collect(),
            active: 0,
            remaining: vec![1; n],
        }),
        cvar: Condvar::new(),
        totals: (0..n).map(|_| AtomicU64::new(0)).collect(),
        seen: (0..n).map(|_| Mutex::new(HashSet::new())).collect(),
    };
    let shared = &shared;
    let workers = jobs.max(1);
    let (tx, rx) = mpsc::channel::<(usize, Option<u64>)>();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            scope.spawn(move || worker(shared, cancel, tx));
        }
        // Drop the original sender so `rx` disconnects once every worker exits
        // (normal drain, or an early return on cancel).
        drop(tx);
        while let Ok((idx, size)) = rx.recv() {
            on_complete(idx, size);
        }
    });
}

struct Job {
    root: usize,
    dir: PathBuf,
}

struct Queue {
    jobs: VecDeque<Job>,
    /// Directories currently being scanned (popped but not yet finished).
    active: usize,
    /// Outstanding (queued or in-flight) directory count per root; the root's
    /// total is final when this reaches 0.
    remaining: Vec<usize>,
}

struct Shared {
    queue: Mutex<Queue>,
    cvar: Condvar,
    totals: Vec<AtomicU64>,
    /// Per-root hard-link dedup set, kept separate from `queue` so file-level
    /// dedup (only for `nlink > 1`) never contends with queue operations.
    seen: Vec<Mutex<HashSet<(u64, u64)>>>,
}

fn worker(shared: &Shared, cancel: Option<&AtomicBool>, tx: mpsc::Sender<(usize, Option<u64>)>) {
    loop {
        // Claim the next directory, or exit when the queue is drained.
        let job = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if is_cancelled(cancel) {
                    return;
                }
                if let Some(job) = q.jobs.pop_front() {
                    q.active += 1;
                    break job;
                }
                if q.active == 0 {
                    return; // nothing queued and nothing in flight — all done
                }
                q = shared.cvar.wait(q).unwrap();
            }
        };

        // Scan the directory OUTSIDE the lock — this is the syscall storm.
        let (bytes, subdirs) = scan_dir(&job.dir, &shared.seen[job.root]);
        shared.totals[job.root].fetch_add(bytes, Ordering::Relaxed);

        // Record this directory done, enqueue its children, detect completion.
        let finished_total = {
            let mut q = shared.queue.lock().unwrap();
            q.active -= 1;
            let added = subdirs.len();
            for dir in subdirs {
                q.jobs.push_back(Job {
                    root: job.root,
                    dir,
                });
            }
            // This dir (−1) plus its children (+added); remaining ≥ 1 here.
            q.remaining[job.root] = q.remaining[job.root] + added - 1;
            let done = q.remaining[job.root] == 0;
            shared.cvar.notify_all();
            // Read the total while holding the lock: the mutex release/acquire
            // edges make every other worker's Relaxed `fetch_add` for this root
            // visible once `remaining` has reached 0.
            done.then(|| shared.totals[job.root].load(Ordering::Relaxed))
        };

        if let Some(total) = finished_total {
            let _ = tx.send((job.root, Some(total)));
        }
    }
}

/// Scan one directory level: sum the sizes of its files (deduping hard links)
/// and return the immediate subdirectories to recurse into. Mirrors the old
/// `compute_directory_size` inner walk exactly — unreadable dirs/entries are
/// skipped (contributing 0), symlinks are never followed.
fn scan_dir(dir: &Path, seen: &Mutex<HashSet<(u64, u64)>>) -> (u64, Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, Vec::new());
    };
    let mut bytes = 0u64;
    let mut subdirs = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if meta.is_dir() {
            subdirs.push(entry.path());
        } else if count_file(&meta, seen) {
            bytes += meta.len();
        }
    }
    (bytes, subdirs)
}

/// Whether this file's bytes should be counted: always for `nlink <= 1`; for a
/// hard-linked file only the first `(dev, ino)` seen (matching `du`).
#[cfg(unix)]
fn count_file(meta: &std::fs::Metadata, seen: &Mutex<HashSet<(u64, u64)>>) -> bool {
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() > 1 {
        seen.lock().unwrap().insert((meta.dev(), meta.ino()))
    } else {
        true
    }
}

#[cfg(not(unix))]
fn count_file(_meta: &std::fs::Metadata, _seen: &Mutex<HashSet<(u64, u64)>>) -> bool {
    true
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Reference: the historical single-threaded recursive walk, kept verbatim
    /// as the differential-test oracle. `walk_all` must equal this byte-for-byte
    /// for every fixture and every `jobs` value.
    fn serial_size(path: &Path) -> u64 {
        fn walk(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
            let Ok(entries) = fs::read_dir(dir) else {
                return 0;
            };
            let mut total = 0u64;
            for entry in entries {
                let Ok(entry) = entry else { continue };
                let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                    continue;
                };
                if meta.is_dir() {
                    total += walk(&entry.path(), seen);
                } else {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::MetadataExt;
                        if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
                            continue;
                        }
                    }
                    total += meta.len();
                }
            }
            total
        }
        let mut seen = HashSet::new();
        walk(path, &mut seen)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A nested tree with files of varied sizes, an empty dir, and a nested
    /// subtree — the common case.
    fn build_tree(root: &Path) {
        write(&root.join("a.txt"), b"hello");
        write(&root.join("b.bin"), &vec![0u8; 4096]);
        write(&root.join("sub/c.txt"), b"nested content");
        write(&root.join("sub/deep/d.txt"), &vec![7u8; 1234]);
        fs::create_dir_all(root.join("sub/empty")).unwrap();
    }

    #[test]
    fn matches_serial_on_nested_tree_for_every_job_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("tree");
        build_tree(&root);
        let roots = vec![root.clone()];
        let expected = serial_size(&root);
        assert!(expected > 0);
        for jobs in [1usize, 2, 4, 8, 16] {
            assert_eq!(
                walk_all(&roots, None, jobs),
                vec![Some(expected)],
                "jobs={jobs}"
            );
        }
    }

    #[test]
    fn multiple_roots_are_indexed_independently() {
        let tmp = tempfile::tempdir().unwrap();
        let mut roots = Vec::new();
        let mut expected = Vec::new();
        for i in 0..5 {
            let root = tmp.path().join(format!("r{i}"));
            build_tree(&root);
            // vary each root so sizes differ
            write(&root.join("extra"), &vec![1u8; 100 * (i + 1)]);
            expected.push(Some(serial_size(&root)));
            roots.push(root);
        }
        assert_eq!(walk_all(&roots, None, 4), expected);
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_counted_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("hl");
        write(&root.join("real"), &vec![9u8; 2000]);
        fs::hard_link(root.join("real"), root.join("linkA")).unwrap();
        fs::hard_link(root.join("real"), root.join("linkB")).unwrap();
        // du-style: the 2000 bytes counted once, not thrice.
        let expected = serial_size(&root);
        for jobs in [1usize, 4] {
            assert_eq!(
                walk_all(std::slice::from_ref(&root), None, jobs),
                vec![Some(expected)]
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("sl");
        write(&root.join("target/big"), &vec![3u8; 9999]);
        std::os::unix::fs::symlink(root.join("target"), root.join("link_to_dir")).unwrap();
        // A dir symlink is a non-dir under symlink_metadata: its own tiny size
        // counts, the 9999-byte target is NOT recursed into twice.
        assert_eq!(
            walk_all(std::slice::from_ref(&root), None, 4),
            vec![Some(serial_size(&root))]
        );
    }

    #[test]
    fn missing_root_reports_some_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let gone = tmp.path().join("does-not-exist");
        assert_eq!(walk_all(&[gone], None, 4), vec![Some(0)]);
    }

    #[test]
    fn empty_roots_is_empty() {
        assert_eq!(walk_all(&[], None, 4), Vec::<Option<u64>>::new());
    }

    #[test]
    fn streaming_reports_each_root_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let mut roots = Vec::new();
        for i in 0..4 {
            let root = tmp.path().join(format!("s{i}"));
            build_tree(&root);
            roots.push(root);
        }
        let mut seen = vec![0usize; roots.len()];
        walk_streaming(&roots, None, 3, |idx, size| {
            seen[idx] += 1;
            assert!(size.is_some());
        });
        assert_eq!(seen, vec![1, 1, 1, 1]);
    }

    #[test]
    fn cancel_before_start_reports_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("c");
        build_tree(&root);
        let cancel = AtomicBool::new(true);
        let mut count = 0;
        walk_streaming(&[root], Some(&cancel), 4, |_, _| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn resolve_jobs_precedence() {
        // env wins when valid and >= 1.
        assert_eq!(resolve_jobs_from(Some("2"), Some(8)), 2);
        // invalid or zero env falls through to config.
        assert_eq!(resolve_jobs_from(Some("nope"), Some(8)), 8);
        assert_eq!(resolve_jobs_from(Some("0"), Some(8)), 8);
        // no env → config when >= 1.
        assert_eq!(resolve_jobs_from(None, Some(3)), 3);
        // config 0 / absent → positive default.
        assert_eq!(resolve_jobs_from(None, Some(0)), default_jobs().max(1));
        assert!(resolve_jobs_from(None, None) >= 1);
    }
}

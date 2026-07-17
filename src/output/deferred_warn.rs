//! Deferred `daft:` warning channel for live-region windows.
//!
//! A raw `eprintln!` while an inline live region owns stderr shreds the
//! render (#720): with the terminal in raw mode each bare `\n` staircases
//! instead of returning the carriage, long lines wrap and scroll the screen
//! underneath the renderer, and later cell-diff repaints land on rows that
//! have since moved — torn tables, duplicated frames, stray spinner glyphs.
//!
//! Best-effort degradation warnings that can fire from hook/executor worker
//! threads while a live region is on screen (the sync/list live tables
//! today) must go through [`warn`] instead of `eprintln!`:
//!
//! * terminal free → the line prints to stderr immediately, unchanged;
//! * a [`LiveRegionGuard`] alive → the line queues (exact duplicates
//!   collapse into a repeat count) and flushes to stderr when the outermost
//!   guard drops — after the region has closed and, for raw-mode windows,
//!   after cooked mode is restored.
//!
//! [`enable_raw_mode_guard`](crate::output::tui::enable_raw_mode_guard)
//! holds a guard for the whole raw-mode window, so the inline-TUI commands
//! get deferral without per-site wiring. The indicatif-region renderers
//! (see `output::term_guard`) share the foreign-bytes hazard and can adopt
//! a guard the same way when needed.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Global queue + live-region depth. One mutex is fine: warnings are rare
/// (degradation paths only) and the flush runs at most once per command.
static STATE: Mutex<WarnQueue> = Mutex::new(WarnQueue::new());

/// Emit a warning line on stderr — immediately when no live region is
/// active, deferred until the region closes otherwise.
pub fn warn(msg: impl Into<String>) {
    if let Some(line) = lock().warn(msg.into()) {
        eprintln!("{line}");
    }
}

/// Marks a live region as owning stderr for its lifetime. Warnings queue
/// while at least one guard is alive; dropping the guard that closes the
/// outermost region flushes them.
#[must_use = "warnings defer only while the guard is alive"]
pub struct LiveRegionGuard(());

pub fn live_region_guard() -> LiveRegionGuard {
    lock().enter();
    LiveRegionGuard(())
}

impl Drop for LiveRegionGuard {
    fn drop(&mut self) {
        for line in lock().exit() {
            eprintln!("{line}");
        }
    }
}

fn lock() -> MutexGuard<'static, WarnQueue> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Pure queue logic, kept separate from the global so tests don't contend
/// over process-wide state.
struct WarnQueue {
    depth: usize,
    entries: Vec<(String, usize)>,
}

impl WarnQueue {
    const fn new() -> Self {
        Self {
            depth: 0,
            entries: Vec::new(),
        }
    }

    /// Returns the line to print immediately, or `None` when it was queued.
    fn warn(&mut self, msg: String) -> Option<String> {
        if self.depth == 0 {
            return Some(msg);
        }
        match self.entries.iter_mut().find(|(seen, _)| *seen == msg) {
            Some((_, count)) => *count += 1,
            None => self.entries.push((msg, 1)),
        }
        None
    }

    fn enter(&mut self) {
        self.depth += 1;
    }

    /// Returns the queued lines to flush — non-empty only when this exit
    /// closes the outermost region.
    fn exit(&mut self) -> Vec<String> {
        self.depth = self.depth.saturating_sub(1);
        if self.depth > 0 {
            return Vec::new();
        }
        self.entries
            .drain(..)
            .map(|(msg, count)| {
                if count > 1 {
                    format!("{msg} (\u{00D7}{count})")
                } else {
                    msg
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_immediately_outside_live_region() {
        let mut q = WarnQueue::new();
        assert_eq!(q.warn("daft: boom".into()), Some("daft: boom".into()));
        assert!(q.exit().is_empty(), "nothing was queued");
    }

    #[test]
    fn defers_and_flushes_in_first_seen_order() {
        let mut q = WarnQueue::new();
        q.enter();
        assert_eq!(q.warn("a".into()), None);
        assert_eq!(q.warn("b".into()), None);
        assert_eq!(q.exit(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn duplicates_collapse_into_a_repeat_count() {
        // The #720 shape: one wedged store, the same three warnings fired
        // once per pruned worktree — nine foreign lines mid-render.
        let mut q = WarnQueue::new();
        q.enter();
        for _ in 0..3 {
            q.warn("daft: failed to open coordinator store".into());
        }
        q.warn("daft: something else".into());
        assert_eq!(
            q.exit(),
            vec![
                "daft: failed to open coordinator store (\u{00D7}3)".to_string(),
                "daft: something else".to_string(),
            ]
        );
    }

    #[test]
    fn nested_regions_flush_only_at_the_outermost_exit() {
        let mut q = WarnQueue::new();
        q.enter(); // raw-mode window
        q.enter(); // TuiRenderer::run inside it
        q.warn("late".into());
        assert!(q.exit().is_empty(), "inner exit must not flush");
        assert_eq!(q.exit(), vec!["late".to_string()]);
    }

    #[test]
    fn queue_resets_after_flush() {
        let mut q = WarnQueue::new();
        q.enter();
        q.warn("first window".into());
        assert_eq!(q.exit().len(), 1);
        // Back to immediate printing, and no stale entries on re-entry.
        assert_eq!(q.warn("now".into()), Some("now".into()));
        q.enter();
        assert_eq!(q.exit(), Vec::<String>::new());
    }

    #[test]
    fn unbalanced_exit_saturates() {
        let mut q = WarnQueue::new();
        assert!(q.exit().is_empty());
        assert_eq!(q.warn("still fine".into()), Some("still fine".into()));
    }
}

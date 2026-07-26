//! One seam for out-of-band notices that must survive a live region.
//!
//! Core code (`core/worktree/merge.rs`, `governor/lane.rs`) needs to tell the
//! user things while it works — "waiting for the merge gate lane", "--no-ff-only
//! overrides committed merge.ff=only", the octopus announcement. Those sites
//! have no `&mut dyn Output` (threading one into core is a long-standing TODO)
//! so they wrote straight to stderr with `eprintln!`.
//!
//! That was fine until the merge rail: an indicatif `MultiProgress` region owns
//! the terminal for exactly the merges that have hooks, which is exactly the
//! condition under which the gate lane can block. So the one line explaining
//! why the merge appears frozen was written underneath the region and erased by
//! its next repaint — the user saw pending rows and no explanation, on what
//! looked like a hang.
//!
//! The command layer installs a sink that routes through the region's
//! `suspend`; everything else keeps the plain `eprintln!` default. This is
//! deliberately the narrowest possible seam — a `Fn(&str)`, not an `Output`
//! — because the real fix is threading `Output` into core, and a wide
//! interim API would compete with it.

use std::sync::OnceLock;
use std::sync::RwLock;

type Sink = Box<dyn Fn(&str) + Send + Sync>;

fn slot() -> &'static RwLock<Option<Sink>> {
    static SLOT: OnceLock<RwLock<Option<Sink>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Route notices through `sink` until [`clear_sink`] is called.
///
/// The caller owns the lifetime: install when a live region starts, clear
/// when it finishes, so a later notice on a torn-down region does not try to
/// suspend something that no longer exists.
pub fn set_sink(sink: Sink) {
    if let Ok(mut guard) = slot().write() {
        *guard = Some(sink);
    }
}

/// Drop any installed sink; notices go back to stderr.
pub fn clear_sink() {
    if let Ok(mut guard) = slot().write() {
        *guard = None;
    }
}

/// Emit one out-of-band line to the user.
pub fn notice(msg: &str) {
    if let Ok(guard) = slot().read()
        && let Some(sink) = guard.as_ref()
    {
        sink(msg);
        return;
    }
    eprintln!("{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn notices_route_through_an_installed_sink_and_revert_on_clear() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        set_sink(Box::new(move |m| {
            sink_seen.lock().unwrap().push(m.to_string());
        }));

        notice("through the region");
        assert_eq!(seen.lock().unwrap().as_slice(), ["through the region"]);

        clear_sink();
        // Back to stderr — nothing further reaches the recorder.
        notice("straight to stderr");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

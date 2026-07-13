//! Process-global SIGINT dispatcher with a swappable behavior slot.
//!
//! `ctrlc::set_handler` is once-per-process, so components that need
//! interrupt behavior (single-key prompts, the plan-execute timeline's
//! region collapse) must not race each other with competing `set_handler`
//! calls — the loser's registration fails silently and Ctrl-C runs whichever
//! behavior happened to install first (the `prompt.rs` exit(0) handler
//! outliving its prompt was the canonical hazard). Instead, the real handler
//! is installed once and dispatches to a replaceable slot.
//!
//! Semantics:
//! - [`set_behavior`] installs the current interrupt behavior (replacing any
//!   previous one); [`clear_behavior`] removes it.
//! - The behavior is **one-shot**: the handler `take`s it before running, so
//!   a second Ctrl-C while the first is still unwinding falls through to the
//!   default hard exit — the force-quit escape hatch.
//! - With no behavior installed, Ctrl-C exits with the conventional
//!   `130` (128 + SIGINT).
//!
//! `daft exec` renders the plan-execute timeline (which arms the region's own
//! collapse behavior here), so it routes its two-stage Ctrl-C through this
//! slot too: after the plan commits it overrides the collapse with a
//! self-re-arming escalation of its cancel flag (a behavior may deliberately
//! return instead of exiting, and re-arm via [`set_behavior`] — the dispatcher
//! releases the slot lock before running it). `daft list --live` and
//! `daft sync` keep their own pre-existing `ctrlc::set_handler` registrations
//! (cooperative cancellation flags) and never render a timeline, so the
//! dispatcher and those handlers never manage the same invocation.

use std::sync::{Mutex, OnceLock};

type Behavior = Box<dyn FnOnce() + Send + 'static>;

static SLOT: Mutex<Option<Behavior>> = Mutex::new(None);
static INSTALLED: OnceLock<()> = OnceLock::new();

fn ensure_installed() {
    INSTALLED.get_or_init(|| {
        // Best-effort: if another component already owns the process handler
        // (daft exec's cancel flag), keep today's behavior — the slot simply
        // never fires.
        let _ = ctrlc::set_handler(|| {
            let behavior = SLOT.lock().ok().and_then(|mut slot| slot.take());
            match behavior {
                Some(behavior) => behavior(),
                None => std::process::exit(130),
            }
        });
    });
}

/// A previously installed behavior, saved by [`swap_behavior`] so a nested
/// phase can put it back. Opaque: the only thing to do with one is hand it
/// to [`restore_behavior`].
pub struct SavedBehavior(Option<Behavior>);

/// Install the interrupt behavior for the current phase of the command.
/// The behavior runs on the signal-handler thread and is expected to exit
/// the process (or deliberately return to let execution continue).
pub fn set_behavior(behavior: impl FnOnce() + Send + 'static) {
    let _ = swap_behavior(behavior);
}

/// Install `behavior` and hand back whatever was installed before. A nested
/// phase (a single-key prompt firing under the timeline's live region)
/// restores the outer behavior with [`restore_behavior`] when it resolves —
/// clearing the slot instead would strand the region without its Ctrl-C
/// collapse.
pub fn swap_behavior(behavior: impl FnOnce() + Send + 'static) -> SavedBehavior {
    ensure_installed();
    match SLOT.lock() {
        Ok(mut slot) => SavedBehavior(slot.replace(Box::new(behavior))),
        Err(_) => SavedBehavior(None),
    }
}

/// Reinstate a behavior saved by [`swap_behavior`] (an empty save restores
/// the default hard exit).
pub fn restore_behavior(saved: SavedBehavior) {
    if let Ok(mut slot) = SLOT.lock() {
        *slot = saved.0;
    }
}

/// Take the installed behavior out of the slot, exactly as the dispatcher
/// would — the caller becomes responsible for running it.
///
/// For code that learns of the interrupt in-band: `console`'s `read_key`
/// re-raises SIGINT and *then* returns `ErrorKind::Interrupted`, so the
/// main thread and the dispatcher thread both know. Racing the dispatcher
/// with [`restore_behavior`] would hand the outer behavior (or the bare
/// default) the exit that belongs to the current phase; taking the slot
/// makes exactly one of the two run it — `None` means the dispatcher won
/// and the caller should simply wait for the process to die.
pub fn take_behavior() -> Option<Box<dyn FnOnce() + Send + 'static>> {
    SLOT.lock().ok().and_then(|mut slot| slot.take())
}

/// Remove the installed behavior (Ctrl-C reverts to the default hard exit).
pub fn clear_behavior() {
    if let Ok(mut slot) = SLOT.lock() {
        *slot = None;
    }
}

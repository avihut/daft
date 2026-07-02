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
//! `daft exec` and `daft list --live` keep their own pre-existing
//! `ctrlc::set_handler` registrations (cooperative cancellation flags).
//! Within one process the registrations race exactly as they always have;
//! neither command renders a timeline, so the dispatcher and those handlers
//! never manage the same invocation.

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

/// Install the interrupt behavior for the current phase of the command.
/// The behavior runs on the signal-handler thread and is expected to exit
/// the process (or deliberately return to let execution continue).
pub fn set_behavior(behavior: impl FnOnce() + Send + 'static) {
    ensure_installed();
    if let Ok(mut slot) = SLOT.lock() {
        *slot = Some(Box::new(behavior));
    }
}

/// Remove the installed behavior (Ctrl-C reverts to the default hard exit).
pub fn clear_behavior() {
    if let Ok(mut slot) = SLOT.lock() {
        *slot = None;
    }
}

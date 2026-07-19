//! Terminal-driver guard shared by indicatif-region renderers.
//!
//! Indicatif tracks the viewport by counting its own drawn lines; any bytes
//! that reach the terminal behind its back desync that counter. The classic
//! offender is the TTY driver echoing `^C` on Ctrl-C — two characters that
//! can wrap the cursor to a fresh line and shift every subsequent
//! cursor-up/erase by one, stranding a stale bar line in scrollback.
//! Disabling `ECHOCTL` for the region's lifetime suppresses the echo.
//!
//! When the rail also reads keys (#729's live `v` toggle) the guard
//! escalates: `ICANON` and `ECHO` come off too, so a keypress arrives
//! without Enter and without printing itself into the live region.
//!
//! **`ISIG` always stays on.** That is what separates this from crossterm's
//! all-or-nothing raw mode: Ctrl-C keeps raising a real `SIGINT`, so the
//! interrupt dispatcher — and `daft exec`'s two-stage SIGTERM/SIGKILL
//! escalation (#663) — are untouched. The corollary is that `^C` is consumed
//! by the line discipline and never reaches a reader as a byte; a key
//! listener neither sees nor needs to handle it.
//!
//! Used by the multi-worktree exec renderer and the plan-execute timeline.

/// The termios saved by the currently live guard, for interrupt-exit paths
/// that cannot reach the guard itself: a single-key prompt's cancel exit can
/// fire while the timeline's region (and its guard) is live, several frames
/// away. One slot suffices — regions never nest.
#[cfg(unix)]
static ACTIVE_ORIGINAL: std::sync::Mutex<Option<nix::sys::termios::Termios>> =
    std::sync::Mutex::new(None);

/// Temporarily disable the terminal driver's `^C` echo on stderr.
///
/// The original termios is restored on drop. Interrupt handlers that exit
/// the process (bypassing drop) should restore via [`EchoCtlGuard::saved`]
/// — or, where the guard is out of reach, [`restore_active_termios`].
#[cfg(unix)]
pub(crate) struct EchoCtlGuard {
    /// Held by-value so the guard owns a valid fd for its lifetime; the
    /// `BorrowedFd` we hand to `tcsetattr` borrows from it.
    stderr: std::io::Stderr,
    original: Option<nix::sys::termios::Termios>,
}

#[cfg(unix)]
impl EchoCtlGuard {
    pub(crate) fn new() -> Self {
        use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
        use std::io::IsTerminal;
        use std::os::fd::AsFd;

        let stderr = std::io::stderr();
        if !stderr.is_terminal() {
            return Self {
                stderr,
                original: None,
            };
        }

        let Ok(current) = tcgetattr(stderr.as_fd()) else {
            return Self {
                stderr,
                original: None,
            };
        };

        let mut modified = current.clone();
        modified.local_flags.remove(LocalFlags::ECHOCTL);
        if tcsetattr(stderr.as_fd(), SetArg::TCSANOW, &modified).is_err() {
            return Self {
                stderr,
                original: None,
            };
        }
        if let Ok(mut active) = ACTIVE_ORIGINAL.lock() {
            *active = Some(current.clone());
        }
        Self {
            stderr,
            original: Some(current),
        }
    }

    /// The termios to restore, for interrupt paths that exit the process
    /// without running drops.
    pub(crate) fn saved(&self) -> Option<nix::sys::termios::Termios> {
        self.original.clone()
    }
}

/// The live key listener's pause flag, when one is watching the terminal.
/// `Some` also means the driver is (or should be) in unbuffered mode. One
/// slot — regions never nest, and only a region reads keys.
#[cfg(unix)]
static KEY_LISTENER: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
    std::sync::Mutex::new(None);

/// Re-apply the region's baseline input discipline: the live guard's
/// original with only `ECHOCTL` cleared. Cooked, line-edited, echoing.
#[cfg(unix)]
fn apply_region_baseline() {
    use nix::sys::termios::{LocalFlags, SetArg, tcgetattr, tcsetattr};
    use std::os::fd::AsFd;

    let Ok(active) = ACTIVE_ORIGINAL.lock() else {
        return;
    };
    let Some(original) = active.as_ref() else {
        return;
    };
    let stderr = std::io::stderr();
    let Ok(mut attrs) = tcgetattr(stderr.as_fd()) else {
        return;
    };
    // Take the original's whole input discipline back — canonical flags,
    // `VMIN`/`VTIME` included — then re-apply the region's own edit.
    attrs.local_flags = original.local_flags;
    attrs.control_chars = original.control_chars;
    attrs.local_flags.remove(LocalFlags::ECHOCTL);
    let _ = tcsetattr(stderr.as_fd(), SetArg::TCSANOW, &attrs);
}

/// Escalate to unbuffered, unechoed key input on top of the baseline:
/// `ICANON` and `ECHO` off, `VMIN=1`/`VTIME=0`. `ISIG` is deliberately left
/// alone — see the module docs.
///
/// `VMIN`/`VTIME` are not optional: with `ICANON` off they alone decide what
/// a read does, and a terminal left at `VMIN=0` by a previous program turns
/// every read into an instant zero-byte return — which a naive listener
/// reads as EOF, or spins on.
#[cfg(unix)]
fn apply_key_input() -> bool {
    use nix::sys::termios::{LocalFlags, SetArg, SpecialCharacterIndices, tcgetattr, tcsetattr};
    use std::os::fd::AsFd;

    let stderr = std::io::stderr();
    let Ok(mut attrs) = tcgetattr(stderr.as_fd()) else {
        return false;
    };
    attrs
        .local_flags
        .remove(LocalFlags::ICANON | LocalFlags::ECHO);
    attrs.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
    attrs.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
    tcsetattr(stderr.as_fd(), SetArg::TCSANOW, &attrs).is_ok()
}

/// Start reading keys unbuffered: escalate the driver and register
/// `paused` so prompts can stand the listener down. Returns whether the
/// escalation took (no live guard, or a non-TTY stderr, means no).
///
/// Called when a listener actually starts — never at region construction.
/// Between the two lies the planning face, where prompts run and no reader
/// exists; escalating there would swallow keystrokes invisibly and hand
/// them to a listener minutes later.
#[cfg(unix)]
pub(crate) fn begin_key_input(paused: std::sync::Arc<std::sync::atomic::AtomicBool>) -> bool {
    if ACTIVE_ORIGINAL.lock().is_ok_and(|g| g.is_none()) {
        return false;
    }
    install_panic_restore();
    if !apply_key_input() {
        return false;
    }
    if let Ok(mut slot) = KEY_LISTENER.lock() {
        *slot = Some(paused);
    }
    true
}

/// Restore the terminal if the process panics while key input is escalated.
///
/// `Drop` is not enough: release builds are `panic = "abort"`, where no
/// destructor runs — the terminal would be left unable to echo the user's
/// typing, which is a far worse failure than the panic itself. Panic hooks
/// *do* run before the abort.
///
/// Installed once and never removed: it chains the previous hook, and
/// [`restore_active_termios`] is a no-op when no guard is live, so leaving
/// it in place costs nothing.
#[cfg(unix)]
fn install_panic_restore() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_active_termios();
            previous(info);
        }));
    });
}

/// Stop reading keys: back to the region's cooked baseline.
#[cfg(unix)]
pub(crate) fn end_key_input() {
    if let Ok(mut slot) = KEY_LISTENER.lock() {
        *slot = None;
    }
    apply_region_baseline();
}

/// Hand the terminal back to a blocking reader — a prompt, or an
/// interactive hook job with inherited stdin — for as long as the returned
/// guard lives.
///
/// Without this the child would run with `ECHO`/`ICANON` off (no visible
/// typing, no line editing) while the rail's listener raced it for the same
/// device. A no-op when nothing is listening, which is every pre-#729 path.
#[cfg(unix)]
pub(crate) fn suspend_key_input() -> KeyInputSuspension {
    let paused = KEY_LISTENER.lock().ok().and_then(|slot| slot.clone());
    if let Some(flag) = &paused {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        apply_region_baseline();
    }
    KeyInputSuspension { paused }
}

/// Restores unbuffered key input when it drops. See [`suspend_key_input`].
#[cfg(unix)]
pub(crate) struct KeyInputSuspension {
    paused: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

#[cfg(unix)]
impl Drop for KeyInputSuspension {
    fn drop(&mut self) {
        if let Some(flag) = &self.paused {
            apply_key_input();
            flag.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(unix)]
impl Drop for EchoCtlGuard {
    fn drop(&mut self) {
        use std::os::fd::AsFd;
        if let Some(original) = self.original.as_ref() {
            if let Ok(mut active) = ACTIVE_ORIGINAL.lock() {
                *active = None;
            }
            // Best-effort restore; if the fd is gone (process torn down),
            // there's nothing useful we can do.
            let _ = nix::sys::termios::tcsetattr(
                self.stderr.as_fd(),
                nix::sys::termios::SetArg::TCSANOW,
                original,
            );
        }
    }
}

/// Restore a termios saved by [`EchoCtlGuard::saved`] (interrupt-exit path).
#[cfg(unix)]
pub(crate) fn restore_termios(saved: &Option<nix::sys::termios::Termios>) {
    use std::os::fd::AsFd;
    if let Some(original) = saved {
        let stderr = std::io::stderr();
        let _ = nix::sys::termios::tcsetattr(
            stderr.as_fd(),
            nix::sys::termios::SetArg::TCSANOW,
            original,
        );
    }
}

/// Restore the termios saved by the currently live guard, if any — the
/// interrupt-exit path for code that never held the guard (the prompt's
/// cancel exit under a live region). No-op when no guard is active.
#[cfg(unix)]
pub(crate) fn restore_active_termios() {
    if let Ok(active) = ACTIVE_ORIGINAL.lock() {
        restore_termios(&active);
    }
}

#[cfg(not(unix))]
pub(crate) struct EchoCtlGuard;

#[cfg(not(unix))]
impl EchoCtlGuard {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn saved(&self) -> Option<()> {
        None
    }
}

#[cfg(not(unix))]
pub(crate) fn restore_termios(_saved: &Option<()>) {}

#[cfg(not(unix))]
pub(crate) fn restore_active_termios() {}

#[cfg(not(unix))]
pub(crate) fn begin_key_input(_paused: std::sync::Arc<std::sync::atomic::AtomicBool>) -> bool {
    false
}

#[cfg(not(unix))]
pub(crate) fn end_key_input() {}

#[cfg(not(unix))]
pub(crate) fn suspend_key_input() -> KeyInputSuspension {
    KeyInputSuspension
}

#[cfg(not(unix))]
pub(crate) struct KeyInputSuspension;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    #[serial]
    fn a_suspension_stands_the_listener_down_and_brings_it_back() {
        let flag = Arc::new(AtomicBool::new(false));
        *KEY_LISTENER.lock().unwrap() = Some(Arc::clone(&flag));
        {
            let _prompt = suspend_key_input();
            assert!(
                flag.load(Ordering::SeqCst),
                "a prompt must stop the listener reading the keys meant for it"
            );
        }
        assert!(
            !flag.load(Ordering::SeqCst),
            "the listener resumes once the prompt is done"
        );
        *KEY_LISTENER.lock().unwrap() = None;
    }

    #[test]
    #[serial]
    fn suspending_with_no_listener_is_inert() {
        // Every pre-#729 prompt: nothing registered, so nothing is paused
        // and the terminal is never touched.
        *KEY_LISTENER.lock().unwrap() = None;
        let suspension = suspend_key_input();
        assert!(suspension.paused.is_none());
    }
}

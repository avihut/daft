//! The rail's live key reader (#729): `v` toggles verbose mid-run.
//!
//! The plan-execute rail is an indicatif *output* region — it had no input
//! path at all, and the `KeyCode`/raw-mode machinery under `output::tui`
//! belongs to a different renderer family (full raw mode, alternate screen,
//! `ISIG` off) that would break both indicatif's drawing and `daft exec`'s
//! two-stage Ctrl-C. So this is deliberately small: one thread, one key, one
//! `/dev/tty` handle, and a partial termios change owned by
//! [`crate::output::term_guard`].
//!
//! Keys are read from a **dup of stderr**, not from stdin and not from
//! `/dev/tty`. Stderr is the device the rail draws on — the whole renderer
//! is gated on `stderr.is_terminal()` — and the one
//! [`crate::output::term_guard`] configures, so reading it is the only
//! choice that cannot end up talking to a different terminal than the user
//! is watching. Stdin is wrong because it may be a pipe or `/dev/null` while
//! the rail still renders; `/dev/tty` is wrong because it resolves through
//! the *controlling* terminal, which a process can lack entirely (a pty with
//! no `setsid`, a container) or which can differ from stderr's device.
//!
//! The one cost is that a write-only stderr cannot be read. Terminals are
//! opened read-write in every normal shell, and if a read does fail the
//! listener simply stops — the rail then renders at its flag-given density,
//! exactly as it did before #729.
//!
//! `^C` never arrives here. `ISIG` stays on, so the line discipline turns it
//! into a `SIGINT` before any reader sees a byte — the interrupt dispatcher
//! and #663's escalation are untouched.

use super::TimelineHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long a read waits before re-checking the stop and pause flags. Also
/// the worst-case delay teardown waits on when it joins.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// A running key listener. Dropping it stops the thread, restores the
/// terminal, and joins — in that order.
pub(super) struct KeyListener {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl KeyListener {
    /// Start watching the rail's terminal for `v`. `None` when it cannot be
    /// read or the driver refuses unbuffered mode, in which case the rail
    /// renders at its flag-given density — exactly as it did before #729.
    #[cfg(unix)]
    pub(super) fn spawn(handle: TimelineHandle) -> Option<Self> {
        use std::io::IsTerminal;
        use std::os::fd::AsFd;

        let stderr = std::io::stderr();
        if !stderr.is_terminal() {
            return None;
        }
        // A dup, so the listener's fd lifetime is its own and closing it
        // never disturbs the region's writes to fd 2.
        let tty = stderr.as_fd().try_clone_to_owned().ok()?;
        let paused = Arc::new(AtomicBool::new(false));
        if !crate::output::term_guard::begin_key_input(Arc::clone(&paused)) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let stop = Arc::clone(&stop);
            let paused = Arc::clone(&paused);
            move || read_keys(&tty, &handle, &stop, &paused)
        });
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }

    #[cfg(not(unix))]
    pub(super) fn spawn(_handle: TimelineHandle) -> Option<Self> {
        None
    }

    /// Stop the thread and hand the terminal back. Idempotent.
    ///
    /// The join must happen with the timeline's mutex *released*: the
    /// listener may be blocked on that very lock inside `toggle_verbose`, so
    /// joining while holding it would deadlock teardown.
    pub(super) fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        crate::output::term_guard::end_key_input();
    }
}

impl Drop for KeyListener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Poll the terminal for single keypresses until told to stop.
#[cfg(unix)]
fn read_keys(
    tty: &std::os::fd::OwnedFd,
    handle: &TimelineHandle,
    stop: &AtomicBool,
    paused: &AtomicBool,
) {
    use nix::poll::{PollFd, PollFlags, PollTimeout};
    use std::os::fd::AsFd;

    while !stop.load(Ordering::SeqCst) {
        // Paused: do not even poll. The byte stays queued in the terminal
        // for whoever owns it now (a prompt, an interactive hook job) —
        // reading it here would steal their input.
        if paused.load(Ordering::SeqCst) {
            std::thread::sleep(POLL_INTERVAL);
            continue;
        }
        let mut fds = [PollFd::new(tty.as_fd(), PollFlags::POLLIN)];
        let timeout = PollTimeout::try_from(POLL_INTERVAL).unwrap_or(PollTimeout::NONE);
        match nix::poll::poll(&mut fds, timeout) {
            Ok(0) => continue,
            Ok(_) => {}
            // EINTR is routine here: every `^C` interrupts the poll.
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
        // Re-check after the wait: a prompt may have claimed the terminal
        // while we were polling. The narrow race left — pausing between
        // this check and the read — can only consume a byte typed *before*
        // the prompt appeared, which was never aimed at it.
        if paused.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
            continue;
        }
        let mut buf = [0u8; 1];
        match nix::unistd::read(tty.as_fd(), &mut buf) {
            Ok(1) => {
                if buf[0] == b'v' || buf[0] == b'V' {
                    handle.toggle_verbose();
                }
                // Every other key is swallowed. With `ECHO` off it would
                // not have printed anyway, and the rail is deliberately not
                // growing a second keymap.
            }
            // A zero-byte read means the terminal went away.
            Ok(_) => break,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(_) => break,
        }
    }
}

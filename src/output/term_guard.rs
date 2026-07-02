//! Terminal-driver guard shared by indicatif-region renderers.
//!
//! Indicatif tracks the viewport by counting its own drawn lines; any bytes
//! that reach the terminal behind its back desync that counter. The classic
//! offender is the TTY driver echoing `^C` on Ctrl-C — two characters that
//! can wrap the cursor to a fresh line and shift every subsequent
//! cursor-up/erase by one, stranding a stale bar line in scrollback.
//! Disabling `ECHOCTL` for the region's lifetime suppresses the echo.
//!
//! Used by the multi-worktree exec renderer and the plan-execute timeline.

/// Temporarily disable the terminal driver's `^C` echo on stderr.
///
/// The original termios is restored on drop. Interrupt handlers that exit
/// the process (bypassing drop) should restore via [`EchoCtlGuard::saved`].
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

#[cfg(unix)]
impl Drop for EchoCtlGuard {
    fn drop(&mut self) {
        use std::os::fd::AsFd;
        if let Some(original) = self.original.as_ref() {
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

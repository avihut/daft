//! POSIX jobserver export for concurrent pre-push hooks (#678 stage 4).
//!
//! The governor bounds how many hook *units* run; this bounds what the
//! units' toolchains do inside. Exporting one shared fifo-style jobserver
//! (`MAKEFLAGS="-jN --jobserver-auth=fifo:<path>"`) into every `git push`
//! environment makes cooperating build tools — GNU make 4.4+, cargo,
//! ninja 1.13+ — draw their inner parallelism from a single machine-wide
//! token pool instead of each assuming it owns all cores, collapsing the
//! `units × cores` thread multiplication for the common cargo/make hooks.
//!
//! Two deliberate approximations, both documented user-facing:
//! - a hook running bare `make` picks the `-jN` up from `MAKEFLAGS` and
//!   becomes parallel (bounded); `daft.governor.jobserver off` exists for
//!   hooks that cannot tolerate that;
//! - non-cooperating tools ignore the pool entirely — that is what the
//!   governor's admission/containment tiers are for.

use std::os::fd::OwnedFd;
use std::path::PathBuf;

/// A fifo-style jobserver living for one sync push phase.
///
/// The fifo is pre-filled with `slots - 1` tokens (make convention: every
/// client owns one implicit job slot and reads tokens only for *extra*
/// jobs). Both ends stay open for the jobserver's lifetime — the reader
/// so client opens never block, the writer so returned tokens are never
/// lost to an empty-reader EOF. The backing tempdir (0700) is removed on
/// drop.
pub struct PushJobserver {
    _dir: tempfile::TempDir,
    fifo: PathBuf,
    slots: usize,
    _reader: OwnedFd,
    _writer: OwnedFd,
}

impl PushJobserver {
    /// Create a jobserver advertising `slots` jobs. `None` on any failure —
    /// the export is an optimization, never a reason to fail a push.
    pub fn create(slots: usize) -> Option<Self> {
        let slots = slots.max(1);
        let dir = tempfile::Builder::new()
            .prefix("daft-jobserver-")
            .tempdir()
            .ok()?;
        let fifo = dir.path().join("jobserver");

        use nix::fcntl::{OFlag, open};
        use nix::sys::stat::Mode;
        nix::unistd::mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).ok()?;
        // Order matters: a blocking O_RDONLY open would wait for a writer,
        // so the read end opens O_NONBLOCK first; the write end can then
        // open normally.
        let reader = open(&fifo, OFlag::O_RDONLY | OFlag::O_NONBLOCK, Mode::empty()).ok()?;
        let writer = open(&fifo, OFlag::O_WRONLY, Mode::empty()).ok()?;

        let tokens = vec![b'+'; slots.saturating_sub(1)];
        if !tokens.is_empty() {
            nix::unistd::write(&writer, &tokens).ok()?;
        }

        Some(Self {
            _dir: dir,
            fifo,
            slots,
            _reader: reader,
            _writer: writer,
        })
    }

    /// The environment pair to inject into each `git push` (the hook
    /// inherits git's environment).
    pub fn env(&self) -> (String, String) {
        (
            "MAKEFLAGS".to_string(),
            format!(
                "-j{} --jobserver-auth=fifo:{}",
                self.slots,
                self.fifo.display()
            ),
        )
    }

    #[cfg(test)]
    fn read_token_nonblocking(&self) -> Option<u8> {
        let mut buf = [0u8; 1];
        match nix::unistd::read(&self._reader, &mut buf) {
            Ok(1) => Some(buf[0]),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_fifo_with_n_minus_one_tokens_and_env() {
        let server = PushJobserver::create(4).expect("jobserver");
        let (key, value) = server.env();
        assert_eq!(key, "MAKEFLAGS");
        assert!(value.starts_with("-j4 --jobserver-auth=fifo:"), "{value}");
        let fifo = PathBuf::from(value.rsplit_once("fifo:").unwrap().1);
        assert!(fifo.exists(), "fifo must exist while the server lives");

        // Exactly slots−1 tokens are drawable; the pool then runs dry
        // (non-blocking read yields nothing) instead of blocking.
        for n in 0..3 {
            assert!(
                server.read_token_nonblocking().is_some(),
                "token {n} must be available"
            );
        }
        assert!(
            server.read_token_nonblocking().is_none(),
            "pool must be dry"
        );

        // A returned token is drawable again (write end stays open).
        nix::unistd::write(&server._writer, b"+").unwrap();
        assert!(server.read_token_nonblocking().is_some());

        let dir = fifo.parent().unwrap().to_path_buf();
        drop(server);
        assert!(!dir.exists(), "tempdir must be removed on drop");
    }

    #[test]
    fn single_slot_pool_has_no_tokens_but_valid_env() {
        let server = PushJobserver::create(1).expect("jobserver");
        assert!(server.env().1.starts_with("-j1 "));
        assert!(server.read_token_nonblocking().is_none());
    }
}

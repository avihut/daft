//! Single-line braille-dot spinner drawn directly to `/dev/tty`.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(crate) fn frame_for_tick(tick: usize) -> &'static str {
    FRAMES[tick % FRAMES.len()]
}

pub struct Spinner {
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn start(label: &str) -> Option<Self> {
        let mut tty = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/tty")
            .ok()?;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let flag = stop_flag.clone();
        let label = label.to_string();
        let thread = thread::spawn(move || {
            let mut tick = 0usize;
            while !flag.load(Ordering::Relaxed) {
                let frame = frame_for_tick(tick);
                let _ = write!(tty, "\r{frame} {label}");
                let _ = tty.flush();
                tick += 1;
                thread::sleep(Duration::from_millis(100));
            }
            let _ = write!(tty, "\r\x1b[K");
            let _ = tty.flush();
        });
        Some(Self {
            stop_flag,
            thread: Some(thread),
        })
    }

    pub fn stop(mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_frames_cycle_through_braille_dots() {
        let frames: Vec<&str> = (0..20).map(frame_for_tick).collect();
        assert_eq!(frames[0], "⠋");
        assert_eq!(frames[1], "⠙");
        assert_eq!(frames[9], "⠏");
        assert_eq!(frames[10], "⠋", "frame sequence must wrap at 10");
        assert_eq!(frames[19], "⠏");
    }
}

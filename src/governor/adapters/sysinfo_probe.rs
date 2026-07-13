//! [`ResourceProbe`] adapters.
//!
//! [`SysinfoProbe`] is the production adapter (sysinfo's safe public API —
//! `forbid(unsafe_code)` stays intact), upgraded with Linux memory PSI read
//! via plain `std::fs`. [`ForcedProbe`] is the deterministic test adapter,
//! seeded from `DAFT_GOVERNOR_FORCE_*` and only ever consulted under
//! `cfg!(any(daft_dev_build, test))` — release builds ignore the variables,
//! mirroring the `DAFT_*_DIR` override idiom in `src/lib.rs`.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::governor::domain::ResourceSample;
use crate::governor::ports::ResourceProbe;

/// Production probe over [`sysinfo`].
pub struct SysinfoProbe {
    system: Mutex<sysinfo::System>,
}

impl SysinfoProbe {
    /// A probe with an empty (not yet refreshed) system handle.
    pub fn new() -> Self {
        Self {
            system: Mutex::new(sysinfo::System::new()),
        }
    }
}

impl Default for SysinfoProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceProbe for SysinfoProbe {
    fn sample(&self) -> ResourceSample {
        let mut system = self.system.lock().unwrap();
        system.refresh_memory();
        ResourceSample {
            mem_total: system.total_memory(),
            mem_available: system.available_memory(),
            swap_used: system.used_swap(),
            psi_some_avg10: read_psi_some_avg10(),
        }
    }
}

/// Linux memory PSI, `some avg10`. The file simply does not exist on
/// macOS or pre-PSI kernels — one failed open per sample is noise-free
/// and cheaper than caching platform state.
fn read_psi_some_avg10() -> Option<f32> {
    let text = std::fs::read_to_string("/proc/pressure/memory").ok()?;
    parse_psi_some_avg10(&text)
}

fn parse_psi_some_avg10(text: &str) -> Option<f32> {
    let line = text.lines().find(|line| line.starts_with("some"))?;
    let avg10 = line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?;
    avg10.parse().ok()
}

/// Deterministic probe for tests: a base sample from
/// `DAFT_GOVERNOR_FORCE_{MEM_TOTAL,MEM_AVAILABLE,SWAP_USED,PSI}` (bytes /
/// percent), optionally re-reading `DAFT_GOVERNOR_FORCE_STATE_FILE` on
/// every sample so an integration test can change pressure mid-run by
/// rewriting the file (`key=value` lines, same key names lowercased:
/// `mem_total`, `mem_available`, `swap_used`, `psi`).
pub struct ForcedProbe {
    base: ResourceSample,
    state_file: Option<PathBuf>,
    /// Last good parse — a test rewriting the state file non-atomically
    /// must not inject a zero sample.
    last: Mutex<ResourceSample>,
}

/// Environment variable names for [`ForcedProbe`].
pub const FORCE_MEM_TOTAL_ENV: &str = "DAFT_GOVERNOR_FORCE_MEM_TOTAL";
pub const FORCE_MEM_AVAILABLE_ENV: &str = "DAFT_GOVERNOR_FORCE_MEM_AVAILABLE";
pub const FORCE_SWAP_USED_ENV: &str = "DAFT_GOVERNOR_FORCE_SWAP_USED";
pub const FORCE_PSI_ENV: &str = "DAFT_GOVERNOR_FORCE_PSI";
pub const FORCE_STATE_FILE_ENV: &str = "DAFT_GOVERNOR_FORCE_STATE_FILE";

impl ForcedProbe {
    /// Build from the environment; `None` when no force variable is set.
    pub fn from_env() -> Option<Self> {
        let mem_total = env_u64(FORCE_MEM_TOTAL_ENV);
        let mem_available = env_u64(FORCE_MEM_AVAILABLE_ENV);
        let swap_used = env_u64(FORCE_SWAP_USED_ENV);
        let psi = std::env::var(FORCE_PSI_ENV)
            .ok()
            .and_then(|v| v.parse::<f32>().ok());
        let state_file = std::env::var(FORCE_STATE_FILE_ENV)
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);

        if mem_total.is_none()
            && mem_available.is_none()
            && swap_used.is_none()
            && psi.is_none()
            && state_file.is_none()
        {
            return None;
        }

        const DEFAULT_TOTAL: u64 = 16 << 30;
        let mem_total = mem_total.unwrap_or(DEFAULT_TOTAL);
        let base = ResourceSample {
            mem_total,
            mem_available: mem_available.unwrap_or(mem_total / 2),
            swap_used: swap_used.unwrap_or(0),
            psi_some_avg10: psi,
        };
        Some(Self {
            base,
            state_file,
            last: Mutex::new(base),
        })
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

impl ResourceProbe for ForcedProbe {
    fn sample(&self) -> ResourceSample {
        let Some(path) = &self.state_file else {
            return self.base;
        };
        let mut last = self.last.lock().unwrap();
        if let Ok(text) = std::fs::read_to_string(path)
            && let Some(sample) = parse_state_file(&text, &self.base)
        {
            *last = sample;
        }
        *last
    }
}

fn parse_state_file(text: &str, base: &ResourceSample) -> Option<ResourceSample> {
    let mut sample = *base;
    let mut any = false;
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "mem_total" => sample.mem_total = value.parse().ok()?,
            "mem_available" => sample.mem_available = value.parse().ok()?,
            "swap_used" => sample.swap_used = value.parse().ok()?,
            "psi" => sample.psi_some_avg10 = Some(value.parse().ok()?),
            _ => continue,
        }
        any = true;
    }
    any.then_some(sample)
}

/// The probe the governor should use: forced when the dev/test override
/// is configured (never in release builds), sysinfo otherwise.
pub fn build_probe() -> Box<dyn ResourceProbe> {
    if cfg!(any(daft_dev_build, test))
        && let Some(forced) = ForcedProbe::from_env()
    {
        return Box::new(forced);
    }
    Box::new(SysinfoProbe::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysinfo_probe_reports_plausible_memory() {
        let probe = SysinfoProbe::new();
        let sample = probe.sample();
        assert!(sample.mem_total > 0, "total memory must be non-zero");
        assert!(
            sample.mem_available <= sample.mem_total,
            "available must not exceed total"
        );
    }

    #[test]
    fn psi_parse_extracts_some_avg10() {
        let text = "some avg10=1.23 avg60=0.50 avg300=0.10 total=12345\n\
                    full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(parse_psi_some_avg10(text), Some(1.23));
        assert_eq!(parse_psi_some_avg10("garbage"), None);
        assert_eq!(parse_psi_some_avg10(""), None);
    }

    #[test]
    fn state_file_overrides_base_and_keeps_last_good() {
        let base = ResourceSample {
            mem_total: 16 << 30,
            mem_available: 8 << 30,
            swap_used: 0,
            psi_some_avg10: None,
        };
        let parsed = parse_state_file("mem_available=1073741824\nswap_used=42\n", &base).unwrap();
        assert_eq!(parsed.mem_available, 1 << 30);
        assert_eq!(parsed.swap_used, 42);
        assert_eq!(parsed.mem_total, 16 << 30);
        // Unknown keys are skipped; a file with none of ours parses to None.
        assert!(parse_state_file("unrelated=1\n", &base).is_none());
        assert!(parse_state_file("", &base).is_none());
        // A half-written value poisons the parse, not the probe.
        assert!(parse_state_file("mem_available=10737418", &base).is_some());
        assert!(parse_state_file("mem_available=garbage", &base).is_none());
    }

    #[test]
    #[serial_test::serial]
    fn forced_probe_absent_without_env() {
        // Ensure a clean slate (other tests in this process may have set them).
        for var in [
            FORCE_MEM_TOTAL_ENV,
            FORCE_MEM_AVAILABLE_ENV,
            FORCE_SWAP_USED_ENV,
            FORCE_PSI_ENV,
            FORCE_STATE_FILE_ENV,
        ] {
            unsafe { std::env::remove_var(var) };
        }
        assert!(ForcedProbe::from_env().is_none());
    }

    #[test]
    #[serial_test::serial]
    fn forced_probe_reads_env_and_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("governor-state");
        unsafe {
            std::env::set_var(FORCE_MEM_TOTAL_ENV, (32u64 << 30).to_string());
            std::env::set_var(FORCE_MEM_AVAILABLE_ENV, (16u64 << 30).to_string());
            std::env::set_var(FORCE_STATE_FILE_ENV, &state);
        }
        let probe = ForcedProbe::from_env().expect("forced probe configured");
        // No file yet: base sample.
        assert_eq!(probe.sample().mem_available, 16 << 30);
        // Rewriting the file flips the next sample.
        std::fs::write(&state, "mem_available=1073741824\n").unwrap();
        assert_eq!(probe.sample().mem_available, 1 << 30);
        // A corrupt rewrite keeps the last good sample.
        std::fs::write(&state, "mem_available=oops\n").unwrap();
        assert_eq!(probe.sample().mem_available, 1 << 30);
        unsafe {
            std::env::remove_var(FORCE_MEM_TOTAL_ENV);
            std::env::remove_var(FORCE_MEM_AVAILABLE_ENV);
            std::env::remove_var(FORCE_STATE_FILE_ENV);
        }
    }
}

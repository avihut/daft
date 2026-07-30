//! Scheme-1 range geometry: how a port range splits into the declared-block
//! region and the ad-hoc region, and how a slug lands in each.
//!
//! Scheme-1 contract (permanent, golden-locked):
//!
//! * `span = end - start + 1`
//! * `num_blocks = (span / 2) / block_size` — the declared region is the
//!   lower half of the range, rounded down to whole blocks:
//!   `[start, start + num_blocks * block_size - 1]`.
//! * The ad-hoc region is everything above it, through `end`. Undeclared
//!   names hash here, per `(slug, var)` — a disjoint region so the
//!   unverifiable wild west can never collide with a declared block.
//! * `block_index = scheme1_hash(scheme, salt, slug) % num_blocks`
//! * declared port = `start + block_index * block_size + offset`
//! * ad-hoc port = `adhoc_start + scheme1_hash(scheme, salt, slug, var) %
//!   adhoc_span`
//!
//! With the defaults (range 20000–32767, block 16): 399 declared blocks in
//! `[20000, 26383]` and 6384 ad-hoc ports in `[26384, 32767]`.

use super::hash::scheme1_hash;

/// The two disjoint regions a range splits into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Regions {
    /// First port of the range (= first declared block's base).
    pub start: u16,
    /// Number of whole declared blocks.
    pub num_blocks: u32,
    /// First port of the ad-hoc region.
    pub adhoc_start: u16,
    /// Ports in the ad-hoc region.
    pub adhoc_span: u32,
}

/// Split a range into declared and ad-hoc regions.
///
/// Returns `None` for degenerate geometry (a region would be empty) — config
/// validation rejects those shapes with a message; callers hitting `None`
/// treat the config as invalid.
pub fn regions(range: (u16, u16), block_size: u16) -> Option<Regions> {
    let (start, end) = range;
    if start == 0 || start > end || block_size == 0 {
        return None;
    }
    let span = u32::from(end) - u32::from(start) + 1;
    let num_blocks = (span / 2) / u32::from(block_size);
    if num_blocks == 0 {
        return None;
    }
    let adhoc_start = u32::from(start) + num_blocks * u32::from(block_size);
    let adhoc_span = u32::from(end) - adhoc_start + 1;
    if adhoc_span == 0 {
        return None;
    }
    Some(Regions {
        start,
        num_blocks,
        adhoc_start: adhoc_start as u16,
        adhoc_span,
    })
}

impl Regions {
    /// The slug's block index within the declared region.
    pub fn block_index(&self, scheme: u32, salt: &str, slug: &str) -> u32 {
        let scheme = scheme.to_string();
        (scheme1_hash(&[&scheme, salt, slug]) % u64::from(self.num_blocks)) as u32
    }

    /// Base port of the slug's declared block.
    pub fn block_base(&self, scheme: u32, salt: &str, slug: &str, block_size: u16) -> u16 {
        self.start + (self.block_index(scheme, salt, slug) * u32::from(block_size)) as u16
    }

    /// Ad-hoc port for an undeclared `(slug, var)` pair.
    pub fn adhoc_port(&self, scheme: u32, salt: &str, slug: &str, var: &str) -> u16 {
        let scheme = scheme.to_string();
        let h = scheme1_hash(&[&scheme, salt, slug, var]);
        self.adhoc_start + (h % u64::from(self.adhoc_span)) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_RANGE: (u16, u16) = (20000, 32767);

    #[test]
    fn default_geometry_golden() {
        let r = regions(DEFAULT_RANGE, 16).unwrap();
        assert_eq!(r.start, 20000);
        assert_eq!(r.num_blocks, 399);
        assert_eq!(r.adhoc_start, 26384);
        assert_eq!(r.adhoc_span, 6384);
    }

    /// End-to-end scheme-1 port goldens, independently computed with a Python
    /// reference. A failure here means every user's ports move on upgrade.
    #[test]
    fn block_base_goldens() {
        let r = regions(DEFAULT_RANGE, 16).unwrap();
        assert_eq!(r.block_base(1, "myapp", "feature-new", 16), 23952);
        assert_eq!(r.block_base(1, "myapp", "master", 16), 23456);
        assert_eq!(r.block_base(1, "backend", "feature-new", 16), 21280);
        assert_eq!(r.block_base(1, "a", "b", 16), 20608);
    }

    #[test]
    fn adhoc_port_goldens() {
        let r = regions(DEFAULT_RANGE, 16).unwrap();
        assert_eq!(r.adhoc_port(1, "myapp", "feature-new", "EXTRA_PORT"), 30742);
        assert_eq!(r.adhoc_port(1, "myapp", "feature-new", "OTHER"), 28336);
    }

    #[test]
    fn adhoc_region_is_disjoint_from_declared() {
        let r = regions(DEFAULT_RANGE, 16).unwrap();
        let declared_end = r.start as u32 + r.num_blocks * 16 - 1;
        assert_eq!(declared_end, 26383);
        assert_eq!(u32::from(r.adhoc_start), declared_end + 1);
        // Every ad-hoc port lands strictly above every declared port.
        for var in ["A", "B", "C", "LONG_VARIABLE_NAME"] {
            let p = r.adhoc_port(1, "s", "w", var);
            assert!(p > declared_end as u16 && p <= 32767);
        }
    }

    #[test]
    fn degenerate_geometry_is_refused() {
        assert_eq!(regions((0, 100), 16), None, "zero start");
        assert_eq!(regions((200, 100), 16), None, "reversed");
        assert_eq!(regions((20000, 20031), 0), None, "zero block");
        // 20 ports with block 16: half-span 10 fits no block.
        assert_eq!(regions((20000, 20019), 16), None);
        // Smallest viable: two blocks' worth.
        assert!(regions((20000, 20031), 16).is_some());
    }

    #[test]
    fn different_block_sizes_change_geometry_not_index_math() {
        let r8 = regions(DEFAULT_RANGE, 8).unwrap();
        assert_eq!(r8.num_blocks, 798);
        let base = r8.block_base(1, "myapp", "feature-new", 8);
        assert!(base >= 20000 && base < r8.adhoc_start);
        assert_eq!((base - 20000) % 8, 0, "base is block-aligned");
    }
}

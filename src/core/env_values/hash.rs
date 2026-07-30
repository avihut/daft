//! Scheme-1 stable hash: FNV-1a-64 with an xor-fold.
//!
//! This function is a **permanent compatibility contract**: derived ports are
//! computed independently on every machine and must agree forever, so the
//! algorithm is written down here rather than delegated to any dependency
//! whose behavior could shift under a version bump (`DefaultHasher` is
//! explicitly ruled out — its algorithm may change across Rust releases,
//! which `src/governor` tolerates for a cache and we cannot for ports).
//!
//! Definition (reproducible in any language):
//!
//! 1. Join the input fields with the byte `0x1F` (ASCII unit separator).
//! 2. FNV-1a-64 over the UTF-8 bytes: `h = 0xcbf29ce484222325`; per byte
//!    `h ^= byte; h = h.wrapping_mul(0x100000001b3)`.
//! 3. Fold: `h ^= h >> 32` — FNV-1a's avalanche is weak on short,
//!    near-identical keys (exactly what worktree names are); folding the high
//!    half into the low half fixes the distribution the `% num_blocks`
//!    reduction actually sees.
//!
//! Shell one-liner equivalent (documented in the recipes):
//! `python3 -c 'import sys; h=0xcbf29ce484222325
//! for b in "\x1f".join(sys.argv[1:]).encode(): h=(h^b)*0x100000001b3 & (2**64-1)
//! print(h ^ (h>>32))' 1 mysalt myslug`

/// FNV-1a 64-bit offset basis.
pub const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
/// FNV-1a 64-bit prime.
pub const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Plain FNV-1a-64 over a byte slice.
pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET_BASIS;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// The scheme-1 hash of a field list: `0x1F`-joined, FNV-1a-64, xor-folded.
pub fn scheme1_hash(fields: &[&str]) -> u64 {
    let mut h = FNV_OFFSET_BASIS;
    for (i, field) in fields.iter().enumerate() {
        if i > 0 {
            h ^= 0x1F;
            h = h.wrapping_mul(FNV_PRIME);
        }
        for &b in field.as_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h ^ (h >> 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical published FNV-1a-64 vectors. If these ever fail, the
    /// implementation drifted and every derived port on every machine moves:
    /// that is a scheme break, not a refactor.
    #[test]
    fn fnv1a_64_canonical_vectors() {
        assert_eq!(fnv1a_64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x85944171f73967e8);
        assert_eq!(fnv1a_64(b"daft"), 0x858a5c6730cac6ac);
    }

    /// Golden scheme-1 hashes (independently computed with a Python
    /// reference implementation). Same warning as above: a change here is a
    /// wire-format break.
    #[test]
    fn scheme1_hash_goldens() {
        assert_eq!(
            scheme1_hash(&["1", "myapp", "feature-new"]),
            0xe93db1a807255c0c
        );
        assert_eq!(scheme1_hash(&["1", "myapp", "master"]), 0xfbf1e95c6a4b8f89);
        assert_eq!(
            scheme1_hash(&["1", "backend", "feature-new"]),
            0x35885c3b94144852
        );
        assert_eq!(scheme1_hash(&["1", "a", "b"]), 0x7a5dae3685b79503);
    }

    /// The separator is load-bearing: without it, ("ab","c") and ("a","bc")
    /// would hash identically.
    #[test]
    fn field_joining_is_unambiguous() {
        assert_ne!(scheme1_hash(&["ab", "c"]), scheme1_hash(&["a", "bc"]));
        assert_eq!(scheme1_hash(&["ab"]), scheme1_hash(&["ab"]));
        // Joined form matches hashing the pre-joined byte string.
        assert_eq!(
            scheme1_hash(&["x", "y"]),
            fnv1a_64(b"x\x1fy") ^ (fnv1a_64(b"x\x1fy") >> 32)
        );
    }
}

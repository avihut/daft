//! Memorable names for fork sandboxes (`daft.start.forkNaming = memorable`).
//!
//! Docker-style adjective-noun pairs: collision-sparse by construction and
//! better *handles* than `master-fork-3` when several sandboxes are alive —
//! `daft go brave-otter` reads back the way it was said. The words are
//! embedded so name generation needs no I/O and no new dependency; the
//! caller supplies entropy (time/pid derived) and claims directories
//! atomically, so this module only has to produce a well-spread, eventually
//! terminating candidate stream.

const ADJECTIVES: &[&str] = &[
    "amber",
    "bold",
    "brave",
    "bright",
    "brisk",
    "calm",
    "clever",
    "cosmic",
    "crisp",
    "curious",
    "deft",
    "eager",
    "fable",
    "fleet",
    "gentle",
    "glad",
    "golden",
    "keen",
    "kind",
    "lively",
    "lucid",
    "lunar",
    "mellow",
    "merry",
    "misty",
    "nimble",
    "noble",
    "quiet",
    "rapid",
    "rustic",
    "silent",
    "solar",
    "spry",
    "steady",
    "sunny",
    "swift",
    "tidy",
    "vivid",
    "wandering",
    "witty",
];

const NOUNS: &[&str] = &[
    "aspen", "badger", "beacon", "birch", "brook", "cedar", "comet", "coral", "crane", "delta",
    "dune", "falcon", "fjord", "gecko", "glade", "grove", "harbor", "heron", "ibis", "inlet",
    "lagoon", "lark", "lynx", "maple", "marmot", "meadow", "otter", "owl", "pine", "plover",
    "prairie", "quill", "reef", "ridge", "sparrow", "summit", "tarn", "thicket", "walrus", "wren",
];

/// After this many pure adjective-noun draws, candidates gain a numeric
/// suffix so the stream terminates against any set of claimed directories.
const PURE_DRAWS: u64 = 16;

/// An endless, well-spread candidate stream seeded by the caller.
///
/// Consecutive draws step both word indices by odd multipliers, so no two
/// draws in a run collide on both words; past [`PURE_DRAWS`] a numeric
/// suffix guarantees eventual uniqueness even in a directory full of
/// claimed names.
pub fn candidates(seed: u64) -> impl Iterator<Item = String> {
    (0u64..).map(move |i| {
        let a = ADJECTIVES[(seed.wrapping_mul(31).wrapping_add(i.wrapping_mul(7))
            % ADJECTIVES.len() as u64) as usize];
        let n = NOUNS[(seed.wrapping_mul(17).wrapping_add(i.wrapping_mul(13)) % NOUNS.len() as u64)
            as usize];
        if i < PURE_DRAWS {
            format!("{a}-{n}")
        } else {
            format!("{a}-{n}-{}", i - PURE_DRAWS + 2)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_distinct_within_a_run() {
        let names: Vec<String> = candidates(42).take(PURE_DRAWS as usize).collect();
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "no early duplicates: {names:?}");
    }

    #[test]
    fn suffixed_candidates_terminate_any_collision_run() {
        let far: Vec<String> = candidates(7).skip(PURE_DRAWS as usize).take(3).collect();
        assert!(
            far.iter()
                .all(|n| n.chars().last().unwrap().is_ascii_digit())
        );
        let unique: std::collections::HashSet<&String> = far.iter().collect();
        assert_eq!(unique.len(), far.len());
    }

    #[test]
    fn names_are_valid_sandbox_dirnames() {
        for name in candidates(123).take(50) {
            assert!(
                super::super::sandbox::sandbox_dirname(&name).is_some(),
                "'{name}' must survive the sandbox naming rules"
            );
        }
    }
}

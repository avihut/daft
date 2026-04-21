//! Branch ownership detection strategies.
//!
//! Computes who "owns" a branch from the commit range `base..branch`,
//! per the strategy configured in `daft.ownership.strategy`. See
//! `docs/superpowers/specs/2026-04-21-ownership-detection-strategies.md`.

/// Strategy for deducing branch ownership from a commit range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipStrategy {
    /// Owner = author of the newest commit. Original daft behavior.
    Tip,
    /// Owner = the current user if they authored any commit in range;
    /// otherwise the tip author.
    Any,
    /// Owner = author of the oldest commit in range.
    First,
    /// Owner = author with the most commits. Ties broken by recency.
    Plurality,
    /// Owner = author with > 50% of commits. No owner if no majority.
    Majority,
    /// Owner = author with highest recency-weighted score: commit at
    /// rank k from tip (k=0 = tip) contributes 1/(k+1). Ties broken by
    /// recency. This is the default.
    RecencyPlurality,
}

impl OwnershipStrategy {
    /// Parse a string value from git config.
    ///
    /// Accepts exact lowercase strings as documented:
    /// `tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`.
    /// Matching is case-insensitive. Returns `None` for unknown values.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "tip" => Some(Self::Tip),
            "any" => Some(Self::Any),
            "first" => Some(Self::First),
            "plurality" => Some(Self::Plurality),
            "majority" => Some(Self::Majority),
            "recency-plurality" => Some(Self::RecencyPlurality),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_all_known_strategies() {
        assert_eq!(
            OwnershipStrategy::parse("tip"),
            Some(OwnershipStrategy::Tip)
        );
        assert_eq!(
            OwnershipStrategy::parse("any"),
            Some(OwnershipStrategy::Any)
        );
        assert_eq!(
            OwnershipStrategy::parse("first"),
            Some(OwnershipStrategy::First)
        );
        assert_eq!(
            OwnershipStrategy::parse("plurality"),
            Some(OwnershipStrategy::Plurality)
        );
        assert_eq!(
            OwnershipStrategy::parse("majority"),
            Some(OwnershipStrategy::Majority)
        );
        assert_eq!(
            OwnershipStrategy::parse("recency-plurality"),
            Some(OwnershipStrategy::RecencyPlurality)
        );
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(
            OwnershipStrategy::parse("Recency-Plurality"),
            Some(OwnershipStrategy::RecencyPlurality)
        );
        assert_eq!(
            OwnershipStrategy::parse("TIP"),
            Some(OwnershipStrategy::Tip)
        );
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(
            OwnershipStrategy::parse("  tip  "),
            Some(OwnershipStrategy::Tip)
        );
    }

    #[test]
    fn parse_returns_none_for_unknown() {
        assert_eq!(OwnershipStrategy::parse(""), None);
        assert_eq!(OwnershipStrategy::parse("owner"), None);
        assert_eq!(OwnershipStrategy::parse("recency"), None);
    }
}

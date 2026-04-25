//! Cleanup policy types and string parsers shared by hook-fire and cleanup paths.

use anyhow::{anyhow, Result};

/// Parse a size string into bytes. Accepts: `1024`, `1KB`, `10MB`, `2GB`.
/// Case-insensitive, no spaces. Plain integer = bytes.
pub fn parse_size(input: &str) -> Result<u64> {
    let s = input.trim();
    let upper = s.to_ascii_uppercase();
    let (num_str, multiplier): (&str, u64) = if let Some(n) = upper.strip_suffix("GB") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = upper.strip_suffix("MB") {
        (n, 1024 * 1024)
    } else if let Some(n) = upper.strip_suffix("KB") {
        (n, 1024)
    } else if let Some(n) = upper.strip_suffix('B') {
        (n, 1)
    } else {
        (upper.as_str(), 1)
    };
    let n: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid size: {input}"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| anyhow!("size overflow: {input}"))
}

/// Parse a duration string into seconds. Accepts: `30m`, `24h`, `7d`.
pub fn parse_duration_str(input: &str) -> Result<u64> {
    let s = input.trim();
    let (num_str, multiplier): (&str, u64) = if let Some(n) = s.strip_suffix('d') {
        (n, 86_400)
    } else if let Some(n) = s.strip_suffix('h') {
        (n, 3_600)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        return Err(anyhow!(
            "invalid duration: {input} (expected suffix d/h/m/s)"
        ));
    };
    let n: u64 = num_str
        .trim()
        .parse()
        .map_err(|_| anyhow!("invalid duration: {input}"))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| anyhow!("duration overflow: {input}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_plain_integer() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }

    #[test]
    fn parse_size_with_units() {
        assert_eq!(parse_size("1KB").unwrap(), 1024);
        assert_eq!(parse_size("10MB").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn parse_size_case_insensitive() {
        assert_eq!(parse_size("10mb").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("10Mb").unwrap(), 10 * 1024 * 1024);
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("10XB").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn parse_duration_basic() {
        assert_eq!(parse_duration_str("30s").unwrap(), 30);
        assert_eq!(parse_duration_str("5m").unwrap(), 300);
        assert_eq!(parse_duration_str("24h").unwrap(), 86_400);
        assert_eq!(parse_duration_str("7d").unwrap(), 7 * 86_400);
    }

    #[test]
    fn parse_duration_rejects_no_suffix() {
        assert!(parse_duration_str("60").is_err());
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        assert!(parse_duration_str("abc").is_err());
        assert!(parse_duration_str("5y").is_err());
    }

    #[test]
    fn parse_size_rejects_overflow() {
        assert!(parse_size("99999999999999GB").is_err());
    }

    #[test]
    fn parse_size_handles_leading_whitespace() {
        assert_eq!(parse_size("  10MB  ").unwrap(), 10 * 1024 * 1024);
    }

    #[test]
    fn parse_duration_rejects_negative() {
        assert!(parse_duration_str("-5m").is_err());
    }

    #[test]
    fn parse_duration_rejects_overflow() {
        // u64::MAX is ~1.84e19; multiplying by 86_400 overflows for any value >= ~2.1e14.
        assert!(parse_duration_str("999999999999999999d").is_err());
    }
}

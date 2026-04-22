//! The `Format` enum and helpers.

use std::fmt;

/// User-selectable output format, one per `--format <value>` enum variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Format {
    Json,
    Ndjson,
    Tsv,
    Csv,
    Yaml,
    Toon,
    Markdown,
}

impl Format {
    pub const ALL: &'static [Format] = &[
        Format::Json,
        Format::Ndjson,
        Format::Tsv,
        Format::Csv,
        Format::Yaml,
        Format::Toon,
        Format::Markdown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Ndjson => "ndjson",
            Format::Tsv => "tsv",
            Format::Csv => "csv",
            Format::Yaml => "yaml",
            Format::Toon => "toon",
            Format::Markdown => "markdown",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_kebab_case() {
        assert_eq!(Format::Json.to_string(), "json");
        assert_eq!(Format::Ndjson.to_string(), "ndjson");
        assert_eq!(Format::Markdown.to_string(), "markdown");
    }

    #[test]
    fn all_covers_every_variant() {
        assert_eq!(Format::ALL.len(), 7);
        assert!(Format::ALL.contains(&Format::Toon));
    }
}

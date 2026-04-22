//! Shared clap args flattened into every command that supports structured emit.

use crate::output::emit::format::Format;

#[derive(clap::Args, Debug, Clone)]
pub struct EmitArgs {
    /// Output format. Mutually exclusive with --template.
    #[arg(long, value_enum, value_name = "FORMAT", conflicts_with = "template")]
    pub format: Option<Format>,

    /// Tera template string. Mutually exclusive with --format.
    #[arg(long, value_name = "STR", conflicts_with = "format")]
    pub template: Option<String>,

    /// Omit header row (tsv/csv only).
    #[arg(long)]
    pub no_headers: bool,
}

impl EmitArgs {
    /// True when the user requested structured emit (via --format or --template).
    pub fn is_structured(&self) -> bool {
        self.format.is_some() || self.template.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Harness {
        #[command(flatten)]
        emit: EmitArgs,
    }

    #[test]
    fn default_is_unstructured() {
        let h = Harness::parse_from(["bin"]);
        assert!(!h.emit.is_structured());
    }

    #[test]
    fn format_sets_structured() {
        let h = Harness::parse_from(["bin", "--format", "json"]);
        assert!(h.emit.is_structured());
        assert_eq!(h.emit.format, Some(Format::Json));
    }

    #[test]
    fn template_sets_structured() {
        let h = Harness::parse_from(["bin", "--template", "{{ x }}"]);
        assert!(h.emit.is_structured());
    }

    #[test]
    fn format_and_template_conflict() {
        let err = Harness::try_parse_from(["bin", "--format", "json", "--template", "{{ x }}"])
            .unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }
}

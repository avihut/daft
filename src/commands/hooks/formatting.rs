/// Colorize a serialized YAML string for terminal output.
///
/// Skips the `---` document separator, colors top-level keys bold,
/// hook names bold+yellow, sub-keys cyan, quoted strings green, and
/// booleans/numbers yellow.
pub(super) fn colorize_yaml_dump(yaml: &str) -> String {
    use crate::styles::{colors_enabled, BOLD, CYAN, DIM, RESET, YELLOW};

    let use_colors = colors_enabled();
    let mut in_hooks = false;
    let mut result = String::new();

    for line in yaml.lines() {
        if line == "---" {
            continue;
        }

        if line.is_empty() {
            result.push('\n');
            continue;
        }

        let indent_len = line.len() - line.trim_start().len();
        let rest = &line[indent_len..];
        let indent = &line[..indent_len];

        // Track entry into the hooks: section (top-level key)
        if indent_len == 0 {
            in_hooks = rest == "hooks:" || rest.starts_with("hooks: ");
        }

        if !use_colors {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        let colored = if let Some(after_dash) = rest.strip_prefix("- ") {
            format!("{DIM}-{RESET} {}", yaml_colorize_value_part(after_dash))
        } else if rest == "-" {
            format!("{DIM}-{RESET}")
        } else if let Some(colon_pos) = yaml_key_colon(rest) {
            let key = &rest[..colon_pos];
            let after_colon = &rest[colon_pos + 1..];

            let colored_key = if in_hooks && indent_len == 2 {
                // Hook name: bold + yellow
                format!("{BOLD}{YELLOW}{key}{RESET}")
            } else if indent_len == 0 {
                // Top-level config key: bold
                format!("{BOLD}{key}{RESET}")
            } else {
                // Sub-key: cyan
                format!("{CYAN}{key}{RESET}")
            };

            if after_colon.is_empty() {
                format!("{colored_key}:")
            } else {
                let val = after_colon.trim_start();
                format!("{colored_key}: {}", yaml_colorize_scalar(val))
            }
        } else {
            yaml_colorize_scalar(rest)
        };

        result.push_str(indent);
        result.push_str(&colored);
        result.push('\n');
    }

    result
}

/// Find the byte position of the `:` separating a YAML key from its value.
///
/// Returns `Some(pos)` for `key: value` (pos of `:`) or for a bare
/// mapping header `key:` (pos of trailing `:`). Returns `None` for
/// plain strings that contain no key-value separator.
fn yaml_key_colon(s: &str) -> Option<usize> {
    if let Some(pos) = s.find(": ") {
        return Some(pos);
    }
    if s.ends_with(':') {
        return Some(s.len() - 1);
    }
    None
}

/// Colorize the content after `- ` in a YAML list item.
///
/// If the content is `key: value`, the key is colored cyan. Otherwise
/// the whole string is treated as a scalar value.
fn yaml_colorize_value_part(s: &str) -> String {
    use crate::styles::{CYAN, RESET};
    if let Some(pos) = yaml_key_colon(s) {
        let key = &s[..pos];
        let after = &s[pos + 1..];
        let colored_key = format!("{CYAN}{key}{RESET}");
        if after.is_empty() {
            format!("{colored_key}:")
        } else {
            format!(
                "{colored_key}: {}",
                yaml_colorize_scalar(after.trim_start())
            )
        }
    } else {
        yaml_colorize_scalar(s)
    }
}

/// Colorize a scalar YAML value.
///
/// - Booleans and null → yellow
/// - Quoted strings → green
/// - Numbers → yellow
/// - Everything else → plain
fn yaml_colorize_scalar(value: &str) -> String {
    use crate::styles::{GREEN, RESET, YELLOW};
    if matches!(value, "true" | "false" | "null" | "~") {
        return format!("{YELLOW}{value}{RESET}");
    }
    if value.starts_with('"') || value.starts_with('\'') {
        return format!("{GREEN}{value}{RESET}");
    }
    if value.parse::<i64>().is_ok() || value.parse::<f64>().is_ok() {
        return format!("{YELLOW}{value}{RESET}");
    }
    value.to_string()
}

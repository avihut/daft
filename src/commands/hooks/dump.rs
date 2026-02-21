use super::find_worktree_root;
use super::formatting::colorize_yaml_dump;
use crate::hooks::yaml_config_loader;
use crate::output::Output;
use crate::styles::dim;
use anyhow::{Context, Result};

/// Dump the merged YAML hooks configuration.
pub(super) fn cmd_dump(output: &mut dyn Output) -> Result<()> {
    let worktree_root = find_worktree_root()?;

    let config = yaml_config_loader::load_merged_config(&worktree_root)
        .context("Failed to load YAML config")?;

    let config = match config {
        Some(c) => c,
        None => {
            output.info(&dim("No daft.yml found."));
            return Ok(());
        }
    };

    let value: serde_yaml::Value =
        serde_yaml::to_value(&config).context("Failed to convert config to YAML value")?;
    let stripped = strip_yaml_nulls(value);
    let yaml = serde_yaml::to_string(&stripped).context("Failed to serialize config")?;
    output.raw(&colorize_yaml_dump(&yaml));

    Ok(())
}

/// Recursively remove null values and empty mappings/sequences from a YAML value.
fn strip_yaml_nulls(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let mut out = serde_yaml::Mapping::new();
            for (k, v) in map {
                if v.is_null() {
                    continue;
                }
                let stripped = strip_yaml_nulls(v);
                match &stripped {
                    serde_yaml::Value::Mapping(m) if m.is_empty() => continue,
                    serde_yaml::Value::Sequence(s) if s.is_empty() => continue,
                    _ => {}
                }
                out.insert(k, stripped);
            }
            serde_yaml::Value::Mapping(out)
        }
        serde_yaml::Value::Sequence(seq) => serde_yaml::Value::Sequence(
            seq.into_iter()
                .filter(|v| !v.is_null())
                .map(strip_yaml_nulls)
                .collect(),
        ),
        other => other,
    }
}

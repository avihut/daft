//! `daft config` — browse and change every daft setting.
//!
//! daft's configuration is spread across git config, `daft.yml`, the global
//! TOML, and a handful of environment overrides, resolved through several
//! different precedence chains. The point of this command is that a user
//! should never have to know which: one place lists everything, says what each
//! setting currently is, and says which layer decided that.
//!
//! The CLI verbs are the scriptable half — `list`, `get`, `set`, `unset` — and
//! the bare command opens the full-screen browser. Agents and scripts use the
//! verbs; the screen is for people.

pub mod remote_sync;
pub mod resolve;
pub mod write;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::core::settings_spec::{Backend, Category, ValueType};
use crate::git::GitCommand;
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::styles::{bold, dim, dim_underline};
use resolve::{Diagnostic, Origin, Resolved, ResolvedSet, Snapshot};
use write::WriteScope;

#[derive(Parser)]
#[command(name = "config")]
#[command(about = "Browse and change daft settings")]
#[command(long_about = r#"
Browse and change daft settings.

Every daft setting in one place, with the value it currently has and the
layer that decided it. Settings live in several stores — git config, the
repository's daft.yml, the global config — resolved through different
precedence chains; this command hides that split behind one list of keys.

  daft config                    Open the settings browser
  daft config list               Every setting, its value, and where it came from
  daft config list --modified    Only the settings something sets
  daft config get <key>          Print one effective value
  daft config get <key> --origin Print it with the full layer-by-layer chain
  daft config set <key> <value>  Change it in this repository
  daft config set --global ...   Change it for every repository
  daft config unset <key>        Remove it, revealing whatever it was masking

Values are validated against the setting's own type before anything is
written, so a bad enum or column spec is refused where you typed it rather
than at the next command that reads it.
"#)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: Option<ConfigCommand>,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// List every setting with its value and origin
    List(ListArgs),
    /// Print one setting's effective value
    Get(GetArgs),
    /// Change a setting
    Set(SetArgs),
    /// Remove a setting, revealing whatever it was masking
    Unset(UnsetArgs),
    /// Configure remote sync behavior
    RemoteSync(remote_sync::Args),
}

#[derive(Args)]
pub struct ListArgs {
    /// Only settings something actually sets
    #[arg(long)]
    modified: bool,

    /// Only settings in this category (checkout, merge, hooks, ...)
    #[arg(long, value_name = "NAME")]
    category: Option<String>,

    #[command(flatten)]
    emit: EmitArgs,
}

#[derive(Args)]
pub struct GetArgs {
    /// The setting to read
    #[arg(value_name = "KEY")]
    key: String,

    /// Show every layer's value and which one won
    #[arg(long)]
    origin: bool,
}

#[derive(Args)]
pub struct SetArgs {
    /// The setting to change
    #[arg(value_name = "KEY")]
    key: String,

    /// The new value
    ///
    /// Hyphen-leading values are taken literally, because several settings
    /// hold flags — `daft.update.args` defaults to `--ff-only`, and requiring
    /// `--` before the most likely value would be a papercut on every use.
    /// Put `--global` before the key, or separate with `--`, if a value could
    /// be mistaken for a flag.
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
    value: String,

    /// Write to global config instead of this repository
    #[arg(long)]
    global: bool,
}

#[derive(Args)]
pub struct UnsetArgs {
    /// The setting to remove
    #[arg(value_name = "KEY")]
    key: String,

    /// Remove from global config instead of this repository
    #[arg(long)]
    global: bool,
}

pub fn run() -> Result<()> {
    let args: Vec<String> = crate::cli::argv().iter().skip(1).cloned().collect();
    let config_args = ConfigArgs::parse_from(args);

    match config_args.command {
        Some(ConfigCommand::List(args)) => cmd_list(&args),
        Some(ConfigCommand::Get(args)) => cmd_get(&args),
        Some(ConfigCommand::Set(args)) => cmd_set(&args),
        Some(ConfigCommand::Unset(args)) => cmd_unset(&args),
        Some(ConfigCommand::RemoteSync(args)) => remote_sync::run(&args),
        None => cmd_default(),
    }
}

/// The bare command. Opens the browser when there is a terminal to draw on,
/// and falls back to `list` when there is not — a piped `daft config` should
/// print something useful rather than refuse.
fn cmd_default() -> Result<()> {
    cmd_list(&ListArgs {
        modified: false,
        category: None,
        emit: EmitArgs::default(),
    })
}

// ─────────────────────────────────────────────────────────────────────────
// Shared plumbing
// ─────────────────────────────────────────────────────────────────────────

/// Resolve everything, from inside a repository or not.
///
/// Outside one there is no local scope, so only the global and system layers
/// answer — but they still answer, which is why running this from a home
/// directory is useful rather than empty.
fn resolved() -> Result<ResolvedSet> {
    let snapshot = if crate::is_git_repository().unwrap_or(false) {
        Snapshot::capture(&GitCommand::new(false))?
    } else {
        Snapshot::capture_global_only()?
    };
    Ok(resolve::resolve_all(&snapshot))
}

fn scope_from_flag(global: bool) -> WriteScope {
    if global {
        WriteScope::Global
    } else {
        WriteScope::Local
    }
}

// ─────────────────────────────────────────────────────────────────────────
// config list
// ─────────────────────────────────────────────────────────────────────────

fn cmd_list(args: &ListArgs) -> Result<()> {
    let set = resolved()?;

    let category = match &args.category {
        Some(name) => Some(parse_category(name)?),
        None => None,
    };

    let rows: Vec<&Resolved> = set
        .settings
        .iter()
        .filter(|r| !args.modified || r.is_set())
        .filter(|r| category.is_none_or(|c| r.spec.category == c))
        .collect();

    if args.emit.is_structured() {
        return emit::emit_and_handle(
            "config list",
            EmitPayload::Tabular(build_table(&rows)),
            &args.emit,
            &mut std::io::stdout(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }

    if rows.is_empty() {
        println!(
            "{}",
            if args.modified {
                "Nothing is set — every setting is running on its default."
            } else {
                "No settings matched."
            }
        );
        return Ok(());
    }

    let key_width = rows
        .iter()
        .map(|r| r.spec.key.chars().count())
        .max()
        .unwrap_or(0);
    let value_width = rows
        .iter()
        .map(|r| r.effective_display().chars().count())
        .max()
        .unwrap_or(0)
        .min(32);

    let mut current = None;
    for row in &rows {
        if current != Some(row.spec.category) {
            if current.is_some() {
                println!();
            }
            println!("{}", dim_underline(row.spec.category.label()));
            current = Some(row.spec.category);
        }

        let value = row.effective_display();
        // Bold marks "something set this", which is the question the list is
        // most often scanned for.
        let shown = if row.is_set() {
            bold(value)
        } else {
            value.to_string()
        };
        let pad = value_width.saturating_sub(value.chars().count());

        println!(
            "  {:key_width$}  {shown}{:pad$}  {}",
            row.spec.key,
            "",
            dim(&row.origin.label()),
        );
    }

    // Unrecognized keys belong to no row, so `get <key> --origin` cannot
    // explain them — they have to be named here or the advice is unactionable.
    if !set.unrecognized.is_empty() {
        println!();
        println!(
            "{}",
            dim_underline("Set in config, but not settings daft knows")
        );
        for entry in &set.unrecognized {
            println!(
                "  {}  {}",
                entry.key,
                dim(&format!("({}) — daft ignores this", entry.scope.label()))
            );
        }
        println!(
            "  {}",
            dim("A mis-cased subsection is a different key to git, which is the usual cause.")
        );
    }

    let attached = set.issue_count() - set.unrecognized.len();
    if attached > 0 {
        println!();
        println!(
            "{}",
            dim(&format!(
                "{attached} setting(s) need attention — run `daft config get <key> --origin` for detail."
            ))
        );
    }

    Ok(())
}

fn parse_category(name: &str) -> Result<Category> {
    Category::all()
        .iter()
        .copied()
        .find(|c| {
            c.label().eq_ignore_ascii_case(name)
                || c.label().replace([' ', '&'], "").eq_ignore_ascii_case(name)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown category: {name}\n\nTry one of: {}",
                Category::all()
                    .iter()
                    .map(|c| c.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

fn build_table(rows: &[&Resolved]) -> Table {
    let mut table = Table::new([
        "key", "label", "category", "value", "origin", "is_set", "type", "default", "backend",
        "help",
    ]);
    for row in rows {
        table = table.row([
            Cell::str(row.spec.key.as_ref()),
            Cell::str(row.spec.label.as_ref()),
            Cell::str(row.spec.category.label()),
            Cell::str(row.effective_display()),
            Cell::str(row.origin.label()),
            Cell::bool(row.is_set()),
            Cell::str(type_name(&row.spec.ty)),
            Cell::str(row.spec.default.value().unwrap_or("")),
            Cell::str(backend_name(&row.spec.backend)),
            Cell::str(row.spec.help.as_ref()),
        ]);
    }
    table
}

fn type_name(ty: &ValueType) -> &'static str {
    match ty {
        ValueType::Bool => "bool",
        ValueType::TriBool => "bool-or-unset",
        ValueType::Enum(_) => "enum",
        ValueType::BoolOrKey => "bool-or-key",
        ValueType::Str => "string",
        ValueType::Path => "path",
        ValueType::Int => "int",
        ValueType::IntOrAuto => "int-or-auto",
        ValueType::Duration(_) => "duration",
        ValueType::Size => "size",
        ValueType::SizeOrPct => "size-or-percent",
        ValueType::Spec(_) => "spec",
        ValueType::LayoutComposite => "layout",
    }
}

fn backend_name(backend: &Backend) -> &'static str {
    match backend {
        Backend::GitConfig => "git-config",
        Backend::DaftYml { .. } => "daft.yml",
        Backend::LayoutChain => "layout-chain",
    }
}

// ─────────────────────────────────────────────────────────────────────────
// config get
// ─────────────────────────────────────────────────────────────────────────

fn cmd_get(args: &GetArgs) -> Result<()> {
    let spec = resolve::lookup(&args.key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let set = resolved()?;
    let Some(row) = set.get(&spec.key) else {
        bail!("{} did not resolve", spec.key);
    };

    if !args.origin {
        // Bare `get` is for scripts: the value on stdout, nothing else, and a
        // non-zero exit when there is no value — the same contract
        // `git config --get` has.
        match &row.effective {
            Some(value) => {
                println!("{value}");
                return Ok(());
            }
            None => std::process::exit(1),
        }
    }

    print_detail(row);
    Ok(())
}

/// The CLI form of the settings screen's detail panel.
fn print_detail(row: &Resolved) {
    println!("{}  {}", bold(&row.spec.label), dim(&row.spec.key));
    println!("{}", row.spec.help);
    println!();

    if let Some(variants) = row.spec.ty.variants() {
        println!("{}", dim_underline("Values"));
        let width = variants
            .iter()
            .map(|(v, _)| v.chars().count())
            .max()
            .unwrap_or(0);
        for (value, gloss) in variants {
            println!("  {value:width$}  {}", dim(gloss));
        }
        println!();
    } else if let Some(hint) = row.spec.ty.format_hint() {
        println!("{} {hint}", dim_underline("Format"));
        println!();
    }

    println!("{}", dim_underline("Where it comes from"));
    for (index, rung) in row.rungs.iter().enumerate() {
        let marker = if row.winner == Some(index) {
            "●"
        } else {
            " "
        };
        let value = rung.value.as_deref().unwrap_or("—");
        let mut line = format!("  {marker} {:12}  {value}", rung.layer.label());
        // The note belongs to a *value* that does nothing. An empty scope is
        // silent, not inert, and labelling it either way is noise.
        if rung.inert.is_some() && rung.value.is_some() {
            line.push_str(&format!(" {}", dim("(set here, but never read)")));
        }
        if let (Some(path), true) = (&rung.origin_path, rung.value.is_some()) {
            line.push_str(&format!("  {}", dim(&path.display().to_string())));
        }
        println!("{line}");
    }

    println!();
    println!(
        "  → effective: {} {}",
        bold(row.effective_display()),
        dim(&format!("({})", row.origin.label()))
    );

    if let Some(rule) = row.spec.default.rule()
        && matches!(row.origin, Origin::Default)
    {
        println!("  {}", dim(&format!("the default works out to {rule}")));
    }

    for diagnostic in &row.diagnostics {
        println!("  {}", describe(diagnostic));
    }
}

/// One diagnostic as a line the user can act on.
pub fn describe(diagnostic: &Diagnostic) -> String {
    match diagnostic {
        Diagnostic::Invalid {
            layer,
            value,
            reason,
        } => format!(
            "✗ the {} value {value:?} is not valid: {reason}",
            layer.label()
        ),
        Diagnostic::Deprecated { alias, replacement } => {
            format!("! {alias} is retired — move the value to {replacement}")
        }
        Diagnostic::Inert { scope, value, .. } => format!(
            "! {value:?} is set at {} but daft only reads this key from global config",
            scope.label()
        ),
        Diagnostic::EnvShadow { layer, value } => format!(
            "! {} sets this to {value:?}, so no config file can change it",
            layer.label()
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// config set / unset
// ─────────────────────────────────────────────────────────────────────────

fn cmd_set(args: &SetArgs) -> Result<()> {
    let spec = resolve::lookup(&args.key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let scope = scope_from_flag(args.global);
    require_repo_for_local(scope)?;

    let set = resolved()?;
    println!("{}", write::set(&spec, scope, &args.value, &set)?);
    Ok(())
}

fn cmd_unset(args: &UnsetArgs) -> Result<()> {
    let spec = resolve::lookup(&args.key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let scope = scope_from_flag(args.global);
    require_repo_for_local(scope)?;

    println!("{}", write::unset(&spec, scope)?);
    Ok(())
}

fn require_repo_for_local(scope: WriteScope) -> Result<()> {
    if scope == WriteScope::Local && !crate::is_git_repository().unwrap_or(false) {
        bail!(
            "not inside a git repository — there is no local config to write.\n\
             Use --global to change the setting for every repository."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_tree_parses_the_quartet() {
        ConfigArgs::command().debug_assert();

        let parsed = ConfigArgs::parse_from(["config", "set", "daft.autocd", "false"]);
        assert!(matches!(parsed.command, Some(ConfigCommand::Set(_))));

        let parsed = ConfigArgs::parse_from(["config", "get", "daft.autocd", "--origin"]);
        match parsed.command {
            Some(ConfigCommand::Get(args)) => assert!(args.origin),
            _ => panic!("expected get"),
        }

        let parsed = ConfigArgs::parse_from(["config", "unset", "--global", "daft.autocd"]);
        match parsed.command {
            Some(ConfigCommand::Unset(args)) => assert!(args.global),
            _ => panic!("expected unset"),
        }

        let parsed = ConfigArgs::parse_from(["config", "list", "--modified"]);
        match parsed.command {
            Some(ConfigCommand::List(args)) => assert!(args.modified),
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn remote_sync_keeps_its_flags() {
        let parsed = ConfigArgs::parse_from(["config", "remote-sync", "--on", "--global"]);
        match parsed.command {
            Some(ConfigCommand::RemoteSync(args)) => {
                assert!(args.on);
                assert!(args.global);
            }
            _ => panic!("expected remote-sync"),
        }
    }

    /// The suggestion list and the completions scripts both key off
    /// `DAFT_CONFIG_SUBCOMMANDS`, so it has to be what clap actually accepts —
    /// otherwise a new verb completes but does not run, or vice versa.
    #[test]
    fn the_suggestion_list_matches_the_command_tree() {
        let mut from_clap: Vec<String> = ConfigArgs::command()
            .get_subcommands()
            .map(|sub| sub.get_name().to_string())
            .collect();
        from_clap.sort();

        let mut listed: Vec<String> = crate::suggest::DAFT_CONFIG_SUBCOMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect();
        listed.sort();

        assert_eq!(from_clap, listed);
        assert_eq!(
            crate::suggest::DAFT_CONFIG_SUBCOMMANDS.to_vec(),
            listed,
            "DAFT_CONFIG_SUBCOMMANDS should be in alphabetical order"
        );
    }

    #[test]
    fn a_bare_invocation_has_no_subcommand() {
        let parsed = ConfigArgs::parse_from(["config"]);
        assert!(parsed.command.is_none());
    }

    #[test]
    fn categories_are_matched_by_label_however_it_is_spelled() {
        assert_eq!(parse_category("checkout").unwrap(), Category::Checkout);
        assert_eq!(parse_category("Merge").unwrap(), Category::Merge);
        // "Push & Sync" has spaces and an ampersand nobody wants to quote.
        assert_eq!(parse_category("pushsync").unwrap(), Category::PushSync);
        assert_eq!(parse_category("daft.yml").unwrap(), Category::RepoFile);

        let err = parse_category("nope").unwrap_err().to_string();
        assert!(err.contains("Checkout"), "the error must list the options");
    }

    #[test]
    fn every_value_type_has_a_name_for_structured_output() {
        // A `_ =>` arm here would silently label a future type "unknown" in
        // every JSON consumer, so the match is exhaustive and this asserts it
        // stays meaningful.
        for spec in crate::core::settings_spec::all_specs() {
            assert!(!type_name(&spec.ty).is_empty());
            assert!(!backend_name(&spec.backend).is_empty());
        }
    }

    #[test]
    fn unknown_keys_suggest_near_spellings() {
        let err = resolve::lookup("daft.merge.stile").unwrap_err();
        assert!(err.contains("Did you mean"), "unhelpful: {err}");
        assert!(err.contains("daft.merge.style"), "unhelpful: {err}");
    }

    #[test]
    fn a_retired_spelling_names_its_replacement() {
        let err = resolve::lookup("daft.fetch.args").unwrap_err();
        assert!(err.contains("daft.update.args"), "unhelpful: {err}");
    }

    #[test]
    fn a_mis_cased_key_still_finds_its_row_except_in_the_subsection() {
        // Git folds the section and the trailing name...
        assert_eq!(
            resolve::lookup("DAFT.AutoCD").unwrap().key,
            crate::core::settings::keys::AUTOCD
        );
        // ...and the registry's spelling is what comes back, so a write puts
        // that in the file rather than the user's casing.
        assert_eq!(
            resolve::lookup("daft.checkout.PUSHVERIFY").unwrap().key,
            crate::core::settings::keys::CHECKOUT_PUSH_VERIFY
        );
        // A mis-cased subsection is a different key to git; accepting it would
        // write a second, inert one.
        assert!(resolve::lookup("daft.checkoutbranch.carry").is_err());
    }
}

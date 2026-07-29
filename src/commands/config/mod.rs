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

pub mod resolve;
pub mod screen;
pub mod write;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::core::settings_spec::{Backend, BehaviorSpec, Category, Preset, ValueType};
use crate::git::GitCommand;
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::styles::{bold, dim, dim_underline};
use resolve::{Diagnostic, Origin, Resolved, ResolvedBehavior, ResolvedSet, Snapshot};
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
  daft config set <key> <value>  Change it for this worktree
  daft config set --global ...   Change it at the shared scope instead
  daft config unset <key>        Remove it, revealing whatever it was masking

Some settings only make sense together, and travel as a named behavior — one
name for the group and for the states it can be in:

  daft config get remote-sync    on, off, or custom
  daft config set remote-sync on Write every setting the state names

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
    /// The setting or behavior to read
    ///
    /// A behavior is a named group of settings that only make sense together
    /// — `remote-sync` is one. Reading it gives the state its settings add up
    /// to, or `custom` when they disagree.
    #[arg(value_name = "KEY")]
    key: String,

    /// Show every layer's value and which one won
    #[arg(long)]
    origin: bool,
}

#[derive(Args)]
pub struct SetArgs {
    /// The setting or behavior to change
    ///
    /// Naming a behavior — `remote-sync` — writes every setting its chosen
    /// state names, in one go.
    #[arg(value_name = "KEY")]
    key: String,

    /// The new value, or a behavior's state
    ///
    /// Hyphen-leading values are taken literally, because several settings
    /// hold flags — `daft.update.args` defaults to `--ff-only`, and requiring
    /// `--` before the most likely value would be a papercut on every use.
    /// Put `--global` before the key, or separate with `--`, if a value could
    /// be mistaken for a flag.
    #[arg(value_name = "VALUE", allow_hyphen_values = true)]
    value: String,

    /// Write at the shared scope rather than this worktree's own
    ///
    /// Where that is depends on what stores the setting: the user's global git
    /// config, the repository's committed daft.yml, or the global layout file.
    /// A daft.yml setting is the one to watch — its shared scope is a tracked
    /// file, so the change lands in the repository's diff rather than
    /// user-wide. Every write says which file it went to.
    #[arg(long)]
    global: bool,
}

#[derive(Args)]
pub struct UnsetArgs {
    /// The setting or behavior to remove
    ///
    /// Naming a behavior clears every setting it covers at this scope, and
    /// reports what that revealed.
    #[arg(value_name = "KEY")]
    key: String,

    /// Remove at the shared scope rather than this worktree's own
    ///
    /// The same three stores `set --global` writes: global git config, the
    /// committed daft.yml, or the global layout file.
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
        None => cmd_default(),
    }
}

/// The bare command. Opens the browser when there is a terminal to draw on,
/// and falls back to `list` when there is not — a piped `daft config` should
/// print something useful rather than refuse.
fn cmd_default() -> Result<()> {
    if interactive() {
        let in_repo = crate::is_git_repository().unwrap_or(false);
        let state = screen::state::ScreenState::new(resolved()?, in_repo, repo_label());
        return screen::run(state);
    }

    cmd_list(&ListArgs {
        modified: false,
        category: None,
        emit: EmitArgs::default(),
    })
}

/// Whether to take over the terminal.
///
/// Stderr, because that is where the screen draws — stdout may well be a pipe
/// while the user is still sitting at a terminal. `DAFT_TESTING` opts out so
/// suites that run daft without a PTY get the list rather than a screen
/// waiting for a keystroke that never comes.
fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal() && std::env::var("DAFT_TESTING").is_err()
}

/// The repository's name for the header, best-effort.
fn repo_label() -> Option<String> {
    crate::get_git_common_dir()
        .ok()
        .and_then(|dir| {
            std::path::Path::new(&dir)
                .parent()
                .and_then(|p| p.file_name())
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| !name.is_empty())
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

    // Behaviors are the entry point — "turn remote sync off" is what someone
    // arrives wanting, and the three keys underneath are the detail. A
    // category filter is asking about settings, so they drop out of it.
    let behaviors: Vec<&ResolvedBehavior> = if category.is_some() {
        Vec::new()
    } else {
        set.behaviors
            .iter()
            .filter(|b| !args.modified || b.is_set(&set.settings))
            .collect()
    };

    if args.emit.is_structured() {
        return emit::emit_and_handle(
            "config list",
            EmitPayload::Tabular(build_table(&rows, &behaviors, &set.settings)),
            &args.emit,
            &mut std::io::stdout(),
        )
        .map_err(|e| anyhow::anyhow!("{e}"));
    }

    if !behaviors.is_empty() {
        println!("{}", dim_underline("Behaviors"));
        let name_width = behaviors
            .iter()
            .map(|b| b.spec.name.chars().count())
            .max()
            .unwrap_or(0);
        for behavior in &behaviors {
            let state = behavior.state_label();
            let shown = if behavior.is_set(&set.settings) {
                bold(state)
            } else {
                state.to_string()
            };
            println!(
                "  {:name_width$}  {shown}  {}",
                behavior.spec.name,
                dim(&format!("{} settings", behavior.members.len())),
            );
            if let Some(note) = behavior.divergence_note(&set.settings) {
                println!("  {:name_width$}  {}", "", dim(&note));
            }
        }
        if !rows.is_empty() {
            println!();
        }
    }

    if rows.is_empty() {
        if behaviors.is_empty() {
            println!(
                "{}",
                if args.modified {
                    "Nothing is set — every setting is running on its default."
                } else {
                    "No settings matched."
                }
            );
        }
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
    // "Behaviors" is a heading in the listing but not a category, so someone
    // who read it off the screen and typed it back deserves the real answer
    // rather than a list that does not contain the word they just saw.
    if name.eq_ignore_ascii_case("behaviors") || name.eq_ignore_ascii_case("behavior") {
        bail!(
            "behaviors are not a category — they are named groups of settings.\n\
             `daft config list` shows them at the top; `daft config get <name>` \
             reads one."
        );
    }

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

/// One table for both kinds of row, told apart by `kind`.
///
/// A behavior has no single store and no single origin — its value is derived
/// from the settings named in `members`. Rather than leave a consumer to infer
/// that from empty cells, `kind` says which shape a row is and `backend` says
/// `derived` outright.
fn build_table(rows: &[&Resolved], behaviors: &[&ResolvedBehavior], all: &[Resolved]) -> Table {
    let mut table = Table::new([
        "kind", "key", "label", "category", "value", "origin", "is_set", "type", "default",
        "backend", "members", "help",
    ]);

    for behavior in behaviors {
        table = table.row([
            Cell::str("behavior"),
            Cell::str(behavior.spec.name),
            Cell::str(behavior.spec.label),
            Cell::str("Behaviors"),
            Cell::str(behavior.state_name()),
            Cell::str("derived"),
            Cell::bool(behavior.is_set(all)),
            Cell::str("preset"),
            Cell::str(behavior.spec.presets[0].name),
            Cell::str("derived"),
            Cell::str(behavior.spec.members.join(",")),
            Cell::str(behavior.spec.help),
        ]);
    }

    for row in rows {
        table = table.row([
            Cell::str("setting"),
            Cell::str(row.spec.key.as_ref()),
            Cell::str(row.spec.label.as_ref()),
            Cell::str(row.spec.category.label()),
            Cell::str(row.effective_display()),
            Cell::str(row.origin.label()),
            Cell::bool(row.is_set()),
            Cell::str(type_name(&row.spec.ty)),
            Cell::str(row.spec.default.value().unwrap_or("")),
            Cell::str(backend_name(&row.spec.backend)),
            Cell::str(""),
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
    let spec = match resolve::lookup_target(&args.key).map_err(|e| anyhow::anyhow!("{e}"))? {
        resolve::Target::Behavior(behavior) => return get_behavior(behavior, args.origin),
        resolve::Target::Setting(spec) => spec,
    };
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

/// `get` for a behavior.
///
/// Unlike a setting, this never exits 1: a behavior always has a state, even
/// when nothing sets a single member — that state is its default preset. The
/// one value it prints that cannot be set back is `custom`, which is what
/// "someone has been setting members individually" is called.
fn get_behavior(behavior: &'static BehaviorSpec, origin: bool) -> Result<()> {
    let set = resolved()?;
    let Some(resolved) = set.behavior(behavior.name) else {
        bail!("{} did not resolve", behavior.name);
    };

    if !origin {
        println!("{}", resolved.state_name());
        return Ok(());
    }

    println!("{}  {}", bold(behavior.label), dim(behavior.name));
    println!("{}", behavior.help);
    println!();

    println!("{}", dim_underline("States"));
    let width = behavior
        .presets
        .iter()
        .map(|preset| preset.name.chars().count())
        .max()
        .unwrap_or(0);
    for preset in behavior.presets {
        println!(
            "  {:width$}  {}  {}",
            preset.name,
            preset.label,
            dim(preset.help)
        );
    }
    println!();

    // The members with their own ladders. A behavior has no rung of its own,
    // so this *is* its provenance — flattening it to one line would be the
    // single-scope answer this command exists to stop giving.
    println!("{}", dim_underline("What it sets"));
    for index in &resolved.members {
        let member = &set.settings[*index];
        println!(
            "  {:32}  {:8}  {}",
            member.spec.key,
            bold(member.effective_display()),
            dim(&member.origin.label())
        );
    }

    println!();
    println!("  → {}", bold(resolved.state_label()));
    if let Some(note) = resolved.divergence_note(&set.settings) {
        println!("    {}", dim(&note));
    }

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
    let target = resolve::lookup_target(&args.key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let scope = scope_from_flag(args.global);
    require_repo_for_local(scope)?;

    let set = resolved()?;
    let message = match target {
        resolve::Target::Behavior(behavior) => {
            let preset = pick_preset(behavior, &args.value)?;
            write::set_behavior(behavior, preset, scope, &set, resolved)?.message
        }
        resolve::Target::Setting(spec) => write::set(&spec, scope, &args.value, &set)?,
    };

    println!("{message}");
    Ok(())
}

fn cmd_unset(args: &UnsetArgs) -> Result<()> {
    let target = resolve::lookup_target(&args.key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let scope = scope_from_flag(args.global);
    require_repo_for_local(scope)?;

    let message = match target {
        resolve::Target::Behavior(behavior) => {
            write::unset_behavior(behavior, scope, resolved)?.message
        }
        resolve::Target::Setting(spec) => write::unset(&spec, scope)?,
    };

    println!("{message}");
    Ok(())
}

/// The preset a user named, or a refusal that lists the real ones.
fn pick_preset(behavior: &'static BehaviorSpec, value: &str) -> Result<&'static Preset> {
    if let Some(preset) = behavior.preset(value) {
        return Ok(preset);
    }

    // `custom` is the one value `get` prints that `set` cannot take back, so
    // it earns its own sentence rather than falling into "expected one of".
    if value.trim().eq_ignore_ascii_case("custom") {
        bail!(
            "custom is what {} reads when its settings disagree, not a state you can \
             ask for.\nSet {} instead, or change the individual settings.",
            behavior.name,
            behavior.preset_names().join(" or ")
        );
    }

    bail!(
        "{value:?} is not a state of {} — expected {}.",
        behavior.name,
        behavior.preset_names().join(" or ")
    )
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

    /// `remote-sync` is no longer a verb of its own — it is a behavior, and it
    /// goes through the same four commands every other setting does.
    #[test]
    fn remote_sync_is_reached_through_the_quartet() {
        let command = ConfigArgs::command();
        let verbs: Vec<&str> = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        assert!(
            !verbs.contains(&"remote-sync"),
            "the subverb was folded into set/get/unset: {verbs:?}"
        );

        let parsed = ConfigArgs::parse_from(["config", "set", "--global", "remote-sync", "on"]);
        match parsed.command {
            Some(ConfigCommand::Set(args)) => {
                assert_eq!(args.key, "remote-sync");
                assert_eq!(args.value, "on");
                assert!(args.global);
            }
            _ => panic!("expected set"),
        }
    }

    #[test]
    fn a_behavior_name_resolves_to_a_behavior_and_a_key_does_not() {
        assert!(matches!(
            resolve::lookup_target("remote-sync"),
            Ok(resolve::Target::Behavior(_))
        ));
        assert!(matches!(
            resolve::lookup_target(crate::core::settings::keys::CHECKOUT_FETCH),
            Ok(resolve::Target::Setting(_))
        ));
    }

    #[test]
    fn a_misspelt_behavior_is_suggested_rather_than_lost_among_keys() {
        let err = resolve::lookup_target("remotesync").unwrap_err();
        assert!(
            err.contains("remote-sync"),
            "behavior names belong in the did-you-mean: {err}"
        );
    }

    #[test]
    fn custom_is_refused_as_a_state_to_ask_for() {
        let behavior = crate::core::settings_spec::find_behavior("remote-sync").unwrap();

        let err = pick_preset(behavior, "custom").unwrap_err().to_string();
        assert!(
            err.contains("not a state you can ask for"),
            "custom deserves its own sentence: {err}"
        );
        assert!(err.contains("off or on"), "and the real states: {err}");

        let err = pick_preset(behavior, "sometimes").unwrap_err().to_string();
        assert!(err.contains("expected off or on"), "{err}");

        assert_eq!(pick_preset(behavior, "ON").unwrap().name, "on");
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

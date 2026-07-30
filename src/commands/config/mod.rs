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

pub mod document;
pub mod resolve;
pub mod screen;
pub mod write;

use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::core::settings_spec::{Backend, BehaviorSpec, Category, Preset, ValueType};
use crate::git::{ConfigEntry, GitCommand};
use crate::output::emit::{self, Cell, EmitArgs, EmitPayload, Table};
use crate::styles::{bold, dim, dim_underline, yellow};
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
  daft config list --global      Only what the shared scope sets
  daft config get <key>          Print one effective value
  daft config get <key> --origin Print it with the full layer-by-layer chain
  daft config get <key> --local  Print what this worktree's own scope sets
  daft config set <key> <value>  Change it for this worktree
  daft config set --global ...   Change it at the shared scope instead
  daft config unset <key>        Remove it, revealing whatever it was masking

--local and --global name one layer, and name the same layer whether you are
reading or writing: `get --local` prints what `set --local` would replace. For
a git-config setting those are the two files git gives the same flags for; for
a daft.yml setting they are the local overlay and the committed config; for the
worktree layout, the repository's own entry and the global default.

A read narrowed to one layer exits 1 when that layer is silent, the way
`git config --get` does, so a script can ask whether something is set here
rather than only what it resolves to.

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

/// The two layer flags on a verb that reads.
///
/// [`WriteScope`] is the return type on purpose. The flag names the rung a
/// write at that scope would land in, which is what lets one pair of words
/// mean something for all three backends: `--local` is local git config for
/// the 77 git-config keys, the `daft.local.yml` overlay for a daft.yml key,
/// and the repository's own entry for the layout row. Read and write resolve
/// it through the same mapping, so `get --local` prints what `set --local`
/// would replace — and cannot drift into naming a different file than the
/// write does.
#[derive(Args, Default)]
struct ReadScopeArgs {
    /// Read the shared scope alone, rather than what daft resolves
    ///
    /// Global git config, the repository's committed daft.yml, or the global
    /// layout file, depending on what stores the setting.
    #[arg(long, conflicts_with = "local")]
    global: bool,

    /// Read this worktree's own scope alone, rather than what daft resolves
    ///
    /// Local git config, the daft.local.yml overlay, or the repository's layout
    /// entry. What comes back is what that layer stores, which is not always
    /// what daft reads: a value outranked from a higher layer still counts, and
    /// so does one daft only ever reads from global config. Use --origin to see
    /// whether a value is in force.
    #[arg(long)]
    local: bool,
}

impl ReadScopeArgs {
    /// The layer to read, or `None` to resolve the whole chain.
    fn selected(&self) -> Option<WriteScope> {
        match (self.global, self.local) {
            (true, _) => Some(WriteScope::Global),
            (_, true) => Some(WriteScope::Local),
            _ => None,
        }
    }
}

/// The two layer flags on a verb that writes. Local is the default.
///
/// "Target" rather than "write to", because `unset` shares this struct and does
/// not write anything — a help line that says it does would be wrong on a third
/// of the surface it appears on.
#[derive(Args)]
struct WriteScopeArgs {
    /// Target the shared scope rather than this worktree's own
    ///
    /// Where that is depends on what stores the setting: the user's global git
    /// config, the repository's committed daft.yml, or the global layout file.
    /// A daft.yml setting is the one to watch — its shared scope is a tracked
    /// file, so a change there lands in the repository's diff rather than
    /// user-wide. Every change says which file it went to.
    #[arg(long, conflicts_with = "local")]
    global: bool,

    /// Target this worktree's own scope — the default
    ///
    /// Local git config, the daft.local.yml overlay, or the repository's
    /// layout entry. Spelling it out changes nothing; it is here because a
    /// script that says which layer it means is one nobody has to guess about.
    #[arg(long)]
    local: bool,
}

impl WriteScopeArgs {
    fn target(&self) -> WriteScope {
        if self.global {
            WriteScope::Global
        } else {
            WriteScope::Local
        }
    }
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
    scope: ReadScopeArgs,

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
    ///
    /// A human affordance only: `--format` always carries the whole ladder, so
    /// a script never has to remember this flag to get a complete answer.
    #[arg(long)]
    origin: bool,

    #[command(flatten)]
    scope: ReadScopeArgs,

    #[command(flatten)]
    emit: EmitArgs,
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

    #[command(flatten)]
    scope: WriteScopeArgs,

    #[command(flatten)]
    emit: EmitArgs,
}

#[derive(Args)]
pub struct UnsetArgs {
    /// The setting or behavior to remove
    ///
    /// Naming a behavior clears every setting it covers at this scope, and
    /// reports what that revealed.
    #[arg(value_name = "KEY")]
    key: String,

    #[command(flatten)]
    scope: WriteScopeArgs,

    #[command(flatten)]
    emit: EmitArgs,
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
        scope: ReadScopeArgs::default(),
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

/// Refuse `--local` where there is no local layer to act on.
///
/// A read needs this as much as a write does, and for a sharper reason: with
/// no repository there is no local rung, so a narrowed read would exit 1 —
/// which means "nothing is set here" and would be reporting a fact about a
/// layer that does not exist. git draws the same line.
fn require_repo_for_local(scope: WriteScope, verb: Verb) -> Result<()> {
    if scope != WriteScope::Local || crate::is_git_repository().unwrap_or(false) {
        return Ok(());
    }

    bail!(
        "not inside a git repository — there is no local config to {}.\n{}",
        match verb {
            Verb::Read => "read",
            Verb::Write => "write",
        },
        match verb {
            Verb::Read =>
                "Use --global to read the shared scope, or drop the flag \
                           for the resolved value.",
            Verb::Write => "Use --global to change the setting for every repository.",
        }
    )
}

/// Which side of the flag pair a refusal is talking about.
#[derive(Clone, Copy)]
enum Verb {
    Read,
    Write,
}

/// Emit one object through the shared formatter.
///
/// Every verb but `list` produces a single document rather than a table, the
/// way `repo info` does — a setting's ladder and a write's record are both one
/// thing with nested detail, and flattening either into rows would lose the
/// nesting that makes them worth reading.
fn emit_document(command: &str, value: serde_json::Value, emit: &EmitArgs) -> Result<()> {
    emit::emit_and_handle(
        command,
        EmitPayload::Document(value),
        emit,
        &mut std::io::stdout(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}

// ─────────────────────────────────────────────────────────────────────────
// config list
// ─────────────────────────────────────────────────────────────────────────

fn cmd_list(args: &ListArgs) -> Result<()> {
    let scope = args.scope.selected();
    if let Some(scope) = scope {
        require_repo_for_local(scope, Verb::Read)?;
    }

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
        // A scope-narrowed listing is the contents of one layer, so a row
        // earns its place by having a value *there* — not by resolving to
        // one. That subsumes --modified, which is why passing both is a
        // no-op rather than a contradiction.
        .filter(|r| scope.is_none_or(|s| r.value_written_at(&r.spec, s).is_some()))
        .collect();

    // Behaviors are the entry point — "turn remote sync off" is what someone
    // arrives wanting, and the three keys underneath are the detail. A
    // category filter is asking about settings, so they drop out of it. So
    // does a scope filter, and for a stronger reason: a behavior's state is
    // defined over what daft reads across every layer, so there is no such
    // thing as its state at one of them.
    let behaviors: Vec<&ResolvedBehavior> = if category.is_some() || scope.is_some() {
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
            EmitPayload::Tabular(build_table(&rows, &behaviors, &set.settings, scope)),
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
            println!("{}", nothing_matched(args, scope));
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
        .map(|r| shown_value(r, scope).chars().count())
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

        let value = shown_value(row, scope);
        // Bold marks "something set this", which is the question the list is
        // most often scanned for. Narrowed to one layer that is every row, so
        // the emphasis would mark nothing and the column that earns the space
        // is the one saying whether the value is actually in force.
        let shown = match scope {
            Some(_) => value.to_string(),
            None if row.is_set() => bold(value),
            None => value.to_string(),
        };
        let pad = value_width.saturating_sub(value.chars().count());
        let note = match scope {
            Some(scope) => row
                .masked_above(&row.spec, scope)
                .map(|layer| yellow(&format!("outranked by {}", layer.label())))
                .unwrap_or_default(),
            None => dim(&row.origin.label()),
        };

        println!("  {:key_width$}  {shown}{:pad$}  {note}", row.spec.key, "");
    }

    // Unrecognized keys belong to no row, so `get <key> --origin` cannot
    // explain them — they have to be named here or the advice is unactionable.
    // A narrowed listing shows only the ones in the layer it is reporting on,
    // for the same reason it drops rows set elsewhere.
    let unrecognized: Vec<&ConfigEntry> = set
        .unrecognized
        .iter()
        .filter(|entry| scope.is_none_or(|s| entry.scope == s.as_config_scope()))
        .collect();
    if !unrecognized.is_empty() {
        println!();
        println!(
            "{}",
            dim_underline("Set in config, but not settings daft knows")
        );
        for entry in &unrecognized {
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

    // Counted over the rows on screen, not the whole configuration: a listing
    // narrowed to one layer promising attention for a diagnostic in another
    // sends the reader looking for a row that is not there.
    let attached: usize = rows.iter().map(|r| r.diagnostics.len()).sum();
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

/// The value a listing shows for a row: the layer's own, or the resolved one.
fn shown_value(row: &Resolved, scope: Option<WriteScope>) -> &str {
    match scope {
        Some(scope) => row.value_written_at(&row.spec, scope).unwrap_or("—"),
        None => row.effective_display(),
    }
}

fn nothing_matched(args: &ListArgs, scope: Option<WriteScope>) -> String {
    match scope {
        Some(scope) => format!("Nothing is set at {}.", scope.label()),
        None if args.modified => {
            "Nothing is set — every setting is running on its default.".to_string()
        }
        None => "No settings matched.".to_string(),
    }
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
///
/// `scope` changes what `value` holds — the layer's own value rather than the
/// resolved one — which is what the caller asked for and the whole point of a
/// narrowed listing. `origin` still names the layer in force, so the two
/// disagreeing is information rather than a contradiction; `outranked_by`
/// spells that out so a consumer does not have to compare them.
fn build_table(
    rows: &[&Resolved],
    behaviors: &[&ResolvedBehavior],
    all: &[Resolved],
    scope: Option<WriteScope>,
) -> Table {
    let mut table = Table::new([
        "kind",
        "key",
        "label",
        "category",
        "value",
        "origin",
        "is_set",
        "type",
        "default",
        "backend",
        "members",
        "outranked_by",
        "help",
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
            Cell::str(""),
            Cell::str(behavior.spec.help),
        ]);
    }

    for row in rows {
        let outranked = scope
            .and_then(|s| row.masked_above(&row.spec, s))
            .map(|layer| layer.label())
            .unwrap_or_default();

        table = table.row([
            Cell::str("setting"),
            Cell::str(row.spec.key.as_ref()),
            Cell::str(row.spec.label.as_ref()),
            Cell::str(row.spec.category.label()),
            Cell::str(shown_value(row, scope)),
            Cell::str(row.origin.label()),
            Cell::bool(row.is_set()),
            Cell::str(type_name(&row.spec.ty)),
            Cell::str(row.spec.default.value().unwrap_or("")),
            Cell::str(backend_name(&row.spec.backend)),
            Cell::str(""),
            Cell::str(outranked),
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
    let scope = args.scope.selected();

    // --origin is the answer to "which layers set this" — it prints all of
    // them and marks the one daft reads. Narrowing it to a single layer would
    // leave a ladder with one rung, which answers nothing the plain form does
    // not already answer better.
    if args.origin && scope.is_some() {
        bail!(
            "--origin already shows every layer, including this one. Use it alone, \
             or use --global/--local alone to print just that layer's value."
        );
    }

    let spec = match resolve::lookup_target(&args.key).map_err(|e| anyhow::anyhow!("{e}"))? {
        resolve::Target::Behavior(behavior) => return get_behavior(behavior, scope, args),
        resolve::Target::Setting(spec) => spec,
    };

    if let Some(scope) = scope {
        require_repo_for_local(scope, Verb::Read)?;
    }

    let set = resolved()?;
    let Some(row) = set.get(&spec.key) else {
        bail!("{} did not resolve", spec.key);
    };

    // Narrowed to one layer, the value is that layer's own and "unset" means
    // that layer stores nothing — which is the literal question, so a value
    // here that daft never reads still counts. `--origin`, and the document's
    // `outranked_by`, are what say whether it is in force.
    let value = match scope {
        Some(scope) => row.value_written_at(&row.spec, scope),
        None => row.effective.as_deref(),
    };

    if args.emit.is_structured() {
        emit_document("config get", document::setting(row, scope), &args.emit)?;
    } else if args.origin {
        print_detail(row);
        return Ok(());
    } else if let Some(value) = value {
        // Bare `get` is for scripts: the value on stdout and nothing else.
        println!("{value}");
    }

    // The same contract `git config --get` has, and the same one whether the
    // caller took the value off stdout or out of a document.
    match value {
        Some(_) => Ok(()),
        // `--origin` printed a whole ladder, including the reason there is no
        // value. Exiting 1 after that would report failure for a question that
        // was answered.
        None if args.origin && !args.emit.is_structured() => Ok(()),
        None => std::process::exit(1),
    }
}

/// `get` for a behavior.
///
/// Unnarrowed this never exits 1: a behavior always has a state, even when
/// nothing sets a single member — that state is its default preset. The one
/// value it prints that cannot be set back is `custom`, which is what "someone
/// has been setting members individually" is called.
///
/// Narrowed to a layer it can exit 1, because a layer need not name a state at
/// all. A behavior has no rung of its own, so "its state here" is only
/// meaningful through its write surface, which is all-or-nothing: `set
/// <behavior> <state> --local` writes every member there and `unset` clears
/// every one. This answers the question those two pose — did such a write
/// happen here, and what did it leave. A layer with only *some* members set
/// names nothing, and saying so is the point: guessing the nearest preset from
/// a partial layer is exactly how the retired `remote-sync --status` came to
/// report "Local only" while daft was fetching.
fn get_behavior(
    behavior: &'static BehaviorSpec,
    scope: Option<WriteScope>,
    args: &GetArgs,
) -> Result<()> {
    if let Some(scope) = scope {
        require_repo_for_local(scope, Verb::Read)?;
    }

    let set = resolved()?;
    let Some(resolved) = set.behavior(behavior.name) else {
        bail!("{} did not resolve", behavior.name);
    };
    let state = match scope {
        Some(scope) => resolved.state_at(scope, &set.settings),
        None => Some(resolved.state.clone()),
    };

    if args.emit.is_structured() {
        emit_document(
            "config get",
            document::behavior(resolved, &set, scope, state.as_ref()),
            &args.emit,
        )?;
    } else if args.origin {
        print_behavior_detail(behavior, resolved, &set);
        return Ok(());
    } else if let Some(state) = &state {
        println!("{}", state.name(behavior));
    }

    match state {
        Some(_) => Ok(()),
        None => std::process::exit(1),
    }
}

/// The CLI form of a behavior's detail: its states, its members' ladders, and
/// where it currently stands.
fn print_behavior_detail(
    behavior: &'static BehaviorSpec,
    resolved: &ResolvedBehavior,
    set: &ResolvedSet,
) {
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
    for line in ladder_lines(row) {
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

/// The "Where it comes from" block, as lines.
///
/// Built rather than printed straight so the mark can be asserted. It has to
/// land on the layer the effective line underneath names, and the two came
/// apart once already: this marked `winner`, which is `None` for every setting
/// nothing sets — most of them — so the common row carried no mark at all, and
/// where a layer held an unparseable value the mark went to that layer while
/// the effective line named the default the loaders had actually fallen back
/// to. `reads_from` is the one the screen's ladder uses, for the same reason.
fn ladder_lines(row: &Resolved) -> Vec<String> {
    let reads_from = row.reads_from();
    row.rungs
        .iter()
        .enumerate()
        .map(|(index, rung)| {
            let marker = if reads_from == Some(index) {
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
            line
        })
        .collect()
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
    let scope = args.scope.target();
    require_repo_for_local(scope, Verb::Write)?;

    let set = resolved()?;
    match target {
        resolve::Target::Behavior(behavior) => {
            let preset = pick_preset(behavior, &args.value)?;
            let done = write::set_behavior(behavior, preset, scope, &set, resolved)?;
            report(
                document::Action::Set,
                behavior_target(behavior, &done),
                scope,
                &done.written,
                &done.message,
                &args.emit,
            )
        }
        resolve::Target::Setting(spec) => {
            let done = write::set(&spec, scope, &args.value, &set)?;
            report(
                document::Action::Set,
                document::Target::Setting(&spec),
                scope,
                &done.written,
                &done.message,
                &args.emit,
            )
        }
    }
}

fn cmd_unset(args: &UnsetArgs) -> Result<()> {
    let target = resolve::lookup_target(&args.key).map_err(|e| anyhow::anyhow!("{e}"))?;
    let scope = args.scope.target();
    require_repo_for_local(scope, Verb::Write)?;

    match target {
        resolve::Target::Behavior(behavior) => {
            let done = write::unset_behavior(behavior, scope, resolved)?;
            report(
                document::Action::Unset,
                behavior_target(behavior, &done),
                scope,
                &done.written,
                &done.message,
                &args.emit,
            )
        }
        resolve::Target::Setting(spec) => {
            let done = write::unset(&spec, scope)?;
            report(
                document::Action::Unset,
                document::Target::Setting(&spec),
                scope,
                &done.written,
                &done.message,
                &args.emit,
            )
        }
    }
}

fn behavior_target<'a>(
    behavior: &'a BehaviorSpec,
    done: &'a write::BehaviorWrite,
) -> document::Target<'a> {
    document::Target::Behavior {
        spec: behavior,
        state: done.state,
    }
}

/// Narrate a write, as prose or as a record.
///
/// One path for both, so the two cannot describe different writes: the
/// document's `message` is the same sentence the text form prints.
fn report(
    action: document::Action,
    target: document::Target<'_>,
    scope: WriteScope,
    written: &[write::Written],
    message: &str,
    emit: &EmitArgs,
) -> Result<()> {
    if !emit.is_structured() {
        println!("{message}");
        return Ok(());
    }

    // The store, not just the scope: a daft.yml setting's "global" is the
    // repository's committed file, and a consumer told only "global" would
    // record a user-wide change when the edit is in the team's diff.
    let store = match &target {
        document::Target::Setting(spec) => scope.label_for(spec),
        // A behavior's members can sit in different backends, so there is no
        // one store to name — each entry carries its own file where the backend
        // knows one.
        document::Target::Behavior { .. } => scope.label(),
    };

    emit_document(
        "config write",
        document::write_record(action, target, scope, store, written, message),
        emit,
    )
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
            Some(ConfigCommand::Unset(args)) => {
                assert_eq!(args.scope.target(), WriteScope::Global)
            }
            _ => panic!("expected unset"),
        }

        let parsed = ConfigArgs::parse_from(["config", "list", "--modified"]);
        match parsed.command {
            Some(ConfigCommand::List(args)) => assert!(args.modified),
            _ => panic!("expected list"),
        }
    }

    /// git's spelling, on every verb, in both directions — and the pair is
    /// mutually exclusive, since a write cannot land in two places.
    #[test]
    fn every_verb_takes_the_same_two_layer_flags() {
        for verb in [
            vec!["config", "get", "daft.remote", "--local"],
            vec!["config", "list", "--local"],
            vec!["config", "set", "--local", "daft.remote", "up"],
            vec!["config", "unset", "--local", "daft.remote"],
        ] {
            let selected = match ConfigArgs::parse_from(verb.clone()).command {
                Some(ConfigCommand::Get(args)) => args.scope.selected(),
                Some(ConfigCommand::List(args)) => args.scope.selected(),
                Some(ConfigCommand::Set(args)) => Some(args.scope.target()),
                Some(ConfigCommand::Unset(args)) => Some(args.scope.target()),
                None => panic!("expected a verb from {verb:?}"),
            };
            assert_eq!(selected, Some(WriteScope::Local), "{verb:?}");
        }

        for verb in [
            vec!["config", "get", "daft.remote", "--local", "--global"],
            vec!["config", "list", "--global", "--local"],
            vec!["config", "set", "--global", "--local", "daft.remote", "up"],
            vec!["config", "unset", "--local", "--global", "daft.remote"],
        ] {
            assert!(
                ConfigArgs::try_parse_from(verb.clone()).is_err(),
                "{verb:?} names two layers for one act"
            );
        }
    }

    /// Absent both flags the two shapes part ways: a read resolves the whole
    /// chain, a write lands locally.
    #[test]
    fn no_flag_means_resolve_for_a_read_and_local_for_a_write() {
        match ConfigArgs::parse_from(["config", "get", "daft.remote"]).command {
            Some(ConfigCommand::Get(args)) => assert_eq!(args.scope.selected(), None),
            _ => panic!("expected get"),
        }
        match ConfigArgs::parse_from(["config", "set", "daft.remote", "up"]).command {
            Some(ConfigCommand::Set(args)) => {
                assert_eq!(args.scope.target(), WriteScope::Local)
            }
            _ => panic!("expected set"),
        }
    }

    /// Regression: the CLI ladder marked `winner`, where the screen's marks
    /// `reads_from`. Both cases the two disagree on, since either alone would
    /// pass against the wrong one.
    #[test]
    fn the_ladder_marks_the_layer_the_effective_value_is_read_from() {
        use crate::core::settings::keys;
        use crate::git::{ConfigEntry, ConfigScope};

        let resolve = |entries: Vec<ConfigEntry>| {
            resolve::resolve_all(&Snapshot {
                entries,
                env: std::collections::HashMap::new(),
                in_repo: true,
                yaml: None,
                layout: None,
            })
        };
        let marked = |row: &Resolved| -> String {
            let lines: Vec<String> = ladder_lines(row)
                .into_iter()
                .filter(|line| line.contains('●'))
                .collect();
            assert_eq!(lines.len(), 1, "exactly one layer is read: {lines:?}");
            lines.into_iter().next().unwrap()
        };

        // Nothing set: `winner` is None here, so marking it marked nothing.
        let set = resolve(vec![]);
        let row = set.get(keys::MERGE_STYLE).expect("registry row");
        assert!(
            marked(row).contains("default"),
            "an unset setting reads its default, and the ladder must say so"
        );

        // A value the type cannot parse: `winner` is local, but the loaders
        // fall back past it to the default rather than to the layer below — so
        // marking local would contradict the effective line under it.
        let set = resolve(vec![ConfigEntry {
            key: keys::MERGE_STYLE.to_string(),
            value: "octopus-ish".to_string(),
            scope: ConfigScope::Local,
            origin_path: None,
        }]);
        let row = set.get(keys::MERGE_STYLE).expect("registry row");
        assert!(row.is_set(), "something did set it, invalid though it is");
        let line = marked(row);
        assert!(
            line.contains("default") && !line.contains("local"),
            "an unparseable value is not what daft reads: {line}"
        );
    }

    /// Every verb emits, and the shape decides the formats: `list` is rows,
    /// the other three are one document each. A row format offered for a
    /// document would be a completion that cannot be accepted.
    #[test]
    fn every_verb_is_registered_as_emit_enabled_in_its_own_shape() {
        use crate::commands::completions::emit_formats_for;

        let list = emit_formats_for("config list").expect("config list emits");
        assert!(
            list.contains(&"tsv"),
            "rows carry the row formats: {list:?}"
        );

        for path in ["config get", "config set", "config unset"] {
            let formats = emit_formats_for(path).unwrap_or_else(|| panic!("{path} emits"));
            assert!(formats.contains(&"json"), "{path}: {formats:?}");
            assert!(
                !formats.contains(&"tsv"),
                "{path} emits one document, which has no rows to tabulate: {formats:?}"
            );
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
                assert_eq!(args.scope.target(), WriteScope::Global);
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

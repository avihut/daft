//! Effective-value and provenance resolution.
//!
//! "What is my configuration?" has no single answer in daft — it is a stack,
//! and the useful question is *which layer won, and what did the others say*.
//! This module answers both, for every setting at once, from one config
//! snapshot.
//!
//! It is deliberately pure: [`resolve_all`] takes a [`Snapshot`] of what the
//! files and environment currently hold and returns [`Resolved`] rows. No
//! subprocess, no gitoxide, no `std::env` — so the precedence rules, which are
//! the part that is easy to get subtly wrong, are unit-testable without a
//! repository on disk.
//!
//! The rules it reproduces, each mirroring a loader in
//! [`crate::core::settings`]:
//!
//! - **Scope precedence** is git's: system < global < local < worktree <
//!   process-scoped. Above the files sits any `DAFT_*` env override a row
//!   declares; below them the built-in default.
//! - **A `global_only` key** is read with `git config --global --get`, so a
//!   value at any other scope is inert — set, visible in `git config`, and
//!   doing nothing.
//! - **A deprecated alias** is a whole-ladder fallback, not a per-scope one:
//!   daft asks for the real key across every scope and only consults the old
//!   spelling when that comes back empty.
//! - **An unparseable value does not fall through to the next scope.** The
//!   loaders fold a bad value into the *default* — so a broken `local` value
//!   masks a perfectly good `global` one. That is worth showing rather than
//!   smoothing over.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::layout::resolver::LayoutSource;
use crate::core::settings_spec::{Backend, SettingLookup, SettingSpec, all_specs, find};
use crate::git::{ConfigEntry, ConfigScope};

// ─────────────────────────────────────────────────────────────────────────
// Input
// ─────────────────────────────────────────────────────────────────────────

/// Everything resolution reads, captured once.
///
/// Taking this as data rather than reaching for the repository is what keeps
/// [`resolve_all`] pure — and what lets the screen re-resolve after a write
/// without re-walking anything but the config.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Every `daft.*` entry visible from the repository, keys as spelled.
    pub entries: Vec<ConfigEntry>,
    /// The `DAFT_*` overrides the registry declares, when set.
    pub env: HashMap<String, String>,
    /// False when daft is running outside a repository: there is no local
    /// scope to read or write, and the screen says so.
    pub in_repo: bool,
    /// The two `daft.yml` files, when there is a repository to read them
    /// from. `None` leaves those rows on their defaults.
    pub yaml: Option<YamlLayers>,
    /// What each layer of the worktree-layout chain says, when there is a
    /// repository to ask. `None` leaves the layout row on its built-in
    /// default, which is what daft would use anyway.
    pub layout: Option<LayoutRungs>,
}

/// Every layer of the layout chain, already read.
///
/// The layout is not a git-config key: six layers across three different
/// stores decide where worktrees go, and the one that surprises people is the
/// repo store outranking the committed `daft.yml` — a team convention a
/// teammate's local choice silently overrides. Showing the whole chain is the
/// point of giving it a row at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayoutRungs {
    /// `--layout` on this invocation. Never set here (the screen has no CLI
    /// flag), but present so the ladder shows the layer exists.
    pub cli: Option<String>,
    /// The per-repo choice in the trust store.
    pub repo_store: Option<String>,
    /// The committed `daft.yml layout:` field.
    pub yaml: Option<String>,
    /// `defaults.layout` in the global TOML.
    pub global: Option<String>,
    /// Inferred from where the worktrees actually are.
    pub detected: Option<String>,
    /// What daft resolves to, and which layer decided.
    pub effective: String,
    pub source: LayoutSource,
}

impl LayoutRungs {
    /// Read the chain from the current repository.
    pub fn capture() -> Option<Self> {
        use crate::core::global_config::GlobalConfig;
        use crate::core::layout::detect::detect_layout;
        use crate::core::layout::resolver::{
            DetectionResult, LayoutResolutionContext, resolve_layout,
        };
        use crate::hooks::{TrustDatabase, yaml_config_loader};

        let global_config = GlobalConfig::load().unwrap_or_default();
        let git_dir = crate::get_git_common_dir().ok()?;
        let trust_db = TrustDatabase::load().unwrap_or_default();

        let yaml = crate::get_current_worktree_path()
            .ok()
            .and_then(|wt| yaml_config_loader::load_merged_config(&wt).ok().flatten())
            .and_then(|cfg| cfg.layout);
        let repo_store = trust_db.get_layout(&git_dir).map(String::from);

        // Detection is only run when nothing explicit is set, matching
        // `daft layout show` — walking the filesystem to answer a question
        // already answered would be a cost for nothing.
        let detection = if repo_store.is_none() && yaml.is_none() {
            Some(detect_layout(&git_dir, &global_config))
        } else {
            None
        };
        let detected = match &detection {
            Some(DetectionResult::Detected(layout)) => Some(layout.name.clone()),
            _ => None,
        };

        let (layout, source) = resolve_layout(&LayoutResolutionContext {
            cli_layout: None,
            repo_store_layout: repo_store.as_deref(),
            yaml_layout: yaml.as_deref(),
            global_config: &global_config,
            detection,
        });

        Some(Self {
            cli: None,
            repo_store,
            yaml,
            global: global_config.defaults.layout.clone(),
            detected,
            effective: layout.name,
            source,
        })
    }

    /// The layers in precedence order, lowest first — the order a ladder
    /// renders in.
    pub(crate) fn rungs(&self) -> Vec<(LayoutSource, Option<String>)> {
        vec![
            (LayoutSource::Detected, self.detected.clone()),
            (LayoutSource::GlobalConfig, self.global.clone()),
            (LayoutSource::YamlConfig, self.yaml.clone()),
            (LayoutSource::RepoStore, self.repo_store.clone()),
            (LayoutSource::Cli, self.cli.clone()),
        ]
    }
}

/// One `daft.yml` file, parsed, with where it came from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct YamlLayer {
    pub path: PathBuf,
    pub doc: serde_yaml::Value,
    /// False for a visitor's own untracked `daft.yml` — the file is the same
    /// shape, but it is not the team's committed convention and the ladder
    /// should not imply it is.
    pub tracked: bool,
}

/// The committed config and the local overlay that outranks it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct YamlLayers {
    pub main: Option<YamlLayer>,
    pub local: Option<YamlLayer>,
}

impl YamlLayers {
    /// Read both files from the current worktree.
    pub fn capture() -> Option<Self> {
        use crate::hooks::yaml_config_loader;

        let worktree = crate::get_current_worktree_path().ok()?;
        let (main_path, _) = yaml_config_loader::find_config_file(&worktree)?;

        let read = |path: &std::path::Path| -> Option<serde_yaml::Value> {
            let text = std::fs::read_to_string(path).ok()?;
            serde_yaml::from_str(&text).ok()
        };

        let tracked = matches!(
            yaml_config_loader::classify_main_config(&worktree),
            yaml_config_loader::ConfigStatus::Tracked
        );
        let main = read(&main_path).map(|doc| YamlLayer {
            path: main_path.clone(),
            doc,
            tracked,
        });
        let local = yaml_config_loader::find_local_config(&main_path).and_then(|path| {
            read(&path).map(|doc| YamlLayer {
                path,
                doc,
                tracked: false,
            })
        });

        Some(Self { main, local })
    }
}

/// The scalar at a dotted path, rendered the way a user would type it.
fn scalar_at(doc: &serde_yaml::Value, path: &str) -> Option<String> {
    let mut current = doc;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    match current {
        serde_yaml::Value::String(text) => Some(text.clone()),
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        // Sequences and mappings are not scalars; the row that owns them says
        // so rather than rendering a shape the editor cannot write back.
        _ => None,
    }
}

/// The ladder label for a layout layer.
pub fn layout_layer_label(source: LayoutSource) -> &'static str {
    match source {
        LayoutSource::Cli => "--layout",
        LayoutSource::RepoStore => "repo store",
        LayoutSource::YamlConfig => "daft.yml",
        LayoutSource::GlobalConfig => "global toml",
        LayoutSource::Detected => "detected",
        LayoutSource::Unresolved => "built-in",
    }
}

impl Snapshot {
    /// Capture the current repository's configuration.
    pub fn capture(git: &crate::git::GitCommand) -> anyhow::Result<Self> {
        let entries = git.daft_config_entries().unwrap_or_default();
        Ok(Self {
            entries,
            env: capture_env(),
            in_repo: true,
            yaml: YamlLayers::capture(),
            layout: LayoutRungs::capture(),
        })
    }

    /// Capture what is visible from outside any repository: system and global
    /// config only.
    pub fn capture_global_only() -> anyhow::Result<Self> {
        Ok(Self {
            entries: crate::git::daft_config_entries_global().unwrap_or_default(),
            env: capture_env(),
            in_repo: false,
            // No repository means no daft.yml, no repo store, and nothing to
            // detect.
            yaml: None,
            layout: None,
        })
    }
}

/// Read the `DAFT_*` overrides any registry row declares.
fn capture_env() -> HashMap<String, String> {
    all_specs()
        .into_iter()
        .filter_map(|spec| spec.env_override)
        .filter_map(|var| {
            std::env::var(var)
                .ok()
                .map(|value| (var.to_string(), value))
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────
// Output
// ─────────────────────────────────────────────────────────────────────────

/// One layer of a setting's ladder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    /// The built-in default, or the inherited parent's effective value.
    Default,
    /// The retired spelling, consulted only when the real key is unset
    /// everywhere.
    Alias { key: String, scope: ConfigScope },
    /// A git config file scope.
    Git(ConfigScope),
    /// An environment variable that outranks every config file.
    Env(&'static str),
    /// One layer of the worktree-layout chain.
    Layout(LayoutSource),
    /// A `daft.yml` file. `tracked` distinguishes the team's committed
    /// convention from a visitor's own untracked copy.
    Yaml { file: String, tracked: bool },
}

impl Layer {
    /// The ladder label.
    pub fn label(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Alias { key, scope } => format!("{key} ({})", scope.label()),
            Self::Git(scope) => scope.label().to_string(),
            Self::Env(var) => format!("${var}"),
            Self::Layout(source) => layout_layer_label(*source).to_string(),
            Self::Yaml { file, tracked } => {
                if *tracked {
                    file.clone()
                } else {
                    format!("{file} (untracked)")
                }
            }
        }
    }
}

/// Why a rung's value does not count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InertReason {
    /// daft reads this key from global config only, so a value here is set
    /// and ignored.
    NotReadAtThisScope,
}

/// One layer's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rung {
    pub layer: Layer,
    /// What this layer says; `None` when it is silent.
    pub value: Option<String>,
    /// The file it came from, when there is one.
    pub origin_path: Option<PathBuf>,
    /// Whether `daft config` can write here.
    pub writable: bool,
    /// Set when the value is present but does not count.
    pub inert: Option<InertReason>,
}

/// Something worth telling the user about a setting's current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// A stored value the setting's own type cannot parse. daft falls back to
    /// the default, so this value is not doing what its author expected.
    Invalid {
        layer: Layer,
        value: String,
        reason: String,
    },
    /// The effective value comes from a retired key spelling.
    Deprecated { alias: String, replacement: String },
    /// Set at a scope daft never reads for this key.
    Inert {
        scope: ConfigScope,
        value: String,
        reason: InertReason,
    },
    /// A process-scoped entry outranks every file — nothing written to a
    /// config file will take effect while it is set.
    EnvShadow { layer: Layer, value: String },
}

/// Where the effective value came from, once everything is accounted for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Nothing sets it.
    Default,
    /// Nothing sets it, and its default is another setting's effective value.
    Inherited(&'static str),
    /// A layer set it.
    Layer(Layer),
    /// A layer set it to something unparseable, so the default applies —
    /// which is not the same as nothing setting it, and should not read that
    /// way.
    DefaultAfterInvalid(Layer),
}

impl Origin {
    /// The parenthetical after an effective value.
    pub fn label(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Inherited(key) => format!("inherited from {key}"),
            Self::Layer(layer) => layer.label(),
            Self::DefaultAfterInvalid(layer) => {
                format!("default — the {} value is not valid", layer.label())
            }
        }
    }
}

/// One setting, fully resolved.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub spec: SettingSpec,
    /// Every layer, lowest precedence first.
    pub rungs: Vec<Rung>,
    /// Index into `rungs` of the layer git would answer with, if any. This is
    /// the ● in the ladder — it can point at an invalid value.
    pub winner: Option<usize>,
    /// What daft actually uses — `None` when nothing sets it and it has no
    /// default, which is a real state for the settings whose absence is
    /// itself meaningful (`daft.merge.edit` lets git decide).
    pub effective: Option<String>,
    pub origin: Origin,
    pub diagnostics: Vec<Diagnostic>,
}

impl Resolved {
    /// Whether anything at all sets this, as opposed to it running on its
    /// default. Drives the "Modified" filter and the bold value in the list.
    pub fn is_set(&self) -> bool {
        self.winner.is_some()
    }

    /// The effective value as a row renders it: the em dash stands for
    /// "nothing", which belongs in the display and not in the data.
    pub fn effective_display(&self) -> &str {
        self.effective.as_deref().unwrap_or("—")
    }

    /// The value stored at `scope`, if any — what the editor pre-selects and
    /// what `unset` would remove.
    pub fn value_at(&self, scope: ConfigScope) -> Option<&str> {
        self.rungs
            .iter()
            .find(|rung| rung.layer == Layer::Git(scope))
            .and_then(|rung| rung.value.as_deref())
    }
}

/// Every setting resolved together, plus what did not resolve to anything.
#[derive(Debug, Clone, Default)]
pub struct ResolvedSet {
    pub settings: Vec<Resolved>,
    /// `daft.*` entries in the config that match no registry row — typos, or
    /// keys from a newer daft. Inert either way, and worth saying so: a
    /// mis-cased subsection is a different key to git and silently does
    /// nothing.
    pub unrecognized: Vec<ConfigEntry>,
    /// The layout chain this was resolved against, carried through so a
    /// caller can reuse it rather than re-walking the filesystem for a write
    /// that cannot have moved it.
    pub layout_rungs: Option<LayoutRungs>,
}

impl ResolvedSet {
    /// Look a setting up by its canonical key.
    pub fn get(&self, key: &str) -> Option<&Resolved> {
        self.settings.iter().find(|r| r.spec.key == key)
    }

    /// Every diagnostic across every setting, plus the unrecognized keys.
    pub fn issue_count(&self) -> usize {
        self.settings
            .iter()
            .map(|r| r.diagnostics.len())
            .sum::<usize>()
            + self.unrecognized.len()
    }
}

impl SettingLookup for ResolvedSet {
    fn effective(&self, key: &str) -> Option<String> {
        self.get(key).and_then(|r| r.effective.clone())
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Git key matching
// ─────────────────────────────────────────────────────────────────────────

/// A git config key split the way git itself compares them.
///
/// Section and value names are case-insensitive; the subsection between them
/// is case-**sensitive**. That asymmetry is not a curiosity — it is why
/// `daft.checkoutbranch.carry` is a different key from
/// `daft.checkoutBranch.carry` and quietly does nothing, and why the screen
/// can report the typo instead of attributing the value to the real row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitKey<'a> {
    pub section: &'a str,
    pub subsection: Option<&'a str>,
    pub name: &'a str,
}

impl<'a> GitKey<'a> {
    pub fn parse(key: &'a str) -> Option<Self> {
        let (section, rest) = key.split_once('.')?;
        if section.is_empty() {
            return None;
        }
        match rest.rsplit_once('.') {
            Some((subsection, name)) if !subsection.is_empty() && !name.is_empty() => Some(Self {
                section,
                subsection: Some(subsection),
                name,
            }),
            Some(_) => None,
            None if !rest.is_empty() => Some(Self {
                section,
                subsection: None,
                name: rest,
            }),
            None => None,
        }
    }

    /// Whether two spellings name the same key under git's rules.
    pub fn matches(&self, other: &GitKey<'_>) -> bool {
        self.section.eq_ignore_ascii_case(other.section)
            && self.subsection == other.subsection
            && self.name.eq_ignore_ascii_case(other.name)
    }
}

/// Whether `found` (as spelled in a config file) names `canonical`.
pub fn keys_match(canonical: &str, found: &str) -> bool {
    match (GitKey::parse(canonical), GitKey::parse(found)) {
        (Some(a), Some(b)) => a.matches(&b),
        _ => canonical == found,
    }
}

/// Find the registry row a user-typed key names.
///
/// Matching follows git's rules, so `DAFT.AutoCD` finds `daft.autocd` — but a
/// mis-cased *subsection* deliberately does not, because to git that is a
/// different key. Silently accepting it would write a second, inert
/// subsection under the guise of setting the real one.
///
/// The returned spec carries the registry's spelling, which is what any
/// subsequent write uses.
pub fn lookup(input: &str) -> Result<SettingSpec, String> {
    let specs = all_specs();

    if let Some(spec) = specs.iter().find(|spec| keys_match(&spec.key, input)) {
        return Ok(spec.clone());
    }

    if let Some(spec) = specs.iter().find(|spec| {
        spec.deprecated_alias
            .as_deref()
            .is_some_and(|alias| keys_match(alias, input))
    }) {
        return Err(format!(
            "{input} is the retired spelling of {}. daft still reads it, but set the \
             current key instead.",
            spec.key
        ));
    }

    let keys: Vec<String> = specs.iter().map(|spec| spec.key.to_string()).collect();
    let similar = crate::suggest::find_similar(input, &keys, 3);

    Err(match similar.len() {
        0 => format!("unknown setting: {input}\n\nRun `daft config list` to see them all."),
        1 => format!("unknown setting: {input}\n\nDid you mean {}?", similar[0]),
        _ => format!(
            "unknown setting: {input}\n\nDid you mean one of these?\n{}",
            similar
                .iter()
                .map(|key| format!("    {key}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    })
}

// ─────────────────────────────────────────────────────────────────────────
// Resolution
// ─────────────────────────────────────────────────────────────────────────

/// Resolve every setting in the registry against `snapshot`.
pub fn resolve_all(snapshot: &Snapshot) -> ResolvedSet {
    let specs = all_specs();

    // Two passes: settings that inherit read their parent's effective value,
    // so the parents have to be resolved first. Only one level of inheritance
    // exists (checkout.pushVerify → pushVerify) and the registry test asserts
    // every `inherits` names a real row, so a single re-pass suffices.
    let first: Vec<Resolved> = specs
        .iter()
        .map(|spec| resolve_one(spec, snapshot, None))
        .collect();

    let settings: Vec<Resolved> = specs
        .iter()
        .zip(&first)
        .map(|(spec, initial)| match spec.inherits {
            Some(parent) => {
                let inherited = first
                    .iter()
                    .find(|r| r.spec.key == parent)
                    .and_then(|r| r.effective.clone());
                resolve_one(spec, snapshot, inherited)
            }
            None => initial.clone(),
        })
        .collect();

    ResolvedSet {
        unrecognized: unrecognized_entries(snapshot, &specs),
        layout_rungs: snapshot.layout.clone(),
        settings,
    }
}

/// Resolve a single setting by key, against a snapshot.
pub fn resolve_key(key: &str, snapshot: &Snapshot) -> Option<Resolved> {
    let spec = find(key)?;
    let inherited = spec
        .inherits
        .and_then(|parent| find(parent).and_then(|p| resolve_one(&p, snapshot, None).effective));
    Some(resolve_one(&spec, snapshot, inherited))
}

/// Config entries that match no registry row.
fn unrecognized_entries(snapshot: &Snapshot, specs: &[SettingSpec]) -> Vec<ConfigEntry> {
    snapshot
        .entries
        .iter()
        .filter(|entry| {
            !specs.iter().any(|spec| {
                keys_match(&spec.key, &entry.key)
                    || spec
                        .deprecated_alias
                        .as_deref()
                        .is_some_and(|alias| keys_match(alias, &entry.key))
            })
        })
        .cloned()
        .collect()
}

fn resolve_one(spec: &SettingSpec, snapshot: &Snapshot, inherited: Option<String>) -> Resolved {
    let mut rungs = Vec::new();
    let mut diagnostics = Vec::new();

    // ── The bottom rung: what applies when nothing is set ────────────────
    let default_value = match (spec.default.value(), &inherited) {
        (Some(value), _) => Some(value.to_string()),
        (None, parent) => parent.clone(),
    };
    rungs.push(Rung {
        layer: Layer::Default,
        value: default_value.clone(),
        origin_path: None,
        writable: false,
        inert: None,
    });

    // ── The deprecated spelling, if the real key is silent everywhere ────
    let real_key_set = snapshot
        .entries
        .iter()
        .any(|entry| keys_match(&spec.key, &entry.key));

    if !real_key_set
        && let Some(alias) = spec.deprecated_alias.as_deref()
        && let Some(entry) = last_entry_for(snapshot, alias)
    {
        rungs.push(Rung {
            layer: Layer::Alias {
                key: alias.to_string(),
                scope: entry.scope,
            },
            value: Some(entry.value.clone()),
            origin_path: entry.origin_path.clone(),
            writable: false,
            inert: None,
        });
        diagnostics.push(Diagnostic::Deprecated {
            alias: alias.to_string(),
            replacement: spec.key.to_string(),
        });
    }

    // ── The layout chain, for the one row that is not a config key ───────
    if spec.backend == Backend::LayoutChain
        && let Some(chain) = &snapshot.layout
    {
        for (source, value) in chain.rungs() {
            rungs.push(Rung {
                layer: Layer::Layout(source),
                value,
                origin_path: None,
                // Two of the six layers are somewhere daft can write. The
                // committed daft.yml is a team convention this screen does not
                // edit, detection is an observation, and --layout is one
                // invocation's argument.
                writable: matches!(source, LayoutSource::RepoStore | LayoutSource::GlobalConfig),
                inert: None,
            });
        }
    }

    // ── The daft.yml files, committed first and overlay above ────────────
    if let Backend::DaftYml { path, .. } = spec.backend
        && let Some(files) = &snapshot.yaml
    {
        for (layer, file) in [(&files.main, "daft.yml"), (&files.local, "daft.local.yml")] {
            let Some(layer) = layer else { continue };
            rungs.push(Rung {
                layer: Layer::Yaml {
                    file: layer
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| file.to_string()),
                    tracked: layer.tracked,
                },
                value: scalar_at(&layer.doc, path),
                origin_path: Some(layer.path.clone()),
                writable: spec.is_writable(),
                inert: None,
            });
        }
    }

    // ── The config files, lowest precedence first ────────────────────────
    //
    // Only for rows git config actually stores. A `daft.yml` scalar or the
    // layout composite has its own chain — S9 and S10 build those; showing
    // them empty git scopes in the meantime would suggest a place to set them
    // that does not exist.
    for &scope in ConfigScope::ladder() {
        if spec.backend != Backend::GitConfig {
            break;
        }
        // Outside a repository there is no local or worktree config to read,
        // and offering to write one would put the value somewhere the user is
        // not standing.
        if !snapshot.in_repo && matches!(scope, ConfigScope::Local | ConfigScope::Worktree) {
            continue;
        }

        let entry = snapshot
            .entries
            .iter()
            .rfind(|entry| entry.scope == scope && keys_match(&spec.key, &entry.key));

        let inert = match (spec.global_only, scope) {
            (true, ConfigScope::Global) | (false, _) => None,
            (true, _) => Some(InertReason::NotReadAtThisScope),
        };

        if let (Some(entry), Some(reason)) = (entry, inert.clone()) {
            diagnostics.push(Diagnostic::Inert {
                scope,
                value: entry.value.clone(),
                reason,
            });
        }

        rungs.push(Rung {
            layer: Layer::Git(scope),
            value: entry.map(|e| e.value.clone()),
            origin_path: entry.and_then(|e| e.origin_path.clone()),
            writable: scope.is_writable() && spec.is_writable() && inert.is_none(),
            inert,
        });
    }

    // ── The environment override, above every file ───────────────────────
    if let Some(var) = spec.env_override
        && let Some(value) = snapshot.env.get(var)
    {
        rungs.push(Rung {
            layer: Layer::Env(var),
            value: Some(value.clone()),
            origin_path: None,
            writable: false,
            inert: None,
        });
    }

    // ── Who wins ─────────────────────────────────────────────────────────
    let winner = rungs
        .iter()
        .enumerate()
        .rfind(|(_, rung)| {
            rung.layer != Layer::Default && rung.value.is_some() && rung.inert.is_none()
        })
        .map(|(index, _)| index);

    // A process-scoped or environment win means no file write can take
    // effect. Say so before the user tries.
    if let Some(index) = winner {
        let rung = &rungs[index];
        if matches!(
            rung.layer,
            Layer::Env(_) | Layer::Git(ConfigScope::Ephemeral)
        ) {
            diagnostics.push(Diagnostic::EnvShadow {
                layer: rung.layer.clone(),
                value: rung.value.clone().unwrap_or_default(),
            });
        }
    }

    // Every stored value is checked, not just the winner: a bad value at a
    // losing scope is still a latent surprise for whoever unsets the one
    // above it.
    for rung in &rungs {
        if rung.layer == Layer::Default {
            continue;
        }
        if let Some(value) = &rung.value
            && let Err(reason) = spec.ty.validate(value)
        {
            diagnostics.push(Diagnostic::Invalid {
                layer: rung.layer.clone(),
                value: value.clone(),
                reason,
            });
        }
    }

    // ── What daft actually uses ──────────────────────────────────────────
    let (effective, origin) = match winner {
        Some(index) => {
            let rung = &rungs[index];
            let value = rung.value.clone().unwrap_or_default();
            if spec.ty.validate(&value).is_ok() {
                (Some(value), Origin::Layer(rung.layer.clone()))
            } else {
                // The loaders fold an unparseable value into the default
                // rather than the next scope down, so a broken local value
                // masks a good global one.
                (
                    default_value.clone(),
                    Origin::DefaultAfterInvalid(rung.layer.clone()),
                )
            }
        }
        None => {
            let origin = match (spec.inherits, &inherited) {
                (Some(parent), Some(_)) => Origin::Inherited(parent),
                _ => Origin::Default,
            };
            (default_value.clone(), origin)
        }
    };

    Resolved {
        spec: spec.clone(),
        rungs,
        winner,
        effective,
        origin,
        diagnostics,
    }
}

/// The highest-precedence entry for a key, across every scope.
///
/// Deprecated aliases are consulted this way — merged, the way
/// `config_get` reads them — rather than per scope.
fn last_entry_for<'a>(snapshot: &'a Snapshot, key: &str) -> Option<&'a ConfigEntry> {
    snapshot
        .entries
        .iter()
        .filter(|entry| keys_match(key, &entry.key))
        .max_by_key(|entry| entry.scope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::settings::keys;

    fn entry(key: &str, value: &str, scope: ConfigScope) -> ConfigEntry {
        ConfigEntry {
            key: key.to_string(),
            value: value.to_string(),
            scope,
            origin_path: None,
        }
    }

    fn snapshot(entries: Vec<ConfigEntry>) -> Snapshot {
        Snapshot {
            entries,
            env: HashMap::new(),
            in_repo: true,
            yaml: None,
            layout: None,
        }
    }

    fn resolve(key: &str, entries: Vec<ConfigEntry>) -> Resolved {
        resolve_key(key, &snapshot(entries)).expect("registry row exists")
    }

    // ── Key matching ─────────────────────────────────────────────────────

    #[test]
    fn git_key_parsing_splits_on_the_outer_dots() {
        let key = GitKey::parse("daft.hooks.output.quiet").unwrap();
        assert_eq!(key.section, "daft");
        assert_eq!(key.subsection, Some("hooks.output"));
        assert_eq!(key.name, "quiet");

        let flat = GitKey::parse("daft.autocd").unwrap();
        assert_eq!(flat.section, "daft");
        assert_eq!(flat.subsection, None);
        assert_eq!(flat.name, "autocd");

        assert!(GitKey::parse("daft").is_none());
        assert!(GitKey::parse("daft.").is_none());
    }

    #[test]
    fn section_and_name_are_case_insensitive_but_subsection_is_not() {
        // Git folds the section and the trailing name...
        assert!(keys_match("daft.autocd", "DAFT.AutoCD"));
        assert!(keys_match(
            "daft.checkout.pushVerify",
            "daft.checkout.pushverify"
        ));
        // ...but a mis-cased subsection is a different key entirely, which is
        // exactly the typo that silently does nothing.
        assert!(!keys_match(
            "daft.checkoutBranch.carry",
            "daft.checkoutbranch.carry"
        ));
        assert!(keys_match(
            "daft.checkoutBranch.carry",
            "DAFT.checkoutBranch.CARRY"
        ));
    }

    // ── Precedence ───────────────────────────────────────────────────────

    #[test]
    fn nothing_set_resolves_to_the_default() {
        let resolved = resolve(keys::CHECKOUT_FETCH, vec![]);
        assert_eq!(resolved.effective.as_deref(), Some("false"));
        assert_eq!(resolved.origin, Origin::Default);
        assert!(!resolved.is_set());
        assert!(resolved.diagnostics.is_empty());
    }

    #[test]
    fn local_outranks_global_outranks_system() {
        let resolved = resolve(
            keys::REMOTE,
            vec![
                entry("daft.remote", "sys", ConfigScope::System),
                entry("daft.remote", "glob", ConfigScope::Global),
                entry("daft.remote", "loc", ConfigScope::Local),
            ],
        );
        assert_eq!(resolved.effective.as_deref(), Some("loc"));
        assert_eq!(
            resolved.origin,
            Origin::Layer(Layer::Git(ConfigScope::Local))
        );

        // Every layer's answer is still on the ladder — that is the point.
        assert_eq!(resolved.value_at(ConfigScope::System), Some("sys"));
        assert_eq!(resolved.value_at(ConfigScope::Global), Some("glob"));
        assert_eq!(resolved.value_at(ConfigScope::Local), Some("loc"));
    }

    #[test]
    fn a_process_scoped_value_outranks_every_file_and_says_so() {
        let resolved = resolve(
            keys::REMOTE,
            vec![
                entry("daft.remote", "loc", ConfigScope::Local),
                entry("daft.remote", "env", ConfigScope::Ephemeral),
            ],
        );
        assert_eq!(resolved.effective.as_deref(), Some("env"));
        assert!(
            resolved
                .diagnostics
                .iter()
                .any(|d| matches!(d, Diagnostic::EnvShadow { .. })),
            "a value no write can displace must be called out: {:?}",
            resolved.diagnostics
        );
    }

    #[test]
    fn an_env_override_outranks_the_config_files() {
        let mut snap = snapshot(vec![entry(
            keys::LIST_SIZE_CONCURRENCY,
            "4",
            ConfigScope::Local,
        )]);
        snap.env
            .insert("DAFT_SIZE_WALK_JOBS".to_string(), "16".to_string());

        let resolved = resolve_key(keys::LIST_SIZE_CONCURRENCY, &snap).unwrap();
        assert_eq!(resolved.effective.as_deref(), Some("16"));
        assert_eq!(
            resolved.origin,
            Origin::Layer(Layer::Env("DAFT_SIZE_WALK_JOBS"))
        );
    }

    #[test]
    fn outside_a_repository_there_is_no_local_rung() {
        let snap = Snapshot {
            entries: vec![entry("daft.remote", "glob", ConfigScope::Global)],
            env: HashMap::new(),
            in_repo: false,
            yaml: None,
            layout: None,
        };
        let resolved = resolve_key(keys::REMOTE, &snap).unwrap();

        assert!(
            !resolved
                .rungs
                .iter()
                .any(|r| r.layer == Layer::Git(ConfigScope::Local)),
            "a local rung outside a repo would offer a write with nowhere to go"
        );
        assert_eq!(resolved.effective.as_deref(), Some("glob"));
    }

    // ── Writability ──────────────────────────────────────────────────────

    #[test]
    fn only_global_and_local_are_writable() {
        let resolved = resolve(keys::REMOTE, vec![]);
        for rung in &resolved.rungs {
            let expected = matches!(
                rung.layer,
                Layer::Git(ConfigScope::Global) | Layer::Git(ConfigScope::Local)
            );
            assert_eq!(
                rung.writable,
                expected,
                "{} writability",
                rung.layer.label()
            );
        }
    }

    #[test]
    fn a_row_owned_by_another_command_is_not_writable_anywhere() {
        let resolved = resolve_key("shared", &snapshot(vec![])).unwrap();
        assert!(resolved.rungs.iter().all(|r| !r.writable));
    }

    // ── global-only keys ─────────────────────────────────────────────────

    #[test]
    fn a_global_only_key_ignores_a_local_value_and_reports_it() {
        let resolved = resolve(
            keys::UPDATE_CHECK,
            vec![
                entry("daft.updateCheck", "false", ConfigScope::Local),
                entry("daft.updateCheck", "true", ConfigScope::Global),
            ],
        );

        assert_eq!(
            resolved.effective.as_deref(),
            Some("true"),
            "the local value must not win"
        );
        assert_eq!(
            resolved.origin,
            Origin::Layer(Layer::Git(ConfigScope::Global))
        );
        assert!(
            resolved.diagnostics.iter().any(|d| matches!(
                d,
                Diagnostic::Inert {
                    scope: ConfigScope::Local,
                    ..
                }
            )),
            "a value that is set and does nothing must be reported: {:?}",
            resolved.diagnostics
        );

        let local = resolved
            .rungs
            .iter()
            .find(|r| r.layer == Layer::Git(ConfigScope::Local))
            .unwrap();
        assert!(!local.writable, "offering a write that would do nothing");
    }

    #[test]
    fn a_global_only_key_with_only_a_local_value_still_runs_on_its_default() {
        let resolved = resolve(
            keys::UPDATE_CHECK,
            vec![entry("daft.updateCheck", "false", ConfigScope::Local)],
        );
        assert_eq!(resolved.effective.as_deref(), Some("true"));
        assert_eq!(resolved.origin, Origin::Default);
        assert!(!resolved.is_set());
    }

    // ── Deprecated aliases ───────────────────────────────────────────────

    #[test]
    fn the_retired_spelling_applies_only_when_the_real_key_is_silent() {
        let resolved = resolve(
            keys::UPDATE_ARGS,
            vec![entry("daft.fetch.args", "--rebase", ConfigScope::Global)],
        );
        assert_eq!(resolved.effective.as_deref(), Some("--rebase"));
        assert!(
            resolved
                .diagnostics
                .iter()
                .any(|d| matches!(d, Diagnostic::Deprecated { .. })),
            "using a retired key should say which key replaced it"
        );
    }

    #[test]
    fn the_real_key_beats_the_alias_from_any_scope() {
        // The loaders ask for the real key across every scope first, so a
        // *global* real value beats a *local* alias — a per-rung fallback
        // would get this backwards.
        let resolved = resolve(
            keys::UPDATE_ARGS,
            vec![
                entry("daft.fetch.args", "--rebase", ConfigScope::Local),
                entry("daft.update.args", "--ff-only", ConfigScope::Global),
            ],
        );
        assert_eq!(resolved.effective.as_deref(), Some("--ff-only"));
        assert!(
            !resolved
                .diagnostics
                .iter()
                .any(|d| matches!(d, Diagnostic::Deprecated { .. })),
            "the alias is not in play, so nothing is deprecated"
        );
    }

    #[test]
    fn a_renamed_hook_still_answers_to_its_old_key() {
        let resolved = resolve(
            "daft.hooks.worktreePostCreate.failMode",
            vec![entry(
                "daft.hooks.postCreate.failMode",
                "warn",
                ConfigScope::Local,
            )],
        );
        assert_eq!(resolved.effective.as_deref(), Some("warn"));
    }

    // ── Inheritance ──────────────────────────────────────────────────────

    #[test]
    fn an_unset_override_inherits_the_key_it_falls_back_to() {
        let resolved = resolve(
            keys::CHECKOUT_PUSH_VERIFY,
            vec![entry("daft.pushVerify", "never", ConfigScope::Global)],
        );
        assert_eq!(resolved.effective.as_deref(), Some("never"));
        assert_eq!(resolved.origin, Origin::Inherited(keys::PUSH_VERIFY));
    }

    #[test]
    fn a_set_override_wins_over_the_key_it_would_inherit() {
        let set = resolve_all(&snapshot(vec![
            entry("daft.pushVerify", "never", ConfigScope::Global),
            entry("daft.checkout.pushVerify", "always", ConfigScope::Local),
        ]));

        assert_eq!(
            set.get(keys::PUSH_VERIFY).unwrap().effective.as_deref(),
            Some("never")
        );
        assert_eq!(
            set.get(keys::CHECKOUT_PUSH_VERIFY)
                .unwrap()
                .effective
                .as_deref(),
            Some("always")
        );
    }

    #[test]
    fn inheritance_reaches_the_parents_default_when_neither_is_set() {
        let resolved = resolve(keys::CHECKOUT_PUSH_VERIFY, vec![]);
        assert_eq!(resolved.effective.as_deref(), Some("auto"));
    }

    // ── Invalid values ───────────────────────────────────────────────────

    #[test]
    fn an_unparseable_value_falls_back_to_the_default_not_the_next_scope() {
        // This mirrors the loaders: parse_bool folds a bad value into the
        // *default*, so a broken local value masks a good global one. Showing
        // it any other way would tell the user their global setting is live
        // when it is not.
        let resolved = resolve(
            keys::CHECKOUT_FETCH,
            vec![
                entry("daft.checkout.fetch", "true", ConfigScope::Global),
                entry("daft.checkout.fetch", "maybe", ConfigScope::Local),
            ],
        );

        assert_eq!(
            resolved.effective.as_deref(),
            Some("false"),
            "the built-in default"
        );
        assert_eq!(
            resolved.origin,
            Origin::DefaultAfterInvalid(Layer::Git(ConfigScope::Local))
        );
        assert!(resolved.origin.label().contains("not valid"));
        assert!(
            resolved
                .diagnostics
                .iter()
                .any(|d| matches!(d, Diagnostic::Invalid { .. }))
        );
    }

    #[test]
    fn a_bad_value_at_a_losing_scope_is_still_reported() {
        let resolved = resolve(
            keys::MERGE_STYLE,
            vec![
                entry("daft.merge.style", "octopus-ish", ConfigScope::Global),
                entry("daft.merge.style", "squash", ConfigScope::Local),
            ],
        );
        assert_eq!(resolved.effective.as_deref(), Some("squash"));
        assert!(
            resolved.diagnostics.iter().any(|d| matches!(
                d,
                Diagnostic::Invalid {
                    layer: Layer::Git(ConfigScope::Global),
                    ..
                }
            )),
            "unsetting local would expose this, so flag it now"
        );
    }

    // ── Unrecognized keys ────────────────────────────────────────────────

    #[test]
    fn a_mis_cased_subsection_is_reported_rather_than_credited() {
        let set = resolve_all(&snapshot(vec![entry(
            "daft.checkoutbranch.carry",
            "false",
            ConfigScope::Local,
        )]));

        // It must NOT be treated as the real setting...
        assert_eq!(
            set.get(keys::CHECKOUT_BRANCH_CARRY)
                .unwrap()
                .effective
                .as_deref(),
            Some("true")
        );
        // ...and the user must be told why their setting does nothing.
        assert_eq!(set.unrecognized.len(), 1);
        assert_eq!(set.unrecognized[0].key, "daft.checkoutbranch.carry");
    }

    #[test]
    fn a_deprecated_alias_is_not_an_unrecognized_key() {
        let set = resolve_all(&snapshot(vec![entry(
            "daft.fetch.args",
            "--rebase",
            ConfigScope::Global,
        )]));
        assert!(set.unrecognized.is_empty());
    }

    #[test]
    fn a_clean_configuration_has_no_issues() {
        let set = resolve_all(&snapshot(vec![
            entry("daft.checkout.fetch", "true", ConfigScope::Local),
            entry("daft.merge.style", "squash", ConfigScope::Global),
        ]));
        assert_eq!(set.issue_count(), 0);
    }

    // ── Whole-set behaviour ──────────────────────────────────────────────

    #[test]
    fn resolving_everything_covers_the_whole_registry() {
        let set = resolve_all(&snapshot(vec![]));
        assert_eq!(set.settings.len(), all_specs().len());
    }

    #[test]
    fn every_effective_value_is_one_a_user_could_type_back() {
        // The get→set round-trip is the invariant: whatever `daft config get`
        // prints, `daft config set` has to accept. A default described as
        // "auto = max(2, cores/4)" reads fine in a ladder and then fails that
        // trip, which is how it got here.
        for resolved in resolve_all(&snapshot(vec![])).settings {
            let Some(value) = &resolved.effective else {
                continue;
            };
            assert!(
                resolved.spec.ty.validate(value).is_ok(),
                "{}: effective {value:?} would be rejected by its own type: {:?}",
                resolved.spec.key,
                resolved.spec.ty.validate(value)
            );
        }
    }

    #[test]
    fn capturing_from_outside_a_repository_reads_the_global_files() {
        // Read-only, and machine-independent in what it asserts: whatever the
        // developer's own ~/.gitconfig holds, nothing from it may come back
        // labelled local — that would offer a write with nowhere to go.
        let snap = Snapshot::capture_global_only().unwrap();
        assert!(!snap.in_repo);
        assert!(
            snap.entries
                .iter()
                .all(|e| matches!(e.scope, ConfigScope::System | ConfigScope::Global)),
            "global capture must not claim repository scopes: {:?}",
            snap.entries
        );
    }

    // ── The layout chain ─────────────────────────────────────────────────

    fn layout_snapshot(rungs: LayoutRungs) -> Snapshot {
        Snapshot {
            in_repo: true,
            layout: Some(rungs),
            ..Default::default()
        }
    }

    #[test]
    fn the_layout_row_shows_every_layer_of_its_chain() {
        let set = resolve_all(&layout_snapshot(LayoutRungs {
            global: Some("sibling".to_string()),
            yaml: Some("contained".to_string()),
            repo_store: Some("nested".to_string()),
            effective: "nested".to_string(),
            source: LayoutSource::RepoStore,
            ..Default::default()
        }));

        let layout = set.get("layout").unwrap();
        let labels: Vec<String> = layout.rungs.iter().map(|r| r.layer.label()).collect();
        assert_eq!(
            labels,
            vec![
                "default",
                "detected",
                "global toml",
                "daft.yml",
                "repo store",
                "--layout"
            ],
            "the ladder has to show all six, in precedence order"
        );

        // The inversion this row exists to make visible: a teammate's local
        // repo-store choice outranks the committed daft.yml convention.
        assert_eq!(layout.effective.as_deref(), Some("nested"));
        assert_eq!(
            layout.origin,
            Origin::Layer(Layer::Layout(LayoutSource::RepoStore))
        );
    }

    #[test]
    fn only_the_two_layers_daft_owns_are_writable() {
        let set = resolve_all(&layout_snapshot(LayoutRungs::default()));
        let layout = set.get("layout").unwrap();

        for rung in &layout.rungs {
            let expected = matches!(
                rung.layer,
                Layer::Layout(LayoutSource::RepoStore | LayoutSource::GlobalConfig)
            );
            assert_eq!(
                rung.writable,
                expected,
                "{} writability — a committed convention, an observation, and \
                 one invocation's flag are none of them places to write",
                rung.layer.label()
            );
        }
    }

    #[test]
    fn the_layout_row_falls_back_to_its_built_in_when_nothing_answers() {
        let set = resolve_all(&layout_snapshot(LayoutRungs::default()));
        let layout = set.get("layout").unwrap();
        assert_eq!(layout.effective.as_deref(), Some("sibling"));
        assert_eq!(layout.origin, Origin::Default);
        assert!(!layout.is_set());
    }

    #[test]
    fn outside_a_repository_the_layout_row_has_no_chain_to_show() {
        let set = resolve_all(&snapshot(vec![]));
        let layout = set.get("layout").unwrap();
        assert_eq!(
            layout.rungs.len(),
            1,
            "no repo store, no daft.yml, nothing to detect"
        );
        assert_eq!(layout.effective.as_deref(), Some("sibling"));
    }

    #[test]
    fn the_resolved_set_answers_cross_key_validators() {
        let set = resolve_all(&snapshot(vec![entry(
            "daft.merge.cleanup",
            "remove-branch",
            ConfigScope::Local,
        )]));

        let spec = find(keys::MERGE_COMMIT).unwrap();
        let validate = spec.validate.unwrap();
        assert!(
            validate(&set, "false").is_err(),
            "the pair check must see the live cleanup value"
        );
        assert!(validate(&set, "true").is_ok());
    }

    #[test]
    fn yml_and_layout_rows_resolve_to_their_defaults_for_now() {
        // S9 and S10 give these rows real ladders; until then they must at
        // least resolve rather than panic or come back blank.
        let set = resolve_all(&snapshot(vec![]));
        assert_eq!(
            set.get("layout").unwrap().effective.as_deref(),
            Some("sibling")
        );
        assert_eq!(
            set.get("log.retention").unwrap().effective.as_deref(),
            Some("7d")
        );

        let rc = set.get("rc").unwrap();
        assert!(
            rc.effective.is_none(),
            "nothing sets it and it has no default"
        );
        assert_eq!(
            rc.effective_display(),
            "—",
            "the em dash is a display concern, not a value"
        );
    }
}

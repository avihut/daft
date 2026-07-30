//! YAML configuration data structures for the hooks system.
//!
//! This module defines the serde-deserializable structs that represent
//! a `daft.yml` configuration file.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::tracking::TrackedAttribute;

/// The hook names that fire on daft's own worktree lifecycle.
///
/// One of the two namespaces `hooks:` accepts; the other is
/// [`crate::hooks::git_stage::GitStage`]'s stage names. They are kept as
/// separate tables because only these have a script-file form under
/// `.daft/hooks/` and a deprecated pre-`worktree-` spelling.
pub const LIFECYCLE_HOOK_NAMES: &[&str] = &[
    "post-clone",
    "worktree-pre-create",
    "worktree-post-create",
    "worktree-pre-remove",
    "worktree-post-remove",
    "pre-merge",
    "post-merge",
];

/// Top-level YAML configuration.
///
/// The main `daft.yml` file maps to this struct. Hook definitions are
/// stored in the `hooks` map, keyed by hook name (e.g., "post-clone",
/// "pre-commit").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct YamlConfig {
    /// Minimum daft version required to use this config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,

    /// Whether to use colored output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<bool>,

    /// Whether to disable TTY detection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_tty: Option<bool>,

    /// Shell RC file to source before running hooks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rc: Option<String>,

    /// Output settings (list of hook names to show output for, or false to suppress all).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputSetting>,

    /// List of additional config files to extend from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<Vec<String>>,

    /// Directory for script files (default: ".daft").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_dir: Option<String>,

    /// Directory for local (gitignored) script files (default: ".daft-local").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_dir_local: Option<String>,

    /// Layout suggestion for this repository.
    ///
    /// Accepts a named layout (e.g., "contained") or an inline template string.
    /// This is a team convention that can be overridden by the user's local
    /// config in repos.json.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,

    /// Paths to share across worktrees via symlinks.
    ///
    /// Each entry is a path relative to the worktree root (e.g., ".env",
    /// ".idea", ".vscode/settings.json"). Daft centralizes these files in
    /// `.git/.daft/shared/` and creates symlinks in each worktree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Vec<String>>,

    /// Paths to copy (CoW-replicate) into each new worktree.
    ///
    /// The independent-copy sibling of [`Self::shared`]: where `shared:`
    /// centralizes one file and symlinks it everywhere, `copy:` gives every
    /// worktree its own private replica — build caches (`target/`,
    /// `node_modules/`, `.gradle/`) that must not be shared but are expensive
    /// to rebuild. Entries must be gitignored; see [`CopyConfig`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copy: Option<CopyConfig>,

    /// Log configuration (retention, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LogConfig>,

    /// Related repositories (the Graph pillar's relations manifest).
    ///
    /// Directed edges keyed by remote URL — portable across machines; the
    /// repo catalog resolves each URL to wherever that repo is cloned
    /// locally. Consumed by `daft exec --related`, `daft start
    /// --with-related`, and `daft repo info`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relations: Option<Vec<crate::catalog::relations::RelationEntry>>,

    /// Committed merge gate policy (see [`MergeConfig`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeConfig>,

    /// Derived per-worktree env values (see [`EnvConfig`]).
    ///
    /// Declares ports and templated values that `daft env` derives
    /// deterministically from the worktree's slug — no allocation, no
    /// registration. Note this is the fourth distinct meaning of "env" in
    /// this schema: job-level `env:` is a literal K→V map, `skip:`/`only:`
    /// `env:` is a truthiness predicate, and `DAFT_*` vars are computed by
    /// the hook environment. This one is the *derived-value declaration*.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<EnvConfig>,

    /// Reusable command fragments, expanded wherever `{name}` appears in a
    /// job's `run:`.
    ///
    /// The point is that a repository's gate and its CI run the same string:
    /// define `lint: "eslint --max-warnings 0"` once and the flags cannot
    /// drift between the two places that use it. Expansion is a single pass
    /// with no recursion — a template naming another template gets the second
    /// one's literal placeholder, not an expansion — because recursive
    /// expansion turns a config typo into a hang.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub templates: Option<HashMap<String, String>>,

    /// Hook definitions, keyed by hook name.
    pub hooks: HashMap<String, HookDef>,

    /// User-invoked task definitions, keyed by task name (`daft run <name>`).
    ///
    /// A task shares the hook-body schema (jobs, parallel/piped/follow,
    /// needs, env, root, skip/only, tags, interactive, background) but is
    /// triggered explicitly rather than by a lifecycle event. The reserved
    /// name `run` is what bare `daft run` executes. Kept as a sibling of
    /// `hooks` — not a custom hook name — so unknown-hook-name validation
    /// stays strict. See #708.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tasks: HashMap<String, HookDef>,
}

/// Committed merge gate policy — the top-level `merge:` block.
///
/// The local equivalent of a branch-protection rule: team policy on what
/// `daft merge` may land, named in git's own vocabulary (the section mirrors
/// gitconfig's `[merge]`). Enforced natively by the merge command — before
/// the pre-merge hooks fire AND re-verified at the moment the ref moves — so
/// the tree the gate tested is the tree that lands. Policy is relaxed only
/// by explicit per-invocation flags (`--no-ff-only`,
/// `--source-worktree any`), never by ambient configuration; the YAML
/// deliberately has no relax spellings, so overlays can add strictness but
/// cannot remove it.
/// Unknown keys are REFUSED here, deliberately diverging from the tolerant
/// parsing the rest of this schema uses. Everywhere else an unrecognized key
/// is forward-compatibility slack: an old binary meets a new key and ignores
/// it. Here the same slack is a silent policy hole — `source-worktree: clean`
/// (kebab instead of snake) or `ffOnly: only` deserializes to an all-`None`
/// block, `gate_from_config_and_overrides` computes "no policy", and every
/// merge lands ungated while the repo believes a boundary is enforced. A
/// typo must be louder than the thing it disables, so the config fails to
/// load instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    /// Fast-forward condition (git's `merge.ff`): `only` refuses any merge
    /// whose source does not already contain the target tip. Combined with
    /// pre-merge hooks running in the source worktree, this is what makes
    /// the tested tree equal the landed tree regardless of merge style.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ff: Option<FfPolicy>,

    /// Requirement on the source branch's worktree: `clean` refuses to merge
    /// a source whose worktree is missing or has uncommitted changes, so the
    /// gate certifies the committed tree — not a dirty working tree the
    /// merge would not include.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_worktree: Option<SourceWorktreePolicy>,
}

/// `merge.ff` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FfPolicy {
    /// Refuse merges that cannot fast-forward.
    Only,
}

/// `merge.source_worktree` values. Intentionally has no `any` spelling —
/// relaxing a committed `clean` is a per-invocation decision
/// (`--source-worktree any`), never something an overlay config can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceWorktreePolicy {
    /// The source must have a checked-out worktree with no uncommitted
    /// changes.
    Clean,
}

/// Output setting: either a list of hook names or false to suppress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OutputSetting {
    /// Suppress all hook output.
    Disabled(bool),
    /// Show output only for these hooks.
    Hooks(Vec<String>),
}

/// The `copy:` block — paths CoW-replicated into each new worktree.
///
/// One untagged key with two spellings, on the model of [`OutputSetting`]:
/// the bare list covers the common case, the map form adds knobs.
///
/// ```yaml
/// # Bare list
/// copy:
///   - target/
///   - node_modules/
///   - "**/dist/"
///
/// # Full map
/// copy:
///   paths: [target/, node_modules/]
///   fallback: copy   # copy | skip (default: copy) — what to do when the
///                    # filesystem cannot reflink
///   max_size: 5GB    # optional per-ENTRY cap; gates the byte-copy fallback
///                    # only, never a reflink (which is near-free)
/// ```
///
/// **Entries must be gitignored.** The engine validates each entry with `git
/// check-ignore` *and* a "nothing tracked underneath" probe (a force-added
/// file inside an ignored directory still disqualifies it); a violation is a
/// per-entry warning row and creation continues.
///
/// Entries may name files or directories, and may contain glob metacharacters
/// (`*`, `?`, `[`), which expand against the source worktree. A trailing `/`
/// is cosmetic and normalized away.
///
/// Merge semantics: an overlay (`daft.local.yml`, an `extends:` file) replaces
/// this key **wholesale**, exactly like `shared:` — there is no element-wise
/// union, so a local override is always a complete restatement.
///
/// Unknown keys inside the map form are tolerated (unlike [`MergeConfig`]): a
/// mistyped knob costs a copy optimization, not a safety boundary. The cost of
/// that tolerance — a misspelled `paths:` reading as "declares nothing" — is
/// paid where it can be seen, by `copy_paths::copy_entries` warning that the
/// block declares no paths, rather than here where refusing it would fail the
/// whole `daft.yml`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum CopyConfig {
    /// Bare list form — paths only, every knob at its default.
    Paths(Vec<String>),
    /// Map form — paths plus the `fallback` / `max_size` knobs.
    Full {
        /// The declared entries.
        paths: Vec<String>,
        /// Behavior when the filesystem cannot reflink. `None` ≡
        /// [`CopyFallback::Copy`].
        #[serde(skip_serializing_if = "Option::is_none")]
        fallback: Option<CopyFallback>,
        /// Per-entry size cap as a human string (`5GB`, `500MB`, `1048576`).
        /// Parsed by `crate::coordinator::clean_policy::parse_size` —
        /// case-insensitive, binary multiples (1KB = 1024), a bare integer is
        /// bytes. Applies to the byte-copy fallback only.
        #[serde(skip_serializing_if = "Option::is_none")]
        max_size: Option<String>,
    },
}

/// Hand-written so a near-miss inside `copy:` says what is wrong with it.
///
/// A derived `#[serde(untagged)]` impl reports **every** shape mistake as
/// `data did not match any variant of untagged enum CopyConfig`, and that
/// message is the whole diagnosis a user gets: the error fails the entire
/// `daft.yml`, so the runtime warning that reports it is the only thing
/// standing between them and a config whose hooks have all quietly stopped
/// running. "Did not match any variant" does not name the key, the line, or
/// the fix.
///
/// So the shapes are dispatched by hand, each with its own sentence, and two
/// spellings a derived impl would have rejected are accepted on purpose:
///
/// * `copy: target/` — a bare scalar, read as a one-entry list;
/// * `paths: target/` — the same sugar one level down, which is the natural
///   thing to write for a single entry and otherwise took the whole file with
///   it.
///
/// `max_size` accepts an unquoted YAML integer beside the string form, since
/// `parse_size` documents a bare integer as bytes and `max_size: 1048576` is
/// the obvious thing to type. Unknown keys are ignored — see the type docs.
impl<'de> Deserialize<'de> for CopyConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(CopyConfigVisitor)
    }
}

struct CopyConfigVisitor;

impl<'de> serde::de::Visitor<'de> for CopyConfigVisitor {
    type Value = CopyConfig;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(
            "a list of paths, or a map with `paths:` (plus optional `fallback:`/`max_size:`)",
        )
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(CopyConfig::Paths(vec![value.to_string()]))
    }

    fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut paths = Vec::new();
        while let Some(entry) = seq.next_element::<String>()? {
            paths.push(entry);
        }
        Ok(CopyConfig::Paths(paths))
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut paths: Option<Vec<String>> = None;
        let mut fallback: Option<CopyFallback> = None;
        let mut max_size: Option<String> = None;

        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "paths" => paths = Some(map.next_value::<PathList>()?.0),
                "fallback" => fallback = map.next_value::<Option<CopyFallback>>()?,
                "max_size" => max_size = map.next_value::<Option<SizeScalar>>()?.map(|s| s.0),
                // Tolerated, by design: a mistyped knob must not fail the file.
                _ => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }

        Ok(CopyConfig::Full {
            paths: paths.unwrap_or_default(),
            fallback,
            max_size,
        })
    }
}

/// A `paths:` value: a list, or a single entry written as a bare scalar.
struct PathList(Vec<String>);

impl<'de> Deserialize<'de> for PathList {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = PathList;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a list of paths, or a single path")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(PathList(vec![value.to_string()]))
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut out = Vec::new();
                while let Some(entry) = seq.next_element::<String>()? {
                    out.push(entry);
                }
                Ok(PathList(out))
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// A `max_size:` value: the string form, or an unquoted integer meaning bytes.
struct SizeScalar(String);

impl<'de> Deserialize<'de> for SizeScalar {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SizeScalar;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a size such as `5GB`, or a plain byte count")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(SizeScalar(value.to_string()))
            }

            fn visit_u64<E: serde::de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(SizeScalar(value.to_string()))
            }

            fn visit_i64<E: serde::de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(SizeScalar(value.to_string()))
            }
        }
        deserializer.deserialize_any(V)
    }
}

impl CopyConfig {
    /// The declared entries, in config order, whichever form was written.
    pub fn paths(&self) -> &[String] {
        match self {
            CopyConfig::Paths(p) => p,
            CopyConfig::Full { paths, .. } => paths,
        }
    }

    /// The effective fallback mode — [`CopyFallback::Copy`] unless the map
    /// form says otherwise.
    pub fn fallback(&self) -> CopyFallback {
        match self {
            CopyConfig::Paths(_) => CopyFallback::default(),
            CopyConfig::Full { fallback, .. } => fallback.unwrap_or_default(),
        }
    }

    /// The raw `max_size` string, if the map form set one. Unparsed — see
    /// [`CopyConfig::Full::max_size`] for the accepted spellings.
    pub fn max_size(&self) -> Option<&str> {
        match self {
            CopyConfig::Paths(_) => None,
            CopyConfig::Full { max_size, .. } => max_size.as_deref(),
        }
    }

    /// True when nothing is declared — the "no copy section at all" case,
    /// which plans no rows and does no work.
    pub fn is_empty(&self) -> bool {
        self.paths().is_empty()
    }
}

/// What `copy:` does with an entry when the filesystem cannot reflink it.
///
/// Lowercase in YAML (`fallback: copy` / `fallback: skip`), but parsed
/// case-insensitively — see the [`Deserialize`] impl for why that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CopyFallback {
    /// Byte-copy the entry anyway (subject to `max_size`). The default: a
    /// warm cache is worth the bytes on most trees.
    #[default]
    Copy,
    /// Leave the entry out and report an attention skip. For trees where a
    /// non-CoW copy would cost more than the rebuild it saves.
    Skip,
}

impl CopyFallback {
    /// Parse a fallback mode from a string (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "copy" => Some(CopyFallback::Copy),
            "skip" => Some(CopyFallback::Skip),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for CopyFallback {
    /// Deserialize case-insensitively via [`CopyFallback::parse`], mirroring
    /// [`crate::hooks::FailMode`] and for the same reason: a bad enum value
    /// fails the *entire* `daft.yml` deserialize, which silently drops every
    /// YAML hook for that operation to the legacy-script fallback. A derived
    /// `rename_all = "lowercase"` impl would put `fallback: Copy` — the
    /// spelling a user naturally writes after reading `CopyFallback::Copy` —
    /// in that blast radius over a capital letter.
    ///
    /// A genuinely unknown value (`fallback: symlink`) still fails, as it
    /// must. Note that the error text below does **not** survive: `copy:` is
    /// an untagged enum, so serde reports the generic "data did not match any
    /// variant of untagged enum CopyConfig" instead. That is inherent to the
    /// untagged surface (every other untagged field in this schema behaves
    /// the same way) — the message is reachable through
    /// [`CopyFallback::parse`], which is what callers with a raw string
    /// should use.
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        CopyFallback::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "invalid copy fallback {s:?}, expected \"copy\" or \"skip\""
            ))
        })
    }
}

/// The `env:` block — deterministic per-worktree env values (#388).
///
/// Every value is a pure function of `(scheme, salt, repo, worktree-slug,
/// declaration)` — computable from any worktree, any repo, any machine,
/// with no allocation registry. Ports hash the worktree to a contiguous
/// block; declared offsets index into it:
///
/// ```yaml
/// env:
///   salt: myapp            # optional; default = project-root dir name.
///                          # Pin it to make values identical across machines.
///   ports:
///     - WEBAPP_PORT        # offset 0 (enum semantics: previous + 1)
///     - STORYBOOK_PORT     # offset 1
///     - API_PORT: 8        # explicit offset resets the counter
///   values:
///     COMPOSE_PROJECT_NAME: "myapp-{worktree_slug}"
///   write: .env            # optional dotenv target for `daft env --write`
/// ```
///
/// Unknown keys inside this block are tolerated (like `copy:`, unlike
/// `merge:`): a mistyped knob costs a derived value, not a safety boundary,
/// and refusing it here would fail the whole `daft.yml` — silently dropping
/// every YAML hook to the legacy-script fallback.
///
/// Merge semantics: scalar knobs (`salt`, `scheme`, `range`, `block_size`,
/// `write`) merge field-level so a `daft.local.yml` can override just the
/// salt (the local "reroll" lever); `ports:` and `values:` replace
/// **wholesale** when the overlay declares them — element-wise merging would
/// scramble the enum-semantics offsets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EnvConfig {
    /// Hash salt. Defaults to the project-root directory name at resolution
    /// time; pinning it in the tracked config is what guarantees identical
    /// values across machines and clone locations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,

    /// Derivation scheme version. Only `1` exists; a future scheme bump is
    /// opt-in precisely because changing the function renumbers every port.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheme: Option<u32>,

    /// Port range as `"START-END"` inclusive. Default `20000-32767` — below
    /// the Linux ephemeral floor (32768) and macOS's (49152), above the
    /// 3000–9999 zone dev tools squat on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,

    /// Ports per worktree block. Default 16.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_size: Option<u16>,

    /// Declared port variables, in offset order (enum semantics — see
    /// [`PortEntry`]). Declaring any schema opts the repo into strictness:
    /// unknown names become errors instead of ad-hoc hashes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<PortEntry>>,

    /// Templated string values, rendered per worktree. Same variable names as
    /// the hook template dialect (`{worktree_slug}`, `{worktree_path}`,
    /// `{worktree_root}`, `{branch}`, `{repo}`) plus `{env:PORT_NAME}` to
    /// embed a declared port. BTreeMap: emission order is alphabetical and
    /// values may not reference each other, so declaration order carries no
    /// meaning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<std::collections::BTreeMap<String, String>>,

    /// Default dotenv target for `daft env --write` (worktree-relative).
    /// Must NOT also appear in `shared:` — a shared symlinked dotenv would
    /// make every worktree overwrite the same central file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<String>,
}

impl EnvConfig {
    /// Declared port names with their resolved offsets, in declaration order.
    ///
    /// Enum semantics, exactly like a C/Rust enum's discriminants: a bare
    /// name takes `previous + 1` (first is 0), an explicit `NAME: n` resets
    /// the counter to `n`. Duplicate names/offsets an invalid config may
    /// produce are preserved here — `validate_config` reports them; consumers
    /// that reached resolution treat the config as validated.
    pub fn resolved_ports(&self) -> Vec<(String, u16)> {
        let mut out = Vec::new();
        let mut next: u16 = 0;
        for entry in self.ports.as_deref().unwrap_or_default() {
            let offset = entry.offset.unwrap_or(next);
            out.push((entry.name.clone(), offset));
            next = offset.saturating_add(1);
        }
        out
    }

    /// The declared range parsed to `(start, end)` inclusive, if present and
    /// well-formed. `None` when the field is absent; `Some(Err)` semantics are
    /// collapsed to `None` here — `validate_config` owns the error message.
    pub fn parsed_range(&self) -> Option<(u16, u16)> {
        parse_port_range(self.range.as_deref()?)
    }
}

/// Parse `"START-END"` into an inclusive port range. Rejects reversed and
/// zero-start ranges.
pub fn parse_port_range(raw: &str) -> Option<(u16, u16)> {
    let (start, end) = raw.split_once('-')?;
    let start: u16 = start.trim().parse().ok()?;
    let end: u16 = end.trim().parse().ok()?;
    (start > 0 && start <= end).then_some((start, end))
}

/// One entry in `env.ports:` — a name with an optional explicit offset.
///
/// Two YAML spellings: a bare string (`- WEBAPP_PORT`, offset = previous + 1)
/// or a one-pair map (`- API_PORT: 8`, explicit offset). See
/// [`EnvConfig::resolved_ports`] for the counter semantics.
#[derive(Debug, Clone, PartialEq)]
pub struct PortEntry {
    /// The env var name (validated as `[A-Z_][A-Z0-9_]*`).
    pub name: String,
    /// Explicit offset within the worktree's block; `None` = auto.
    pub offset: Option<u16>,
}

impl Serialize for PortEntry {
    /// Round-trips the two YAML spellings: bare string when the offset is
    /// auto, one-pair map when explicit.
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self.offset {
            None => serializer.serialize_str(&self.name),
            Some(offset) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry(&self.name, &offset)?;
                map.end()
            }
        }
    }
}

/// Hand-written for the same reason as [`CopyConfig`]: a derived
/// `#[serde(untagged)]` impl reports every shape mistake as "data did not
/// match any variant", the error fails the entire `daft.yml`, and a failed
/// deserialize silently drops every YAML hook to the legacy-script fallback.
/// Each rejected shape gets its own sentence instead.
impl<'de> Deserialize<'de> for PortEntry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = PortEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a port name (`- WEBAPP_PORT`) or a one-pair map (`- API_PORT: 8`)")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(PortEntry {
                    name: value.to_string(),
                    offset: None,
                })
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let Some((name, offset)) = map.next_entry::<String, u16>()? else {
                    return Err(serde::de::Error::custom(
                        "empty map in `env.ports`; write `- NAME` or `- NAME: offset`",
                    ));
                };
                if map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {
                    return Err(serde::de::Error::custom(format!(
                        "`env.ports` entry starting at `{name}` holds more than one pair; \
                         each list item is one `NAME: offset` (did you forget the `-` on \
                         the next line?)"
                    )));
                }
                Ok(PortEntry {
                    name,
                    offset: Some(offset),
                })
            }
        }
        deserializer.deserialize_any(V)
    }
}

// Re-export from executor so that format-agnostic types are defined once.
pub use crate::executor::{BackgroundOutput, LogConfig};

/// Definition for a single hook type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HookDef {
    /// Whether jobs in this hook default to background execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,

    /// Run jobs in parallel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,

    /// Run jobs sequentially, stop on first failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub piped: Option<bool>,

    /// Run jobs sequentially, continue on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow: Option<bool>,

    /// Tags to exclude at hook level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_tags: Option<Vec<String>>,

    /// Glob patterns excluded from every file-aware job in this hook —
    /// appended to each job's own `exclude:` list. Does not by itself make a
    /// job file-aware (a job with no `glob:`/`exclude:`/`files:` and no
    /// `{changed_files}` template ignores it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,

    /// Skip condition at hook level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<SkipCondition>,

    /// Only condition at hook level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<OnlyCondition>,

    /// List of jobs to execute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<JobDef>>,

    /// Legacy alias for jobs (commands map).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commands: Option<HashMap<String, CommandDef>>,

    /// Failure mode for this hook: `abort` (fatal) or `warn` (report and
    /// continue). Committed here it is a repo-wide default; a git-config
    /// `daft.hooks.<hookName>.failMode` overrides it (see the executor's
    /// `resolve_fail_mode`). Has no effect on `tasks:` entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_mode: Option<super::FailMode>,

    /// Shell command producing this hook's file list, replacing whatever the
    /// hook would otherwise offer (the staged files for a commit stage, the
    /// merge range for a merge hook).
    ///
    /// Resolved once and shared by every job, unlike the job-level `files:`
    /// which runs per job. Declaring it is a statement about what the hook
    /// gates, so it replaces rather than unions with git's answer — a union
    /// would make the declaration mean less than it says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<String>,
}

/// Target operating system for platform constraints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TargetOs {
    Macos,
    Linux,
    Windows,
}

impl TargetOs {
    /// Return the OS string as used by `std::env::consts::OS`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetOs::Macos => "macos",
            TargetOs::Linux => "linux",
            TargetOs::Windows => "windows",
        }
    }
}

/// Target CPU architecture for platform constraints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetArch {
    X86_64,
    Aarch64,
}

impl TargetArch {
    /// Return the arch string as used by `std::env::consts::ARCH`.
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetArch::X86_64 => "x86_64",
            TargetArch::Aarch64 => "aarch64",
        }
    }
}

/// A platform constraint that can be a single value or a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformConstraint<T> {
    Single(T),
    List(Vec<T>),
}

impl<T> PlatformConstraint<T> {
    /// Return the values as a slice.
    pub fn as_slice(&self) -> &[T] {
        match self {
            PlatformConstraint::Single(v) => std::slice::from_ref(v),
            PlatformConstraint::List(v) => v,
        }
    }
}

/// A run command that can be a simple string or OS-keyed map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunCommand {
    /// Simple string command (runs on all platforms).
    Simple(String),
    /// OS-keyed map of commands.
    Platform(HashMap<TargetOs, PlatformRunCommand>),
}

/// A platform-specific run command (string or list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlatformRunCommand {
    /// Single command string.
    Simple(String),
    /// List of commands joined with " && ".
    List(Vec<String>),
}

impl RunCommand {
    pub fn resolve_for_current_os(&self) -> Option<String> {
        match self {
            RunCommand::Simple(s) => Some(s.clone()),
            RunCommand::Platform(map) => {
                let current_os = Self::current_target_os()?;
                map.get(&current_os).map(|cmd| cmd.to_command_string())
            }
        }
    }

    pub fn is_platform(&self) -> bool {
        matches!(self, RunCommand::Platform(_))
    }

    pub fn current_target_os() -> Option<TargetOs> {
        match std::env::consts::OS {
            "macos" => Some(TargetOs::Macos),
            "linux" => Some(TargetOs::Linux),
            "windows" => Some(TargetOs::Windows),
            _ => None,
        }
    }
}

impl PlatformRunCommand {
    pub fn to_command_string(&self) -> String {
        match self {
            PlatformRunCommand::Simple(s) => s.clone(),
            PlatformRunCommand::List(cmds) => cmds.join(" && "),
        }
    }
}

/// A value that can be a single string or a list of strings.
///
/// Used by glob-pattern fields (`glob:`, `changed:`) so the one-pattern case
/// reads without list syntax.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrList {
    Single(String),
    List(Vec<String>),
}

impl StringOrList {
    /// Return the values as a slice.
    pub fn as_slice(&self) -> &[String] {
        match self {
            StringOrList::Single(v) => std::slice::from_ref(v),
            StringOrList::List(v) => v,
        }
    }
}

/// A single job definition within a hook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct JobDef {
    /// Optional name for the job (used for merging and display).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Human-readable description of what this job does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Shell command to run (simple string or OS-keyed map).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<RunCommand>,

    /// Script file to run (relative to source_dir).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,

    /// Runner for script files (e.g., "bash", "python").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,

    /// Arguments to pass to the script.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,

    /// Working directory (relative to worktree root).
    ///
    /// Supports template variables; an absolute result (e.g. from
    /// `{merge_source_path}`) replaces the base entirely. A `{merge_…}`
    /// template that cannot resolve fails the hook rather than silently
    /// running the job in the hook's own cwd.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,

    /// Tags for this job (for filtering).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,

    /// Glob patterns selecting the changed files this job cares about.
    ///
    /// A single pattern or a list. Patterns match repository-root-relative
    /// paths with standard doublestar semantics (`**` spans zero or more
    /// directories, `*` never crosses `/`, braces expand) and ignore
    /// `root:`. The file list comes from the hook's operation (for merge
    /// hooks: the files the sources changed relative to the target) or the
    /// job's own `files:` command. When no changed file matches, the job is
    /// skipped. Declaring `glob:` on a hook type with no changed-file
    /// source and no `files:` is a configuration error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glob: Option<StringOrList>,

    /// Glob patterns removed from this job's changed-file list (see `glob`).
    ///
    /// Applied after `glob:` selection; hook-level `exclude:` patterns are
    /// appended. `exclude:` alone (no `glob:`) selects every changed file
    /// outside the excluded paths — the job is skipped when nothing else
    /// changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude: Option<Vec<String>>,

    /// Shell command producing this job's file list, replacing the hook's
    /// own changed-file source.
    ///
    /// Runs via `sh -c` in the hook's working directory and must emit one
    /// repository-root-relative path per line. An empty result skips the
    /// job; a non-zero exit fails the hook.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<String>,

    /// Skip condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<SkipCondition>,

    /// Only condition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub only: Option<OnlyCondition>,

    /// Restrict job to specific CPU architectures.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<PlatformConstraint<TargetArch>>,

    /// Extra environment variables.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    /// Custom failure message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_text: Option<String>,

    /// Whether this job needs TTY/stdin (forces sequential).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,

    /// Priority for execution ordering (lower runs first).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,

    /// Names of jobs that must complete before this job runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs: Option<Vec<String>>,

    /// Nested group of jobs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupDef>,

    /// Worktree attributes this job tracks.
    /// When a tracked attribute changes, the job is re-run with teardown/setup semantics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracks: Option<Vec<TrackedAttribute>>,

    /// Run this job in the background (overrides hook-level default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,

    /// Output behavior for background execution.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_output: Option<BackgroundOutput>,

    /// Log configuration for this job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<LogConfig>,

    /// Restrict this job's file list by what the paths *are*, after `glob:`
    /// and `exclude:` have selected them.
    ///
    /// Accepts `text`, `binary`, `executable`, `not executable`, `symlink`,
    /// `not symlink`; several are ANDed. A formatter handed a PNG because it
    /// matched `assets/**` is the case this exists for — the glob describes
    /// where files live, not what they contain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_types: Option<StringOrList>,

    /// Re-stage this job's files after it runs, so a formatter's edits land
    /// in the commit being made rather than as a surprise diff afterwards.
    ///
    /// Only meaningful on the commit-family stages, where there is an index
    /// to stage into; declaring it elsewhere is a validation error rather
    /// than a silent no-op.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_fixed: Option<bool>,

    /// Feed the stage's stdin to this job's command.
    ///
    /// `pre-push` and `post-rewrite` are handed a payload on stdin that a job
    /// may want to read as a stream. The dispatcher drained it (a process
    /// cannot read stdin twice), so daft replays it into the jobs that ask.
    /// Incompatible with `interactive:` and `background:`, which own stdin
    /// for other reasons.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_stdin: Option<bool>,
}

/// Legacy command definition (alias for JobDef).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CommandDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skip: Option<SkipCondition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

impl CommandDef {
    /// Convert a legacy CommandDef to a JobDef.
    pub fn to_job_def(&self, name: &str) -> JobDef {
        JobDef {
            name: Some(name.to_string()),
            run: self.run.as_ref().map(|r| RunCommand::Simple(r.clone())),
            script: self.script.clone(),
            runner: self.runner.clone(),
            tags: self.tags.clone(),
            skip: self.skip.clone(),
            env: self.env.clone(),
            ..Default::default()
        }
    }
}

/// Skip condition: bool, string, platform map, or list of skip rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkipCondition {
    /// Always skip (true) or never skip (false).
    Bool(bool),
    /// Skip if this env var is set and truthy.
    EnvVar(String),
    /// OS-keyed map of skip rules.
    Platform(HashMap<TargetOs, Vec<SkipRule>>),
    /// List of skip rules (any match → skip).
    Rules(Vec<SkipRule>),
}

/// A single skip rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SkipRule {
    /// Named condition: "merge" or "rebase".
    Named(String),
    /// Structured condition.
    Structured(SkipRuleStructured),
}

/// Structured skip rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkipRuleStructured {
    /// Skip if current ref matches this pattern.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_pattern: Option<String>,
    /// Skip if this env var is set and truthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Skip if this command exits 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Skip if any changed file matches these glob patterns (same matching
    /// semantics and changed-file source as the job-level `glob:` field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<StringOrList>,
    /// Human-readable description of why this skip rule exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// Only condition: mirrors SkipCondition but with inverse semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OnlyCondition {
    /// Only run if true, never run if false.
    Bool(bool),
    /// Only run if this env var is set and truthy.
    EnvVar(String),
    /// OS-keyed map of only rules.
    Platform(HashMap<TargetOs, Vec<OnlyRule>>),
    /// List of only rules (all must match → run).
    Rules(Vec<OnlyRule>),
}

/// A single only rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OnlyRule {
    /// Named condition: "merge" or "rebase".
    Named(String),
    /// Structured condition.
    Structured(OnlyRuleStructured),
}

/// Structured only rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnlyRuleStructured {
    /// Only run if current ref matches this pattern.
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_pattern: Option<String>,
    /// Only run if this env var is set and truthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Only run if this command exits 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Only run if at least one changed file matches these glob patterns
    /// (same matching semantics and changed-file source as the job-level
    /// `glob:` field).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed: Option<StringOrList>,
    /// Human-readable description of why this only rule exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub desc: Option<String>,
}

/// A group of jobs that runs as a unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GroupDef {
    /// Run grouped jobs in parallel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,
    /// Run grouped jobs sequentially, stop on first failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub piped: Option<bool>,
    /// Nested jobs in this group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jobs: Option<Vec<JobDef>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_ports_two_spellings_and_enum_offsets() {
        let yaml = r#"
env:
  salt: myapp
  ports:
    - WEBAPP_PORT
    - STORYBOOK_PORT
    - API_PORT: 8
    - METRICS_PORT
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let env = config.env.expect("env section parses");
        assert_eq!(env.salt.as_deref(), Some("myapp"));
        assert_eq!(
            env.resolved_ports(),
            vec![
                ("WEBAPP_PORT".to_string(), 0),
                ("STORYBOOK_PORT".to_string(), 1),
                ("API_PORT".to_string(), 8),
                ("METRICS_PORT".to_string(), 9),
            ]
        );
    }

    #[test]
    fn env_port_entry_multi_pair_map_names_the_mistake() {
        // A missing `-` folds two declarations into one map entry; the error
        // must say so instead of the generic untagged-enum shrug.
        let yaml = "env:\n  ports:\n    - API_PORT: 8\n      METRICS_PORT: 9\n";
        let err = serde_yaml::from_str::<YamlConfig>(yaml).expect_err("multi-pair map must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("more than one pair"),
            "error should explain the shape: {msg}"
        );
    }

    #[test]
    fn env_port_entries_roundtrip_their_spelling() {
        let entries = vec![
            PortEntry {
                name: "WEBAPP_PORT".into(),
                offset: None,
            },
            PortEntry {
                name: "API_PORT".into(),
                offset: Some(8),
            },
        ];
        let yaml = serde_yaml::to_string(&entries).unwrap();
        // Bare name stays a bare scalar; explicit offset stays a one-pair map.
        assert!(yaml.contains("- WEBAPP_PORT"), "bare spelling kept: {yaml}");
        assert!(yaml.contains("API_PORT: 8"), "map spelling kept: {yaml}");
        let back: Vec<PortEntry> = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back, entries);
    }

    #[test]
    fn env_unknown_keys_are_tolerated() {
        // Like copy:, unlike merge:: a mistyped knob must not fail the file
        // (a failed deserialize silently drops every hook to legacy scripts).
        let yaml = "env:\n  salt: x\n  blocksize: 8\n  ports:\n    - A_PORT\n";
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let env = config.env.unwrap();
        assert_eq!(env.block_size, None, "typo'd key is ignored, not adopted");
        assert_eq!(env.resolved_ports().len(), 1);
    }

    #[test]
    fn env_absent_serializes_sparsely() {
        let config = YamlConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(!yaml.contains("env:"), "no env litter in output: {yaml}");
    }

    #[test]
    fn parse_port_range_cases() {
        assert_eq!(parse_port_range("20000-32767"), Some((20000, 32767)));
        assert_eq!(parse_port_range(" 3000 - 4000 "), Some((3000, 4000)));
        assert_eq!(parse_port_range("5000-5000"), Some((5000, 5000)));
        assert_eq!(parse_port_range("4000-3000"), None, "reversed");
        assert_eq!(parse_port_range("0-100"), None, "zero start");
        assert_eq!(parse_port_range("20000"), None, "no dash");
        assert_eq!(parse_port_range("a-b"), None);
        assert_eq!(parse_port_range("1-70000"), None, "beyond u16");
    }

    #[test]
    fn test_minimal_config() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: setup
        run: echo "hello"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.hooks.contains_key("worktree-post-create"));
        let hook = &config.hooks["worktree-post-create"];
        let jobs = hook.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name.as_deref(), Some("setup"));
        match &jobs[0].run {
            Some(RunCommand::Simple(s)) => assert_eq!(s, "echo \"hello\""),
            other => panic!("Expected Simple, got {other:?}"),
        }
    }

    #[test]
    fn fail_mode_deserializes_case_insensitively() {
        use crate::hooks::FailMode;

        // The git-config `failMode` surface is case-insensitive
        // (FailMode::parse lowercases), so a committed `daft.yml fail_mode:`
        // must accept the same spellings. Critically, a mis-cased value must
        // NOT fail the whole YamlConfig deserialize — that would silently drop
        // every hook for the operation to the legacy-script fallback.
        for (spelling, expected) in [
            ("abort", FailMode::Abort),
            ("Abort", FailMode::Abort),
            ("ABORT", FailMode::Abort),
            ("warn", FailMode::Warn),
            ("WARN", FailMode::Warn),
        ] {
            let yaml = format!(
                "hooks:\n  worktree-post-create:\n    fail_mode: {spelling}\n    \
                 jobs:\n      - run: \"true\"\n"
            );
            let config: YamlConfig = serde_yaml::from_str(&yaml)
                .unwrap_or_else(|e| panic!("{spelling:?} should parse, got: {e}"));
            assert_eq!(
                config.hooks["worktree-post-create"].fail_mode,
                Some(expected),
                "{spelling:?} should deserialize to {expected}"
            );
        }

        // A genuine typo still errors — loudly, naming the bad value — rather
        // than being silently accepted.
        let err = serde_yaml::from_str::<YamlConfig>(
            "hooks:\n  worktree-post-create:\n    fail_mode: abrot\n    jobs:\n      - run: \"true\"\n",
        )
        .expect_err("a non-abort/warn value must fail to parse");
        assert!(
            err.to_string().contains("abrot"),
            "error should name the bad value: {err}"
        );
    }

    #[test]
    fn test_tasks_section_parses_full_job_schema() {
        // A task shares the hook body schema: parallel, and per-job env,
        // root, and needs all deserialize the same way.
        let yaml = r#"
tasks:
  run:
    parallel: true
    jobs:
      - name: backend
        run: docker compose up
        env:
          COMPOSE_PROJECT_NAME: "api-{worktree_slug}"
      - name: web
        run: pnpm dev
        root: frontend
        needs: [backend]
  seed-db:
    jobs:
      - name: seed
        run: ./scripts/seed.sh
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tasks.len(), 2);
        assert!(config.hooks.is_empty());

        let run = &config.tasks["run"];
        assert_eq!(run.parallel, Some(true));
        let jobs = run.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 2);

        let backend = &jobs[0];
        assert_eq!(backend.name.as_deref(), Some("backend"));
        assert_eq!(
            backend.env.as_ref().unwrap().get("COMPOSE_PROJECT_NAME"),
            Some(&"api-{worktree_slug}".to_string())
        );

        let web = &jobs[1];
        assert_eq!(web.root.as_deref(), Some("frontend"));
        assert_eq!(web.needs.as_deref(), Some(&["backend".to_string()][..]));

        assert!(config.tasks.contains_key("seed-db"));
    }

    #[test]
    fn test_empty_tasks_not_serialized() {
        // A config without tasks must not emit `tasks: {}` litter.
        let config = YamlConfig {
            hooks: {
                let mut m = HashMap::new();
                m.insert(
                    "worktree-post-create".to_string(),
                    HookDef {
                        jobs: Some(vec![JobDef {
                            name: Some("setup".to_string()),
                            run: Some(RunCommand::Simple("pnpm install".to_string())),
                            ..Default::default()
                        }]),
                        ..Default::default()
                    },
                );
                m
            },
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(
            !yaml.contains("tasks:"),
            "empty tasks must be omitted from serialized output:\n{yaml}"
        );
    }

    #[test]
    fn test_empty_config() {
        let yaml = "";
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.hooks.is_empty());
        assert!(config.min_version.is_none());
    }

    #[test]
    fn test_full_config() {
        let yaml = r#"
min_version: "1.0.20"
colors: true
no_tty: false
source_dir: ".daft"
extends:
  - shared.yml
hooks:
  worktree-pre-create:
    parallel: true
    jobs:
      - name: lint
        run: cargo clippy
        tags:
          - lint
        priority: 1
      - name: format
        run: cargo fmt --check
        tags:
          - format
        priority: 2
  worktree-post-create:
    jobs:
      - name: install-deps
        run: npm install
        skip: CI
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.min_version.as_deref(), Some("1.0.20"));
        assert_eq!(config.colors, Some(true));
        assert_eq!(config.extends.as_ref().unwrap().len(), 1);

        let pre_create = &config.hooks["worktree-pre-create"];
        assert_eq!(pre_create.parallel, Some(true));
        let jobs = pre_create.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].priority, Some(1));
        assert_eq!(jobs[1].priority, Some(2));

        let post_create = &config.hooks["worktree-post-create"];
        let jobs = post_create.jobs.as_ref().unwrap();
        assert_eq!(jobs.len(), 1);
        // skip: CI should parse as EnvVar
        match &jobs[0].skip {
            Some(SkipCondition::EnvVar(v)) => assert_eq!(v, "CI"),
            other => panic!("Expected EnvVar, got {other:?}"),
        }
    }

    #[test]
    fn test_skip_condition_bool() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
        skip: true
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        match &job.skip {
            Some(SkipCondition::Bool(true)) => {}
            other => panic!("Expected Bool(true), got {other:?}"),
        }
    }

    #[test]
    fn test_skip_condition_rules() {
        let yaml = r#"
hooks:
  worktree-post-create:
    skip:
      - merge
      - ref: "release/*"
      - env: SKIP_HOOKS
      - run: "test -f .skip-hooks"
    jobs:
      - name: test
        run: echo test
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = &config.hooks["worktree-post-create"];
        match &hook.skip {
            Some(SkipCondition::Rules(rules)) => {
                assert_eq!(rules.len(), 4);
                match &rules[0] {
                    SkipRule::Named(s) => assert_eq!(s, "merge"),
                    other => panic!("Expected Named, got {other:?}"),
                }
                match &rules[1] {
                    SkipRule::Structured(s) => {
                        assert_eq!(s.ref_pattern.as_deref(), Some("release/*"));
                    }
                    other => panic!("Expected Structured with ref, got {other:?}"),
                }
            }
            other => panic!("Expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_job_glob_string_and_list_forms() {
        let yaml = r#"
hooks:
  pre-merge:
    exclude:
      - "**/*.lock"
    jobs:
      - name: single
        run: cargo check
        glob: "src/**"
      - name: many
        run: lint {changed_files}
        glob:
          - "*.{js,ts}"
          - "web/**"
        exclude:
          - "web/generated/**"
      - name: custom
        run: verify
        files: "git ls-files src"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = &config.hooks["pre-merge"];
        assert_eq!(
            hook.exclude.as_deref(),
            Some(&["**/*.lock".to_string()][..])
        );

        let jobs = hook.jobs.as_ref().unwrap();
        match jobs[0].glob.as_ref().unwrap() {
            StringOrList::Single(s) => assert_eq!(s, "src/**"),
            other => panic!("expected Single, got {other:?}"),
        }
        match jobs[1].glob.as_ref().unwrap() {
            StringOrList::List(l) => assert_eq!(l, &["*.{js,ts}", "web/**"]),
            other => panic!("expected List, got {other:?}"),
        }
        assert_eq!(
            jobs[1].exclude.as_deref(),
            Some(&["web/generated/**".to_string()][..])
        );
        assert_eq!(jobs[2].files.as_deref(), Some("git ls-files src"));
    }

    #[test]
    fn test_merge_gate_policy_parses() {
        let yaml = r#"
merge:
  ff: only
  source_worktree: clean
hooks: {}
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let merge = config.merge.unwrap();
        assert_eq!(merge.ff, Some(FfPolicy::Only));
        assert_eq!(merge.source_worktree, Some(SourceWorktreePolicy::Clean));

        // Partial blocks parse; absent keys stay None.
        let config: YamlConfig = serde_yaml::from_str("merge:\n  ff: only\n").unwrap();
        let merge = config.merge.unwrap();
        assert_eq!(merge.ff, Some(FfPolicy::Only));
        assert_eq!(merge.source_worktree, None);
    }

    #[test]
    fn test_merge_gate_policy_rejects_unknown_values_loudly() {
        // `any` is deliberately NOT a YAML spelling — relaxing policy is a
        // per-invocation flag decision, so an overlay config cannot do it.
        let err = serde_yaml::from_str::<YamlConfig>("merge:\n  source_worktree: any\n")
            .expect_err("'any' must not be accepted in config");
        assert!(err.to_string().contains("any"), "{err}");

        let err = serde_yaml::from_str::<YamlConfig>("merge:\n  ff: never\n")
            .expect_err("unknown ff value must fail to parse");
        assert!(err.to_string().contains("never"), "{err}");
    }

    /// A mistyped policy KEY must fail to load, not deserialize to "no
    /// policy". Unknown keys are tolerated everywhere else in this schema;
    /// inside `merge:` they would silently disable the gate the repo thinks
    /// it committed.
    #[test]
    fn test_merge_gate_policy_rejects_unknown_keys_loudly() {
        // Kebab-case instead of the snake_case field name.
        let err = serde_yaml::from_str::<YamlConfig>("merge:\n  source-worktree: clean\n")
            .expect_err("a mistyped policy key must not parse to an empty policy");
        assert!(err.to_string().contains("source-worktree"), "{err}");

        // camelCase instead of snake_case.
        let err = serde_yaml::from_str::<YamlConfig>("merge:\n  ffOnly: only\n")
            .expect_err("a mistyped policy key must not parse to an empty policy");
        assert!(err.to_string().contains("ffOnly"), "{err}");

        // The correct spellings still load.
        let cfg: YamlConfig =
            serde_yaml::from_str("merge:\n  ff: only\n  source_worktree: clean\n").unwrap();
        let merge = cfg.merge.expect("merge block should parse");
        assert_eq!(merge.ff, Some(FfPolicy::Only));
        assert_eq!(merge.source_worktree, Some(SourceWorktreePolicy::Clean));
    }

    #[test]
    fn test_changed_rule_parses_in_skip_and_only() {
        let yaml = r#"
hooks:
  pre-merge:
    jobs:
      - name: docs-gate
        run: build-docs
        only:
          - changed: "docs/**"
      - name: heavy
        run: heavy-check
        skip:
          - changed:
              - "*.md"
              - "docs/**"
            desc: docs-only change
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let jobs = config.hooks["pre-merge"].jobs.as_ref().unwrap();

        match jobs[0].only.as_ref().unwrap() {
            OnlyCondition::Rules(rules) => match &rules[0] {
                OnlyRule::Structured(s) => match s.changed.as_ref().unwrap() {
                    StringOrList::Single(p) => assert_eq!(p, "docs/**"),
                    other => panic!("expected Single, got {other:?}"),
                },
                other => panic!("expected Structured, got {other:?}"),
            },
            other => panic!("expected Rules, got {other:?}"),
        }
        match jobs[1].skip.as_ref().unwrap() {
            SkipCondition::Rules(rules) => match &rules[0] {
                SkipRule::Structured(s) => {
                    match s.changed.as_ref().unwrap() {
                        StringOrList::List(l) => assert_eq!(l, &["*.md", "docs/**"]),
                        other => panic!("expected List, got {other:?}"),
                    }
                    assert_eq!(s.desc.as_deref(), Some("docs-only change"));
                }
                other => panic!("expected Structured, got {other:?}"),
            },
            other => panic!("expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_commands_legacy_alias() {
        let yaml = r#"
hooks:
  worktree-post-create:
    commands:
      lint:
        run: cargo clippy
      format:
        run: cargo fmt --check
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = &config.hooks["worktree-post-create"];
        let cmds = hook.commands.as_ref().unwrap();
        assert_eq!(cmds.len(), 2);
        assert!(cmds.contains_key("lint"));
        assert!(cmds.contains_key("format"));
    }

    #[test]
    fn test_group_def() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: checks
        group:
          parallel: true
          jobs:
            - name: lint
              run: cargo clippy
            - name: format
              run: cargo fmt --check
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        let group = job.group.as_ref().unwrap();
        assert_eq!(group.parallel, Some(true));
        assert_eq!(group.jobs.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn test_output_setting_disabled() {
        let yaml = r#"
output: false
hooks: {}
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.output {
            Some(OutputSetting::Disabled(false)) => {}
            other => panic!("Expected Disabled(false), got {other:?}"),
        }
    }

    #[test]
    fn test_output_setting_hooks_list() {
        let yaml = r#"
output:
  - worktree-post-create
  - post-clone
hooks: {}
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        match &config.output {
            Some(OutputSetting::Hooks(h)) => {
                assert_eq!(h.len(), 2);
                assert_eq!(h[0], "worktree-post-create");
            }
            other => panic!("Expected Hooks list, got {other:?}"),
        }
    }

    #[test]
    fn test_env_vars_on_job() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
        env:
          RUST_BACKTRACE: "1"
          MY_VAR: hello
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        let env = job.env.as_ref().unwrap();
        assert_eq!(env.get("RUST_BACKTRACE").unwrap(), "1");
        assert_eq!(env.get("MY_VAR").unwrap(), "hello");
    }

    #[test]
    fn test_command_def_to_job_def() {
        let cmd = CommandDef {
            run: Some("cargo test".to_string()),
            tags: Some(vec!["test".to_string()]),
            ..Default::default()
        };
        let job = cmd.to_job_def("my-test");
        assert_eq!(job.name.as_deref(), Some("my-test"));
        match &job.run {
            Some(RunCommand::Simple(s)) => assert_eq!(s, "cargo test"),
            other => panic!("Expected Simple, got {other:?}"),
        }
        assert!(job.needs.is_none());
    }

    #[test]
    fn test_needs_deserialize() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: install-npm
        run: npm install
      - name: npm-build
        run: npm run build
        needs: [install-npm]
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let jobs = config.hooks["worktree-post-create"].jobs.as_ref().unwrap();
        assert!(jobs[0].needs.is_none());
        assert_eq!(
            jobs[1].needs.as_deref().unwrap(),
            &["install-npm".to_string()]
        );
    }

    #[test]
    fn test_needs_absent() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        assert!(job.needs.is_none());
    }

    #[test]
    fn test_needs_empty() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo test
        needs: []
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["worktree-post-create"].jobs.as_ref().unwrap()[0];
        assert!(job.needs.as_ref().unwrap().is_empty());
    }

    #[test]
    fn test_job_description() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        description: Install Homebrew package manager
        run: echo "install brew"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        assert_eq!(
            job.description.as_deref(),
            Some("Install Homebrew package manager")
        );
    }

    #[test]
    fn test_skip_rule_desc() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        run: echo "install"
        skip:
          - run: "command -v brew"
            desc: Brew is already installed
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.skip {
            Some(SkipCondition::Rules(rules)) => match &rules[0] {
                SkipRule::Structured(s) => {
                    assert_eq!(s.desc.as_deref(), Some("Brew is already installed"));
                    assert_eq!(s.run.as_deref(), Some("command -v brew"));
                }
                other => panic!("Expected Structured, got {other:?}"),
            },
            other => panic!("Expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_only_rule_desc() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-deps
        run: npm install
        only:
          - run: "test -f package.json"
            desc: Only when package.json exists
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.only {
            Some(OnlyCondition::Rules(rules)) => match &rules[0] {
                OnlyRule::Structured(s) => {
                    assert_eq!(s.desc.as_deref(), Some("Only when package.json exists"));
                }
                other => panic!("Expected Structured, got {other:?}"),
            },
            other => panic!("Expected Rules, got {other:?}"),
        }
    }

    #[test]
    fn test_arch_single() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: arm-setup
        arch: aarch64
        run: echo "arm"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.arch {
            Some(PlatformConstraint::Single(arch)) => assert_eq!(*arch, TargetArch::Aarch64),
            other => panic!("Expected Single(Aarch64), got {other:?}"),
        }
    }

    #[test]
    fn test_arch_list() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: multi-arch
        arch: [x86_64, aarch64]
        run: echo "multi"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.arch {
            Some(PlatformConstraint::List(arch_list)) => {
                assert_eq!(arch_list.len(), 2);
                assert_eq!(arch_list[0], TargetArch::X86_64);
                assert_eq!(arch_list[1], TargetArch::Aarch64);
            }
            other => panic!("Expected List, got {other:?}"),
        }
    }

    #[test]
    fn test_run_simple_string() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: test
        run: echo hello
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.run {
            Some(RunCommand::Simple(s)) => assert_eq!(s, "echo hello"),
            other => panic!("Expected Simple, got {other:?}"),
        }
    }

    #[test]
    fn test_run_os_map() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.run {
            Some(RunCommand::Platform(map)) => {
                assert_eq!(map.len(), 2);
                match &map[&TargetOs::Macos] {
                    PlatformRunCommand::Simple(s) => assert_eq!(s, "brew install mise"),
                    other => panic!("Expected Simple, got {other:?}"),
                }
            }
            other => panic!("Expected Platform, got {other:?}"),
        }
    }

    #[test]
    fn test_run_os_map_single_os() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-brew
        run:
          macos: /bin/bash -c "$(curl -fsSL https://example.com)"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.run {
            Some(RunCommand::Platform(map)) => {
                assert_eq!(map.len(), 1);
                assert!(map.contains_key(&TargetOs::Macos));
            }
            other => panic!("Expected Platform, got {other:?}"),
        }
    }

    #[test]
    fn test_skip_platform_map() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: install-mise
        run:
          macos: brew install mise
          linux: curl https://mise.run | sh
        skip:
          macos:
            - run: "brew list mise"
              desc: mise is already installed via brew
          linux:
            - run: "command -v mise"
              desc: mise is already installed
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.skip {
            Some(SkipCondition::Platform(map)) => {
                assert_eq!(map.len(), 2);
                assert!(map.contains_key(&TargetOs::Macos));
                assert!(map.contains_key(&TargetOs::Linux));
            }
            other => panic!("Expected Platform, got {other:?}"),
        }
    }

    #[test]
    fn test_only_platform_map() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - name: setup
        run:
          macos: echo mac
          linux: echo linux
        only:
          macos:
            - run: "test -f Brewfile"
              desc: Only when Brewfile exists
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let job = &config.hooks["post-clone"].jobs.as_ref().unwrap()[0];
        match &job.only {
            Some(OnlyCondition::Platform(map)) => {
                assert_eq!(map.len(), 1);
                assert!(map.contains_key(&TargetOs::Macos));
            }
            other => panic!("Expected Platform, got {other:?}"),
        }
    }

    #[test]
    fn test_yaml_config_with_layout() {
        let yaml = r#"
layout: contained
hooks:
  post-clone:
    jobs:
      - run: echo hello
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.layout, Some("contained".into()));
    }

    #[test]
    fn test_yaml_config_without_layout() {
        let yaml = r#"
hooks:
  post-clone:
    jobs:
      - run: echo hello
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.layout, None);
    }

    #[test]
    fn test_yaml_config_with_inline_template_layout() {
        let yaml = r#"
layout: "../.worktrees/{{ repo }}/{{ branch | sanitize }}"
hooks: {}
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.layout,
            Some("../.worktrees/{{ repo }}/{{ branch | sanitize }}".into())
        );
    }

    #[test]
    fn test_tracks_field_deserializes() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: mise-trust
        run: mise trust
        tracks: [path]
      - name: docker-up
        run: ./scripts/docker-up.sh
        tracks: [path, branch]
      - name: bun-install
        run: bun install
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = config.hooks.get("worktree-post-create").unwrap();
        let jobs = hook.jobs.as_ref().unwrap();

        // mise-trust tracks path
        assert_eq!(jobs[0].tracks.as_ref().unwrap(), &[TrackedAttribute::Path]);
        // docker-up tracks both
        assert_eq!(
            jobs[1].tracks.as_ref().unwrap(),
            &[TrackedAttribute::Path, TrackedAttribute::Branch]
        );
        // bun-install has no tracks
        assert!(jobs[2].tracks.is_none());
    }

    #[test]
    fn test_shared_files_parsing() {
        let yaml = r#"
shared:
  - .env
  - .idea
  - .vscode/settings.json
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let shared = config.shared.unwrap();
        assert_eq!(shared.len(), 3);
        assert_eq!(shared[0], ".env");
        assert_eq!(shared[1], ".idea");
        assert_eq!(shared[2], ".vscode/settings.json");
    }

    #[test]
    fn test_shared_files_empty_when_missing() {
        let yaml = r#"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo hi
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.shared.is_none());
    }

    #[test]
    fn copy_bare_list_form_parses_with_default_knobs() {
        let yaml = r#"
copy:
  - target/
  - node_modules/
  - "**/dist/"
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let copy = config.copy.unwrap();
        assert!(matches!(copy, CopyConfig::Paths(_)));
        assert_eq!(copy.paths(), ["target/", "node_modules/", "**/dist/"]);
        assert_eq!(copy.fallback(), CopyFallback::Copy);
        assert_eq!(copy.max_size(), None);
    }

    #[test]
    fn copy_full_map_form_parses_every_knob() {
        let yaml = r#"
copy:
  paths:
    - target/
    - node_modules/
  fallback: skip
  max_size: 5GB
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let copy = config.copy.unwrap();
        assert_eq!(copy.paths(), ["target/", "node_modules/"]);
        assert_eq!(copy.fallback(), CopyFallback::Skip);
        assert_eq!(copy.max_size(), Some("5GB"));
    }

    #[test]
    fn copy_max_size_accepts_an_unquoted_integer() {
        // `parse_size` documents a bare integer as bytes, so this is the
        // obvious thing to write — and against a plain Option<String> it did
        // not merely fail this key: the untagged `copy:` enum failed the WHOLE
        // daft.yml, dropping every YAML hook to the legacy-script fallback.
        for yaml in [
            "copy:\n  paths: [target]\n  max_size: 1048576\n",
            "copy:\n  paths: [target]\n  max_size: \"1048576\"\n",
        ] {
            let config: YamlConfig = serde_yaml::from_str(yaml).expect(yaml);
            assert_eq!(config.copy.unwrap().max_size(), Some("1048576"), "{yaml}");
        }

        // The rest of the file survives either spelling.
        let config: YamlConfig =
            serde_yaml::from_str("shared:\n  - .env\ncopy:\n  paths: [target]\n  max_size: 5000\n")
                .unwrap();
        assert_eq!(config.shared.unwrap(), [".env"]);
    }

    #[test]
    fn copy_full_map_form_defaults_the_omitted_knobs() {
        // The map form without knobs must behave exactly like the bare list.
        let yaml = "copy:\n  paths: [target/]\n";
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let copy = config.copy.unwrap();
        assert_eq!(copy.paths(), ["target/"]);
        assert_eq!(copy.fallback(), CopyFallback::Copy);
        assert_eq!(copy.max_size(), None);
    }

    #[test]
    fn copy_absent_is_none_and_never_serialized() {
        let config: YamlConfig = serde_yaml::from_str("hooks: {}\n").unwrap();
        assert!(config.copy.is_none());
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(
            !yaml.contains("copy:"),
            "unset copy must be omitted:\n{yaml}"
        );
    }

    #[test]
    fn copy_fallback_parses_case_insensitively() {
        // A bad enum value fails the ENTIRE daft.yml deserialize, which
        // silently drops every YAML hook to the legacy-script fallback (the
        // hazard `FailMode`'s custom impl exists for). `fallback: Copy` — the
        // spelling a user writes after reading the Rust variant name — must
        // not land in that blast radius over a capital letter.
        for (spelling, expected) in [
            ("copy", CopyFallback::Copy),
            ("Copy", CopyFallback::Copy),
            ("COPY", CopyFallback::Copy),
            ("skip", CopyFallback::Skip),
            ("Skip", CopyFallback::Skip),
            ("SKIP", CopyFallback::Skip),
        ] {
            let yaml = format!("copy:\n  paths: [t/]\n  fallback: {spelling}\n");
            let config: YamlConfig = serde_yaml::from_str(&yaml)
                .unwrap_or_else(|e| panic!("{spelling:?} should parse, got: {e}"));
            assert_eq!(config.copy.unwrap().fallback(), expected);
        }
    }

    #[test]
    fn copy_rejects_an_unknown_fallback_value() {
        // A genuinely invalid value must fail to load, never deserialize to a
        // silent default — `fallback: symlink` silently meaning `copy` would
        // byte-copy caches the user asked daft to leave alone.
        //
        // And the message must NAME it. The failure takes the whole `daft.yml`
        // with it, so the runtime warning reporting this string is the entire
        // diagnosis the user gets; a derived untagged impl discarded its
        // variants' errors and said only "data did not match any variant of
        // untagged enum CopyConfig", which names neither the key nor the value
        // nor the fix. That is what the hand-written `Deserialize` is for.
        let err = serde_yaml::from_str::<YamlConfig>("copy:\n  paths: [t/]\n  fallback: symlink\n")
            .expect_err("an unknown fallback value must fail to parse");
        let text = err.to_string();
        assert!(
            text.contains("symlink"),
            "must name the offending value: {err}"
        );
        assert!(
            text.contains("copy") && text.contains("skip"),
            "must name the accepted values: {err}"
        );

        assert_eq!(CopyFallback::parse("symlink"), None);
        assert_eq!(CopyFallback::parse("copy"), Some(CopyFallback::Copy));
        assert_eq!(CopyFallback::parse("SKIP"), Some(CopyFallback::Skip));
    }

    #[test]
    fn copy_map_without_paths_key_parses_to_an_empty_full_form() {
        // `paths` defaults precisely so this reaches a diagnosis instead of
        // failing the whole file: unknown keys inside the map form are
        // tolerated, so a misspelled `paths:` lands here. `copy_entries` warns
        // that the block declares nothing, and `daft hooks validate` errors.
        let config: YamlConfig = serde_yaml::from_str("copy:\n  fallback: skip\n").unwrap();
        let copy = config.copy.unwrap();
        assert!(copy.is_empty());
        assert_eq!(copy.fallback(), CopyFallback::Skip);
    }

    #[test]
    fn copy_roundtrips_through_serialization_in_both_forms() {
        for cfg in [
            CopyConfig::Paths(vec!["target/".into()]),
            CopyConfig::Full {
                paths: vec!["target/".into()],
                fallback: Some(CopyFallback::Skip),
                max_size: Some("500MB".into()),
            },
        ] {
            let config = YamlConfig {
                copy: Some(cfg.clone()),
                ..Default::default()
            };
            let yaml = serde_yaml::to_string(&config).unwrap();
            let back: YamlConfig = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(back.copy, Some(cfg), "roundtrip failed for:\n{yaml}");
            assert!(!yaml.contains("null"), "no null litter:\n{yaml}");
        }
    }

    #[test]
    fn test_deserialize_background_job() {
        let yaml = r#"
hooks:
  worktree-post-create:
    background: true
    jobs:
      - name: warm build
        run: cargo build
      - name: install deps
        run: pnpm install
        background: false
        log:
          retention: "14d"
      - name: silent job
        run: echo hello
        background_output: silent
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        let hook = config.hooks.get("worktree-post-create").unwrap();
        assert_eq!(hook.background, Some(true));

        let jobs = hook.jobs.as_ref().unwrap();
        // Job 0: inherits hook-level background (no override)
        assert_eq!(jobs[0].background, None);
        // Job 1: explicit override
        assert_eq!(jobs[1].background, Some(false));
        assert_eq!(
            jobs[1].log.as_ref().unwrap().retention,
            Some("14d".to_string())
        );
        // Job 2: background_output
        assert_eq!(jobs[2].background_output, Some(BackgroundOutput::Silent));
    }

    #[test]
    fn test_deserialize_top_level_log_config() {
        let yaml = r#"
log:
  retention: "30d"
hooks:
  worktree-post-create:
    jobs:
      - name: test
        run: echo hi
"#;
        let config: YamlConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            config.log.as_ref().unwrap().retention,
            Some("30d".to_string())
        );
    }

    #[test]
    fn log_config_parses_all_new_fields() {
        let yaml = r#"
retention: 14d
max_log_size: 20MB
max_total_size: 1GB
keep_last: 5
stale_running_after: 12h
"#;
        let cfg: LogConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.retention.as_deref(), Some("14d"));
        assert_eq!(cfg.max_log_size.as_deref(), Some("20MB"));
        assert_eq!(cfg.max_total_size.as_deref(), Some("1GB"));
        assert_eq!(cfg.keep_last, Some(5));
        assert_eq!(cfg.stale_running_after.as_deref(), Some("12h"));
    }
}

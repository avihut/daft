//! Deterministic per-worktree env values (#388) — the pure derivation core.
//!
//! Every value is a function of `(scheme, salt, worktree-slug, declaration)`
//! and nothing else: no allocation registry, no store, no I/O. Any party can
//! compute any worktree's values — across repos, across machines, even
//! before the worktree exists — which is the whole point: determinism is the
//! communication channel.
//!
//! This module is **zero-I/O by construction**. Callers (the `daft env`
//! command, hook/task injection, exec) supply the effective config, the
//! slug, and any filesystem-derived context; nothing here reads disk, spawns
//! processes, or consults the environment — with the one documented
//! exception of [`spawn_injection`]'s parent-env filter, whose predicate is
//! injectable for tests.
//!
//! Layout: [`hash`] owns the scheme-1 hash contract, [`offsets`] the range
//! geometry, [`address`] the `[repo:]VAR[@worktree]` grammar. This file owns
//! the effective spec, name resolution, the `values:` template expander, and
//! the collision scan.

pub mod address;
pub mod hash;
pub mod offsets;

use crate::hooks::yaml_config::EnvConfig;
use offsets::Regions;
use std::collections::BTreeMap;

/// Default port range: below the Linux ephemeral floor (32768) and macOS's
/// (49152), above the 3000–9999 zone dev tools squat on.
pub const DEFAULT_RANGE: (u16, u16) = (20000, 32767);
/// Default ports per worktree block.
pub const DEFAULT_BLOCK_SIZE: u16 = 16;
/// The only derivation scheme this daft knows.
pub const SCHEME: u32 = 1;

/// The effective env declaration after defaults are applied — the resolved
/// form of daft.yml's `env:` (plus, at the command layer, any CLI overrides).
#[derive(Debug, Clone, PartialEq)]
pub struct EnvSpec {
    pub scheme: u32,
    /// Effective salt. Defaults to the project-root directory name; pinning
    /// `salt:` in the tracked config is what makes values identical across
    /// machines and clone locations.
    pub salt: String,
    pub range: (u16, u16),
    pub block_size: u16,
    /// Declared ports with resolved offsets, in declaration order.
    pub ports: Vec<(String, u16)>,
    /// Declared value templates (rendered per worktree, emitted alphabetically).
    pub values: BTreeMap<String, String>,
    /// Default dotenv target for `--write`.
    pub write: Option<String>,
    /// Whether the repo declared an `env:` section at all. Declaring opts the
    /// repo into strictness: unknown names error instead of resolving ad-hoc.
    pub declared: bool,
}

impl EnvSpec {
    /// Build the effective spec from an optional config section.
    ///
    /// `default_salt` is the repo identity (project-root directory name) —
    /// used verbatim when the config pins no `salt:`.
    pub fn from_config(env: Option<&EnvConfig>, default_salt: &str) -> Self {
        let declared = env.is_some();
        let env_default = EnvConfig::default();
        let env = env.unwrap_or(&env_default);
        EnvSpec {
            scheme: env.scheme.unwrap_or(SCHEME),
            salt: env.salt.clone().unwrap_or_else(|| default_salt.to_string()),
            range: env.parsed_range().unwrap_or(DEFAULT_RANGE),
            block_size: env.block_size.unwrap_or(DEFAULT_BLOCK_SIZE),
            ports: env.resolved_ports(),
            values: env.values.clone().unwrap_or_default(),
            write: env.write.clone(),
            declared,
        }
    }

    /// The range geometry, if the spec's numbers are viable.
    pub fn regions(&self) -> Result<Regions, DerivationError> {
        offsets::regions(self.range, self.block_size).ok_or(DerivationError::DegenerateRange {
            range: self.range,
            block_size: self.block_size,
        })
    }

    /// Base port of `slug`'s block.
    pub fn block_base(&self, slug: &str) -> Result<u16, DerivationError> {
        Ok(self
            .regions()?
            .block_base(self.scheme, &self.salt, slug, self.block_size))
    }

    /// The declared port for `name` in `slug`'s block, if `name` is declared.
    pub fn declared_port(&self, slug: &str, name: &str) -> Result<Option<u16>, DerivationError> {
        let Some((_, offset)) = self.ports.iter().find(|(n, _)| n == name) else {
            return Ok(None);
        };
        Ok(Some(self.block_base(slug)? + offset))
    }

    /// The ad-hoc port for an undeclared `(slug, name)` pair — always
    /// computable, lands in the region disjoint from every declared block.
    pub fn adhoc_port(&self, slug: &str, name: &str) -> Result<u16, DerivationError> {
        Ok(self
            .regions()?
            .adhoc_port(self.scheme, &self.salt, slug, name))
    }

    /// Resolve every declared value for `slug`: ports in declaration order,
    /// then rendered `values:` templates alphabetically.
    ///
    /// Names in daft's own `DAFT_*` namespace are **skipped**, not resolved.
    /// This is the path whose output becomes a job's process environment, so
    /// resolving one would hash `DAFT_BRANCH_NAME` into a port number and
    /// then overwrite the real branch name with it — `extra_env` is applied
    /// last in `HookEnvironment::from_context`. `validate_config` refuses
    /// such a declaration outright, but validation only runs on demand, so
    /// this is the load-bearing guard for a config that never saw it. One
    /// bad name loses only itself: the rest of the schema still resolves,
    /// and [`reserved_names`] lets callers say what was dropped.
    pub fn resolve_all(
        &self,
        slug: &str,
        ctx: &ValueContext<'_>,
    ) -> Result<Vec<ResolvedValue>, DerivationError> {
        let mut out = Vec::with_capacity(self.ports.len() + self.values.len());
        if !self.ports.is_empty() {
            let base = self.block_base(slug)?;
            for (name, offset) in &self.ports {
                if is_reserved_name(name) {
                    continue;
                }
                out.push(ResolvedValue {
                    name: name.clone(),
                    value: (base + offset).to_string(),
                    source: ValueSource::DeclaredPort { offset: *offset },
                });
            }
        }
        for (name, template) in &self.values {
            if is_reserved_name(name) {
                continue;
            }
            out.push(ResolvedValue {
                name: name.clone(),
                value: self.expand_value(name, template, slug, ctx)?,
                source: ValueSource::Value,
            });
        }
        Ok(out)
    }

    /// Declared names that fall in daft's reserved namespace, in a stable
    /// order. Empty for every valid schema; non-empty means [`resolve_all`]
    /// skipped something and the caller should say so.
    ///
    /// [`resolve_all`]: Self::resolve_all
    pub fn reserved_names(&self) -> Vec<&str> {
        let ports = self.ports.iter().map(|(name, _)| name.as_str());
        let values = self.values.keys().map(String::as_str);
        ports
            .chain(values)
            .filter(|name| is_reserved_name(name))
            .collect()
    }

    /// Resolve one name for `slug`: declared port, declared value, or —
    /// only when `adhoc` allows it — an ad-hoc port.
    pub fn resolve_one(
        &self,
        slug: &str,
        name: &str,
        ctx: &ValueContext<'_>,
        adhoc: AdHocPolicy,
    ) -> Result<ResolvedValue, DerivationError> {
        // Daft's own variables are never derived — without this an ad-hoc
        // resolution would confidently hash DAFT_BRANCH_NAME into a
        // meaningless port number. `resolve_all` skips them for the same
        // reason; here the caller named one explicitly, so say so.
        if is_reserved_name(name) {
            return Err(DerivationError::ReservedName {
                name: name.to_string(),
            });
        }
        if let Some(port) = self.declared_port(slug, name)? {
            let offset = port - self.block_base(slug)?;
            return Ok(ResolvedValue {
                name: name.to_string(),
                value: port.to_string(),
                source: ValueSource::DeclaredPort { offset },
            });
        }
        if let Some(template) = self.values.get(name) {
            return Ok(ResolvedValue {
                name: name.to_string(),
                value: self.expand_value(name, template, slug, ctx)?,
                source: ValueSource::Value,
            });
        }
        let allowed = match adhoc {
            AdHocPolicy::Allow => true,
            AdHocPolicy::Forbid => false,
            AdHocPolicy::UnlessDeclaredSchema => !self.declared,
        };
        if !allowed {
            return Err(DerivationError::UnknownName {
                name: name.to_string(),
            });
        }
        Ok(ResolvedValue {
            name: name.to_string(),
            value: self.adhoc_port(slug, name)?.to_string(),
            source: ValueSource::AdHoc,
        })
    }

    /// Render a `values:` template for `slug`.
    ///
    /// Deliberately a confined dialect, not the hook substituter: the same
    /// variable *names* (`{worktree_slug}`, `{worktree_path}`,
    /// `{worktree_root}`, `{branch}`, `{repo}`) plus `{env:PORT_NAME}` for
    /// embedding a declared port — but unresolved placeholders are **errors**
    /// here (fail-closed), where the hook substituter leaves them intact.
    /// `{env:...}` may reference declared ports only — values referencing
    /// values would introduce ordering/cycles for no expressive gain.
    fn expand_value(
        &self,
        value_name: &str,
        template: &str,
        slug: &str,
        ctx: &ValueContext<'_>,
    ) -> Result<String, DerivationError> {
        let mut out = String::with_capacity(template.len());
        let mut rest = template;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                // No closing brace: literal text, not a placeholder.
                out.push_str(&rest[open..]);
                rest = "";
                break;
            };
            let token = &after[..close];
            let replacement = if let Some(port_name) = token.strip_prefix("env:") {
                if self.values.contains_key(port_name) {
                    return Err(DerivationError::ValueReferencesValue {
                        value: value_name.to_string(),
                        referenced: port_name.to_string(),
                    });
                }
                match self.declared_port(slug, port_name)? {
                    Some(port) => port.to_string(),
                    None => {
                        return Err(DerivationError::UnknownPortRef {
                            value: value_name.to_string(),
                            referenced: port_name.to_string(),
                        });
                    }
                }
            } else {
                match token {
                    "worktree_slug" => slug.to_string(),
                    "repo" => ctx.repo.to_string(),
                    "worktree_path" => ctx
                        .worktree_path
                        .ok_or_else(|| DerivationError::UnavailableVar {
                            value: value_name.to_string(),
                            var: "worktree_path".to_string(),
                        })?
                        .to_string(),
                    "worktree_root" => ctx
                        .worktree_root
                        .ok_or_else(|| DerivationError::UnavailableVar {
                            value: value_name.to_string(),
                            var: "worktree_root".to_string(),
                        })?
                        .to_string(),
                    "branch" => ctx
                        .branch
                        .ok_or_else(|| DerivationError::UnavailableVar {
                            value: value_name.to_string(),
                            var: "branch".to_string(),
                        })?
                        .to_string(),
                    unknown => {
                        return Err(DerivationError::UnknownPlaceholder {
                            value: value_name.to_string(),
                            placeholder: unknown.to_string(),
                        });
                    }
                }
            };
            out.push_str(&replacement);
            rest = &after[close + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    /// Slugs (other than `this_slug`) whose block collides with
    /// `this_slug`'s. Deterministic and pure — callers supply the enumerated
    /// worktree slugs.
    pub fn block_collisions(
        &self,
        this_slug: &str,
        other_slugs: &[String],
    ) -> Result<Vec<String>, DerivationError> {
        let regions = self.regions()?;
        let mine = regions.block_index(self.scheme, &self.salt, this_slug);
        Ok(other_slugs
            .iter()
            .filter(|s| s.as_str() != this_slug)
            .filter(|s| regions.block_index(self.scheme, &self.salt, s) == mine)
            .cloned()
            .collect())
    }
}

/// Whether `name` belongs to daft's own reserved namespace.
///
/// The one predicate every path shares, so the query surface, the listing,
/// and the injection map can never disagree about which names derivation
/// owns.
pub fn is_reserved_name(name: &str) -> bool {
    name.starts_with("DAFT_")
}

/// Whether an undeclared name may resolve to an ad-hoc port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdHocPolicy {
    /// Always allow (the `--ad-hoc` escape hatch).
    Allow,
    /// Never (injection paths: ad-hoc names are unenumerable, so they are
    /// never injected).
    Forbid,
    /// The default: allowed only when the repo declared no `env:` schema —
    /// declaring opts into strictness, where a miss is a typo, not intent.
    UnlessDeclaredSchema,
}

/// Context for rendering `values:` templates. Optional fields cover the
/// compute-before-create case: a hypothetical worktree has a slug but no
/// path, root, or branch — templates referencing those error instead of
/// guessing.
#[derive(Debug, Clone, Default)]
pub struct ValueContext<'a> {
    pub repo: &'a str,
    pub worktree_path: Option<&'a str>,
    pub worktree_root: Option<&'a str>,
    pub branch: Option<&'a str>,
}

/// Where a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueSource {
    DeclaredPort {
        offset: u16,
    },
    Value,
    AdHoc,
    /// One of daft's own `DAFT_*` job variables, read from worktree state.
    /// Constructed by the `daft env` command only — the derivation core
    /// never produces it (`resolve_one` refuses the prefix outright).
    Context,
}

impl ValueSource {
    /// Short label for tables and structured output.
    pub fn label(&self) -> &'static str {
        match self {
            ValueSource::DeclaredPort { .. } => "port",
            ValueSource::Value => "value",
            ValueSource::AdHoc => "ad-hoc",
            ValueSource::Context => "context",
        }
    }
}

/// One resolved `NAME=value`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedValue {
    pub name: String,
    pub value: String,
    pub source: ValueSource,
}

/// Errors a derivation can produce. Every variant names its fix — a wrong
/// port is two services silently fighting, so nothing here degrades to a
/// guess.
#[derive(Debug, Clone, PartialEq)]
pub enum DerivationError {
    DegenerateRange { range: (u16, u16), block_size: u16 },
    UnknownName { name: String },
    ReservedName { name: String },
    UnknownPlaceholder { value: String, placeholder: String },
    UnknownPortRef { value: String, referenced: String },
    ValueReferencesValue { value: String, referenced: String },
    UnavailableVar { value: String, var: String },
}

impl std::fmt::Display for DerivationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DerivationError::DegenerateRange { range, block_size } => write!(
                f,
                "range {}-{} cannot fit declared and ad-hoc regions with blocks of {} ports",
                range.0, range.1, block_size
            ),
            DerivationError::UnknownName { name } => {
                write!(f, "'{name}' is not declared in this repo's env: section")
            }
            DerivationError::ReservedName { name } => write!(
                f,
                "'{name}' is reserved; the DAFT_ prefix belongs to daft's own \
                 variables, which are read from worktree state rather than derived"
            ),
            DerivationError::UnknownPlaceholder { value, placeholder } => write!(
                f,
                "env.values.{value} references unknown placeholder '{{{placeholder}}}'"
            ),
            DerivationError::UnknownPortRef { value, referenced } => write!(
                f,
                "env.values.{value} references '{{env:{referenced}}}', which is not a declared port"
            ),
            DerivationError::ValueReferencesValue { value, referenced } => write!(
                f,
                "env.values.{value} references '{{env:{referenced}}}', which is itself a value; \
                 values may reference declared ports only"
            ),
            DerivationError::UnavailableVar { value, var } => write!(
                f,
                "env.values.{value} references '{{{var}}}', which is not known here \
                 (the worktree may not exist yet)"
            ),
        }
    }
}

impl std::error::Error for DerivationError {}

/// The never-clobber injection filter: declared values destined for a child
/// process's environment, **minus** any name already set in the parent
/// environment (dotenv convention — `VAR=x cmd` stays the universal
/// override, and stacking injection tiers is idempotent).
pub fn spawn_injection(resolved: &[ResolvedValue]) -> BTreeMap<String, String> {
    spawn_injection_with(resolved, |name| std::env::var_os(name).is_some())
}

/// [`spawn_injection`] with an injectable "is already set" predicate.
pub fn spawn_injection_with(
    resolved: &[ResolvedValue],
    parent_has: impl Fn(&str) -> bool,
) -> BTreeMap<String, String> {
    resolved
        .iter()
        .filter(|v| !parent_has(&v.name))
        .map(|v| (v.name.clone(), v.value.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::yaml_config::PortEntry;

    fn spec_with(ports: &[(&str, Option<u16>)], values: &[(&str, &str)]) -> EnvSpec {
        let env = EnvConfig {
            salt: Some("myapp".into()),
            ports: Some(
                ports
                    .iter()
                    .map(|(n, o)| PortEntry {
                        name: n.to_string(),
                        offset: *o,
                    })
                    .collect(),
            ),
            values: Some(
                values
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            ..Default::default()
        };
        EnvSpec::from_config(Some(&env), "ignored-default")
    }

    fn ctx() -> ValueContext<'static> {
        ValueContext {
            repo: "myapp",
            worktree_path: Some("/p/feature-new"),
            worktree_root: Some("/p"),
            branch: Some("feature/new"),
        }
    }

    /// End-to-end golden: block base 23952 for (myapp, feature-new) — see
    /// offsets.rs goldens — so declared ports are base + offset.
    #[test]
    fn declared_ports_are_block_base_plus_offset() {
        let spec = spec_with(&[("WEBAPP_PORT", None), ("API_PORT", Some(8))], &[]);
        assert_eq!(spec.block_base("feature-new").unwrap(), 23952);
        assert_eq!(
            spec.declared_port("feature-new", "WEBAPP_PORT").unwrap(),
            Some(23952)
        );
        assert_eq!(
            spec.declared_port("feature-new", "API_PORT").unwrap(),
            Some(23960)
        );
        assert_eq!(spec.declared_port("feature-new", "NOPE").unwrap(), None);
    }

    /// The DAFT_ prefix never derives — under any policy, even the
    /// permissive ones. Without the guard, `--ad-hoc` (or a schema-less
    /// repo) would hash DAFT_BRANCH_NAME into a confidently wrong port.
    #[test]
    fn reserved_daft_names_never_derive() {
        let spec = spec_with(&[("WEBAPP_PORT", None)], &[]);
        for policy in [
            AdHocPolicy::Allow,
            AdHocPolicy::Forbid,
            AdHocPolicy::UnlessDeclaredSchema,
        ] {
            let err = spec
                .resolve_one("feature-new", "DAFT_BRANCH_NAME", &ctx(), policy)
                .unwrap_err();
            assert_eq!(
                err,
                DerivationError::ReservedName {
                    name: "DAFT_BRANCH_NAME".into()
                }
            );
        }
    }

    #[test]
    fn resolve_all_orders_ports_then_values_alphabetically() {
        let spec = spec_with(
            &[("B_PORT", None), ("A_PORT", None)],
            &[("Z_NAME", "z"), ("A_NAME", "a")],
        );
        let names: Vec<String> = spec
            .resolve_all("feature-new", &ctx())
            .unwrap()
            .into_iter()
            .map(|v| v.name)
            .collect();
        // Ports keep declaration order (offsets!), values are alphabetical.
        assert_eq!(names, vec!["B_PORT", "A_PORT", "A_NAME", "Z_NAME"]);
    }

    #[test]
    fn values_expand_slug_repo_and_port_refs() {
        let spec = spec_with(
            &[("API_PORT", None)],
            &[
                ("PROJECT", "{repo}-{worktree_slug}"),
                ("API_URL", "http://localhost:{env:API_PORT}/x"),
            ],
        );
        let all = spec.resolve_all("feature-new", &ctx()).unwrap();
        let get = |n: &str| {
            all.iter()
                .find(|v| v.name == n)
                .map(|v| v.value.clone())
                .unwrap()
        };
        assert_eq!(get("PROJECT"), "myapp-feature-new");
        assert_eq!(get("API_URL"), "http://localhost:23952/x");
    }

    #[test]
    fn value_errors_are_fail_closed_and_named() {
        // Unknown placeholder.
        let spec = spec_with(&[], &[("X", "{typo}")]);
        let err = spec.resolve_all("s", &ctx()).unwrap_err();
        assert!(matches!(err, DerivationError::UnknownPlaceholder { .. }));

        // {env:...} referencing an undeclared port.
        let spec = spec_with(&[], &[("X", "{env:NOPE}")]);
        let err = spec.resolve_all("s", &ctx()).unwrap_err();
        assert!(matches!(err, DerivationError::UnknownPortRef { .. }));

        // {env:...} referencing another value.
        let spec = spec_with(&[], &[("X", "{env:Y}"), ("Y", "y")]);
        let err = spec.resolve_all("s", &ctx()).unwrap_err();
        assert!(matches!(err, DerivationError::ValueReferencesValue { .. }));

        // A var the caller could not supply (compute-before-create).
        let spec = spec_with(&[], &[("X", "{branch}")]);
        let bare = ValueContext {
            repo: "r",
            ..Default::default()
        };
        let err = spec.resolve_all("s", &bare).unwrap_err();
        assert!(matches!(err, DerivationError::UnavailableVar { .. }));

        // Text with braces that is not a placeholder passes through.
        let spec = spec_with(&[], &[("X", "a{b")]);
        assert_eq!(spec.resolve_all("s", &ctx()).unwrap()[0].value, "a{b");
    }

    #[test]
    fn strictness_follows_declaration() {
        let declared = spec_with(&[("A_PORT", None)], &[]);
        let err = declared
            .resolve_one("s", "TYPO_PORT", &ctx(), AdHocPolicy::UnlessDeclaredSchema)
            .unwrap_err();
        assert!(matches!(err, DerivationError::UnknownName { .. }));

        // --ad-hoc overrides strictness; lands in the ad-hoc region.
        let v = declared
            .resolve_one("s", "TYPO_PORT", &ctx(), AdHocPolicy::Allow)
            .unwrap();
        assert_eq!(v.source, ValueSource::AdHoc);
        assert!(v.value.parse::<u16>().unwrap() >= 26384);

        // No schema at all: everything ad-hoc, zero config.
        let undeclared = EnvSpec::from_config(None, "repo-dir");
        assert!(!undeclared.declared);
        let v = undeclared
            .resolve_one("s", "ANY_PORT", &ctx(), AdHocPolicy::UnlessDeclaredSchema)
            .unwrap();
        assert_eq!(v.source, ValueSource::AdHoc);
        assert_eq!(undeclared.salt, "repo-dir", "default salt = repo dir name");
    }

    #[test]
    fn injection_never_clobbers_parent_env() {
        let resolved = vec![
            ResolvedValue {
                name: "A_PORT".into(),
                value: "1".into(),
                source: ValueSource::DeclaredPort { offset: 0 },
            },
            ResolvedValue {
                name: "B_PORT".into(),
                value: "2".into(),
                source: ValueSource::DeclaredPort { offset: 1 },
            },
        ];
        let injected = spawn_injection_with(&resolved, |name| name == "A_PORT");
        assert!(!injected.contains_key("A_PORT"), "parent-set var respected");
        assert_eq!(injected.get("B_PORT").map(String::as_str), Some("2"));
    }

    #[test]
    fn collision_scan_flags_same_block_only() {
        let spec = spec_with(&[("A_PORT", None)], &[]);
        // A slug always collides with itself under the same salt — feed a
        // synthetic twin plus non-colliding names and assert filtering.
        let mine = spec
            .regions()
            .unwrap()
            .block_index(1, "myapp", "feature-new");
        let others = vec![
            "feature-new".to_string(), // self — excluded
            "master".to_string(),
            "other".to_string(),
        ];
        let collisions = spec.block_collisions("feature-new", &others).unwrap();
        for c in &collisions {
            assert_ne!(c, "feature-new");
            assert_eq!(spec.regions().unwrap().block_index(1, "myapp", c), mine);
        }
        for s in others.iter().filter(|s| !collisions.contains(s)) {
            if s != "feature-new" {
                assert_ne!(spec.regions().unwrap().block_index(1, "myapp", s), mine);
            }
        }
    }

    #[test]
    fn cli_override_semantics_ride_the_spec() {
        // "daft env --salt candidate" previews a full remap: same spec,
        // different salt, different block — deterministic both ways.
        let a = spec_with(&[("A_PORT", None)], &[]);
        let mut b = a.clone();
        b.salt = "candidate".into();
        assert_ne!(
            a.block_base("feature-new").unwrap(),
            b.block_base("feature-new").unwrap()
        );
    }
}

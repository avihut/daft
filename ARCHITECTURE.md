# Architecture

This document records daft's architectural intent and long-term direction.

Operational rules ("do X this way", "don't do Y") live in
[CLAUDE.md](./CLAUDE.md). This file owns the **why** and the **where we're
going**. When making architectural decisions or introducing a new subsystem,
read both: CLAUDE.md tells you the conventions; this file tells you which
direction those conventions are pointing.

## Today

Daft is a multicall Rust binary that wraps git for worktree management. The tree
is currently one Cargo crate, organized as a layered architecture with hexagonal
boundaries around the coordinator subsystem:

```
Application                src/commands/*, src/main.rs, IPC dispatch
   |                       vertical slice — one command = one file
   v
Subsystem boundary
   src/coordinator/ports/     traits owned by the consuming subsystem
   src/coordinator/adapters/  concrete impls (sqlite_store,
                              system_clock, unix_process)
   |
   v
Domain                     src/coordinator/domain/
   pure logic — no SQL, no nix, no std::env
   |
   v
Store                      src/store/
   SQLite + migrations + typed row models + repos
   no coordinator types, no business logic
```

`reconcile_active_jobs` is the canonical worked example — a pure domain
function, port-driven from the outside, six fake-adapter unit tests. PR #508
(redb → SQLite migration) is what proved the layering: swapping the entire
persistence backend was a single-adapter-file change.

## Guiding principles

### Hexagonal at subsystem boundaries

A subsystem with meaningful domain logic (today: coordinator; future: hooks,
trust, repo catalog) talks to the outside world through ports. Adapters
implement them. Domain code never imports infrastructure crates.

Hard rules:

- **Domain never imports `rusqlite`, `nix`, or `std::env`**.
- **Store never imports domain types** — it returns typed row structs (`JobRow`,
  `RepoPolicyRow`); domain shapes live above.
- **Adapters are the only translation layer** — cross-cutting concerns (env
  scrub, status-enum ↔ TEXT, policy merge semantics) belong here.

This is what makes the layer unit-testable without spawning processes or
touching disk, and what made redb → SQLite a surgical replacement instead of a
rewrite.

### Functional core inside domain modules

Within a domain module, prefer pure functions over snapshots:

```rust
// snapshot in
pub struct ReconcileInput<'a> {
    pub active_jobs: &'a [JobRow],
    pub alive_pgids: &'a HashSet<u32>,
    pub now: DateTime<Utc>,
}

// decision out
pub struct ReconcileDecision {
    pub mark_crashed: Vec<JobRow>,
}

// no traits, no I/O, no clock — just values
pub fn reconcile(input: ReconcileInput) -> ReconcileDecision { ... }
```

The adapter is the imperative shell: read via the ports, hand the snapshot to
the pure core, apply the decision via the ports. Tests assert against returned
values, not against fake-adapter interactions ("did you call this method with
this argument?" tests check wiring, not behavior).

This composes with hexagonal — hexagonal at the **boundary**, functional core
**inside**. Most daft domain logic is batch-shaped (reconcile, cancel-matching,
prune-budget, retention policy, trust evaluation), so FCIS fits cleanly. When
logic legitimately needs to interleave I/O with computation, leave it in the
imperative shell rather than forcing FCIS to fit.

### Vertical slice at the CLI command layer

`src/commands/<name>.rs` is the de facto pattern — each command is a
self-contained file with its clap `Args` struct, body, and helpers. **Do not
impose hexagonal on commands.** They are mostly thin: parse args, call into a
subsystem, format output. Ceremony would drown the value.

When a command's body grows enough to have its own domain logic, lift that into
the subsystem it belongs to, not into a parallel structure inside `commands/`.

### Other principles that earn their keep

- **SQLite is the only structured-data store.** See CLAUDE.md "Database &
  Storage". Future registries (trust, repo catalog) migrate to `src/store/` in
  their own PRs.
- **Spawn-self, not fork.** Forced by `forbid(unsafe_code)`. Contract in
  CLAUDE.md "Background processes".
- **All persistence flows through `src/store/connection.rs::bring_up`.**
  Security defaults (PRAGMAs, 0600/0700 perms, `application_id` check) are
  non-bypassable.
- **No pre-1.0 back-compat.** Old on-disk formats auto-wipe with a stderr
  banner, not read-fallback'd. Cut cleanly.

## Long-term direction (landposts, not commitments)

These are pinned intentions, not scheduled work. They guide which direction
current decisions should point, so we don't accumulate technical debt against
them.

### Crate decomposition (gitoxide-style)

Daft today is one Cargo crate. The long-term target is a workspace of focused
crates — one concern each, narrow public APIs, independent versioning. This is
gitoxide's organizing principle (one crate per git subsystem) applied to daft:

```
daft-types          shared value types (JobRow, RepoPolicy, JobAddress, ...)
                    no traits, no I/O — just data
                    everyone depends on this

daft-store          SQLite store (connection, pool, migrations, repos)
                    exposes SqliteJobsStore (concrete) + StorePort (trait)

daft-coordinator    domain logic (reconcile, cancel-matching, lifecycle)
                    generic over StorePort, ProcessPort, ClockPort
                    no SQL, no nix, no std::env — compiles on wasm32

daft-hooks          hook discovery + execution
                    owns FilesystemPort, TrustPort

daft-trust          trust registry (planned migration to daft-store)
                    internal: pure rule evaluation; TrustPort at boundary

daft-layout         layout resolution
                    pure functions; no ports

daft-cli            the daft binary
                    wires daft-store::SqliteJobsStore into
                    daft-coordinator etc.
```

**This is not a near-term refactor.** Multi-crate workspaces have ongoing cost:
separate `Cargo.toml`s, version coordination, longer clean builds,
internal-cycles management during refactors. The internal hexagonal in PR #508
already buys most of the architectural property; the **crate split**
specifically unlocks the IDE spin-out described below. Defer until a concrete
second consumer exists.

When the split happens, the principle is:

- **Apply hexagonal at crate boundaries** for crates the second consumer (the
  IDE) will plug different backends into. Those become the natural port
  locations.
- **Concrete types within a crate** are fine. Don't manufacture ports for
  one-impl-in-production seams.
- **Crates with no plausible swap stay closed.** `daft-layout` is a pure
  resolver — no ports, just functions.

### Native agentic IDE spin-out

This is the **why** for crate-boundary ports. The IDE will reuse daft's
worktree, hook, trust, and coordinator logic but with different infrastructure:

| Port             | daft CLI today          | IDE future                                         |
| ---------------- | ----------------------- | -------------------------------------------------- |
| `Store`          | `SqliteJobsStore`       | IDE-managed state, in-memory, or alternative DB    |
| `ProcessControl` | `UnixProcess` (signals) | Integrated with IDE runtime, possibly in-process   |
| `Clock`          | `SystemClock`           | Real time, plus mockable in IDE test infra         |
| `Filesystem`     | `std::fs` (future port) | Project-view abstraction, virtual fs for cloud IDE |

Without ports at the crate boundary, the IDE forks daft's code or accepts
implementation choices wholesale. Both are bad. With ports, the IDE does
`cargo add daft-coordinator` and provides its own adapters.

The hexagonal pattern earns its biggest dividend here: the internal hexagonal
that lives today gets **lifted** from "inside one crate" to "at a crate edge"
when the spin-out begins. The trait shapes don't change; only their visibility
does.

Concretely: once the split is done, `daft-coordinator` depends only on
`daft-types` and its own port traits — **not on `daft-store`**. That means it
compiles on `wasm32-unknown-unknown` (no SQLite, no signals) and can be embedded
in any IDE without dragging the entire daft binary along.

### Open design questions

To be decided when the spin-out starts, not before:

- **Sync vs async ports.** Today's ports are sync. SQLite calls complete in
  microseconds, so sync is fine for daft. The IDE will live in an async runtime;
  lean toward sync ports + `spawn_blocking` at the IDE boundary. Switching is a
  SemVer break, so pick before publishing crates publicly.
- **`dyn` vs generics for cross-crate ports.** Lean `dyn` for stable ABI,
  simpler signatures, and ergonomics. Generics earn their keep only in hot
  loops, none of which currently exist in domain logic.
- **Granularity of `daft-types`.** One shared types crate, or each crate owns
  its types and adapters translate? Lean toward one shared crate; revisit if it
  becomes a dependency-magnet.
- **Async-in-trait shape.** Stable `async fn` in traits (Rust 1.75+) works for
  generics but not for `dyn`; `async-trait` macro adds a small runtime cost.
  Decision pairs with the sync-vs-async question.

## What to consciously reject

These patterns have been considered for daft and intentionally not adopted.
Future contributors: if you find yourself reaching for one, re-read this section
and check whether the reason still applies.

- **CQRS** (separate write model from read model). The writer/reader pool split
  in `src/store/pool.rs` already gives the practical benefit (fast reads,
  ordered writes). Conceptual CQRS would be ceremony.
- **Event sourcing.** Daft has clear current state; no audit/replay requirement.
  The footgun cost of event versioning and projection rebuilds isn't justified.
- **Microservices / serverless / event-driven architecture at the system
  level.** Daft is one binary on one host. System-level patterns don't apply.
- **Pure layered (N-tier).** Invites ORM/row-type leakage into business code.
  The redb era's `meta.json` workaround was a milder version of this failure
  mode; SQLite + ports prevented it from recurring.
- **Onion / Clean Architecture as labels.** Same family as hexagonal, more
  ceremony, no extra value. The vocabulary in tree is fine; don't rename.
- **Other embedded databases** (sled, fjall, redb, RocksDB, persy). See
  CLAUDE.md "Database & Storage" for the full reasoning.
- **`fork()` for background work.** `libc::fork()` is intrinsically `unsafe fn`;
  conflicts with Critical Rule #4. Spawn-self is the pattern.
- **ADR-per-decision** (one structured file per architectural decision, Nygard
  template). Useful at multi-architect scale; overhead for a pre-1.0
  single-maintainer project. This document is the right granularity until that
  changes — sections can split into `architecture/decisions/NNNN-*.md` later if
  needed.

## Decision log

Shaping decisions and where to find their full context:

- **redb → SQLite migration** — PR #508. Why: redb's process-exclusive file lock
  didn't match daft's coordinator-plus-CLI access pattern, which forced a
  `meta.json` dual-write as a workaround. SQLite WAL mode + reader/writer pool
  split solves it natively.
- **Hexagonal coordinator internals** — PR #508. Why: prove the layering by
  extracting one domain module (reconcile) before forcing all of `process.rs`
  through it. Future domain extractions (cancel, lifecycle) follow the same
  shape, FCIS-style.
- **`forbid(unsafe_code)` in production code paths** — Critical Rule #4. Why:
  every dependency choice and architectural pattern has to compose with this.
  Drives the SQLite-over-LMDB choice (`rusqlite` has a fully safe public API;
  `heed::Env::open` is `unsafe fn`) and the spawn-not-fork pattern.
- **No pre-1.0 back-compat for on-disk formats** — auto-wipe with stderr banner.
  Why: daft is effectively pre-release; back-compat would lock in immature
  decisions and double the surface area of every storage change.

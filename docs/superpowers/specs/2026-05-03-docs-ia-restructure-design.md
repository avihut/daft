# Docs IA Restructure: Pillars, Diátaxis, and the Cookbook

**Status:** Approved (brainstorm) **Branch:** `daft-398/docs/cookbook` **Date:**
2026-05-03

## Context

This branch began as "add a cookbook / recipes section to the daft docs." The
exploration of mise's docs IA revealed a deeper problem: the existing docs are a
catchall with no narrative spine. The branch scope is therefore widened from
"add a cookbook" to **a full docs information-architecture (IA) restructure that
includes the cookbook as one section of the new shape.**

The networking pillar and the full git-hooks drop-in (lefthook-style) are
features still in flight. They are out of scope for this branch and tracked as
follow-up issues. The IA reflects them only as roadmap surfaces, with honest
scope statements on the relevant pages.

## The problem with today's docs

A Diátaxis audit of the current corpus (`getting-started/` + `guide/` + `cli/` +
root) found:

- **Reference-heavy:** ~51 of ~60 pages are Reference (CLI ref + config + meta)
- **Explanation-starved:** 1 of 60 pages is Explanation (`worktree-workflow.md`)
- **Mixed-purpose pages:** `guide/layouts.md`, `guide/hooks.md`,
  `guide/multi-remote.md` each blend Explanation + Reference + How-to
- **No narrative:** the "Guide" sidebar group is a misnomer — it's a bag of
  feature pages with no ordering or unifying thesis
- **No cookbook:** recipes for adopting daft alongside common tooling (mise,
  direnv, nvm, pyenv) and scenarios (monorepo, fork workflow) don't exist as a
  discoverable surface

Newcomers can't find a "why daft" page. Power users hunting reference get good
results. The middle audience — converts who want to deepen their adoption — is
unserved.

## Daft's product shape

Daft has **three idempotent pillars** that a user may opt into independently:

1. **Worktrees** — code isolation via per-branch directories, layouts, adopting
   existing repos, multi-remote, exec across worktrees
2. **Hooks** — automation surface bound to code-evolution boundaries (see thesis
   below). Today scoped to worktree lifecycle; planned to expand to a full
   lefthook-style git-hooks drop-in plus merge hooks
3. **Networking** — coordinating changes across repos. Future feature

A user may adopt only worktrees, only hooks, or only networking (when it lands).
The IA must reflect this independence.

The pillars are loosely tied by a unifying thesis:

> **Parallelize development through isolation; coordinate across repos via
> networking.**

This thesis lives on the landing page and on a single "Why daft" Explanation
page, not as the IA backbone.

## The hooks-as-boundaries thesis

Hooks are the most conceptually rich pillar. Surfaced during parallel work on
the merge feature, the unifying frame is:

> **daft hooks define clear boundaries as your code evolves. They are a local,
> parallel approach to GitHub Actions — every stage of code's lifecycle gets a
> well-defined gate.**

Mapped to hook types:

| Stage                             | Hook type                                      | Boundary semantics                                                                             |
| --------------------------------- | ---------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Start of isolated dev             | Worktree hooks (`worktree-pre/post-create`)    | Set up local dev env (deps, services)                                                          |
| Sealing a unit of change          | Commit hooks (lefthook drop-in, future)        | Progressive code-replication boundary — format, lint, fast tests before the change is recorded |
| Letting a change escape isolation | Merge hooks (future)                           | PR-check parity — full tests, integration, security gates before code leaves the branch        |
| Reclaiming an isolated env        | Worktree teardown (`worktree-pre/post-remove`) | Teardown, persist artifacts, sync state                                                        |
| End of clone setup                | `post-clone`                                   | One-shot bootstrap of a fresh repo                                                             |

This frame distinguishes daft hooks from lefthook in two ways:

- **Lefthook is commit-time-only.** daft covers the full code-evolution
  lifecycle, with commit hooks as one stage among many.
- **Boundaries before changes leave the dev's machine** rather than after they
  reach the central repo. CI shifts left.

The Hooks pillar Overview opens with this frame.

## Decision: option C′ — pillar IA + Diátaxis within each pillar

**Pillar-driven sidebar with per-pillar Overview (Explanation) pages, peer
Diátaxis sections inside each pillar, and unified cross-pillar Cookbook +
Reference + About sections.**

Rationale:

- Three idempotent pillars demand pillar-driven IA — purpose-only Diátaxis would
  create false unity across surfaces a single user may never touch.
- Per-pillar Overview pages give each pillar an Explanation entry-point, fixing
  the corpus-wide Explanation gap without forcing a global "Concepts" section.
- Cookbook is elevated to a top-level section (not buried under About as in
  mise) — recipes are the adoption gateway, not a curiosity.
- Reference is unified, not pillar-fragmented — daft's reference is small enough
  that one place is more discoverable than three.
- Diátaxis quadrants are honored _inside_ each pillar: Overview = Explanation,
  How-tos and Reference live as peer pages within the pillar.

## Sidebar structure

```
Top nav:
  Worktrees | Hooks | Cookbook | v{X.Y.Z} | GitHub

Sidebar:
─ Getting Started
    Installation               (How-to)
    Quick Start                (Tutorial — narrates the worktree adoption arc)
    Shell Integration          (How-to)

─ Worktrees                                                    ★ pillar
    Overview                   (Explanation: code isolation, the gradient)
    Layouts                    (Explanation — what & why)
    Adopting existing repos    (How-to)
    Multi-remote               (Explanation + How-to)
    Running commands across worktrees   (How-to)
    Shortcuts                  (How-to)

─ Hooks                                                        ★ pillar
    Overview                   (Explanation: boundaries thesis)
    Lifecycle hooks            (Reference: types, triggers, env)
    Job orchestration          (Explanation: parallelism, deps, conditions)
    Hooks YAML reference       (Reference: full daft.yml schema)
    Trust & security           (Explanation)
    Roadmap                    (Explanation: full git hooks drop-in, merge hooks)

─ Cookbook                                                     (How-to corpus)
    By tooling                 (mise, direnv, nvm, pyenv, asdf)
    By language                (Node, Python, Rust, Go, …)
    By scenario                (monorepo, big repos, fork workflow, CI integration)
    Each recipe tagged with which pillar(s) it touches.

─ Reference                                                    (Reference corpus)
    CLI                        (collapsed, autogen — daft + git-worktree-* + shortcuts)
    Configuration              (`git config daft.*`)
    Output Formats
    Agent Skill                (`daft-worktree-workflow`)

─ About
    Why daft                   (Explanation — the parallel-dev thesis tying pillars)
    Glossary                   (Reference)
    FAQ                        (How-to)
    Troubleshooting            (How-to)
    Comparison                 (Explanation — vs git worktree, vs lefthook, vs gitup)
    Contributing               (How-to / meta)
    Changelog                  (Reference / meta)
    Roadmap: Networking        (Explanation — status: in design)
```

## Migration plan

Per the Diátaxis audit table:

| Existing path                                | Action                                             | New location                                                                                                             |
| -------------------------------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `getting-started/installation.md`            | Keep                                               | `getting-started/installation.md`                                                                                        |
| `getting-started/quick-start.md`             | Keep, expand to walk the gradient inside Worktrees | `getting-started/quick-start.md`                                                                                         |
| `getting-started/shell-integration.md`       | Keep                                               | `getting-started/shell-integration.md`                                                                                   |
| `guide/worktree-workflow.md`                 | Move + rename — becomes Worktrees Overview         | `worktrees/index.md`                                                                                                     |
| `guide/layouts.md`                           | Split                                              | `worktrees/layouts.md` (Explanation core) + table of layouts inlined; deep reference defers to CLI ref for `daft layout` |
| `guide/adopting-existing-repos.md`           | Move                                               | `worktrees/adopting-existing-repos.md`                                                                                   |
| `guide/multi-remote.md`                      | Split                                              | `worktrees/multi-remote.md` (Explanation + How-to)                                                                       |
| `guide/running-commands-across-worktrees.md` | Move                                               | `worktrees/running-commands.md`                                                                                          |
| `guide/shortcuts.md`                         | Move                                               | `worktrees/shortcuts.md`                                                                                                 |
| `guide/hooks.md`                             | Split                                              | `hooks/index.md` (Overview) + `hooks/lifecycle.md` (Reference) + `hooks/yaml-reference.md` (Reference)                   |
| `guide/configuration.md`                     | Move                                               | `reference/configuration.md`                                                                                             |
| `guide/output-formats.md`                    | Move                                               | `reference/output-formats.md`                                                                                            |
| `guide/claude-skill.md`                      | Move                                               | `reference/agent-skill.md`                                                                                               |
| `cli/*.md`                                   | Keep — CLI Reference autogen                       | `reference/cli/*.md` (path move only)                                                                                    |
| `contributing.md`                            | Move                                               | `about/contributing.md`                                                                                                  |
| `changelog.md`                               | Move                                               | `about/changelog.md`                                                                                                     |
| `index.md`                                   | Update hero copy to reflect pillar IA + thesis     | `index.md`                                                                                                               |

**New pages to write (this branch):**

- `worktrees/index.md` — pillar Overview (replaces `guide/worktree-workflow.md`)
- `hooks/index.md` — pillar Overview with the boundaries thesis
- `hooks/job-orchestration.md` — Explanation extracted from current
  `guide/hooks.md`
- `hooks/trust-and-security.md` — Explanation extracted from current
  `guide/hooks.md`
- `hooks/roadmap.md` — Explanation stub for full git hooks drop-in + merge hooks
- `cookbook/index.md` — Cookbook landing with by-tooling / by-language /
  by-scenario taxonomy
- `cookbook/by-tooling/{mise,direnv,nvm,pyenv,asdf}.md` — initial set
- `cookbook/by-language/{node,python,rust,go}.md` — initial set
- `cookbook/by-scenario/{monorepo,fork-workflow,ci-integration}.md` — initial
  set
- `about/index.md` or `about/why-daft.md` — the unifying thesis
- `about/glossary.md` — terms
- `about/faq.md` — extracted from common questions
- `about/troubleshooting.md` — extracted from common issues
- `about/comparison.md` — vs git worktree, vs lefthook, vs gitup
- `about/networking-roadmap.md` — placeholder for the future pillar

**Pages NOT written this branch (deferred to follow-up issues):**

- Full Networking pillar content (overview, concepts, recipes)
- Full git hooks drop-in pillar pages (lefthook-replacement story, commit-stage
  hook reference)
- Merge hooks pillar pages (PR-check-parity story, merge-stage hook reference)

These deferred pages get tombstone links from the relevant Roadmap pages.

## Sidebar config changes

`docs/.vitepress/config.ts` updates:

- `nav` array: replace 4-item nav with
  `Worktrees | Hooks | Cookbook | v{ver} | GitHub`
- `sidebar` array: replace the 5 existing groups with the 6 groups above
- `srcExclude` unchanged
- All redirects from old URLs to new — see "URL redirects" section below

## URL redirects

Bookmarks, search engine results, and external links pointing at the current
URLs (`/guide/hooks`, `/guide/layouts`, etc.) must keep working. Implementation:

- Use VitePress `transformPageData` to emit `<meta http-equiv="refresh">` on
  legacy pages, OR emit a small set of stub pages at the old paths that redirect
  via JS, OR (preferred) configure Cloudflare Pages `_redirects` since the site
  is hosted there.

The exact mechanism is decided in the implementation plan, not this design.

## Diátaxis audit (reference)

Full classification table (input to the migration plan above):

| Doc                                          | Quadrant                    | Pillar         | Action                             |
| -------------------------------------------- | --------------------------- | -------------- | ---------------------------------- |
| `index.md`                                   | Landing                     | —              | Keep, update copy                  |
| `getting-started/installation.md`            | How-to                      | —              | Keep                               |
| `getting-started/quick-start.md`             | Tutorial                    | Worktrees      | Keep, expand                       |
| `getting-started/shell-integration.md`       | How-to                      | —              | Keep                               |
| `guide/worktree-workflow.md`                 | Explanation                 | Worktrees      | Move → `worktrees/index.md`        |
| `guide/layouts.md`                           | Mixed (Expl + Ref)          | Worktrees      | Split                              |
| `guide/adopting-existing-repos.md`           | How-to                      | Worktrees      | Move                               |
| `guide/hooks.md`                             | Mixed (Expl + Ref + How-to) | Hooks          | Split into 4 pages                 |
| `guide/running-commands-across-worktrees.md` | How-to                      | Worktrees      | Move                               |
| `guide/shortcuts.md`                         | Reference                   | Worktrees      | Move                               |
| `guide/multi-remote.md`                      | Mixed (Expl + Ref + How-to) | Worktrees      | Split                              |
| `guide/configuration.md`                     | Reference                   | —              | Move to `reference/`               |
| `guide/output-formats.md`                    | Reference                   | —              | Move to `reference/`               |
| `guide/claude-skill.md`                      | How-to                      | Hooks-adjacent | Move to `reference/agent-skill.md` |
| `contributing.md`                            | Meta                        | —              | Move to `about/`                   |
| `changelog.md`                               | Meta-Reference              | —              | Move to `about/`                   |

## Success criteria

1. **Quadrant balance:** Explanation grows from 1 page to ≥6 pages (worktrees
   overview, hooks overview, layouts explanation, multi-remote explanation,
   why-daft, comparison, hooks-job-orchestration, trust-and-security)
2. **Cookbook discoverability:** top-nav and sidebar both surface Cookbook;
   recipes are tagged by pillar
3. **Pillar independence:** a user can land on `/hooks/` and find a complete
   pillar Overview + Reference + Roadmap without needing to read Worktrees pages
   first
4. **No dead links:** legacy URLs redirect to their new homes
5. **Honest scoping:** Hooks pillar pages clearly label which hook types are
   shipping vs roadmap; Networking is honestly marked "in design"

## Risks

- **Aspirational IA over-promise.** Hooks pillar exists in the IA before the
  full git-hooks drop-in lands. Mitigation: every page that references future
  scope has an explicit "Status: in design" or "Roadmap" callout. Mise does this
  with experimental backends.
- **Bookmark breakage.** Mitigated by URL redirects (see above).
- **Migration churn during the work.** This is a big diff. Land it in a single
  branch (this one), then resume incremental docs work on master with the new
  shape in place.
- **Cookbook shape may need iteration.** The by-tooling / by-language /
  by-scenario taxonomy is a guess. Initial recipes may surface a better cut.
  This is fine — the structure is a starting taxonomy, not a contract.

## Out of scope (follow-up issues)

The following work is captured in dedicated GitHub issues created from this
branch, with cross-links from the relevant Roadmap pages:

- **Issue:** Full git hooks drop-in (lefthook-style) — pillar pages, commit hook
  reference, migration guide from lefthook, comparison page
- **Issue:** Merge hooks — pillar pages, merge-stage hook reference, PR-check
  parity story
- **Issue:** Networking pillar content — overview, concepts, cross-repo
  coordination story, recipes
- **Issue:** Visual rebrand (deferred until the IA lands and is stable)

## Implementation sequencing

This design intentionally does not include a step-by-step plan. The
implementation plan is generated by `superpowers:writing-plans` next, taking
this design as input. The plan will sequence:

1. New pillar Overview pages first (Worktrees + Hooks + Why-daft) — enables the
   IA shape to render even before content migrations finish
2. Sidebar config update second — the new shape becomes navigable
3. Migrations of existing pages (move, rename, split) — drains the legacy
   `guide/` group
4. Cookbook content — initial recipe set per the by-tooling / by-language /
   by-scenario taxonomy
5. URL redirects — final link-stability pass
6. Roadmap stub pages and follow-up issue cross-links — close the loop on
   deferred features

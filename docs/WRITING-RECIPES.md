# Writing recipes

Internal style guide for the recipe pages in `docs/recipes/`. Excluded from the
docs site build via `srcExclude`. Read this before writing or revising any
recipe.

## What recipes are for

Recipes turn daft's idea — lifecycle automation per worktree — into
copy-paste-able setups for real projects. The reader's mental model is: "I have
a project that looks like X; I'm hitting pain Y; show me the `daft.yml` that
fixes it." Every recipe is judged against that test.

## Page shapes

### Pattern (atomic problem, one solution)

```
## Starting state
## What changes
## Recipe
## Variants
## Idempotency & safety
## Where to next
```

### Walkthrough (project shape, several patterns threaded)

```
## Starting state
## Patterns we'll thread
## Step N: <pattern N applied>
...
## Final daft.yml
## What you got
## Where to next
```

### Reference (per-tool tables, anti-patterns)

`sharing-caches.md` and the `anti-patterns/*.md` pages have their own shapes.
The pattern shape doesn't apply — don't try to force it.

## The starting-state vignette

Open every pattern and walkthrough with a concrete vignette. Three required
properties:

1. **A specific filesystem.** File names, structure. Not "a Python project" —
   "you have `pyproject.toml` + `uv.lock` + a `bin/setup.sh`."
2. **A specific ritual.** What devs currently do. The README's literal setup
   steps. The git-pull-then-X muscle memory.
3. **A specific pain that's already happened.** Real failure modes, real
   symptoms. "Bugs that 'only repro on Alex's machine' turn out to be Node 20 vs
   18."

The vignette is the bridge from "what's in front of the reader right now" to
"the recipe." Without it, the reader has to reverse-engineer motivation from the
variants list. With it, "yes this is mine" or "no, mine is different — read
pattern X" is the first thing they can decide.

### What does NOT belong in the vignette

**The test:** if changing the value doesn't change the recipe, drop the detail.

| Don't include                  | Why                                              |
| ------------------------------ | ------------------------------------------------ |
| Repo age ("two months old")    | Recipe doesn't change at any age                 |
| Team headcount ("3 engineers") | Recipe doesn't change at 3, 7, or 70             |
| Specific timing ("last week")  | Recurrence matters; the cadence usually doesn't  |
| Dep counts ("~200 deps")       | Causes slowness, but recipe just needs "is slow" |
| Roles ("a contractor")         | Flavor text; the failure shape is what matters   |

What stays: the failure modes, the ritual shape, and the **recurrence without
the cadence** ("sooner or later," "each rotation"). Those are load-bearing.

### The "reach for daft" sentence

End the vignette with one sentence that bridges to "What changes." Something
like:

> The reach for daft: stop relying on muscle memory. Tool versions should
> _activate_ on cd, not when you remember to run a command.

It names what the user is reaching for and frames the recipe as the answer. One
sentence; resist the urge to summarize the whole recipe here.

## The Recipe section is self-contained

The reader should be able to copy the Recipe section's `daft.yml` plus its
supporting files and have a working setup. **No forward references.** Every env
var, every command, every supporting file shown in the section is defined within
it. A reader who stops reading after Recipe should still get a working hook.

If the minimal recipe needs port allocation, fold port allocation into the
Recipe block. Don't put it in a "Allocating ports" subsection between Recipe and
Variants.

This is the rule the original `services-with-ports` violated. Don't repeat that.

## Variants are single-axis

State the axis up front: _"By language."_ / _"By runtime."_ / _"By source."_ /
_"By resource type."_ Within a single recipe, all variants must be siblings on
that one axis.

**Wrong:** the original `env-vars-and-secrets` mixed source (vault, sops), scope
(per-job env), and derivation (branch-port hash). They aren't siblings.

**Right:** `env-vars-and-secrets` now has variants strictly by _source_. Derived
values (the branch-port hash) live in their own subsection at the same level as
Variants.

Things that _aren't_ variants but feel like they want to be:

- Comparisons ("X is the precursor to Y") — drop, or rewrite as actual usage
  guidance.
- Structural notes ("this hook can run in parallel by default") — move to
  Idempotency & safety.
- Cross-cutting concerns (sccache, layer caching) — give them their own named
  subsection.

## Walkthroughs cite patterns; they don't re-derive them

A walkthrough is a vignette-with-detail showing several patterns threaded into
one project. Each step says: "Apply [pattern X]. The project-specific twist here
is …"

Walkthroughs **do not** re-explain `--frozen-lockfile`, `cargo fetch --locked`,
`COMPOSE_PROJECT_NAME`, or any other pattern internals. The pattern owns the
why. The walkthrough owns the application.

If you find yourself re-explaining a pattern's mechanics in a walkthrough, the
explanation belongs in the pattern.

## Cross-link density

Replace the old `Composes well with` + `See also` + `Anti-patterns` trio with a
single **Where to next** of ≤3 prioritized links. Order:

1. The most common next pattern this composes with
2. The matching reference / anti-pattern page (if any)
3. The relevant Hooks pillar reference

Average outbound link count per page: aim for ≤6 total (Where-to-next plus
inline references in prose). 10+ becomes furniture; the reader stops choosing.

## Anti-pattern handling

- **Inline `:::warning` blocks** for in-context gotchas — they sit next to the
  code that motivates them. Example: "Don't run codegen in a warmup."
- **Single inline link** to the anti-pattern reference page for major ones
  (shared mutable state, secrets in version-controlled hooks).
- **No bottom Anti-patterns bullet list.** It duplicates the inline warnings and
  pads cross-link density.

### VitePress directive format — get this right

Two interacting rules:

1. The closing `:::` **must be on its own line.** Otherwise the directive
   doesn't terminate, and the next `## Heading` gets swallowed into the
   warning/tip box.
2. Prettier (`proseWrap: always`) joins consecutive non-blank lines into one
   paragraph — so without blank-line separators, prettier will rejoin the title
   with the first content line and pull the closing `:::` back onto the last
   content line. The fix is **blank lines** around the content block. Prettier
   respects them; the renderer respects them.

The format that survives both VitePress and prettier:

```
::: warning Title on its own line

Content paragraph. Multiple paragraphs are fine; just keep them inside
the block.

:::
```

Wrong (will be silently broken by prettier):

```
::: warning Title bleeds into the first line of content
Content paragraph. ←── prettier joins this with the closing
:::
```

If you ever see "step N is missing from the TOC" or "the next section is
rendered inside a warning box," check the previous `:::` block first — prettier
almost certainly rejoined a closing onto a content line.

## Style

- Second person, active voice. "You have," "you're typing," "your README."
- One concrete example beats two abstractions.
- YAML examples must be valid against the current daft schema. When in doubt,
  check `docs/hooks/yaml-reference.md`.
- No emojis. No exclamation points.
- Tables are good for "command vs idempotent" or "tool vs share-or-not"
  comparisons. Don't tablify everything.
- Past tense works for the vignette's pain stories without specific timing: "A
  working `.env` once got DM'd to a contractor." The "once" carries recurrence
  without claiming a date.

## Verification checklist (per page)

Before committing a recipe rewrite or new page, walk this list:

- [ ] Vignette satisfies the three properties (filesystem / ritual / pain)
- [ ] Vignette has no team-size, repo-age, dep-count, or specific-timing details
- [ ] Recipe section is self-contained (no forward references to subsections
      that complete it)
- [ ] Variants share a single named axis
- [ ] Anti-patterns are inline `:::warning` blocks, not a bottom list
- [ ] **Where to next** is ≤3 prioritized links
- [ ] Total outbound link count ≤6 across the whole page
- [ ] `cd docs && bunx vitepress build` passes (strict link checking,
      `ignoreDeadLinks: false`)
- [ ] `mise run docs:site:check` passes (biome on the config + theme)

## Reference recipes

When in doubt, mirror the structure of these:

- Pattern (simplest): `docs/recipes/toolchain-bootstrap.md`
- Pattern with composed teardown: `docs/recipes/services-with-ports.md`
- Pattern with declarative-vs-imperative split:
  `docs/recipes/declarative-envs.md`
- Walkthrough threading 2 patterns: `docs/recipes/walkthroughs/rust-binary.md`
- Walkthrough threading 4 patterns:
  `docs/recipes/walkthroughs/node-monorepo-services.md`

## Process

- One commit per page.
- Conventional Commits: `docs(recipes): <imperative>` —
  `rewrite <slug> with <reason>` is a fine title shape.
- Each commit ends with a passing build + biome (the pre-commit hook runs
  prettier; the pre-push hook runs the build).
- Bundle related changes (e.g., a narrative refresh of all 7 patterns) into one
  branch, but keep per-page commits inside the branch.

## Where this guide lives

This file is at `docs/WRITING-RECIPES.md` and excluded from the VitePress build
via `srcExclude` in `docs/.vitepress/config.ts`. Adding new excluded meta-files
follows the same pattern.

# The landing page

`docs/index.md` (frontmatter only) plus this directory is daft's front door. It
has one job: a visitor who knows git should understand in under thirty seconds
what daft does, see it do it, and be able to install it. It is not a docs index
(the nav is), not a feature list (the pillar overviews are), not a comparison
(`docs/about/comparison.md` is).

The shape was settled in #467/#386 and is deliberate; change the contents, not
the shape. Everything below the hero renders through the `home-hero-after` slot
(`theme/index.ts`); the markdown body is empty.

## Shape, top to bottom

| Section     | Source                                                      | What it is                                                                                                                                          |
| ----------- | ----------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Hero        | `docs/index.md` frontmatter, `InstallCta.vue`, `install.ts` | Eyebrow, headline, a tagline that states the mechanism, and the install line as the call to action (platform tabs, copy, a meta line onward).       |
| The session | `DemoStage.vue`, `graph/hero-script.ts`                     | `HERO_SCRIPT` replayed live through the two standard viewers on one player — daft doing the whole workflow. Steppable: click any command.           |
| Five points | `points.ts`, `HomeLanding.vue`, `PointScene.vue`            | In workflow order — start, switch, span, agents, ship. Each: a claim, two or three sentences of mechanism, one link into the docs, a demonstration. |
| The close   | `HomeLanding.vue` (`dl-band`)                               | The plain-git statement, the install line again, and the two buttons.                                                                               |

There is no stats strip. Add one between the hero meta line and the session the
day there are real numbers to show (stars, installs); never placeholders, never
numbers that are not fetched.

Styles: `home.css` here is page layout only. The viewers and the paired stage
(`.dl-term`, `.dg-*`, `.dl-stage`) live in `graph/viewers.css` — they are shared
with the playground and with docs embeds, so never restyle them from here; size
a stage with `--dl-stage-h`.

## The discipline

- **The install line is the call to action.** One command per platform, a copy
  button, no second decision above the fold. `install.ts` must match the primary
  commands in `docs/getting-started/installation.md`; change both in the same
  edit — that page is the full list, this is the fold.
- **Copy is mechanism, not slogan.** Every sentence names something daft does in
  git terms — a verb, a directory, a hook, a port, a branch. If a sentence could
  sit on any devtool's landing page, cut it. Test a paragraph by asking what the
  reader could now type.
- **Every claim is true of shipped daft today.** No "soon", no roadmap, no flags
  that are not released. A verb's behavior changes in the CLI first, the landing
  second, never the reverse.
- **Exactly five points, in workflow order.** The numbering is information (the
  order you would do these things on day one), not decoration. To say something
  new, retire or rewrite a point — do not append a sixth. A new verb earns a
  point only if it changes what a visitor would do on day one; otherwise it
  belongs in a point's second sentence, or in the docs.
- **Demonstrations come from the verb registry.** A scene is `VerbInvocation[]`
  through `buildScenario` (`graph/verbs.ts`), never hand-authored `StepDef`s, so
  a point cannot show something the verbs do not do. Scenes start from `clone`
  so the world builds from nothing, stay under ~35 seconds and seven
  invocations, and loop while on screen. One static demonstration (the
  before/after transcript of point 02) is allowed because its proof is the loop
  you stop typing, not a graph change — do not add more.
- **Every scene is a golden.** `tests/golden/compiled.test.ts` records each
  point's compiled digest in `__goldens__/landing-points.json`; a registry
  change that rewrites a landing scene shows up there. Regenerate with
  `UPDATE_GOLDENS=1 mise run docs:test:golden` and review the diff like code.
- **One link per point**, into the pillar or reference page that proves it.
- **The hero stays player + viewer only** (rule inherited from
  `graph/CLAUDE.md`): nothing it does is hero code. Its dev handle is
  `__daftPlayer`; each point scene gets `__daftPoint_<id>` so a harness can
  drive each and the hero's pixel anchor keeps its player.
- **Fills a big screen, works on a phone.** `.dl-wrap` runs to 1400px and the
  stages clamp their height by viewport width; everything must still read at
  390px, with stages stacking terminal over graph.
- **Palette and tokens come from `custom.css`**; this page introduces no colors
  of its own. No emoji.

## Updating it as daft improves

- **A verb lands or changes behavior** — the registry first (`graph/verbs.ts`
  - `catalog.ts`, see `graph/CLAUDE.md` "Extending"). Then ask whether any
    point's story changed: edit `points.ts` (copy and/or invocations), run
    `UPDATE_GOLDENS=1 mise run docs:test:golden`, review the diff, and update
    `tests/ui/pack.landing.spec.ts` if a title or the numbering moved — the spec
    pins both on purpose.
- **An install method changes** — `install.ts` and
  `docs/getting-started/installation.md`, same edit.
- **The headline or tagline changes** — `docs/index.md` (`hero.text`,
  `hero.tagline`, and `description`, which feeds the social card).
- **The hero story changes** — `graph/hero-script.ts` (until #860 converges it
  onto the registry), then recapture the local pixel anchor
  (`DAFT_PIXEL_BASELINE=capture`) and say so in the PR.
- **There are real numbers** — add the stats strip (see Shape); fetch, do not
  type.

## Verification

`mise run docs:site:check` (baseline: exactly two pre-existing warnings in
`config.ts`), `mise run docs:test:golden`, `mise run docs:site:build`, and
`DOCS_PORT=5191 mise run docs:test:ui` — `pack.landing.spec.ts` pins the install
line, the five points and their order, the per-scene players, and the
reduced-motion landing; `pack.hero.spec.ts` owns the hero canvas. Look at the
page in both themes at 1440, 1920, and 390 before calling it done.

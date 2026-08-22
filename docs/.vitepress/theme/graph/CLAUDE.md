# The daft diagram language

This directory owns how daft draws its world: repos, worktrees, and the
connections between them. The landing hero is the flagship rendering, but the
same engine and grammar are meant for diagrams across the docs — static or
animated. If a page needs to show repo/worktree structure, use this language
(via `RepoDiagram.vue`), not an ad-hoc SVG or a screenshot.

The generic machinery — the language contract, engine, render core, transcript,
the editor and its document model, and the media exports — is **dumbshow**
(github.com/avihut/dumbshow), published as `@dumbshow/core` (everything
framework-free: contract, engine, render core, transcript, document model,
exports, the editor stylesheet) and `@dumbshow/vue` (the editor); this directory
is daft's **language pack**. Files: `pack.ts` (`DAFT_PACK` — daft assembled
against the package's `DiagramLanguage` contract, plus the daft-typed aliases
consumers import), `render.ts` (the daft act vocabulary, scene state, palette,
canvas draw, and hit priority; it also re-exports the package's camera/view math
so every daft-side consumer has one import spot), `seed.ts` (the pack side of a
document's opening state — seed schema semantics, seed mutations, entity
renames), `RepoDiagram.vue` and `RepoTerminal.vue` (the two viewers), `verbs.ts`
(the verb layer: daft commands → canonical beats over a world model),
`catalog.ts` (a demo scenario per verb), `hero-script.ts` (the landing story),
`gallery.ts` (scripts the playground replays), `Playground.vue` (the
`/playground` page), `composer/` (the pack side of the `/composer` editor —
element semantics, inspector views and panes, the docs embed template; see "The
composer" below). Viewer styles live in `viewers.css` under the `.dg-`/`.dl-`
prefixes (the landing's own styles in `../landing/home.css`, playground styles
in the page component; editor styles ship with the package and are imported by
`docs/composer.md` as `@dumbshow/core/style.css`). Change this file and the
renderer together — the grammar here is normative, not descriptive.

## The language-pack seam

The machinery lives in `@dumbshow/core` (the editor in `@dumbshow/vue`); meaning
flows into it only as the injected `DAFT_PACK` plus pack components passed as
props — the composer page (`docs/composer.md`) is where daft meets the editor,
handing in the pack and the pack's `InspectorPane`. dumbshow never names a daft
concept; its CLAUDE.md (in the dumbshow repo) owns the editor's hard rules —
document model, derive-only, pure mutations, rebuild-never-remount,
never-autoplay, exports-never-lie — and the theming token contract. When daft
needs a new hook in the pack contract, the change lands in dumbshow first (with
its boxes reference pack implementing the hook — that's the honesty check), then
the pack here adopts it.

Dependency law on this side: pack files import the package roots —
`@dumbshow/core` for the machinery and the editor's published document API
(`ComposerDoc`, the mutations, the selection types), `@dumbshow/vue` only where
a Vue component is mounted (the composer page, the inspector's `AttributesForm`)
— the way plugins consume a host SDK. Deep imports into either package are
forbidden; everything arrives through `@dumbshow/core`,
`@dumbshow/core/style.css`, or `@dumbshow/vue`. The one sanctioned exception is
the UI-test harness's `DUMBSHOW_FS` constant (tests/ui/helpers.ts), a dev-server
`/@fs` URL at the built dist for in-page imports. Greppable check — run from
`docs/`, it must print only that helpers.ts definition:

```sh
rg -P '@dumbshow/(core|vue)/(?!style\.css)' --glob '!**/node_modules/**' .
```

Host wiring the machinery no longer hardcodes, and where it lives now: the
dev-only `__daftPlayer` window handle is passed at every `createPlayer` site
(`devHandle: import.meta.env.DEV ? "__daftPlayer" : undefined` — the package
cannot dev-gate itself; `import.meta.env.DEV` inlines to false in library
builds); the composer page passes the docs back link, the vitepress dark-mode
ref as `v-model:is-dark` (bound to the ref's `.value`, so the toolbar's toggle
writes the theme through `update:isDark`), and `file-tag="daft"` (draft key
`daft-composer-draft-v1`, `.daft.json` downloads); the shell input's aria copy
is `shellInputLabel` in `pack.ts`. The document format v1 keeps its daft-shaped
seed schema on purpose; a generic seed schema is a document-version bump owned
by dumbshow.

Working against dumbshow source:
`DUMBSHOW_SRC=<path to a dumbshow checkout> mise run docs:site` serves
`@dumbshow/core` and `@dumbshow/vue` from that checkout's `packages/*/src`
instead of the installed dists (an env-gated alias in `.vitepress/config.ts`;
the UI helpers' `DUMBSHOW_FS` follows it), so machinery edits hot-reload on
`/composer`, and `DUMBSHOW_SRC=… mise run docs:test:ui` runs the whole suite
against them — that suite is dumbshow's regression net until the `editor.*`
specs copy over. `DOCS_PORT=<port>` runs a second dev server or suite beside a
sibling worktree's (Playwright reuses whatever already answers at its URL, so a
link is never tested against another worktree's server). The link is a dev and
test affordance only: builds, the goldens (`bun test` resolves the installed
package), and CI always use the pinned version — a dumbshow change lands by
publishing, then `bun add --exact` here. After a bump, `docs:site:setup` (which
every docs task depends on) drops Vite's dependency pre-bundle when `bun.lock`
changed — VitePress's Vite 5.4 keys that cache on `bun.lockb` only, so without
it the dev server and the UI suite would keep running the previous package while
the build used the new one.

## Vocabulary

- **Repo** — ink disc, its name in halo-colored mono inside. The disc grows to
  fit the label; never truncate a repo name.
- **Worktree** — small satellite node on a curved hairline edge from its repo.
  Its branch name sits outside it in faint mono, full name, clamped into the
  canvas.
- **Default branch (`main`)** — the one gold-filled node per repo, ink-ringed.
  Gold marks "the trunk you return to", nothing else.
- **Merge** — a gold payload dot slides from the worktree along its edge into
  the repo disc, which swallows it, grows for a beat, and pulses gold. The
  worktree dims hollow behind it: halo fill, faint ring, dimmed label and badge.
  Hollow is a state you can see _before_ cleanup — it is why `prune` exists.
- **Teardown** (`remove`, `prune`) — the mirror of setup: the same dashed ring
  in rust, spinning the opposite way, while the node dissolves in place. Safe on
  hollow shells — their work already traveled home at merge time. Rust is
  destruction; it never decorates.
- **Sync signal** — on `daft sync` (and `daft update`), a small teal dot travels
  from the repo along each edge out to every worktree, ending in a teal blink:
  base updated, rebase available.
- **Carry payload** — on `daft carry`, a gold payload dot flies a bowed curve
  from worktree to worktree: uncommitted changes moving to the branch they
  belong in. On a move the source node dims while the payload is in the air;
  with `--copy` it doesn't.
- **Rename** — the old label rises out as the new one settles in (3px
  cross-fade) under a gold attention ring. Renaming a repo also morphs its disc
  between the widths the two labels measure. The act rewrites every scene
  reference (relations, arcs, carries, syncs) so nothing detaches.
- **Relation (repo ↔ repo)** — dashed teal line, slowly crawling. Teal is
  connective tissue. On `repo unlink` (`unrelate`) the line retracts toward its
  first endpoint, the crawl reverses, and it dies in rust.
- **Repo removal** (`repo-remove`) — the repository leaves whole: its live
  worktrees tear down staggered (each rust ring runs free), relations and
  feature arcs touching it fade on the same curve, and the disc shrinks to
  nothing under one collapsing rust ring. Arcs also fade — never pop — when a
  single worktree endpoint is removed.
- **Feature arc (worktree ↔ worktree)** — thin gold curve linking the same
  branch across repos: one feature, several dedicated worktrees.
- **Port badge** — small mono chip (`:3001`) hanging off a worktree: the visible
  proof of non-conflicting per-worktree resources.
- **Setup ring** — a dashed teal circle spinning around a fresh worktree while
  its post-create hooks run, fading when it's ready. _Every_ worktree gets one
  (default ~1.1s from birth); script `boot` acts extend the window for emphasis.
  The setup commands themselves print only in the paired terminal — the diagram
  never renders command text.
- **Agent bot** — a small purple-gradient robot head docked on a worktree: an AI
  coding agent is working there. The node blends toward purple and moves in
  quantized mechanical steps (against the smooth ambient float) — it is being
  transformed — with a slow purple ring pulse. Purple belongs to agents alone;
  it must not be confused with teal (setup/connection). When the agent leaves
  (`agent-leave`), the bot fades and drifts off, the purple blend decays, and
  one purple ring collapses inward — the node is yours again. Agents arrive and
  depart through _events_ (`agent joins`/`agent leaves` in the op registry),
  never as a side effect of a daft verb.
- **Creation pulse** — one expanding gold ring when anything is born, and when a
  merge lands. Gold pulses mean "look here", so keep them rare.

## Color law

Five colors carry meaning; everything else is neutral ink/paper drawn from the
live theme tokens (`--vp-c-text-*`, `--vp-c-bg-soft`, `--daft-gold`). The editor
chrome on `/composer` reads only dumbshow's `--dx-*` tokens (with neutral
defaults); `theme/custom.css` maps those onto the same daft palette in one
`:root` block, so the chrome and the canvas agree in both themes:

- **Gold** — wayfinding and attention: `main`, creation/merge pulses, feature
  arcs, the `daft` verb in the shell.
- **Teal** (`#1b9aaa`, both themes) — connection and setup: relations, setup
  rings.
- **Purple** (`#8a63d2`, gradient `#9d74e8 → #6a48c4`) — AI coding agents,
  nothing else.
- **Rust** (`#c75c1e`) — destructive moments only (remove/prune rings).
- **Ink/paper neutrals** — structure. The canvas reads theme tokens at draw
  time, so diagrams follow light/dark switches without re-rendering from Vue.

Never introduce a sixth signifying color; if a new concept needs marking, change
_form_ (shape, hollowness, dashing), not hue.

## Typography & sizing

Labels, badges, and ticks are **screen-space**: constant pixel size at every
zoom (mono 10px labels, 10.5px repo names, 9–9.5px badges/ticks). Node positions
are **world-space** and go through the camera. This is deliberate — zooming
spreads geometry, never shrinks text below legibility. Labels get a halo stroke
in the panel background color and are clamped inside the canvas.

## Motion

- Spawn: cubic-out growth with a slight overshoot; satellites extend along their
  edge.
- Idle: nodes breathe (±2–3px sine float). Pause freezes breathing — a paused
  diagram is fully still.
- Camera: rect-to-rect ease over ~1.6s. Start tight on one repo; widen only when
  the story adds repos.
- Determinism: no randomness — angles, distances, and phases hash from labels,
  so every replay and every seek renders the identical scene.
- Reduced motion: no autoplay, no floats, no dash crawl; the diagram becomes a
  stepper of settled frames (each step shown at its end state).

## Scripts, players, and viewers

A diagram is a `StepDef[]` script: beats of terminal lines (`cmd`/`out`), scene
`act`s, `cam` moves, and `pause`s. `compile` lays them on one absolute timeline.
Playback then splits into one **headless player** and any number of **viewers**:

- `createPlayer({ script, autoplay, loop })` owns the clock — play state, seeks,
  rate, looping. It renders nothing.
- Viewers subscribe (`onFrame`/`onStep`/`onPlayState`) and derive everything
  they show from the compiled script plus the clock. `RepoDiagram.vue` is the
  canvas viewer, `RepoTerminal.vue` the shell viewer — a viewer must never run
  its own timers, or seeking desynchronizes the pair. State is event-sourced: a
  viewer that sees time move backward (seek, loop wrap) replays from scratch, so
  pausing, stepping, and scrubbing all reduce to "set the clock".
- A viewer used alone creates and owns its player. A host composing viewers
  creates one player and passes it to each via the `player` prop — a ref
  starting `null`, attached on arrival — so shared viewers cannot drift. The
  player's owner also owns visibility gating (`observeVisibility`).

The landing hero (`landing/DemoStage.vue`) is deliberately nothing more than
this: `HERO_SCRIPT` + `loop: true` through the two standard viewers. Any
capability the hero needs must land as a player or viewer feature, never as
hero-only code. The landing's five point scenes (`landing/PointScene.vue`) are
the same composition over registry-built scripts; the viewers' and the paired
stage's styles live in `graph/viewers.css`, shared by the landing, the
playground, and docs embeds.

Clicking the canvas toggles play; `settle()` (and reduced motion) land on a
step's end state. The shell speaks the same color law for important operations:
the `daft` verb in gold, setup printouts in teal (`tone: "ok"`), agent lines in
purple (`tone: "agent"`), destructive output in rust (`tone: "rust"`),
commentary dim. Don't tone routine output — correspondence only means something
if it's scarce. Step navigation lives in the terminal: commands are the
affordance — clicking any command (or focusing it and pressing Enter) jumps to
that step's checkpoint, the command just typed, paused (`seekCheckpoint`), so
play resumes by executing it. There are no gutter markers; the command text
itself is the control. The terminal scrolls freely; it auto-follows the tail
only when the reader is already there.

## The verb layer

Acts are the atoms; daft verbs are the standard molecules. `verbs.ts` expands
each documented verb into its canonical beats — terminal lines and scene acts
bound as one unit — so every diagram renders a verb identically and the shell
can never drift from the graph. A scenario is a serializable list of
`VerbInvocation`s replayed by `buildScenario` over a world model that:

- generates truthful output (`daft list` rows and sync's prune summary come from
  what actually exists),
- places repos deterministically and computes cameras (`camFor` widens the
  fit-rect as repos join; the first three spots match the hero),
- gates and targets verbs (the composer offers only valid choices; an invocation
  whose prerequisites vanished skips cleanly — `mapping` marks it `-1` — instead
  of corrupting the scene),
- encodes the grammar's own rules: a cross-repo `start` relates the entering
  repo to every carrier of the branch and arcs to the latest one, `ship` merges
  only where the branch lives, `sync` prunes only merged shells.

Hand-written `StepDef[]` scripts stay valid — the hero predates the layer and
keeps its tuned pacing until the scenario rework — but new scripts should be
verb invocations first, raw beats only where a story needs a moment the layer
cannot say yet. `catalog.ts` gives every verb a demo scenario whose `focus`
names the invocation that is the verb; the catalog seeks straight to that
checkpoint (context built instantly by event replay) and plays the verb.

## Playground

`/playground` (`docs/playground.md`, unlisted for now) is the reference stage —
two modes:

- **Gallery** — replay full scripts from `gallery.ts`.
- **Verbs** — the catalog: pick a verb, land on its checkpoint with context
  already built, and watch it execute. The per-verb reference.

Transport (play/pause, step jumps, scrubber, rate, loop) and the
diagram/terminal/both layout switch apply in both modes. Iterate on animation
work here, not against the landing page. When the language grows, grow the
catalog and gallery in the same change — the vocabulary tour exists so every act
kind stays visible in isolation. Building scenarios interactively is the
composer's job, not the playground's.

## The composer

`/composer` (`docs/composer.md`, `layout: false`, mounted client-only) is the
full-window visual editor. The editor itself — the panes and locked layout,
document model, derivation pipeline, exports, and their hard rules (one
document/no modes, everything derives, pure mutations, rebuild-never-remount,
never-autoplay, exports-never-lie) — is dumbshow's; read the dumbshow repo's
CLAUDE.md before touching editor behavior. What is daft's, here in `composer/`:

- **`elements.ts`** — entity semantics: the draggable element catalog, what a
  canvas drop MEANS (seed mutations vs timeline ops), entity labels and
  selection, which hits drag (relations only select), and the three pointer
  markers the editor composes over every frame — selection ring (crisp gold),
  hover halo (faint gold, a hint of fill), drag ring (lifted, glowing). All
  identity-based: a marker finds its entity in the frame's hits, so it rides
  through seeks, rebuilds, and the live drag preview.
- **Relations are entities.** `render.ts` registers every live relation as a
  segment hit (`kind: "rel"`, picked under discs within 6px of the line);
  selecting one shows the relation card — seed relations unrelate from the
  Attributes pane, timeline-born ones point at `repo link` / `repo unlink`.
- **`views.ts` + `InspectorPane.vue` + `EntityAttributes.vue`** — the right-hand
  inspector: the World | daft | Files tab rows and the entity/op attribute
  forms, built over the package's generic `AttributesForm`.
- **`export/embed.ts`** — the docs-embed export template
  (`<RepoDiagram :script=.../>` + `<RepoTerminal .../>`), wired into the pack as
  `embedSnippet`.
- **`seed.ts`'s rename law** — `renameEntity` rewrites a name in seed, args,
  composite `repo:branch` targets, and placement keys up to the `rename` op that
  renames that entity away; callers freeze current geometry first
  (`scenePlacements` plus the pack's `freezePlacements`, both in `seed.ts`) or
  hash-derived positions teleport. It takes a `RenameTarget`, never a bare name:
  a branch belongs to one repo, and `web:checkout` and `orders:checkout` are two
  worktrees of one feature — renaming the selected one leaves its sibling alone.
  A repo rename has no op that renames it away, so it rewrites the whole
  timeline.
- **The seed and the placements are daft's JSON** (document format v2):
  `seed.ts` declares their schemas (`Seed`/`SeedRepo`/`SeedWt`, `Placements`
  keyed by repo name and `"repo:branch"`), validates them
  (`parseSeed`/`parsePlacements` — a throw becomes the document's own parse
  failure), and owns every write to them (`setRepoPlacement`, `setWtPlacement`,
  `freezePlacements`, the seed mutations). The package stores and migrates both
  halves unread and carries them across document versions verbatim, so a schema
  change here is versioned inside this JSON — never by bumping the document
  version. `seedOpeningStep` returns null when the seed declares nothing;
  relations alone still open the scene.
- **Placements are polar and freeze in families** (pack geometry law): repos pin
  as world `{x,y}`; worktrees pin as `{ang,dist}` around their repo — edge bend,
  label side, badge side, growth, and the sync ping all read the polar form.
  Unpinned angles derive from sibling order and label hashes, so dragging one
  worktree first freezes every sibling's current slot, and renames do the same —
  otherwise an insert or rename silently rotates or teleports neighbors.
  Dragging a repo onto another repo relates them; worktrees never change repos
  by drag (that meaning belongs to `carry`). A node drag previews live through
  the very mutation the drop commits (the editor's law — see dumbshow's
  CLAUDE.md), and canvas edits land settled on the playhead's step, never the
  end.

The composer's behavior catalog lives in `composer/test-scenarios.md` —
stable-ID scenarios (Setup/Steps/Expect) run by the Playwright suite in
`docs/tests/ui/`, with the harness traps (draft-debounce reset protocol,
synthetic pointer recipes, computed canvas positions) recorded up top.
`editor.*` specs pin machinery behavior (they copy to dumbshow's harness
eventually); `pack.*` specs pin daft-language behavior and stay here. Extend the
catalog in the same change that adds composer behavior — pack-side additions
land here, machinery additions land in dumbshow.

### North star — the daft sandbox

The composer's interactive shell is deliberately the seed of the **daft
sandbox**: a future web terminal on the docs site where people play with daft
before installing it. The world model, verb fidelity, shell parser, and Files
view are that sandbox's machinery. Every composer decision must hold that future
— keep the language layer (`engine`, `render`, `transcript`, `verbs`) importable
without the editor, keep verbs truthful to the real CLI surface, and keep world
state serializable.

## Embedding in docs

```vue
<RepoDiagram :script="SCRIPT" />
<!-- animated diagram, controls -->
<RepoDiagram :script="SCRIPT" still />
<!-- static settled frame -->
<RepoTerminal :script="SCRIPT" />
<!-- self-typing shell session -->
```

Both viewers are registered globally (theme `enhanceApp`), so docs pages can
embed them straight from markdown. `landing/PointScene.vue` is the reference
host for pairing the two on one shared player. Give the host element a size (the
canvas fills it) and a `--vp-c-bg-soft`-like background. Static diagrams for
docs pages are one-step scripts with `still` — the full grammar (hollow, badges,
arcs) works in a single frame.

## Extending

New meaning = new `Act` kind in `render.ts` + a rendering rule + an entry in
this file, in the same change. Keep acts declarative (data, not callbacks) so
scripts stay serializable and replays stay deterministic.

The vocabulary tracks daft's command surface. When a new command lands — or an
existing one significantly changes graph-visible behavior (anything that
creates, removes, or reshapes what diagrams show: worktrees, repos, relations,
agents, sync) — coordinate it with this engine in the same change or a filed
follow-up: add or extend the verb macro in `verbs.ts`, give it a catalog demo in
`catalog.ts`, and add an `Act` kind only when the meaning is genuinely new. The
root CLAUDE.md's "Adding a New Command" checklist points here; commands with no
graph-visible effect are exempt.

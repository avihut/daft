# The daft diagram language

This directory owns how daft draws its world: repos, worktrees, and the
connections between them. The landing hero is the flagship rendering, but the
same engine and grammar are meant for diagrams across the docs — static or
animated. If a page needs to show repo/worktree structure, use this language
(via `RepoDiagram.vue`), not an ad-hoc SVG or a screenshot.

Files: `engine.ts` (timeline compiler, event-sourced player, canvas renderer),
`RepoDiagram.vue` (embeddable component: canvas + optional step controls),
`hero-script.ts` (the landing story). Styles live in `../home.css` under the
`.dg-` prefix. Change this file and the renderer together — the grammar here is
normative, not descriptive.

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
- **Sync signal** — on `daft sync`, a small teal dot travels from the repo along
  each edge out to every worktree, ending in a teal blink: base updated, rebase
  available.
- **Relation (repo ↔ repo)** — dashed teal line, slowly crawling. Teal is
  connective tissue.
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
  it must not be confused with teal (setup/connection).
- **Creation pulse** — one expanding gold ring when anything is born, and when a
  merge lands. Gold pulses mean "look here", so keep them rare.

## Color law

Five colors carry meaning; everything else is neutral ink/paper drawn from the
live theme tokens (`--vp-c-text-*`, `--vp-c-bg-soft`, `--daft-gold`):

- **Gold** — wayfinding and attention: `main`, creation/merge pulses, feature
  arcs, the active checkpoint.
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

## Scripts, time, and interaction

A diagram is a `StepDef[]` script: beats of terminal lines (`cmd`/`out`), scene
`act`s, `cam` moves, and `pause`s. `compile` lays them on one absolute timeline;
the player is event-sourced — pausing and seeking just move the clock and
replay. Clicking the canvas toggles play; `settle()` (and reduced motion) land
on a step's end state.

When a diagram is paired with a terminal (the hero), the terminal renders from
the same compiled timeline via `@tick` — never from its own timers, or seeking
desynchronizes them. The shell speaks the same color law for important
operations: the `daft` verb in gold, setup printouts in teal (`tone: "ok"`),
agent lines in purple (`tone: "agent"`), destructive output in rust
(`tone: "rust"`), commentary dim. Don't tone routine output — correspondence
only means something if it's scarce. Step navigation lives in the terminal: each
step's first command carries a checkpoint circle in the left gutter, and
clicking any command jumps to that step's checkpoint — the command just typed,
paused (`seekCheckpoint`) — so play resumes by executing it. The terminal
scrolls freely; it auto-follows the tail only when the reader is already there.

## Embedding in docs

```vue
<RepoDiagram :script="SCRIPT" />
<!-- animated, controls -->
<RepoDiagram :script="SCRIPT" :controls="false" />
<RepoDiagram :script="SCRIPT" still />
<!-- static settled frame -->
```

Give the host element a size (the canvas fills it) and a `--vp-c-bg-soft`-like
background. Static diagrams for docs pages are one-step scripts with `still` —
the full grammar (hollow, badges, arcs) works in a single frame.

## Extending

New meaning = new `Act` kind in `engine.ts` + a rendering rule + an entry in
this file, in the same change. Keep acts declarative (data, not callbacks) so
scripts stay serializable and replays stay deterministic.

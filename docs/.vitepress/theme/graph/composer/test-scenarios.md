# Composer test scenarios

A catalog of documented behaviors for `/composer`, written to become Playwright
UI tests. Every scenario was exercised by hand during the build; the
Setup/Steps/Expect blocks record exactly what was done and what held. Scenario
IDs are stable — reference them from specs (`CMP-D2`, `CMP-S7`).

**These scenarios are now executable.** UI specs live in `docs/tests/ui/`
(`mise run docs:test:ui`); pure module-level invariants live in
`docs/tests/golden/` (`mise run docs:test:golden`, bun test, no browser) with
committed compiled digests under `__goldens__/` (regenerate deliberately via
`UPDATE_GOLDENS=1`). A change to composer behavior updates the scenario here AND
its spec in the same commit.

**Extraction note.** The editor machinery lives in `@avihut/dumbshow`; this
suite runs against `/composer` as integrated and is the extraction's parity net.
`editor.*` files (groups A–D, E1–E3 + E5, F, I–K) pin generic editor mechanics —
they are candidates to copy into dumbshow's own harness (under its boxes
reference pack) and stay here as integration coverage until then. `pack.*` files
(E4 rename semantics, G views, H shell, L hero/goldens) pin daft-language
behavior and never move. Group L's module-level goldens import the machinery
from the package and the pack from source — byte-identical compiled output
across that seam is the extraction tripwire.

## Harness notes (read first — these are the traps)

- **Reset protocol.** The draft autosaves on an 800ms debounce, so a naive
  `localStorage.clear()` races a pending write and the next mount restores stale
  rows. The reliable reset: clear → wait ≥900ms → clear → reload → wait for
  `.dx-app` (and ~700ms for the async chunk on first load). Assert `.dx-vrow`
  count is 0 before building anything.
- **Synthetic pointers.** The dnd controller listens on the pressed element
  (pointer capture is try/caught for synthetic events), so dispatch
  `PointerEvent`s directly on it with `clientX/Y`, `button: 0`, `bubbles: true`.
  A _tap_ is down+up within 4px; a _drag_ is down, one move past 4px, moves to
  the target, then up at the target. `pointercancel` aborts without dropping.
- **Canvas positions are computed, never scanned.** Repo discs (r≈21px) slip
  through coarse probe grids. Compute screen points: world coords from
  `player.compiled.events` (repo acts carry x/y; wt acts may carry pinned
  ang/dist), camera via `camFor` + `makeView` imported from
  `/@fs/<abs>/verbs.ts` and `/@fs/<abs>/render.ts`, offset by the canvas
  bounding rect.
- **Module-level asserts** import through the dev server: pack modules via
  `/@fs/<abs>/graph/...` (`GRAPH_FS` in helpers.ts), machinery via the built
  package dist (`DUMBSHOW_FS` →
  `node_modules/@avihut/dumbshow/dist/dumbshow.js`) — bare specifiers don't
  resolve in-page. The dev player handle is `window.__daftPlayer`, passed by
  every daft `createPlayer` site as `devHandle` (last-writer-wins across
  rebuilds — grab it fresh after every edit).
- **Timing.** DOM settles ~150–300ms after an edit (Vue flush + player rebuild +
  settle). Never assert immediately after dispatch.
- **Text content is whitespace-condensed** (Vue): a skipped row reads
  `listskipped`. Assert on `.dx-vrow b` (the verb) and dedicated elements, not
  on split row text.
- **Clipboard is blocked headless** — script/embed entries fall back to a
  download and show a notice either way; assert the notice.
- **webm records in real time** (duration ≈ document duration) — test with a
  one-op document; **GIF** encodes fast at low fps/small sizes.
- The playground/hero regression anchor: the six-point hero canvas fingerprint
  (seek six fixed times, hash `toDataURL()`); current values live in the git log
  (`docs: repo catalog verbs …` commit).

## A · Document lifecycle

- **CMP-A1 Empty state.** Fresh load → no timeline rows, canvas empty-state
  text, Export disabled, player bar shows 0:00.0/0:00.0, transport disabled.
- **CMP-A2 Draft autosave + restore.** Build clone+start → wait ≥900ms → reload
  → both rows restored; "Draft restored" notice appears at mount (assert quickly
  — it auto-dismisses after 4s).
- **CMP-A3 Save.** Save downloads `<slug>.daft.json`; parsing it yields
  `{version, title, seed, timeline, placements}` and `parseDoc` roundtrips.
- **CMP-A4 Open validates.** Opening `{"version":99}` shows the "newer composer"
  notice and leaves the current document untouched; malformed JSON likewise
  ("not valid JSON").
- **CMP-A5 Corrupt draft discarded.** Write garbage under the versioned draft
  key → reload → clean start, key removed.
- **CMP-A6 Title → filename.** Renaming the toolbar title updates the filename
  preview (`ship-a-feature.daft.json`) without rebuilding the player (playhead
  untouched).

## B · Building from the catalog

- **CMP-B1 Chip tap appends.** Tapping `clone` inserts an op at the insertion
  point (end when nothing is selected), selects it, and the player lands
  settled+paused on its step.
- **CMP-B2 World-aware seeding.** After clone(web), tapping `start` seeds branch
  `checkout` and repo `web` (the specs' own field defaults against the world at
  the insertion point).
- **CMP-B3 Groups derive from the registry.** Verbs group lists every
  non-typedOnly verb (no `cd` chip); Events group lists agent joins/leaves and
  forge merges as dashed chips (purple dot on agent ones); Meta has chapter and
  beat.
- **CMP-B4 Search filters** across groups; empty result shows the no-matches
  note; clearing restores.

## C · Timeline editing

- **CMP-C1 Select parks.** Clicking a row selects it and parks the playhead on
  that step's settled end, paused; clicking again deselects (position kept).
- **CMP-C2 Reorder buttons.** Moving `start` above its `clone` marks it `.dead`
  with the "skipped" tag (derive mapping -1); moving back restores.
- **CMP-C3 Delete.** The row's ✕ removes the item; selection adjusts (same-index
  cleared, later indices shift).
- **CMP-C4 Chapters.** A chapter chip inserts a rail notch (no full row); names
  bloom on rail hover (staggered transition-delay); notch click selects; the
  Attributes form renames it and the bloom label updates.
- **CMP-C5 Beats.** A beat inserts a slim dashed row; its seconds stretch the
  preceding step (compare `compiled.duration` before/after); a beat with no
  preceding step maps to -1.

## D · Drag and drop

- **CMP-D1 Tap vs drag threshold.** Down+up under 4px = the click behavior; past
  4px a ghost chip appears and follows the pointer, cleaned up on drop.
- **CMP-D2 Chip → timeline.** Dragging `exec` over the rows shows the gold
  insertion line at the resolved index; release inserts exactly there (verify by
  `.dx-vrow b` order), selects it, lands paused.
- **CMP-D3 Honest skips on drop.** An op dropped above its prerequisites shows
  `.dead` + "skipped" immediately.
- **CMP-D4 Row reorder by drag** with after-removal semantics (drop below the
  last row moves to the end).
- **CMP-D5 Element drops.** repo → lands at the drop point (placement written;
  works on an EMPTY canvas via the default camera); worktree → onto a seed repo
  takes the polar slot under the pointer (wt act carries ang/dist); agent → onto
  a seed feature worktree sets its flag; each invalid target (worktree on empty
  space, agent on a timeline-born wt, element chip on the timeline) surfaces its
  specific notice and stores nothing.
- **CMP-D6 Node drags.** Repo → empty spot rewrites `placements.repos` and the
  scene follows exactly; repo → another repo relates them (duplicate → "already
  related" notice); worktree → new polar slot with **every sibling frozen
  first** (placements gain all `repo:*` keys incl. main); worktree → another
  repo refuses ("carry moves the changes").

## E · Attributes

- **CMP-E1 Op fields re-resolve** against the world before the op: choices offer
  only valid targets; a value whose target vanished renders as "(gone)" instead
  of snapping.
- **CMP-E2 Arg edit rewrites the projection.** Changing start's branch to
  `perf/cache` rewrites the transcript command and the row label; lands paused
  on that step.
- **CMP-E3 Silent switch** ("Visible in the terminal") flips the flag;
  timing/duration unchanged (compare compiled durations).
- **CMP-E4 Entity rename is doc-wide.** Renaming repo `web` → `store` from the
  entity form rewrites the clone URL in the transcript, seed, composite
  `repo:branch` args, and placement keys; geometry is frozen first (no sibling
  teleport); selection follows the new name; the rewrite stops at an in-story
  `rename` op targeting the same entity.
- **CMP-E5 Seed toggles.** Seed worktrees expose port/agent/merged and delete;
  verb-born entities show read-only state and point at their step.

## F · Canvas selection

- **CMP-F1 Pick + ring.** Clicking a disc/node selects the entity, shows the
  entity form, and paints the gold overlay ring (canvas hash changes); clicking
  empty space clears both.
- **CMP-F2 Ring tracks.** The ring follows its entity across seeks and survives
  rebuilds; it vanishes gracefully when the entity does.

## G · State views (World | daft | Files)

- **CMP-G1 World rows** show gold dot for main, hollow dot for merged, agent
  chip, port chips, relation lines. Row click selects the entity (ring follows);
  the selected entity highlights its row.
- **CMP-G2 daft registry** lists repos with `~/name` paths + relations.
- **CMP-G3 Files tree** renders `~` with one folder per branch; `start` moves
  the gold cwd marker into the new worktree; hollow folders stay until `prune`
  removes them.
- **CMP-G4 Playhead awareness.** Parking on an earlier step trims all three
  views to the world as of that step.

## H · The shell

- **CMP-H1 Typed insert.** `daft clone git@github.com:acme/web.git` lands a row
  and the projection re-renders it; input clears.
- **CMP-H2 Ephemeral errors.** `daft go billing` prints the rust unknown-repo
  error; nothing is stored; the next valid line clears it; a rebuild clears it.
- **CMP-H3 cd.** `cd ~/web/main` inserts a cd row and moves cwd (Files marker);
  `cd ../checkout` walks `..`; `cd main` from inside a worktree errors like a
  real filesystem; `cd ~/nope` is ephemeral.
- **CMP-H4 cwd-aware verbs.** Bare `daft push` from `~/web/main` refuses
  ("nothing to push from web/main"); from an unpushed feature worktree it pushes
  that worktree. Non-daft lines get the "daft stories" redirect.
- **CMP-H5 Insertion point.** With a row selected (playhead parked there), a
  typed command inserts right after it — transcript order proves it.
- **CMP-H6 Edit in place.** Clicking the parked step's command swaps to a
  prefilled input; Enter with the same verb updates args (projection rewrites);
  a different available verb replaces the op (+"x became y" notice); an
  unavailable/invalid line is refused ephemerally with the doc untouched and the
  editor open; Escape cancels; jumping away closes it.
- **CMP-H7 The eye.** Toggling hides the step from exports: the editor dims the
  line (`.hiddenop`) and keeps it; `compiled.term` marks it hidden
  (viewers/exports omit).

## I · Player

- **CMP-I1 No autoplay, ever.** After every mutation path (chip, drop, reorder,
  delete, args, rename, shell insert, shell edit, open, restore)
  `__daftPlayer.playing()` is false and the clock sits on the affected step's
  settled end.
- **CMP-I2 Transport.** Play/pause toggles; prev/next step; jump to start/end;
  play pressed at the end restarts from 0.
- **CMP-I3 Scrubber** seeks by pointer (capture held while dragging); chapter
  notches sit at their step's start time and seek on click; names bloom on bar
  hover.
- **CMP-I4 Speed and loop** wire through to the player.
- **CMP-I5 Terminal click** jumps to the step's checkpoint (cue + 0.02, paused)
  and selects the owning row.

## J · Panes and chrome

- **CMP-J1 Scrubber toggle** hides only the player bar; everything else stays.
- **CMP-J2 Minimize matrix.** Timeline chevron → in-pane flap restores; catalog
  chevron collapses its body; BOTH minimized → the sidepane folds to the flap
  rail (`.dx-body.side-min`); the « control collapses directly; either flap
  restores its section and unfolds the pane.
- **CMP-J3 Terminal minimize** → bottom flap restores.
- **CMP-J4 Theme toggle** flips `html.dark` and back; the canvas repaints in the
  new palette (theme observer).

## K · Exports

- **CMP-K1 Menu gating + growth.** Disabled on an empty scene; entries: PNG,
  GIF, webm (only when `MediaRecorder` supports it), Compiled script, Docs
  embed.
- **CMP-K2 PNG.** `renderPngBlob` returns image/png at exactly 2x the requested
  size; two renders of the same clock are byte-identical.
- **CMP-K3 GIF.** GIF89a magic; header dimensions match; a 2000px request caps
  at 900 on the long edge; progress reaches `ceil(duration*fps)+1`.
- **CMP-K4 webm.** Mime detection; EBML magic (`1A 45 DF A3`); real-time pacing
  (document the cost in the test timeout); progress completes.
- **CMP-K5 Script/embed.** Script JSON parses and recompiles to the same
  duration; the embed snippet contains both viewers and the still-variant
  comment; clicking surfaces a notice (clipboard or fallback download).

## L · Language invariants (module-level, no UI)

- **CMP-L1 Registry shape.** 21 ops: 18 verbs (incl. typed-only `cd`) + 3
  events; events return null from `command()`; `VERBS` excludes events.
- **CMP-L2 Catalog demos build clean** (every demo's mapping has no -1) and the
  vocabulary tour covers all 15 act kinds.
- **CMP-L3 command() agrees with run()** for every catalog invocation.
- **CMP-L4 Verb semantics.** push marks pushed and shrinks its pool; forge
  merges only pushed worktrees and is gated until one exists; merge lands and
  removes; prune only with hollow shells; sync `--push` marks survivors; rename
  never teleports (ang/dist byte-stable) and rewrites every scene reference;
  repo-remove staggers teardown 0.12s apart and fades touching rels/arcs; arcs
  fade (never pop) on endpoint removal.
- **CMP-L5 Hero fingerprint.** The six-point landing-hero canvas hashes match
  the values recorded in the latest fingerprint commit — the regression tripwire
  for any renderer change.
- **CMP-L6 Offline determinism.** Fresh-vs-incremental and post-rewind renders
  are byte-identical; attached-canvas renders match the live viewer exactly (the
  grayscale-vs-LCD text AA nuance is the only sanctioned difference, and only
  for detached canvases).

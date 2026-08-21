/**
 * DAFT_PACK — daft spoken as a diagram language.
 *
 * This is the daft side of the language-pack seam (language.ts): the op
 * registry, the world, the scene vocabulary and its drawing, and the
 * shell grammar, assembled into one object. Today it is wiring over the
 * existing modules; the extraction moves the generic machinery out and
 * leaves this pack (plus the modules it re-exports) as daft's adapter.
 * Consumers migrate onto it commit by commit — new code reaches daft
 * meaning through the pack, never by importing verbs/render directly
 * from the generic side.
 *
 * The type aliases below are the same move for types: the engine's
 * generic shapes bound to daft's acts, so consumers can say `StepDef`
 * and mean "a daft step" without touching the generic machinery.
 */

import type {
  Beat as BeatOf,
  Compiled as CompiledOf,
  DiagramLanguage,
  Placements,
  Player as PlayerOf,
  SceneEvent as SceneEventOf,
  Seed,
  StepDef as StepDefOf,
} from "@avihut/dumbshow";
import {
  canvasDrop,
  dragRing,
  ELEMENTS,
  type EntitySelection,
  entityLabel,
  hoverRing,
  isDraggable,
  selectionFromHit,
  selectionRing,
} from "./composer/elements";
import { embedSnippet } from "./composer/export/embed";
import {
  type Act,
  applyAct,
  createScene,
  drawScene,
  type Hit,
  pick,
  readPalette,
  type Scene,
} from "./render";
import { scenePlacements, seedOpeningStep, worldFromSeed } from "./seed";
import { parseCommand } from "./shell";
import {
  camFor,
  emptyWorld,
  OPS,
  patchWtPlacements,
  type World,
} from "./verbs";

export type { Act, Scene } from "./render";
export type { World } from "./verbs";

/** The engine's shapes spoken with daft acts. */
export type Beat = BeatOf<Act>;
export type StepDef = StepDefOf<Act>;
export type SceneEvent = SceneEventOf<Act>;
export type Compiled = CompiledOf<Act>;
export type Player = PlayerOf<Act>;

export const DAFT_PACK: DiagramLanguage<World, Act, Scene, StepDef> = {
  ops: OPS,
  emptyWorld,
  scene: { createScene, applyAct, drawScene, camFor, readPalette, pick },
  seed: {
    world: (seed, placements) =>
      worldFromSeed(seed as Seed, placements as Placements),
    step: (world, placements) =>
      seedOpeningStep(world, placements as Placements),
  },
  placements: {
    patchStep: (step, placements) =>
      patchWtPlacements(step.beats, (placements as Placements).wts),
    fromCompiled: (compiled) => scenePlacements(compiled as Compiled),
  },
  entities: {
    elements: ELEMENTS,
    label: (hit) => entityLabel(hit as Hit),
    select: (hit) => selectionFromHit(hit as Hit),
    selectionOverlay: (sel) => selectionRing(sel as EntitySelection),
    hoverOverlay: (sel) => hoverRing(sel as EntitySelection),
    dragOverlay: (sel) => dragRing(sel as EntitySelection),
    draggable: (hit) => isDraggable(hit as Hit),
    canvasDrop: (drop) => canvasDrop(drop as Parameters<typeof canvasDrop>[0]),
  },
  parseCommand: (line, world) => parseCommand(line, world, world.cwd ?? "~"),
  shellVerb: (text) => (text.startsWith("daft") ? "daft" : ""),
  shellInputLabel: "Type a daft command — it lands on the timeline",
  embedSnippet,
};

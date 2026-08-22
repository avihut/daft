/**
 * Element drop semantics — what dropping each catalog entity means.
 *
 * Elements build the SEED: the state that exists before the story runs,
 * drawn with the full grammar but never as commands. A repo lands where
 * you drop it; a worktree drops onto a seed repo and takes the polar slot
 * under the pointer; an agent docks on a seed worktree. Entities the
 * timeline created are the timeline's to change — dropping onto them
 * explains the verb or event that owns that meaning instead.
 */

import type {
  Compiled as CompiledOf,
  ComposerDoc,
  ElementSpec,
} from "@dumbshow/core";
import type { Act, Hit } from "../render";
import {
  addSeedRepo,
  addSeedWt,
  freezePlacements,
  scenePlacements,
  seedOf,
  setRepoPlacement,
  setSeedRel,
  setWtPlacement,
  updateSeedWt,
} from "../seed";
import { nextBranch, nextRepoName, type World } from "../verbs";

type Compiled = CompiledOf<Act>;

/** The daft element palette — what the catalog offers to drop. */
export const ELEMENTS: ElementSpec[] = [
  { id: "repo", label: "repo", icon: "repo" },
  { id: "worktree", label: "worktree", icon: "wt" },
  { id: "agent", label: "agent", icon: "agent" },
  { id: "relation", label: "relation", icon: "rel" },
];

/** The entity a hit names — worktrees by branch, repos by name, relations
 * as the pair they join. */
export function entityLabel(hit: Hit): string {
  if (hit.kind === "rel") return `${hit.a} ↔ ${hit.b}`;
  return hit.kind === "wt" ? hit.wt : hit.repo;
}

/** The selection value a picked hit means — what the editor carries. */
export function selectionFromHit(hit: Hit): EntitySelection {
  if (hit.kind === "rel") return { kind: "rel", a: hit.a, b: hit.b };
  return hit.kind === "repo"
    ? { kind: "repo", repo: hit.repo }
    : { kind: "wt", repo: hit.repo, wt: hit.wt };
}

/** Relations never move by drag — they follow their repos; a press on one
 * selects it however far it travels. */
export function isDraggable(hit: Hit): boolean {
  return hit.kind !== "rel";
}

export type EntitySelection =
  | { kind: "repo"; repo: string; wt?: undefined }
  | { kind: "wt"; repo: string; wt: string }
  | { kind: "rel"; a: string; b: string };

/** Does the seed (not the timeline) own this relation? */
export function isSeedRel(doc: ComposerDoc, a: string, b: string): boolean {
  return seedOf(doc).rels.some(
    ([x, y]) => (x === a && y === b) || (x === b && y === a),
  );
}

/** The daft gold, read live from the theme (falls back to the brand hex). */
function goldToken(): string {
  return (
    getComputedStyle(document.documentElement)
      .getPropertyValue("--daft-gold")
      .trim() || "#d99a21"
  );
}

/** The frame hit a selection refers to, when the entity is on screen. */
function hitFor(sel: EntitySelection, hits: Hit[]): Hit | undefined {
  return hits.find((x) => {
    if (sel.kind === "rel")
      return (
        x.kind === "rel" &&
        ((x.a === sel.a && x.b === sel.b) || (x.a === sel.b && x.b === sel.a))
      );
    if (sel.kind === "repo") return x.kind === "repo" && x.repo === sel.repo;
    return x.kind === "wt" && x.repo === sel.repo && x.wt === sel.wt;
  });
}

type Marker = (ctx: CanvasRenderingContext2D, hits: Hit[]) => void;

/**
 * The pointer markers, in daft's gold — painted over every frame from that
 * frame's hits, so they track their entity through seeks, rebuilds, and the
 * live drag preview. Matching a selection to its hit is language logic:
 * only the pack knows a worktree from its repo, or which segment a relation
 * is. Nodes get rings around the disc; relations get the same weights laid
 * along the line.
 */
function marker(
  sel: EntitySelection,
  style: {
    width: number;
    alpha: number;
    pad: number;
    glow?: boolean;
    fill?: number;
  },
): Marker {
  const gold = goldToken();
  return (octx, hits) => {
    const h = hitFor(sel, hits);
    if (!h) return;
    octx.save();
    octx.strokeStyle = gold;
    octx.lineWidth = style.width;
    octx.globalAlpha = style.alpha;
    if (style.glow) {
      octx.shadowColor = gold;
      octx.shadowBlur = 14;
    }
    if (h.kind === "rel") {
      octx.lineCap = "round";
      octx.beginPath();
      octx.moveTo(h.x1, h.y1);
      octx.lineTo(h.x2, h.y2);
      octx.stroke();
    } else {
      if (style.fill) {
        octx.fillStyle = gold;
        octx.globalAlpha = style.fill;
        octx.beginPath();
        octx.arc(h.sx, h.sy, h.r + style.pad, 0, Math.PI * 2);
        octx.fill();
        octx.globalAlpha = style.alpha;
      }
      octx.beginPath();
      octx.arc(h.sx, h.sy, h.r + style.pad, 0, Math.PI * 2);
      octx.stroke();
    }
    octx.restore();
  };
}

/** The selection ring: crisp gold. */
export function selectionRing(sel: EntitySelection): Marker {
  return marker(sel, { width: 2, alpha: 0.9, pad: 5 });
}

/** The hover marker: a faint halo — the pointer can take this. */
export function hoverRing(sel: EntitySelection): Marker {
  return marker(sel, { width: 1.5, alpha: 0.45, pad: 5, fill: 0.08 });
}

/** The drag marker: the lifted ring, glowing — this is moving with you.
 * Relations never lift. */
export function dragRing(sel: EntitySelection): Marker | null {
  if (sel.kind === "rel") return null;
  return marker(sel, { width: 2.5, alpha: 1, pad: 6, glow: true });
}

export type ElementDrop =
  | {
      doc: ComposerDoc;
      select?: EntitySelection;
    }
  | { error: string };

function isSeedRepo(doc: ComposerDoc, name: string): boolean {
  return seedOf(doc).repos.some((r) => r.name === name);
}

function isSeedWt(doc: ComposerDoc, repo: string, branch: string): boolean {
  return (
    seedOf(doc)
      .repos.find((r) => r.name === repo)
      ?.wts.some((w) => w.branch === branch) === true
  );
}

export function dropElement(
  doc: ComposerDoc,
  world: World,
  elementId: string,
  wx: number,
  wy: number,
  over: Hit | null,
): ElementDrop {
  switch (elementId) {
    case "repo": {
      const name = nextRepoName(world);
      const base = 3000 + world.repos.length * 1000;
      let next = addSeedRepo(doc, {
        name,
        wts: [{ branch: "main", port: `:${base}` }],
      });
      next = setRepoPlacement(next, name, {
        x: Math.round(wx),
        y: Math.round(wy),
      });
      return { doc: next, select: { kind: "repo", repo: name } };
    }
    case "worktree": {
      const repoName = over?.repo;
      if (!repoName)
        return { error: "Drop a worktree onto a repo to give it a home." };
      if (!isSeedRepo(doc, repoName))
        return {
          error: `${repoName} is created by the timeline — grow it with daft start instead.`,
        };
      const repo = world.repos.find((r) => r.name === repoName);
      if (!repo) return { error: `No repo named ${repoName} in the scene.` };
      const branch = nextBranch(world);
      const port = `:${repo.nextPort}`;
      let next = addSeedWt(doc, repoName, { branch, port });
      const ang = Math.atan2(wy - repo.y, wx - repo.x);
      const dist = Math.min(
        Math.max(Math.hypot(wx - repo.x, wy - repo.y), 48),
        150,
      );
      next = setWtPlacement(next, `${repoName}:${branch}`, {
        ang: Math.round(ang * 1000) / 1000,
        dist: Math.round(dist),
      });
      return {
        doc: next,
        select: { kind: "wt", repo: repoName, wt: branch },
      };
    }
    case "agent": {
      if (over?.kind !== "wt")
        return { error: "Drop the agent onto a worktree." };
      if (!isSeedWt(doc, over.repo, over.wt))
        return {
          error:
            "That worktree is created by the timeline — agents join it through the agent joins event.",
        };
      if (over.wt === "main")
        return { error: "Agents work on feature worktrees, not main." };
      return {
        doc: updateSeedWt(doc, over.repo, over.wt, { agent: true }),
        select: { kind: "wt", repo: over.repo, wt: over.wt },
      };
    }
    case "relation":
      return {
        error: "Drag one repo onto another on the canvas to relate them.",
      };
    default:
      return { error: `Nothing to drop for "${elementId}".` };
  }
}

export type CanvasDropOutcome =
  | { doc: ComposerDoc; select?: EntitySelection }
  | { error: string }
  | null;

/**
 * The canvas half of the drop funnel — element chips build the seed where
 * they land, node drags place entities, and repo onto repo relates them.
 * Moved verbatim from the editor's applyDrop: this is language, not
 * plumbing (what a drop MEANS is daft's to say).
 */
export function canvasDrop(drop: {
  doc: ComposerDoc;
  world: World;
  compiled: Compiled;
  source: { kind: "element"; id: string } | { kind: "node"; hit: Hit };
  wx: number;
  wy: number;
  over: Hit | null;
}): CanvasDropOutcome {
  const { doc, world, compiled, source, wx, wy, over } = drop;
  if (source.kind === "element") {
    const dropped = dropElement(doc, world, source.id, wx, wy, over);
    if ("error" in dropped) return dropped;
    return { doc: dropped.doc, select: dropped.select };
  }
  const hit = source.hit;
  // Relations are not draggable (isDraggable); the funnel stays total.
  if (hit.kind === "rel") return null;
  if (hit.kind === "repo") {
    if (over?.kind === "repo" && over.repo !== hit.repo) {
      const next = setSeedRel(doc, hit.repo, over.repo, true);
      if (next === doc)
        return {
          error: `${hit.repo} and ${over.repo} are already related.`,
        };
      return { doc: next, select: { kind: "repo", repo: hit.repo } };
    }
    return {
      doc: setRepoPlacement(doc, hit.repo, {
        x: Math.round(wx),
        y: Math.round(wy),
      }),
      select: { kind: "repo", repo: hit.repo },
    };
  }
  // A worktree stays with its repo: dragging re-seats its polar slot.
  if (over?.kind === "repo" && over.repo !== hit.repo)
    return {
      error: "Worktrees stay with their repo — carry moves the changes.",
    };
  const repo = world.repos.find((r) => r.name === hit.repo);
  if (!repo) return null;
  const ang = Math.atan2(wy - repo.y, wx - repo.x);
  const dist = Math.min(
    Math.max(Math.hypot(wx - repo.x, wy - repo.y), 48),
    150,
  );
  // Freeze every sibling's current slot first — angles derive from
  // sibling order, so an unpinned neighbor would otherwise rotate.
  const all = scenePlacements(compiled);
  const siblings: typeof all.wts = {};
  for (const [key, p] of Object.entries(all.wts))
    if (key.startsWith(`${hit.repo}:`)) siblings[key] = p;
  let next = freezePlacements(doc, { repos: {}, wts: siblings });
  next = setWtPlacement(next, `${hit.repo}:${hit.wt}`, {
    ang: Math.round(ang * 1000) / 1000,
    dist: Math.round(dist),
  });
  return { doc: next, select: { kind: "wt", repo: hit.repo, wt: hit.wt } };
}

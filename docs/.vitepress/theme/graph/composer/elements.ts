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

import {
  type Compiled as CompiledOf,
  type ComposerDoc,
  type ElementSpec,
  freezePlacements,
  setRepoPlacement,
  setWtPlacement,
} from "@avihut/dumbshow";
import type { Act, Hit } from "../render";
import {
  addSeedRepo,
  addSeedWt,
  scenePlacements,
  setSeedRel,
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

/** The entity a drag ghost names — worktrees by branch, repos by name. */
export function entityLabel(hit: Hit): string {
  return hit.kind === "wt" ? hit.wt : hit.repo;
}

/** The selection value a picked hit means — what the editor carries. */
export function selectionFromHit(hit: Hit): EntitySelection {
  return hit.kind === "repo"
    ? { kind: "repo", repo: hit.repo }
    : { kind: "wt", repo: hit.repo, wt: hit.wt };
}

/**
 * The selection ring: painted over every frame from that frame's hits, in
 * daft's gold. Matching a selection to its hit is language logic — only
 * the pack knows a worktree from its repo.
 */
export function selectionRing(
  sel: EntitySelection,
): (ctx: CanvasRenderingContext2D, hits: Hit[]) => void {
  const gold =
    getComputedStyle(document.documentElement)
      .getPropertyValue("--daft-gold")
      .trim() || "#d99a21";
  return (octx, hits) => {
    const h = hits.find((x) =>
      sel.kind === "repo"
        ? x.kind === "repo" && x.repo === sel.repo
        : x.kind === "wt" && x.repo === sel.repo && x.wt === sel.wt,
    );
    if (!h) return;
    octx.strokeStyle = gold;
    octx.lineWidth = 2;
    octx.globalAlpha = 0.9;
    octx.beginPath();
    octx.arc(h.sx, h.sy, h.r + 5, 0, Math.PI * 2);
    octx.stroke();
    octx.globalAlpha = 1;
  };
}

export type EntitySelection = {
  kind: "repo" | "wt";
  repo: string;
  wt?: string;
};

export type ElementDrop =
  | {
      doc: ComposerDoc;
      select?: EntitySelection;
    }
  | { error: string };

function isSeedRepo(doc: ComposerDoc, name: string): boolean {
  return doc.seed.repos.some((r) => r.name === name);
}

function isSeedWt(doc: ComposerDoc, repo: string, branch: string): boolean {
  return (
    doc.seed.repos
      .find((r) => r.name === repo)
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

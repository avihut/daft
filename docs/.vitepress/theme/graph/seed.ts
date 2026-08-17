/**
 * daft seed semantics — the pack side of a document's opening state.
 *
 * A composer document's seed declares what exists before the story runs
 * (repos, worktrees, relations) and its placements pin author-dragged
 * geometry. Both are document JSON whose schema this language defined, so
 * reading them is pack work: building the pre-story world, drawing the
 * opening scene step, and replaying compiled events into the placement
 * shapes an author's drag would freeze. The generic editor reaches these
 * through the seed/placements hooks on DAFT_PACK (language.ts), never by
 * importing this module. Type imports go the other way on purpose: packs
 * implement against the editor's document shapes (./composer/doc.ts).
 */

import type {
  Beat as BeatOf,
  Compiled as CompiledOf,
  ComposerDoc,
  DocItem,
  Placements,
  RepoPlacement,
  Seed,
  SeedRepo,
  SeedWt,
  StepDef as StepDefOf,
  VerbArgs,
  WtPlacement,
} from "@avihut/dumbshow";
import { type Act, applyAct, createScene } from "./render";
import {
  addRepo,
  camFor,
  emptyWorld,
  patchWtPlacements,
  type World,
} from "./verbs";

type Beat = BeatOf<Act>;
type StepDef = StepDefOf<Act>;
type Compiled = CompiledOf<Act>;

export function worldFromSeed(seed: Seed, placements: Placements): World {
  const world = emptyWorld();
  if (Object.keys(placements.repos).length)
    world.spots = structuredClone(placements.repos);
  for (const sr of seed.repos) {
    const repo = addRepo(world, sr.name);
    const base = 3000 + (world.repos.length - 1) * 1000;
    repo.wts = sr.wts.map((w) => ({
      branch: w.branch,
      ...(w.port ? { port: w.port } : {}),
      agent: w.agent === true,
      merged: w.merged === true,
      removed: false,
    }));
    repo.nextPort =
      base + 1 + repo.wts.filter((w) => w.branch !== "main").length;
  }
  for (const [a, b] of seed.rels) world.rels.push([a, b]);
  return world;
}

/** The opening scene: everything the seed declares, drawn without a shell. */
export function seedOpeningStep(world: World, placements: Placements): StepDef {
  const beats: Beat[] = [];
  for (const repo of world.repos) {
    beats.push({
      act: { kind: "repo", repo: repo.name, x: repo.x, y: repo.y },
    });
    for (const wt of repo.wts) {
      beats.push({ act: { kind: "wt", repo: repo.name, wt: wt.branch } });
      if (wt.port)
        beats.push({
          act: { kind: "port", repo: repo.name, wt: wt.branch, port: wt.port },
        });
      if (wt.agent)
        beats.push({ act: { kind: "agent", repo: repo.name, wt: wt.branch } });
      if (wt.merged)
        beats.push({ act: { kind: "merged", repo: repo.name, wt: wt.branch } });
    }
  }
  for (const [a, b] of world.rels)
    beats.push({ act: { kind: "relate", a, b } });
  beats.push({ pause: 0.8 });
  patchWtPlacements(beats, placements.wts);
  return { title: "Scene", cam: camFor(world), beats, silent: true };
}

/**
 * The geometry a compiled scene actually renders with — replayed through
 * the same applyAct the canvas uses, so hash-derived angles come out
 * exactly as drawn. Feed this to `freezePlacements` before renames or
 * sibling inserts to pin what the author currently sees.
 */
export function scenePlacements(compiled: Compiled): Placements {
  const scene = createScene();
  for (const ev of compiled.events) applyAct(scene, ev.act, ev.at);
  const repos: Record<string, RepoPlacement> = {};
  const wts: Record<string, WtPlacement> = {};
  for (const r of scene.repos) {
    repos[r.label] = { x: r.x, y: r.y };
    for (const w of r.wts)
      wts[`${r.label}:${w.label}`] = { ang: w.ang, dist: w.dist };
  }
  return { repos, wts };
}

/* ----------------------------- seed mutations ----------------------------- */

function clone(doc: ComposerDoc): ComposerDoc {
  return structuredClone(doc);
}

export function addSeedRepo(doc: ComposerDoc, repo: SeedRepo): ComposerDoc {
  if (doc.seed.repos.some((r) => r.name === repo.name)) return doc;
  const next = clone(doc);
  next.seed.repos.push(structuredClone(repo));
  return next;
}

export function removeSeedRepo(doc: ComposerDoc, name: string): ComposerDoc {
  const next = clone(doc);
  next.seed.repos = next.seed.repos.filter((r) => r.name !== name);
  next.seed.rels = next.seed.rels.filter(([a, b]) => a !== name && b !== name);
  delete next.placements.repos[name];
  for (const key of Object.keys(next.placements.wts))
    if (key.startsWith(`${name}:`)) delete next.placements.wts[key];
  return next;
}

export function addSeedWt(
  doc: ComposerDoc,
  repoName: string,
  wt: SeedWt,
): ComposerDoc {
  const repo = doc.seed.repos.find((r) => r.name === repoName);
  if (!repo || repo.wts.some((w) => w.branch === wt.branch)) return doc;
  const next = clone(doc);
  next.seed.repos
    .find((r) => r.name === repoName)
    ?.wts.push(structuredClone(wt));
  return next;
}

export function removeSeedWt(
  doc: ComposerDoc,
  repoName: string,
  branch: string,
): ComposerDoc {
  const next = clone(doc);
  const repo = next.seed.repos.find((r) => r.name === repoName);
  if (!repo) return doc;
  repo.wts = repo.wts.filter((w) => w.branch !== branch);
  delete next.placements.wts[`${repoName}:${branch}`];
  return next;
}

export function updateSeedWt(
  doc: ComposerDoc,
  repoName: string,
  branch: string,
  patch: Partial<Omit<SeedWt, "branch">>,
): ComposerDoc {
  const next = clone(doc);
  const wt = next.seed.repos
    .find((r) => r.name === repoName)
    ?.wts.find((w) => w.branch === branch);
  if (!wt) return doc;
  Object.assign(wt, patch);
  return next;
}

export function setSeedRel(
  doc: ComposerDoc,
  a: string,
  b: string,
  on: boolean,
): ComposerDoc {
  const has = doc.seed.rels.some(
    ([x, y]) => (x === a && y === b) || (x === b && y === a),
  );
  if (has === on) return doc;
  const next = clone(doc);
  if (on) next.seed.rels.push([a, b]);
  else
    next.seed.rels = next.seed.rels.filter(
      ([x, y]) => !((x === a && y === b) || (x === b && y === a)),
    );
  return next;
}

/* ------------------------------ rename-entity ---------------------------- */

function renameValue(
  value: unknown,
  kind: "repo" | "branch",
  from: string,
  to: string,
): unknown {
  if (typeof value !== "string") return value;
  if (value === from) return to;
  // Composite "repo:branch" targets rewrite on their matching side.
  if (kind === "repo" && value.startsWith(`${from}:`))
    return `${to}${value.slice(from.length)}`;
  if (kind === "branch" && value.endsWith(`:${from}`))
    return `${value.slice(0, value.length - from.length)}${to}`;
  return value;
}

function renameArgs(
  args: VerbArgs,
  kind: "repo" | "branch",
  from: string,
  to: string,
): VerbArgs {
  const out: VerbArgs = {};
  for (const [key, value] of Object.entries(args)) {
    out[key] = Array.isArray(value)
      ? value.map((v) => renameValue(v, kind, from, to))
      : renameValue(value, kind, from, to);
  }
  return out;
}

/**
 * The Attributes-panel rename: rewrite an entity's name everywhere it is
 * referenced — seed, op args, placements — from its creation up to the
 * first `rename` op that renames it (later items already refer to the new
 * name that op introduced). This is what makes the transcript projection
 * rewrite every past command instead of leaving stale errors.
 *
 * Callers should freeze currently-derived geometry first (via
 * `freezePlacements`) — worktree angles derive from label hashes, so a
 * rename would otherwise teleport siblings.
 */
export function renameEntity(
  doc: ComposerDoc,
  kind: "repo" | "branch",
  from: string,
  to: string,
): ComposerDoc {
  if (!to.trim() || from === to) return doc;
  const next = clone(doc);

  if (kind === "repo") {
    for (const repo of next.seed.repos) if (repo.name === from) repo.name = to;
    next.seed.rels = next.seed.rels.map(([a, b]) => [
      a === from ? to : a,
      b === from ? to : b,
    ]);
    const rp = next.placements.repos[from];
    if (rp) {
      delete next.placements.repos[from];
      next.placements.repos[to] = rp;
    }
  } else {
    for (const repo of next.seed.repos)
      for (const wt of repo.wts) if (wt.branch === from) wt.branch = to;
  }

  for (const [key, p] of Object.entries(next.placements.wts)) {
    const renamed = renameValue(key, kind, from, to) as string;
    if (renamed !== key) {
      delete next.placements.wts[key];
      next.placements.wts[renamed] = p;
    }
  }

  let stopped = false;
  next.timeline = next.timeline.map((item): DocItem => {
    if (stopped || item.kind !== "op") return item;
    // A rename op that renames this entity ends the bulk rewrite — items
    // after it already refer to the name that op introduced. Its target may
    // be spelled bare or as a composite "repo:branch".
    const rewritesIt =
      item.op === "rename" &&
      Object.values(item.args).some(
        (v) =>
          typeof v === "string" &&
          (v === from || v.endsWith(`:${from}`) || v.startsWith(`${from}:`)),
      );
    const out: DocItem = {
      ...item,
      args: renameArgs(item.args, kind, from, to),
    };
    if (rewritesIt) stopped = true;
    return out;
  });

  return next;
}

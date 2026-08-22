/**
 * daft seed semantics — the pack side of a document's opening state.
 *
 * A composer document's seed declares what exists before the story runs
 * (repos, worktrees, relations) and its placements pin author-dragged
 * geometry. Under document format v2 both are PACK-OWNED JSON: the schemas
 * below are daft's. The generic side stores, serializes, and migrates them
 * without reading their shape — they cross document-version bumps verbatim,
 * so a change to these schemas is versioned inside this JSON, never by the
 * document version — and hands them back through the seed/placements hooks
 * on DAFT_PACK (language.ts): validating them, building the pre-story
 * world, drawing the opening scene step, replaying compiled events into the
 * placement shapes an author's drag would freeze, and writing pins back as
 * whole values (core offers no key-level helper for JSON it cannot read).
 * The generic editor never imports this module; packs implement against the
 * editor's document shapes (@dumbshow/core).
 */

import type {
  Beat as BeatOf,
  Compiled as CompiledOf,
  ComposerDoc,
  DocItem,
  StepDef as StepDefOf,
  VerbArgs,
} from "@dumbshow/core";
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

/* ------------------------------ the schemas ------------------------------ */

/** Explicit world-space repo position, written when the author drags. */
export interface RepoPlacement {
  x: number;
  y: number;
}

/** Explicit polar worktree placement around its repo. */
export interface WtPlacement {
  ang: number;
  dist: number;
}

/**
 * Author-pinned geometry. Worktree keys are `"repo:branch"`. Anything not
 * listed keeps its deterministic hash-derived position.
 */
export interface Placements {
  repos: Record<string, RepoPlacement>;
  wts: Record<string, WtPlacement>;
}

export interface SeedWt {
  branch: string;
  port?: string;
  agent?: boolean;
  merged?: boolean;
}

export interface SeedRepo {
  name: string;
  wts: SeedWt[];
}

/** The world as the story opens — rendered as scene, never as commands. */
export interface Seed {
  repos: SeedRepo[];
  rels: [string, string][];
}

/** What `emptyDoc(DAFT_PACK)` writes. */
export function emptySeed(): Seed {
  return { repos: [], rels: [] };
}

export function emptyPlacements(): Placements {
  return { repos: {}, wts: {} };
}

/**
 * A document's two pack-owned halves, read as daft's schemas. Every
 * document the editor holds came through `emptyDoc`/`parseDoc` with this
 * pack, so the shape is guaranteed; these are the one place the cast lives.
 */
export function seedOf(doc: ComposerDoc): Seed {
  return doc.seed as Seed;
}

export function placementsOf(doc: ComposerDoc): Placements {
  return doc.placements as Placements;
}

/* ------------------------------- validation ------------------------------ */

/**
 * Reject one document's JSON. The document model prefixes the message with
 * the half it came from (`Not a composer document: seed — …`).
 */
function bad(msg: string): never {
  throw new Error(msg);
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function isName(v: unknown): v is string {
  return typeof v === "string" && v.trim().length > 0;
}

function isFinite2(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

/** Validate a document's seed JSON; throws naming what is wrong. */
export function parseSeed(raw: unknown): Seed {
  if (!isRecord(raw)) bad("the seed is not an object");
  const repos: SeedRepo[] = [];
  if (!Array.isArray(raw.repos)) bad("seed.repos is not a list");
  for (const r of raw.repos as unknown[]) {
    if (!isRecord(r) || !isName(r.name)) bad("a seed repo has no name");
    if (!Array.isArray(r.wts)) bad(`seed repo ${r.name} has no worktrees`);
    const wts: SeedWt[] = [];
    for (const w of r.wts as unknown[]) {
      if (!isRecord(w) || !isName(w.branch))
        bad(`a worktree in ${r.name} has no branch`);
      wts.push({
        branch: w.branch,
        ...(isName(w.port) ? { port: w.port } : {}),
        ...(w.agent === true ? { agent: true } : {}),
        ...(w.merged === true ? { merged: true } : {}),
      });
    }
    repos.push({ name: r.name, wts });
  }
  const rels: [string, string][] = [];
  if (!Array.isArray(raw.rels)) bad("seed.rels is not a list");
  for (const rel of raw.rels as unknown[]) {
    if (
      !Array.isArray(rel) ||
      rel.length !== 2 ||
      !isName(rel[0]) ||
      !isName(rel[1])
    )
      bad("a relation is not a [repo, repo] pair");
    rels.push([rel[0], rel[1]]);
  }
  return { repos, rels };
}

/** Validate a document's placements JSON; throws naming what is wrong. */
export function parsePlacements(raw: unknown): Placements {
  if (!isRecord(raw)) bad("placements is not an object");
  const repos: Record<string, RepoPlacement> = {};
  if (raw.repos !== undefined) {
    if (!isRecord(raw.repos)) bad("placements.repos is not an object");
    for (const [name, p] of Object.entries(raw.repos)) {
      if (!isRecord(p) || !isFinite2(p.x) || !isFinite2(p.y))
        bad(`the placement for repo ${name} is malformed`);
      repos[name] = { x: p.x, y: p.y };
    }
  }
  const wts: Record<string, WtPlacement> = {};
  if (raw.wts !== undefined) {
    if (!isRecord(raw.wts)) bad("placements.wts is not an object");
    for (const [key, p] of Object.entries(raw.wts)) {
      if (!isRecord(p) || !isFinite2(p.ang) || !isFinite2(p.dist))
        bad(`the placement for worktree ${key} is malformed`);
      wts[key] = { ang: p.ang, dist: p.dist };
    }
  }
  return { repos, wts };
}

/* ------------------------------ seed → scene ----------------------------- */

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

/**
 * The opening scene: everything the seed declares, drawn without a shell —
 * or null when it declares nothing, so the story opens on its first
 * timeline step. Relations alone still open the scene: a relation between
 * repos the timeline creates later renders once both ends exist.
 */
export function seedOpeningStep(
  world: World,
  placements: Placements,
): StepDef | null {
  if (!world.repos.length && !world.rels.length) return null;
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

/* ------------------------------- placements ------------------------------ */

/*
 * Placement writes are pure (document in, document out) and whole-value:
 * the clone is the next document, and its placements are rewritten in
 * place before it leaves — the same thing `setPlacements` does, spelled
 * where the schema is known.
 */

function clone(doc: ComposerDoc): ComposerDoc {
  return structuredClone(doc);
}

/** Pin a repo at a world position; `null` unpins it. */
export function setRepoPlacement(
  doc: ComposerDoc,
  name: string,
  p: RepoPlacement | null,
): ComposerDoc {
  const next = clone(doc);
  const repos = placementsOf(next).repos;
  if (p) repos[name] = { x: p.x, y: p.y };
  else delete repos[name];
  return next;
}

/** Pin a worktree's polar slot (key `"repo:branch"`); `null` unpins it. */
export function setWtPlacement(
  doc: ComposerDoc,
  key: string,
  p: WtPlacement | null,
): ComposerDoc {
  const next = clone(doc);
  const wts = placementsOf(next).wts;
  if (p) wts[key] = { ang: p.ang, dist: p.dist };
  else delete wts[key];
  return next;
}

/**
 * Pin currently-derived geometry into the document without overriding pins
 * the author already made (existing entries win). Used before mutations
 * that would otherwise shuffle hash-derived positions — renames, sibling
 * inserts — so the scene the author sees is the scene that persists.
 */
export function freezePlacements(
  doc: ComposerDoc,
  derived: Placements,
): ComposerDoc {
  const next = clone(doc);
  const cur = placementsOf(next);
  for (const [name, p] of Object.entries(derived.repos))
    if (!cur.repos[name]) cur.repos[name] = { ...p };
  for (const [key, p] of Object.entries(derived.wts))
    if (!cur.wts[key]) cur.wts[key] = { ...p };
  return next;
}

/* ----------------------------- seed mutations ----------------------------- */

export function addSeedRepo(doc: ComposerDoc, repo: SeedRepo): ComposerDoc {
  if (seedOf(doc).repos.some((r) => r.name === repo.name)) return doc;
  const next = clone(doc);
  seedOf(next).repos.push(structuredClone(repo));
  return next;
}

export function removeSeedRepo(doc: ComposerDoc, name: string): ComposerDoc {
  const next = clone(doc);
  const seed = seedOf(next);
  const placements = placementsOf(next);
  seed.repos = seed.repos.filter((r) => r.name !== name);
  seed.rels = seed.rels.filter(([a, b]) => a !== name && b !== name);
  delete placements.repos[name];
  for (const key of Object.keys(placements.wts))
    if (key.startsWith(`${name}:`)) delete placements.wts[key];
  return next;
}

export function addSeedWt(
  doc: ComposerDoc,
  repoName: string,
  wt: SeedWt,
): ComposerDoc {
  const repo = seedOf(doc).repos.find((r) => r.name === repoName);
  if (!repo || repo.wts.some((w) => w.branch === wt.branch)) return doc;
  const next = clone(doc);
  seedOf(next)
    .repos.find((r) => r.name === repoName)
    ?.wts.push(structuredClone(wt));
  return next;
}

export function removeSeedWt(
  doc: ComposerDoc,
  repoName: string,
  branch: string,
): ComposerDoc {
  const next = clone(doc);
  const repo = seedOf(next).repos.find((r) => r.name === repoName);
  if (!repo) return doc;
  repo.wts = repo.wts.filter((w) => w.branch !== branch);
  delete placementsOf(next).wts[`${repoName}:${branch}`];
  return next;
}

export function updateSeedWt(
  doc: ComposerDoc,
  repoName: string,
  branch: string,
  patch: Partial<Omit<SeedWt, "branch">>,
): ComposerDoc {
  const next = clone(doc);
  const wt = seedOf(next)
    .repos.find((r) => r.name === repoName)
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
  const has = seedOf(doc).rels.some(
    ([x, y]) => (x === a && y === b) || (x === b && y === a),
  );
  if (has === on) return doc;
  const next = clone(doc);
  const seed = seedOf(next);
  if (on) seed.rels.push([a, b]);
  else
    seed.rels = seed.rels.filter(
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
  const seed = seedOf(next);
  const placements = placementsOf(next);

  if (kind === "repo") {
    for (const repo of seed.repos) if (repo.name === from) repo.name = to;
    seed.rels = seed.rels.map(([a, b]) => [
      a === from ? to : a,
      b === from ? to : b,
    ]);
    const rp = placements.repos[from];
    if (rp) {
      delete placements.repos[from];
      placements.repos[to] = rp;
    }
  } else {
    for (const repo of seed.repos)
      for (const wt of repo.wts) if (wt.branch === from) wt.branch = to;
  }

  for (const [key, p] of Object.entries(placements.wts)) {
    const renamed = renameValue(key, kind, from, to) as string;
    if (renamed !== key) {
      delete placements.wts[key];
      placements.wts[renamed] = p;
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

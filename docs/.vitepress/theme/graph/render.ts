/**
 * daft diagram renderer — scene state and canvas drawing.
 *
 * The engine (./engine.ts) owns time: it compiles scripts onto one absolute
 * timeline and runs the headless player. This module owns space: the
 * event-sourced Scene, the acts that mutate it, and the pure draw of any
 * moment onto a canvas. Everything here is a function of (events ≤ t, t) —
 * no timers, no randomness; angles and phases derive from label hashes —
 * which is what lets the live viewer and an offline exporter produce
 * identical pixels from the same compiled script.
 *
 * Node positions live in world space and go through the camera; node sizes,
 * labels, and badges are screen-space constants so text stays legible at
 * every zoom. The design language itself (what gold, teal, purple, hollow,
 * and the bot mean) is codified in ./CLAUDE.md — change that file and this
 * one together.
 */

import {
  type CamKey,
  camRectAt,
  createSceneCursor as coreSceneCursor,
  ease,
  makeView,
  type SceneCursor as SceneCursorOf,
  type SceneEvent,
} from "@avihut/dumbshow";

// The generic scaffolding lives in the dumbshow package; the camera and view
// math stay reachable from here so every daft-side consumer has one import
// spot (the UI-test helpers import them through this module via /@fs).
export { camRectAt, makeView, type View } from "@avihut/dumbshow";

/* --------------------------------- acts ---------------------------------- */

/**
 * The daft act vocabulary — the atoms of the diagram language. The engine
 * schedules acts opaquely (it reads nothing beyond `kind`); every payload
 * shape here belongs to this module's applyAct and drawScene. New meaning =
 * a new kind here + a rendering rule + a CLAUDE.md entry, together.
 */
export type Act =
  | { kind: "repo"; repo: string; x: number; y: number }
  | { kind: "wt"; repo: string; wt: string; ang?: number; dist?: number }
  | { kind: "boot"; repo: string; wt: string; secs: number }
  | { kind: "port"; repo: string; wt: string; port: string }
  | { kind: "agent"; repo: string; wt: string }
  | { kind: "agent-leave"; repo: string; wt: string }
  | { kind: "relate"; a: string; b: string }
  | { kind: "unrelate"; a: string; b: string }
  | { kind: "arc"; a: [string, string]; b: [string, string] }
  | { kind: "carry"; a: [string, string]; b: [string, string]; copy?: boolean }
  | { kind: "rename"; repo: string; wt?: string; to: string }
  | { kind: "merged"; repo: string; wt: string }
  | { kind: "remove"; repo: string; wt: string }
  | { kind: "repo-remove"; repo: string }
  | { kind: "sync"; repo: string };

/* ------------------------------ scene state ------------------------------ */

export interface WtState {
  label: string;
  ang: number;
  dist: number;
  birth: number;
  ph: number;
  boot?: { at: number; secs: number };
  port?: { at: number; text: string };
  agent?: { at: number; left?: number };
  merged?: number;
  removed?: number;
  /** Set by a rename act — the label it cross-fades from. */
  prev?: { label: string; at: number };
}

export interface RepoState {
  label: string;
  x: number;
  y: number;
  birth: number;
  ph: number;
  baseAng: number;
  wts: WtState[];
  /** Set by a rename act — the label (and disc width) it morphs from. */
  prev?: { label: string; at: number };
  /** Set by a repo-remove act — the whole orbit collapses from here. */
  removed?: number;
}

export interface RelState {
  a: string;
  b: string;
  birth: number;
  /** Set by unrelate/repo-remove — the line retracts and dies in rust. */
  removed?: number;
}

export interface ArcState {
  a: [string, string];
  b: [string, string];
  birth: number;
  /** Stamped when an endpoint dies — the arc fades on the removal curve. */
  removed?: number;
}

export interface CarryState {
  a: [string, string];
  b: [string, string];
  birth: number;
  copy: boolean;
}

export interface SyncState {
  repo: string;
  at: number;
}

export interface Scene {
  repos: RepoState[];
  rels: RelState[];
  arcs: ArcState[];
  carries: CarryState[];
  syncs: SyncState[];
}

export function createScene(): Scene {
  return { repos: [], rels: [], arcs: [], carries: [], syncs: [] };
}

function hash(s: string): number {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = (h * 33) ^ s.charCodeAt(i);
  return h >>> 0;
}

const TAU = Math.PI * 2;
const GOLDEN_ANGLE = 2.39996;

function findRepo(scene: Scene, label: string): RepoState | undefined {
  return scene.repos.find((r) => r.label === label);
}

function findWt(scene: Scene, repo: string, wt: string): WtState | undefined {
  return findRepo(scene, repo)?.wts.find((w) => w.label === wt && !w.removed);
}

export function applyAct(scene: Scene, act: Act, at: number): void {
  switch (act.kind) {
    case "repo":
      scene.repos.push({
        label: act.repo,
        x: act.x,
        y: act.y,
        birth: at,
        ph: (hash(act.repo) % 628) / 100,
        baseAng: ((hash(act.repo) % 360) * TAU) / 360,
        wts: [],
      });
      break;
    case "wt": {
      const repo = findRepo(scene, act.repo);
      if (!repo) return;
      const h = hash(act.repo + act.wt);
      // Author-pinned polar placement rides on the act; everything else
      // keeps the deterministic hash-derived slot.
      repo.wts.push({
        label: act.wt,
        ang: act.ang ?? repo.baseAng + repo.wts.length * GOLDEN_ANGLE,
        dist: act.dist ?? 76 + (h % 3) * 12,
        birth: at,
        ph: (h % 628) / 100,
      });
      break;
    }
    case "boot": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt) wt.boot = { at, secs: act.secs };
      break;
    }
    case "port": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt) wt.port = { at, text: act.port };
      break;
    }
    case "agent": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt) wt.agent = { at };
      break;
    }
    case "agent-leave": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt?.agent && wt.agent.left === undefined) wt.agent.left = at;
      break;
    }
    case "relate":
      scene.rels.push({ a: act.a, b: act.b, birth: at });
      break;
    case "unrelate":
      for (const rel of scene.rels) {
        const match =
          (rel.a === act.a && rel.b === act.b) ||
          (rel.a === act.b && rel.b === act.a);
        if (match && rel.removed === undefined) rel.removed = at;
      }
      break;
    case "arc":
      scene.arcs.push({ a: act.a, b: act.b, birth: at });
      break;
    case "carry":
      scene.carries.push({
        a: act.a,
        b: act.b,
        birth: at,
        copy: act.copy === true,
      });
      break;
    case "rename": {
      // Labels are identity here, so a rename rewrites every reference the
      // scene holds — otherwise relations, arcs, and carries would detach.
      const repo = findRepo(scene, act.repo);
      if (!repo) return;
      if (act.wt !== undefined) {
        const wt = repo.wts.find((w) => w.label === act.wt && !w.removed);
        if (!wt) return;
        wt.prev = { label: wt.label, at };
        wt.label = act.to;
        for (const arc of scene.arcs) {
          if (arc.a[0] === act.repo && arc.a[1] === act.wt)
            arc.a = [act.repo, act.to];
          if (arc.b[0] === act.repo && arc.b[1] === act.wt)
            arc.b = [act.repo, act.to];
        }
        for (const c of scene.carries) {
          if (c.a[0] === act.repo && c.a[1] === act.wt)
            c.a = [act.repo, act.to];
          if (c.b[0] === act.repo && c.b[1] === act.wt)
            c.b = [act.repo, act.to];
        }
      } else {
        repo.prev = { label: repo.label, at };
        repo.label = act.to;
        for (const rel of scene.rels) {
          if (rel.a === act.repo) rel.a = act.to;
          if (rel.b === act.repo) rel.b = act.to;
        }
        for (const arc of scene.arcs) {
          if (arc.a[0] === act.repo) arc.a = [act.to, arc.a[1]];
          if (arc.b[0] === act.repo) arc.b = [act.to, arc.b[1]];
        }
        for (const c of scene.carries) {
          if (c.a[0] === act.repo) c.a = [act.to, c.a[1]];
          if (c.b[0] === act.repo) c.b = [act.to, c.b[1]];
        }
        for (const sy of scene.syncs) {
          if (sy.repo === act.repo) sy.repo = act.to;
        }
      }
      break;
    }
    case "merged": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt) wt.merged = at;
      break;
    }
    case "remove": {
      const wt = findWt(scene, act.repo, act.wt);
      if (!wt) return;
      wt.removed = at;
      // Arcs touching the dying worktree fade out on the same curve
      // instead of popping the moment their endpoint disappears.
      for (const arc of scene.arcs) {
        const touches =
          (arc.a[0] === act.repo && arc.a[1] === act.wt) ||
          (arc.b[0] === act.repo && arc.b[1] === act.wt);
        if (touches && arc.removed === undefined) arc.removed = at;
      }
      break;
    }
    case "repo-remove": {
      const repo = findRepo(scene, act.repo);
      if (!repo || repo.removed !== undefined) return;
      repo.removed = at;
      // Live worktrees tear down staggered so their rust rings run free;
      // relations and arcs touching the repo fade on the same curve.
      let i = 0;
      for (const wt of repo.wts) {
        if (wt.removed === undefined) {
          wt.removed = at + i * 0.12;
          i++;
        }
      }
      for (const rel of scene.rels) {
        if (
          (rel.a === act.repo || rel.b === act.repo) &&
          rel.removed === undefined
        )
          rel.removed = at;
      }
      for (const arc of scene.arcs) {
        if (
          (arc.a[0] === act.repo || arc.b[0] === act.repo) &&
          arc.removed === undefined
        )
          arc.removed = at;
      }
      break;
    }
    case "sync":
      scene.syncs.push({ repo: act.repo, at });
      break;
  }
}

/** The core replay cursor bound to the daft scene. */
export type SceneCursor = SceneCursorOf<Scene>;

export function createSceneCursor(events: SceneEvent<Act>[]): SceneCursor {
  return coreSceneCursor(events, { createScene, applyAct });
}

/* -------------------------------- palette -------------------------------- */

export interface Palette {
  ink: string;
  muted: string;
  faint: string;
  gold: string;
  halo: string;
}

const TEAL = "#1b9aaa";
const RUST = "#c75c1e";
const AGENT = "#8a63d2";
const AGENT_GRAD_A = "#9d74e8";
const AGENT_GRAD_B = "#6a48c4";
const MONO = "ui-monospace, 'SF Mono', Menlo, Consolas, monospace";
/** Quantized micro-offsets — agents move their node in mechanical steps. */
const MECH: [number, number][] = [
  [0, 0],
  [1.3, -0.9],
  [-0.9, 1.1],
  [0.9, 0.9],
];

/** Read the live theme tokens — call again when the theme class flips. */
export function readPalette(): Palette {
  const style = getComputedStyle(document.documentElement);
  const token = (name: string) => style.getPropertyValue(name).trim();
  return {
    ink: token("--vp-c-text-1"),
    muted: token("--vp-c-text-2"),
    faint: token("--vp-c-text-3"),
    gold: token("--daft-gold"),
    halo: token("--vp-c-bg-soft"),
  };
}

/* --------------------------------- draw ---------------------------------- */

/**
 * Pickable geometry recorded by the draw pass itself — same measured
 * radii, same screen positions, no second layout path to drift.
 */
export type Hit =
  | { kind: "repo"; repo: string; sx: number; sy: number; r: number }
  | {
      kind: "wt";
      repo: string;
      wt: string;
      sx: number;
      sy: number;
      r: number;
    }
  | {
      /** A live relation, as the screen segment a → b. */
      kind: "rel";
      a: string;
      b: string;
      x1: number;
      y1: number;
      x2: number;
      y2: number;
    };

/** Distance from a point to a segment. */
function segmentDistance(
  px: number,
  py: number,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
): number {
  const dx = x2 - x1;
  const dy = y2 - y1;
  const len2 = dx * dx + dy * dy;
  const u =
    len2 === 0
      ? 0
      : Math.min(Math.max(((px - x1) * dx + (py - y1) * dy) / len2, 0), 1);
  return Math.hypot(px - (x1 + u * dx), py - (y1 + u * dy));
}

/** Topmost hit at a screen point: worktrees win over their repo discs,
 * discs over relation lines (within 6px of the segment). */
export function pick(hits: Hit[], sx: number, sy: number): Hit | null {
  for (const h of hits) {
    if (h.kind === "wt" && Math.hypot(sx - h.sx, sy - h.sy) <= h.r + 5)
      return h;
  }
  for (const h of hits) {
    if (h.kind === "repo" && Math.hypot(sx - h.sx, sy - h.sy) <= h.r) return h;
  }
  for (const h of hits) {
    if (
      h.kind === "rel" &&
      segmentDistance(sx, sy, h.x1, h.y1, h.x2, h.y2) <= 6
    )
      return h;
  }
  return null;
}

export interface DrawSceneOptions {
  scene: Scene;
  t: number;
  width: number;
  height: number;
  palette: Palette;
  cams: CamKey[];
  reduced: boolean;
  /** When provided, the draw pass records pickable geometry into it. */
  hits?: Hit[];
}

/** Draw one moment of a scene. Pure: same scene + t + palette, same pixels. */
export function drawScene(
  ctx: CanvasRenderingContext2D,
  opts: DrawSceneOptions,
): void {
  const { scene, t, width, height, palette, cams, reduced } = opts;
  ctx.clearRect(0, 0, width, height);
  const view = makeView(camRectAt(cams, t, reduced), width, height);

  function labelText(
    x: number,
    y: number,
    text: string,
    color: string,
    alpha: number,
    align: "left" | "right",
    size = 10,
  ): void {
    ctx.font = `${size}px ${MONO}`;
    const w = ctx.measureText(text).width;
    let x0 = align === "right" ? x - w : x;
    x0 = Math.min(Math.max(4, x0), Math.max(4, width - 4 - w));
    ctx.textAlign = "left";
    ctx.globalAlpha = alpha;
    ctx.lineWidth = 3;
    ctx.strokeStyle = palette.halo;
    ctx.lineJoin = "round";
    ctx.strokeText(text, x0, y);
    ctx.fillStyle = color;
    ctx.fillText(text, x0, y);
    ctx.globalAlpha = 1;
  }

  function ring(
    x: number,
    y: number,
    r: number,
    color: string,
    alpha: number,
  ): void {
    if (alpha <= 0) return;
    ctx.globalAlpha = alpha;
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    ctx.arc(x, y, r, 0, TAU);
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  /** A small robot head — the coding-agent marker, purple gradient. */
  function drawBot(x: number, y: number, s: number, alpha: number): void {
    if (s <= 0.05) return;
    const grad = ctx.createLinearGradient(x - 7, y - 10, x + 7, y + 6);
    grad.addColorStop(0, AGENT_GRAD_A);
    grad.addColorStop(1, AGENT_GRAD_B);
    ctx.globalAlpha = alpha;
    ctx.strokeStyle = grad;
    ctx.fillStyle = grad;
    ctx.lineWidth = 1.3;
    ctx.beginPath();
    ctx.moveTo(x, y - 4.5 * s);
    ctx.lineTo(x, y - 7.5 * s);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(x, y - 8.6 * s, 1.5 * s, 0, TAU);
    ctx.fill();
    ctx.beginPath();
    ctx.roundRect(x - 6.5 * s, y - 4.5 * s, 13 * s, 9 * s, 2.6 * s);
    ctx.fill();
    ctx.fillStyle = palette.halo;
    ctx.fillRect(x - 4.1 * s, y - 1.7 * s, 2.6 * s, 3.2 * s);
    ctx.fillRect(x + 1.5 * s, y - 1.7 * s, 2.6 * s, 3.2 * s);
    ctx.globalAlpha = 1;
  }

  function wtAlpha(wt: WtState): number {
    if (!wt.removed) return 1;
    return Math.max(0, 1 - (t - wt.removed) / 1.0);
  }

  function wtScreenPos(repo: RepoState, wt: WtState): [number, number, number] {
    const grown = ease((t - wt.birth) / 0.7);
    const float = reduced ? 0 : Math.sin(t * 0.7 + wt.ph) * 3;
    const wx = repo.x + Math.cos(wt.ang) * wt.dist * grown;
    const wy = repo.y + Math.sin(wt.ang) * wt.dist * grown;
    return [view.sx(wx), view.sy(wy) + float, grown];
  }

  function repoScreenPos(repo: RepoState): [number, number] {
    const float = reduced ? 0 : Math.sin(t * 0.55 + repo.ph) * 2.4;
    return [view.sx(repo.x), view.sy(repo.y) + float];
  }

  /** Screen position of a worktree endpoint — removed ones included, so
   * fading arcs and carry flights always have somewhere to end. */
  function nodePos(ref: [string, string]): [number, number] | null {
    const repo = findRepo(scene, ref[0]);
    if (!repo) return null;
    const wt = repo.wts.find((w) => w.label === ref[1]);
    if (!wt) return null;
    const p = wtScreenPos(repo, wt);
    return [p[0], p[1]];
  }

  for (const rel of scene.rels) {
    const a = findRepo(scene, rel.a);
    const b = findRepo(scene, rel.b);
    if (!a || !b) continue;
    // Unlinked: the dashed line retracts toward `a`, the crawl reverses,
    // and the connection dies in rust.
    const dying = rel.removed !== undefined && t >= rel.removed;
    if (dying && reduced) continue;
    const dp = dying ? Math.min((t - (rel.removed as number)) / 0.9, 1) : 0;
    if (dp >= 1) continue;
    const pa = repoScreenPos(a);
    const pb = repoScreenPos(b);
    if (opts.hits && rel.removed === undefined)
      opts.hits.push({
        kind: "rel",
        a: rel.a,
        b: rel.b,
        x1: pa[0],
        y1: pa[1],
        x2: pb[0],
        y2: pb[1],
      });
    const e2 = ease(dp);
    const ex = pa[0] + (pb[0] - pa[0]) * (1 - e2);
    const ey = pa[1] + (pb[1] - pa[1]) * (1 - e2);
    ctx.globalAlpha = 0.5 * ease((t - rel.birth) / 0.9) * (1 - dp * 0.6);
    ctx.strokeStyle = dying && dp >= 0.45 ? RUST : TEAL;
    ctx.lineWidth = 1.4;
    ctx.setLineDash([5, 6]);
    ctx.lineDashOffset = reduced ? 0 : dying ? t * 9 : -t * 9;
    ctx.beginPath();
    ctx.moveTo(pa[0], pa[1]);
    ctx.lineTo(ex, ey);
    ctx.stroke();
    ctx.setLineDash([]);
    ctx.globalAlpha = 1;
  }

  for (const arc of scene.arcs) {
    const dying = arc.removed !== undefined && t >= arc.removed;
    if (dying && reduced) continue;
    const dp = dying ? Math.min((t - (arc.removed as number)) / 0.9, 1) : 0;
    if (dp >= 1) continue;
    const pa = nodePos(arc.a);
    const pb = nodePos(arc.b);
    if (!pa || !pb) continue;
    const merged = findWt(scene, arc.a[0], arc.a[1])?.merged;
    const alpha =
      (merged ? 0.22 : 0.45) * ease((t - arc.birth) / 0.9) * (1 - dp);
    const mx = (pa[0] + pb[0]) / 2;
    const my = (pa[1] + pb[1]) / 2;
    const dx = pb[0] - pa[0];
    const dy = pb[1] - pa[1];
    const len = Math.hypot(dx, dy) || 1;
    ctx.globalAlpha = alpha;
    ctx.strokeStyle = palette.gold;
    ctx.lineWidth = 1.3;
    ctx.beginPath();
    ctx.moveTo(pa[0], pa[1]);
    ctx.quadraticCurveTo(
      mx - (dy / len) * 26,
      my + (dx / len) * 26,
      pb[0],
      pb[1],
    );
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  // Carry flights: a gold payload dot on a bowed curve between worktrees —
  // uncommitted changes moving home. On a move (not --copy) the source node
  // dims while the payload is in the air.
  const carryDimmed = new Set<string>();
  if (!reduced) {
    for (const c of scene.carries) {
      const m = (t - c.birth) / 0.8;
      if (m < 0 || m >= 1) continue;
      if (!c.copy) carryDimmed.add(`${c.a[0]}:${c.a[1]}`);
      const pa = nodePos(c.a);
      const pb = nodePos(c.b);
      if (!pa || !pb) continue;
      const dx = pb[0] - pa[0];
      const dy = pb[1] - pa[1];
      const len = Math.hypot(dx, dy) || 1;
      const bx = (pa[0] + pb[0]) / 2 - (dy / len) * 26;
      const by = (pa[1] + pb[1]) / 2 + (dx / len) * 26;
      const s = ease(m);
      const px =
        (1 - s) * (1 - s) * pa[0] + 2 * (1 - s) * s * bx + s * s * pb[0];
      const py =
        (1 - s) * (1 - s) * pa[1] + 2 * (1 - s) * s * by + s * s * pb[1];
      ctx.fillStyle = palette.gold;
      ctx.beginPath();
      ctx.arc(px, py, 3.4, 0, TAU);
      ctx.fill();
    }
  }

  for (const repo of scene.repos) {
    // Repo removal: the disc shrinks under a collapsing rust ring while
    // everything it anchors fades on the same curve.
    const goneP =
      repo.removed !== undefined && t >= repo.removed
        ? Math.min((t - repo.removed) / 0.9, 1)
        : 0;
    if (goneP >= 1 || (reduced && goneP > 0)) continue;
    const repoAlpha = 1 - goneP;
    const origin = repoScreenPos(repo);

    for (const wt of repo.wts) {
      const alpha = wtAlpha(wt) * repoAlpha;
      if (alpha <= 0) continue;
      const p = wtScreenPos(repo, wt);
      const merged = wt.merged !== undefined && t >= wt.merged;
      const carryDim = carryDimmed.has(`${repo.label}:${wt.label}`) ? 0.55 : 1;
      const dim = (merged ? 0.45 : 1) * carryDim;

      ctx.globalAlpha = 0.45 * alpha * dim;
      ctx.strokeStyle = palette.muted;
      ctx.lineWidth = 1.3;
      const bendX = (origin[0] + p[0]) / 2 + Math.cos(wt.ang + 1.57) * 7;
      const bendY = (origin[1] + p[1]) / 2 + Math.sin(wt.ang + 1.57) * 7;
      ctx.beginPath();
      ctx.moveTo(origin[0], origin[1]);
      ctx.quadraticCurveTo(bendX, bendY, p[0], p[1]);
      ctx.stroke();
      ctx.globalAlpha = 1;

      // Merge payload: a gold dot carries the finished work home along the
      // edge into the repo, which swallows it (and grows a beat — see the
      // repo pass). The worktree dims hollow behind it.
      if (wt.merged !== undefined && !reduced) {
        const m = (t - wt.merged) / 0.8;
        if (m >= 0 && m < 1) {
          const s = 1 - ease(m);
          const px =
            (1 - s) * (1 - s) * origin[0] +
            2 * (1 - s) * s * bendX +
            s * s * p[0];
          const py =
            (1 - s) * (1 - s) * origin[1] +
            2 * (1 - s) * s * bendY +
            s * s * p[1];
          ctx.fillStyle = palette.gold;
          ctx.beginPath();
          ctx.arc(px, py, 3.4, 0, TAU);
          ctx.fill();
        }
      }

      const isMain = wt.label === "main";
      const r = isMain ? 7 : 5.5;
      // An agent's stay has a departure: after `left`, the bot drifts off,
      // the purple blend decays, and a purple ring collapses inward over
      // ~0.8s — then the node is fully yours again.
      const leftAt = wt.agent?.left;
      const leaveP =
        leftAt !== undefined && t >= leftAt
          ? Math.min((t - leftAt) / 0.8, 1)
          : 0;
      const agentOn =
        wt.agent !== undefined &&
        t >= wt.agent.at &&
        !merged &&
        !wt.removed &&
        leaveP < 1;
      const machining = agentOn && leaveP === 0;
      // The agent "machines" its node: stepped micro-offsets, deliberately
      // mechanical against the smooth ambient float. Labels and badges stay
      // put for legibility.
      let nx = p[0];
      let ny = p[1];
      if (machining && !reduced) {
        const mech = MECH[Math.floor(t * 3 + wt.ph) % MECH.length];
        nx += mech[0];
        ny += mech[1];
      }
      if (opts.hits && wt.removed === undefined)
        opts.hits.push({
          kind: "wt",
          repo: repo.label,
          wt: wt.label,
          sx: nx,
          sy: ny,
          r,
        });
      ctx.globalAlpha = alpha * carryDim;
      if (merged) {
        ctx.fillStyle = palette.halo;
        ctx.beginPath();
        ctx.arc(nx, ny, r, 0, TAU);
        ctx.fill();
        ctx.strokeStyle = palette.faint;
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(nx, ny, r, 0, TAU);
        ctx.stroke();
      } else {
        ctx.fillStyle = isMain ? palette.gold : palette.ink;
        ctx.beginPath();
        ctx.arc(nx, ny, r, 0, TAU);
        ctx.fill();
        if (isMain) {
          ctx.strokeStyle = palette.ink;
          ctx.lineWidth = 1.4;
          ctx.beginPath();
          ctx.arc(nx, ny, r, 0, TAU);
          ctx.stroke();
        }
      }
      if (agentOn) {
        // The node blends toward the agent purple while it's being worked;
        // the blend decays as the agent leaves.
        const blend = reduced
          ? 0.3
          : 0.26 + 0.12 * (Math.floor(t * 2 + wt.ph) % 2);
        ctx.globalAlpha = alpha * blend * (1 - leaveP);
        ctx.fillStyle = AGENT;
        ctx.beginPath();
        ctx.arc(nx, ny, r, 0, TAU);
        ctx.fill();
      }
      ctx.globalAlpha = 1;

      // Rename: the old label rises out, the new one settles in, and a gold
      // attention ring marks the moment.
      const lx = p[0] + Math.cos(wt.ang) * 12;
      const ly = p[1] + Math.sin(wt.ang) * 12 + 3;
      const la = 0.95 * alpha * dim * p[2];
      const lalign = Math.cos(wt.ang) < -0.3 ? "right" : "left";
      const wrn = wt.prev && !reduced ? Math.min((t - wt.prev.at) / 0.7, 1) : 1;
      if (wt.prev && wrn < 1) {
        const e2 = ease(wrn);
        labelText(
          lx,
          ly - 3 * e2,
          wt.prev.label,
          palette.faint,
          la * (1 - e2),
          lalign,
        );
        labelText(
          lx,
          ly + 3 * (1 - e2),
          wt.label,
          palette.faint,
          la * e2,
          lalign,
        );
      } else {
        labelText(lx, ly, wt.label, palette.faint, la, lalign);
      }
      if (!reduced && wt.prev) {
        const age = (t - wt.prev.at) / 0.9;
        if (age >= 0 && age < 1)
          ring(p[0], p[1], 8 + age * 16, palette.gold, (1 - age) * 0.7);
      }

      // Setup ring: every worktree is set up as it's created — a dashed
      // teal circle spins around the node while hooks run. Script `boot`
      // acts extend the window for emphasis; 1.1s is the default. The
      // setup commands themselves print only in the paired terminal.
      const su = wt.boot ?? { at: wt.birth, secs: 1.1 };
      if (!wt.removed && t >= su.at && t <= su.at + su.secs + 0.3) {
        const fadeIn = Math.min(1, (t - su.at) / 0.25);
        const fadeOut = Math.min(1, (su.at + su.secs + 0.3 - t) / 0.3);
        ctx.globalAlpha = alpha * 0.9 * Math.min(fadeIn, fadeOut);
        ctx.strokeStyle = TEAL;
        ctx.lineWidth = 1.5;
        ctx.setLineDash([4, 4.5]);
        ctx.lineDashOffset = reduced ? 0 : -t * 26;
        ctx.beginPath();
        ctx.arc(p[0], p[1], r + 5.5, 0, TAU);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;
      }

      // Teardown mirrors setup: the same dashed ring in rust, spinning the
      // other way, while the node dissolves under it. Merged shells lose
      // nothing — their work already traveled home at merge time.
      if (wt.removed && t >= wt.removed && t <= wt.removed + 0.9) {
        const td = (t - wt.removed) / 0.9;
        const fadeIn = Math.min(1, (t - wt.removed) / 0.15);
        ctx.globalAlpha = 0.9 * fadeIn * (1 - td);
        ctx.strokeStyle = RUST;
        ctx.lineWidth = 1.5;
        ctx.setLineDash([4, 4.5]);
        ctx.lineDashOffset = reduced ? 0 : t * 26;
        ctx.beginPath();
        ctx.arc(p[0], p[1], r + 5.5, 0, TAU);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;
      }

      if (wt.port && t >= wt.port.at) {
        const pop = ease((t - wt.port.at) / 0.45);
        ctx.font = `9px ${MONO}`;
        const tw = ctx.measureText(wt.port.text).width;
        // Centered under the node — or above it when the branch label
        // already occupies the space below.
        const bx = p[0] - (tw + 10) / 2;
        const by = Math.sin(wt.ang) > 0.45 ? p[1] - 24 : p[1] + 9;
        const ba = alpha * dim * pop;
        ctx.globalAlpha = ba;
        ctx.fillStyle = palette.halo;
        ctx.strokeStyle = palette.faint;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.roundRect(bx, by, tw + 10, 14, 7);
        ctx.fill();
        ctx.globalAlpha = ba * 0.55;
        ctx.stroke();
        ctx.globalAlpha = ba;
        ctx.fillStyle = palette.muted;
        ctx.fillText(wt.port.text, bx + 5, by + 10.5);
        ctx.globalAlpha = 1;
      }

      if (wt.agent && agentOn) {
        const grow = ease((t - wt.agent.at) / 0.5);
        const bob = reduced ? 0 : (Math.floor(t * 2 + wt.ph) % 2) * -1.4;
        // Sit opposite the port badge (which flips above downward labels).
        const drift = reduced ? 0 : leaveP * 9;
        const botY = Math.sin(wt.ang) > 0.45 ? ny + 17 : ny - 13;
        drawBot(nx + 12, botY + bob - drift, grow, alpha * (1 - leaveP));
        if (!reduced && machining) {
          const pulse = ((t - wt.agent.at) % 2.6) / 2.6;
          ring(nx, ny, r + 2 + pulse * 14, AGENT, (1 - pulse) * 0.5 * alpha);
        }
        if (!reduced && leaveP > 0) {
          // Departure: the slow pulse gives way to one collapsing ring.
          ring(nx, ny, r + 2 + (1 - leaveP) * 12, AGENT, (1 - leaveP) * 0.55);
        }
      }

      if (!reduced) {
        const born = (t - wt.birth) / 1.1;
        if (born < 1)
          ring(p[0], p[1], 9 + born * 24, palette.gold, (1 - born) * 0.8);
        if (wt.merged !== undefined) {
          const age = (t - wt.merged) / 1.1;
          if (age >= 0 && age < 1)
            ring(p[0], p[1], 8 + age * 20, palette.gold, (1 - age) * 0.6);
        }
      }
    }

    const born = ease((t - repo.birth) / 0.6);
    ctx.font = `600 10.5px ${MONO}`;
    const radiusFor = (label: string): number =>
      Math.max(21, ctx.measureText(label).width / 2 + 8);
    // A rename morphs the disc between the widths its labels measure.
    const rn =
      repo.prev && !reduced ? Math.min((t - repo.prev.at) / 0.7, 1) : 1;
    const rnE = ease(rn);
    const base =
      repo.prev && rn < 1
        ? radiusFor(repo.prev.label) * (1 - rnE) + radiusFor(repo.label) * rnE
        : radiusFor(repo.label);
    let radius = base * (born < 1 ? born * (1 + 0.25 * (1 - born)) : 1);
    radius *= 1 - ease(goneP);
    if (radius <= 0.5) continue;
    if (opts.hits && repo.removed === undefined)
      opts.hits.push({
        kind: "repo",
        repo: repo.label,
        sx: origin[0],
        sy: origin[1],
        r: radius,
      });
    // Swallowing a merge payload makes the repo grow for a beat.
    if (!reduced) {
      for (const wt of repo.wts) {
        if (wt.merged !== undefined) {
          const arr = (t - (wt.merged + 0.8)) / 0.5;
          if (arr >= 0 && arr < 1) radius *= 1 + 0.12 * Math.sin(Math.PI * arr);
        }
      }
    }
    ctx.globalAlpha = repoAlpha;
    ctx.fillStyle = palette.ink;
    ctx.beginPath();
    ctx.arc(origin[0], origin[1], radius, 0, TAU);
    ctx.fill();
    ctx.fillStyle = palette.halo;
    ctx.textAlign = "center";
    if (repo.prev && rn < 1) {
      ctx.globalAlpha = 1 - rnE;
      ctx.fillText(repo.prev.label, origin[0], origin[1] + 3.5 - 3 * rnE);
      ctx.globalAlpha = rnE;
      ctx.fillText(repo.label, origin[0], origin[1] + 3.5 + 3 * (1 - rnE));
      ctx.globalAlpha = 1;
    } else {
      ctx.fillText(repo.label, origin[0], origin[1] + 3.5);
    }
    ctx.globalAlpha = 1;
    ctx.textAlign = "left";
    if (!reduced && goneP > 0) {
      // Teardown at repo scale: one rust ring collapsing inward.
      ring(
        origin[0],
        origin[1],
        radius + 4 + (1 - ease(goneP)) * 18,
        RUST,
        (1 - goneP) * 0.7,
      );
    }
    if (!reduced) {
      const age = (t - repo.birth) / 1.2;
      if (age < 1)
        ring(
          origin[0],
          origin[1],
          radius + 4 + age * 26,
          palette.gold,
          (1 - age) * 0.8,
        );
      if (repo.prev) {
        const rage = (t - repo.prev.at) / 0.9;
        if (rage >= 0 && rage < 1)
          ring(
            origin[0],
            origin[1],
            radius + 4 + rage * 20,
            palette.gold,
            (1 - rage) * 0.7,
          );
      }
      // Merge payloads land: the repo acknowledges with a gold pulse.
      for (const wt of repo.wts) {
        if (wt.merged !== undefined) {
          const landed = (t - wt.merged - 0.8) / 0.9;
          if (landed >= 0 && landed < 1)
            ring(
              origin[0],
              origin[1],
              radius + 3 + landed * 20,
              palette.gold,
              (1 - landed) * 0.7,
            );
        }
      }
      // Sync signal: the repo pings each worktree along its edge — base
      // updated, rebase available.
      for (const sy of scene.syncs) {
        if (sy.repo !== repo.label) continue;
        repo.wts.forEach((wt, wi) => {
          if (wt.removed !== undefined && t > wt.removed + 0.2) return;
          const m = (t - (sy.at + wi * 0.12)) / 0.7;
          if (m < 0 || m >= 1.4) return;
          const pw = wtScreenPos(repo, wt);
          if (m < 1) {
            const s = ease(m);
            const bx = (origin[0] + pw[0]) / 2 + Math.cos(wt.ang + 1.57) * 7;
            const by = (origin[1] + pw[1]) / 2 + Math.sin(wt.ang + 1.57) * 7;
            const qx =
              (1 - s) * (1 - s) * origin[0] +
              2 * (1 - s) * s * bx +
              s * s * pw[0];
            const qy =
              (1 - s) * (1 - s) * origin[1] +
              2 * (1 - s) * s * by +
              s * s * pw[1];
            ctx.fillStyle = TEAL;
            ctx.beginPath();
            ctx.arc(qx, qy, 2.4, 0, TAU);
            ctx.fill();
          } else {
            const blink = (m - 1) / 0.4;
            ring(pw[0], pw[1], 6 + blink * 9, TEAL, (1 - blink) * 0.6);
          }
        });
      }
    }
  }
}

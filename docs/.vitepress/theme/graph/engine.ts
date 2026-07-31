/**
 * daft diagram engine — the repo-graph visual language, animated.
 *
 * A diagram is a script of steps; each step is a list of beats (terminal
 * lines, scene acts, camera moves). `compile` flattens the script onto one
 * absolute timeline, and the player renders any moment of it from scratch:
 * state is event-sourced, so pausing, stepping, and seeking backwards are
 * all just "set the clock and replay". Nothing in here is random — angles
 * and phases derive from label hashes — so every replay is identical.
 *
 * Node positions live in world space and go through the camera; node sizes,
 * labels, and badges are screen-space constants so text stays legible at
 * every zoom. The design language itself (what gold, teal, purple, hollow,
 * and the bot mean) is codified in ./CLAUDE.md — change that file and this
 * one together.
 */

export interface CamRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Act =
  | { kind: "repo"; repo: string; x: number; y: number }
  | { kind: "wt"; repo: string; wt: string }
  | { kind: "boot"; repo: string; wt: string; secs: number }
  | { kind: "port"; repo: string; wt: string; port: string }
  | { kind: "agent"; repo: string; wt: string }
  | { kind: "relate"; a: string; b: string }
  | { kind: "arc"; a: [string, string]; b: [string, string] }
  | { kind: "merged"; repo: string; wt: string }
  | { kind: "remove"; repo: string; wt: string }
  | { kind: "sync"; repo: string };

/**
 * Output-line tones mirror the diagram's color law: `ok` teal = setup and
 * checks, `agent` purple = AI agents, `rust` = destructive operations,
 * `dim` = commentary. Commands render their `daft` verb in gold.
 */
export type TermTone = "ok" | "dim" | "agent" | "rust";

export type Beat =
  | { cmd: string }
  | { out: string; tone?: TermTone }
  | { act: Act }
  | { pause: number }
  | { cam: CamRect };

export interface StepDef {
  title: string;
  cam: CamRect;
  beats: Beat[];
}

export interface TermLine {
  at: number;
  /** For cmd lines, when typing finishes; for output lines, equal to `at`. */
  typed: number;
  kind: "cmd" | "out" | TermTone;
  text: string;
  /** Index of the step this line belongs to. */
  step: number;
  /** True on the first command of a step — the clickable checkpoint. */
  checkpoint: boolean;
}

interface SceneEvent {
  at: number;
  act: Act;
}

interface CamKey {
  at: number;
  rect: CamRect;
}

export interface CompiledStep {
  at: number;
  end: number;
  /** Checkpoint pause point: just after the step's first command is typed. */
  cue: number;
  title: string;
}

export interface Compiled {
  duration: number;
  steps: CompiledStep[];
  term: TermLine[];
  events: SceneEvent[];
  cams: CamKey[];
}

const TYPE_SECS_PER_CHAR = 0.024;
const CMD_LEAD = 0.45;
const CMD_TAIL = 0.4;
const OUT_GAP = 0.55;
const STEP_TAIL = 1.5;
const CAM_TWEEN = 1.6;

export function compile(steps: StepDef[]): Compiled {
  const term: TermLine[] = [];
  const events: SceneEvent[] = [];
  const cams: CamKey[] = [];
  const outSteps: CompiledStep[] = [];
  let t = 0.6;

  steps.forEach((step, si) => {
    const at = t;
    let cue = at;
    let firstCmd = true;
    cams.push({ at, rect: step.cam });
    for (const beat of step.beats) {
      if ("cmd" in beat) {
        const dur = CMD_LEAD + beat.cmd.length * TYPE_SECS_PER_CHAR;
        term.push({
          at: t,
          typed: t + dur,
          kind: "cmd",
          text: beat.cmd,
          step: si,
          checkpoint: firstCmd,
        });
        if (firstCmd) cue = t + dur;
        firstCmd = false;
        t += dur + CMD_TAIL;
      } else if ("out" in beat) {
        const kind = beat.tone ?? "out";
        term.push({
          at: t,
          typed: t,
          kind,
          text: beat.out,
          step: si,
          checkpoint: false,
        });
        t += OUT_GAP;
      } else if ("act" in beat) {
        events.push({ at: t, act: beat.act });
      } else if ("cam" in beat) {
        cams.push({ at: t, rect: beat.cam });
      } else {
        t += beat.pause;
      }
    }
    t += STEP_TAIL;
    outSteps.push({ at, end: t, cue, title: step.title });
  });

  return { duration: t + 0.8, steps: outSteps, term, events, cams };
}

/* ------------------------------ scene state ------------------------------ */

interface WtState {
  label: string;
  ang: number;
  dist: number;
  birth: number;
  ph: number;
  boot?: { at: number; secs: number };
  port?: { at: number; text: string };
  agent?: { at: number };
  merged?: number;
  removed?: number;
}

interface RepoState {
  label: string;
  x: number;
  y: number;
  birth: number;
  ph: number;
  baseAng: number;
  wts: WtState[];
}

interface RelState {
  a: string;
  b: string;
  birth: number;
}

interface ArcState {
  a: [string, string];
  b: [string, string];
  birth: number;
}

interface SyncState {
  repo: string;
  at: number;
}

interface Scene {
  repos: RepoState[];
  rels: RelState[];
  arcs: ArcState[];
  syncs: SyncState[];
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

function applyAct(scene: Scene, act: Act, at: number): void {
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
      repo.wts.push({
        label: act.wt,
        ang: repo.baseAng + repo.wts.length * GOLDEN_ANGLE,
        dist: 76 + (h % 3) * 12,
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
    case "relate":
      scene.rels.push({ a: act.a, b: act.b, birth: at });
      break;
    case "arc":
      scene.arcs.push({ a: act.a, b: act.b, birth: at });
      break;
    case "merged": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt) wt.merged = at;
      break;
    }
    case "remove": {
      const wt = findWt(scene, act.repo, act.wt);
      if (wt) wt.removed = at;
      break;
    }
    case "sync":
      scene.syncs.push({ repo: act.repo, at });
      break;
  }
}

/* -------------------------------- palette -------------------------------- */

interface Palette {
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

function readPalette(): Palette {
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

/* -------------------------------- player --------------------------------- */

export interface PlayerOptions {
  script: StepDef[];
  /** Ignored (forced off) when the user prefers reduced motion. */
  autoplay?: boolean;
  /** Wrap to the start when the timeline ends (default). Off = end paused. */
  loop?: boolean;
}

/**
 * The headless timeline player: one clock per diagram, no rendering. Viewers
 * (the canvas diagram, the terminal) subscribe via `onFrame` and derive their
 * entire presentation from the compiled script plus the clock value — which
 * is what keeps any number of viewers in perfect sync, and what makes seeks
 * in either direction safe: a viewer that sees time move backward replays
 * its state from scratch.
 */
export interface Player {
  compiled: Compiled;
  reducedMotion: boolean;
  playing(): boolean;
  clock(): number;
  play(): void;
  pause(): void;
  toggle(): void;
  /** Jump to an absolute time (seconds); play state is left untouched. */
  seek(t: number): void;
  seekStep(index: number): void;
  /**
   * Jump to a step's checkpoint — its first command just typed — and pause,
   * so play resumes by executing that command.
   */
  seekCheckpoint(index: number): void;
  /** Jump to the settled end state of a step without animating into it. */
  settle(index: number): void;
  next(): void;
  prev(): void;
  current(): number;
  setRate(rate: number): void;
  setLoop(on: boolean): void;
  /**
   * Visibility gating, owned by whoever created the player: `suspend` stops
   * the frame loop without touching play state, `resume` picks it back up.
   */
  suspend(): void;
  resume(): void;
  onFrame(fn: (t: number) => void): () => void;
  onStep(fn: (index: number) => void): () => void;
  onPlayState(fn: (playing: boolean) => void): () => void;
  destroy(): void;
}

function ease(x: number): number {
  return 1 - (1 - Math.min(Math.max(x, 0), 1)) ** 3;
}

export function createPlayer(opts: PlayerOptions): Player {
  const compiled = compile(opts.script);
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  let clock = 0;
  let playing = false;
  let suspended = false;
  let raf = 0;
  let lastFrame = 0;
  let stepIdx = -1;
  let rate = 1;
  let loop = opts.loop !== false;

  const frameSubs = new Set<(t: number) => void>();
  const stepSubs = new Set<(index: number) => void>();
  const playSubs = new Set<(playing: boolean) => void>();

  function emitFrame(): void {
    for (const fn of frameSubs) fn(clock);
  }

  function currentStep(): number {
    let i = 0;
    for (let k = 0; k < compiled.steps.length; k++) {
      if (compiled.steps[k].at <= clock) i = k;
    }
    return i;
  }

  function notifyStep(): void {
    const i = currentStep();
    if (i !== stepIdx) {
      stepIdx = i;
      for (const fn of stepSubs) fn(i);
    }
  }

  function schedule(): void {
    if (playing && !suspended && !raf) {
      lastFrame = performance.now();
      raf = requestAnimationFrame(frame);
    }
  }

  function setPlaying(on: boolean): void {
    if (playing === on) return;
    playing = on;
    for (const fn of playSubs) fn(on);
    schedule();
  }

  function frame(now: number): void {
    raf = 0;
    if (!playing || suspended) return;
    const dt = Math.min((now - lastFrame) / 1000, 0.05) * rate;
    lastFrame = now;
    clock += dt;
    if (clock > compiled.duration) {
      if (loop) {
        clock = 0; // viewers see time move backward and replay from scratch
      } else {
        clock = compiled.duration;
        setPlaying(false);
      }
    }
    notifyStep();
    emitFrame();
    schedule();
  }

  function setClock(t: number): void {
    clock = Math.min(Math.max(t, 0), compiled.duration);
    notifyStep();
    emitFrame();
  }

  function stepAt(index: number): CompiledStep {
    return compiled.steps[
      Math.min(Math.max(index, 0), compiled.steps.length - 1)
    ];
  }

  function settle(index: number): void {
    setClock(stepAt(index).end - 0.05);
  }

  function seekStep(index: number): void {
    // Reduced motion lands on the settled end of the step; otherwise the
    // step replays animated from its start.
    if (reduced) {
      settle(index);
      return;
    }
    setClock(stepAt(index).at);
  }

  function seekCheckpoint(index: number): void {
    setPlaying(false);
    if (reduced) {
      settle(index);
      return;
    }
    const step = stepAt(index);
    setClock(Math.min(step.cue + 0.02, step.end - 0.05));
  }

  const player: Player = {
    compiled,
    reducedMotion: reduced,
    playing: () => playing,
    clock: () => clock,
    play: () => setPlaying(true),
    pause: () => setPlaying(false),
    toggle: () => setPlaying(!playing),
    seek: setClock,
    seekStep,
    seekCheckpoint,
    settle,
    next: () =>
      seekStep(Math.min(currentStep() + 1, compiled.steps.length - 1)),
    prev: () => seekStep(Math.max(currentStep() - 1, 0)),
    current: currentStep,
    setRate(value: number) {
      rate = Math.min(Math.max(value, 0.1), 4);
    },
    setLoop(on: boolean) {
      loop = on;
    },
    suspend() {
      suspended = true;
      if (raf) {
        cancelAnimationFrame(raf);
        raf = 0;
      }
    },
    resume() {
      suspended = false;
      schedule();
    },
    onFrame(fn) {
      frameSubs.add(fn);
      return () => frameSubs.delete(fn);
    },
    onStep(fn) {
      stepSubs.add(fn);
      return () => stepSubs.delete(fn);
    },
    onPlayState(fn) {
      playSubs.add(fn);
      return () => playSubs.delete(fn);
    },
    destroy() {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      frameSubs.clear();
      stepSubs.clear();
      playSubs.clear();
    },
  };

  if (reduced) seekStep(compiled.steps.length - 1);
  else if (opts.autoplay !== false) setPlaying(true);
  notifyStep();

  // Dev-only handle so the timeline can be driven from the console/tests.
  if (import.meta.env.DEV) Object.assign(window, { __daftPlayer: player });

  return player;
}

/**
 * Pause a player's frame loop while `el` is offscreen. Wire this from the
 * component that owns the player — a solo viewer, or the host composing two
 * viewers around one — and disconnect with the returned function.
 */
export function observeVisibility(el: Element, player: Player): () => void {
  const io = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (entry.isIntersecting) player.resume();
        else player.suspend();
      }
    },
    { threshold: 0.05 },
  );
  io.observe(el);
  return () => io.disconnect();
}

/* ------------------------------ canvas view ------------------------------ */

export interface DiagramView {
  destroy(): void;
}

/**
 * The diagram viewer: attaches the canvas renderer to a player. Scene state
 * is event-sourced from the compiled script — when the clock moves backward
 * (loop wrap, seek), the scene resets and replays, so any moment renders
 * identically no matter how it was reached.
 */
export function attachDiagram(
  canvas: HTMLCanvasElement,
  player: Player,
): DiagramView | null {
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const compiled = player.compiled;
  const reduced = player.reducedMotion;

  let palette = readPalette();
  let width = 0;
  let height = 0;
  let scene: Scene = { repos: [], rels: [], arcs: [], syncs: [] };
  let evIdx = 0;
  let lastT = 0;

  function resetScene(): void {
    scene = { repos: [], rels: [], arcs: [], syncs: [] };
    evIdx = 0;
  }

  function syncScene(t: number): void {
    if (t < lastT) resetScene();
    lastT = t;
    while (evIdx < compiled.events.length && compiled.events[evIdx].at <= t) {
      const ev = compiled.events[evIdx++];
      applyAct(scene, ev.act, ev.at);
    }
  }

  /* ------------------------------ rendering ------------------------------ */

  function camRect(t: number): CamRect {
    const keys = compiled.cams;
    let prev = keys[0].rect;
    let cur = keys[0];
    for (const key of keys) {
      if (key.at <= t) {
        prev = cur.rect;
        cur = key;
      }
    }
    const p = reduced ? 1 : ease((t - cur.at) / CAM_TWEEN);
    return {
      x: prev.x + (cur.rect.x - prev.x) * p,
      y: prev.y + (cur.rect.y - prev.y) * p,
      w: prev.w + (cur.rect.w - prev.w) * p,
      h: prev.h + (cur.rect.h - prev.h) * p,
    };
  }

  interface View {
    sx: (wx: number) => number;
    sy: (wy: number) => number;
  }

  function makeView(t: number): View {
    const rect = camRect(t);
    const s = Math.min(width / rect.w, height / rect.h);
    return {
      sx: (wx) => width / 2 + (wx - rect.x) * s,
      sy: (wy) => height / 2 + (wy - rect.y) * s,
    };
  }

  function labelText(
    x: number,
    y: number,
    text: string,
    color: string,
    alpha: number,
    align: "left" | "right",
    size = 10,
  ): void {
    if (!ctx) return;
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
    if (!ctx || alpha <= 0) return;
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
    if (!ctx || s <= 0.05) return;
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

  function wtAlpha(wt: WtState, t: number): number {
    if (!wt.removed) return 1;
    return Math.max(0, 1 - (t - wt.removed) / 1.0);
  }

  function wtScreenPos(
    repo: RepoState,
    wt: WtState,
    t: number,
    view: View,
  ): [number, number, number] {
    const grown = ease((t - wt.birth) / 0.7);
    const float = reduced ? 0 : Math.sin(t * 0.7 + wt.ph) * 3;
    const wx = repo.x + Math.cos(wt.ang) * wt.dist * grown;
    const wy = repo.y + Math.sin(wt.ang) * wt.dist * grown;
    return [view.sx(wx), view.sy(wy) + float, grown];
  }

  function repoScreenPos(
    repo: RepoState,
    t: number,
    view: View,
  ): [number, number] {
    const float = reduced ? 0 : Math.sin(t * 0.55 + repo.ph) * 2.4;
    return [view.sx(repo.x), view.sy(repo.y) + float];
  }

  function nodePos(
    ref: [string, string],
    t: number,
    view: View,
  ): [number, number] | null {
    const repo = findRepo(scene, ref[0]);
    if (!repo) return null;
    const wt = repo.wts.find((w) => w.label === ref[1] && !w.removed);
    if (!wt) return null;
    const p = wtScreenPos(repo, wt, t, view);
    return [p[0], p[1]];
  }

  function draw(): void {
    if (!ctx) return;
    const t = lastT;
    ctx.clearRect(0, 0, width, height);
    const view = makeView(t);

    for (const rel of scene.rels) {
      const a = findRepo(scene, rel.a);
      const b = findRepo(scene, rel.b);
      if (!a || !b) continue;
      const pa = repoScreenPos(a, t, view);
      const pb = repoScreenPos(b, t, view);
      ctx.globalAlpha = 0.5 * ease((t - rel.birth) / 0.9);
      ctx.strokeStyle = TEAL;
      ctx.lineWidth = 1.4;
      ctx.setLineDash([5, 6]);
      ctx.lineDashOffset = reduced ? 0 : -t * 9;
      ctx.beginPath();
      ctx.moveTo(pa[0], pa[1]);
      ctx.lineTo(pb[0], pb[1]);
      ctx.stroke();
      ctx.setLineDash([]);
      ctx.globalAlpha = 1;
    }

    for (const arc of scene.arcs) {
      const pa = nodePos(arc.a, t, view);
      const pb = nodePos(arc.b, t, view);
      if (!pa || !pb) continue;
      const merged = findWt(scene, arc.a[0], arc.a[1])?.merged;
      const alpha = (merged ? 0.22 : 0.45) * ease((t - arc.birth) / 0.9);
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

    for (const repo of scene.repos) {
      const origin = repoScreenPos(repo, t, view);

      for (const wt of repo.wts) {
        const alpha = wtAlpha(wt, t);
        if (alpha <= 0) continue;
        const p = wtScreenPos(repo, wt, t, view);
        const merged = wt.merged !== undefined && t >= wt.merged;
        const dim = merged ? 0.45 : 1;

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
        const agentOn =
          wt.agent !== undefined && t >= wt.agent.at && !merged && !wt.removed;
        // The agent "machines" its node: stepped micro-offsets, deliberately
        // mechanical against the smooth ambient float. Labels and badges stay
        // put for legibility.
        let nx = p[0];
        let ny = p[1];
        if (agentOn && !reduced) {
          const mech = MECH[Math.floor(t * 3 + wt.ph) % MECH.length];
          nx += mech[0];
          ny += mech[1];
        }
        ctx.globalAlpha = alpha;
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
          // The node blends toward the agent purple while it's being worked.
          const blend = reduced
            ? 0.3
            : 0.26 + 0.12 * (Math.floor(t * 2 + wt.ph) % 2);
          ctx.globalAlpha = alpha * blend;
          ctx.fillStyle = AGENT;
          ctx.beginPath();
          ctx.arc(nx, ny, r, 0, TAU);
          ctx.fill();
        }
        ctx.globalAlpha = 1;

        labelText(
          p[0] + Math.cos(wt.ang) * 12,
          p[1] + Math.sin(wt.ang) * 12 + 3,
          wt.label,
          palette.faint,
          0.95 * alpha * dim * p[2],
          Math.cos(wt.ang) < -0.3 ? "right" : "left",
        );

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
          const botY = Math.sin(wt.ang) > 0.45 ? ny + 17 : ny - 13;
          drawBot(nx + 12, botY + bob, grow, alpha);
          if (!reduced) {
            const pulse = ((t - wt.agent.at) % 2.6) / 2.6;
            ring(nx, ny, r + 2 + pulse * 14, AGENT, (1 - pulse) * 0.5 * alpha);
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
      const base = Math.max(21, ctx.measureText(repo.label).width / 2 + 8);
      let radius = base * (born < 1 ? born * (1 + 0.25 * (1 - born)) : 1);
      // Swallowing a merge payload makes the repo grow for a beat.
      if (!reduced) {
        for (const wt of repo.wts) {
          if (wt.merged !== undefined) {
            const arr = (t - (wt.merged + 0.8)) / 0.5;
            if (arr >= 0 && arr < 1)
              radius *= 1 + 0.12 * Math.sin(Math.PI * arr);
          }
        }
      }
      ctx.fillStyle = palette.ink;
      ctx.beginPath();
      ctx.arc(origin[0], origin[1], radius, 0, TAU);
      ctx.fill();
      ctx.fillStyle = palette.halo;
      ctx.textAlign = "center";
      ctx.fillText(repo.label, origin[0], origin[1] + 3.5);
      ctx.textAlign = "left";
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
            const pw = wtScreenPos(repo, wt, t, view);
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

  /* ------------------------------- wiring -------------------------------- */

  const offFrame = player.onFrame((t) => {
    syncScene(t);
    draw();
  });

  const themeObserver = new MutationObserver(() => {
    palette = readPalette();
    draw();
  });
  themeObserver.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class"],
  });

  const resizeObserver = new ResizeObserver(() => {
    const rect = canvas.getBoundingClientRect();
    if (!rect.width) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    width = rect.width;
    height = rect.height;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw();
  });
  resizeObserver.observe(canvas);

  const onClick = (): void => {
    if (!reduced) player.toggle();
  };
  canvas.addEventListener("click", onClick);

  syncScene(player.clock());
  draw();

  return {
    destroy() {
      offFrame();
      themeObserver.disconnect();
      resizeObserver.disconnect();
      canvas.removeEventListener("click", onClick);
    },
  };
}

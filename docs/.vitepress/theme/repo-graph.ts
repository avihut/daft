/**
 * Repo-graph renderer for the landing demo (#467, direction B).
 *
 * Draws repos as ink discs, worktrees as satellite nodes, and cross-repo
 * relations as dashed teal lines on a transparent canvas. The landing
 * terminal replays daft commands against this API; the interactive
 * "graph game" follow-up extends this same module rather than replacing it.
 *
 * Colors are read from the live VitePress theme tokens so the canvas
 * follows light/dark switches without a re-render from Vue.
 */

interface Worktree {
  label: string;
  ang: number;
  dist: number;
  birth: number;
  ph: number;
  dead: number;
  mergeStart: number;
}

interface Repo {
  label: string;
  fx: number;
  fy: number;
  baseAng: number;
  birth: number;
  ph: number;
  wts: Worktree[];
}

interface Relation {
  a: number;
  b: number;
  birth: number;
}

interface Pulse {
  repo: number;
  wt: number;
  start: number;
}

interface Palette {
  ink: string;
  muted: string;
  faint: string;
  gold: string;
  halo: string;
}

export interface RepoGraph {
  addRepo(label: string, fx: number, fy: number): number;
  addWorktree(repo: number, label: string): void;
  relate(a: number, b: number): void;
  merge(repo: number, label: string): void;
  reset(): void;
  destroy(): void;
}

const TEAL = "#1b9aaa";
const TAU = Math.PI * 2;
const GOLDEN_ANGLE = 2.39996;
const MONO = "ui-monospace, 'SF Mono', Menlo, Consolas, monospace";
const MERGE_SECS = 1.0;

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

export function createRepoGraph(canvas: HTMLCanvasElement): RepoGraph | null {
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;

  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const start = performance.now();
  const now = () => (performance.now() - start) / 1000;
  const ease = (x: number) => 1 - (1 - Math.min(Math.max(x, 0), 1)) ** 3;

  let width = 0;
  let height = 0;
  let visible = false;
  let raf = 0;
  let palette = readPalette();

  let repos: Repo[] = [];
  let relations: Relation[] = [];
  let pulses: Pulse[] = [];

  const themeObserver = new MutationObserver(() => {
    palette = readPalette();
    if (!raf) draw();
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

  const visibilityObserver = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        visible = entry.isIntersecting;
        if (visible && !raf) raf = requestAnimationFrame(loop);
      }
    },
    { threshold: 0.05 },
  );
  visibilityObserver.observe(canvas);

  function birthStamp(): number {
    // Under reduced motion everything is born settled.
    return reduced ? now() - 9 : now();
  }

  function repoXY(repo: Repo, t: number): [number, number] {
    const float = reduced ? 0 : Math.sin(t * 0.55 + repo.ph) * 2.6;
    return [repo.fx * width, repo.fy * height + float];
  }

  function worktreeXY(
    repo: Repo,
    wt: Worktree,
    t: number,
    origin: [number, number],
  ): [number, number, number] {
    const grown = ease((t - wt.birth) / 0.7);
    const float = reduced ? 0 : Math.sin(t * 0.7 + wt.ph) * 3;
    let x = origin[0] + Math.cos(wt.ang) * wt.dist * grown;
    let y = origin[1] + Math.sin(wt.ang) * wt.dist * grown + float;
    if (wt.mergeStart) {
      const m = ease((t - wt.mergeStart) / MERGE_SECS);
      const main = repo.wts[0];
      const target: [number, number] = main
        ? [
            origin[0] + Math.cos(main.ang) * main.dist,
            origin[1] + Math.sin(main.ang) * main.dist,
          ]
        : origin;
      x += (target[0] - x) * m;
      y += (target[1] - y) * m;
    }
    return [x, y, grown];
  }

  function alphaOf(wt: Worktree, t: number): number {
    if (wt.dead) return Math.max(0, 1 - (t - wt.dead) / 0.6);
    if (wt.mergeStart) {
      const m = (t - wt.mergeStart) / MERGE_SECS;
      if (m > 0.65) return Math.max(0, 1 - (m - 0.65) / 0.35);
    }
    return 1;
  }

  function label(
    x: number,
    y: number,
    text: string,
    color: string,
    alpha: number,
    align: "left" | "right",
  ): void {
    if (!ctx) return;
    ctx.font = `10px ${MONO}`;
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

  function draw(): void {
    if (!ctx) return;
    const t = now();
    ctx.clearRect(0, 0, width, height);

    for (const rel of relations) {
      const a = repos[rel.a];
      const b = repos[rel.b];
      if (!a || !b) continue;
      const pa = repoXY(a, t);
      const pb = repoXY(b, t);
      ctx.globalAlpha = 0.55 * ease((t - rel.birth) / 0.9);
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

    for (const repo of repos) {
      const origin = repoXY(repo, t);
      for (const wt of repo.wts) {
        const alpha = alphaOf(wt, t);
        if (alpha <= 0) continue;
        const p = worktreeXY(repo, wt, t, origin);
        ctx.globalAlpha = 0.45 * alpha;
        ctx.strokeStyle = palette.muted;
        ctx.lineWidth = 1.3;
        const bendX = (origin[0] + p[0]) / 2 + Math.cos(wt.ang + 1.57) * 7;
        const bendY = (origin[1] + p[1]) / 2 + Math.sin(wt.ang + 1.57) * 7;
        ctx.beginPath();
        ctx.moveTo(origin[0], origin[1]);
        ctx.quadraticCurveTo(bendX, bendY, p[0], p[1]);
        ctx.stroke();
        ctx.globalAlpha = alpha;
        const isMain = wt.label === "main";
        ctx.fillStyle = isMain ? palette.gold : palette.ink;
        ctx.beginPath();
        ctx.arc(p[0], p[1], isMain ? 7 : 5.5, 0, TAU);
        ctx.fill();
        if (isMain) {
          ctx.strokeStyle = palette.ink;
          ctx.lineWidth = 1.4;
          ctx.beginPath();
          ctx.arc(p[0], p[1], 7, 0, TAU);
          ctx.stroke();
        }
        ctx.globalAlpha = 1;
        label(
          p[0] + Math.cos(wt.ang) * 12,
          p[1] + Math.sin(wt.ang) * 12 + 3,
          wt.label,
          palette.faint,
          0.95 * alpha * p[2],
          Math.cos(wt.ang) < -0.3 ? "right" : "left",
        );
      }

      const born = ease((t - repo.birth) / 0.6);
      ctx.font = `600 10.5px ${MONO}`;
      const base = Math.max(23, ctx.measureText(repo.label).width / 2 + 8);
      const radius = base * (born < 1 ? born * (1 + 0.25 * (1 - born)) : 1);
      ctx.fillStyle = palette.ink;
      ctx.beginPath();
      ctx.arc(origin[0], origin[1], radius, 0, TAU);
      ctx.fill();
      ctx.fillStyle = palette.halo;
      ctx.textAlign = "center";
      ctx.fillText(repo.label, origin[0], origin[1] + 3.5);
      ctx.textAlign = "left";
    }

    if (!reduced) {
      pulses = pulses.filter((pulse) => {
        const age = (t - pulse.start) / 1.1;
        if (age < 0) return true;
        if (age >= 1) return false;
        const repo = repos[pulse.repo];
        if (!repo) return false;
        const origin = repoXY(repo, t);
        const wt = pulse.wt >= 0 ? repo.wts[pulse.wt] : undefined;
        const c = wt ? worktreeXY(repo, wt, t, origin) : origin;
        ctx.globalAlpha = (1 - age) * 0.8;
        ctx.strokeStyle = palette.gold;
        ctx.lineWidth = 1.6;
        ctx.beginPath();
        ctx.arc(c[0], c[1], 10 + age * 26, 0, TAU);
        ctx.stroke();
        ctx.globalAlpha = 1;
        return true;
      });
    }
  }

  function loop(): void {
    raf = 0;
    if (!visible) return;
    const t = now();
    for (const repo of repos) {
      repo.wts = repo.wts.filter((wt) => {
        if (wt.mergeStart && (t - wt.mergeStart) / MERGE_SECS >= 1)
          return false;
        if (wt.dead && t - wt.dead > 0.7) return false;
        return true;
      });
    }
    draw();
    raf = requestAnimationFrame(loop);
  }

  return {
    addRepo(labelText, fx, fy) {
      repos.push({
        label: labelText,
        fx: fx + (Math.random() - 0.5) * 0.03,
        fy: fy + (Math.random() - 0.5) * 0.03,
        baseAng: Math.random() * TAU,
        birth: birthStamp(),
        ph: Math.random() * 7,
        wts: [],
      });
      if (!reduced)
        pulses.push({ repo: repos.length - 1, wt: -1, start: now() });
      return repos.length - 1;
    },
    addWorktree(repoIndex, labelText) {
      const repo = repos[repoIndex];
      if (!repo) return;
      repo.wts.push({
        label: labelText,
        ang: repo.baseAng + repo.wts.length * GOLDEN_ANGLE,
        dist: 64 + Math.random() * 26,
        birth: birthStamp(),
        ph: Math.random() * 7,
        dead: 0,
        mergeStart: 0,
      });
      if (!reduced) {
        pulses.push({ repo: repoIndex, wt: repo.wts.length - 1, start: now() });
      }
    },
    relate(a, b) {
      relations.push({ a, b, birth: birthStamp() });
    },
    merge(repoIndex, labelText) {
      const repo = repos[repoIndex];
      if (!repo) return;
      const wt = repo.wts.find(
        (w, i) => i > 0 && !w.dead && !w.mergeStart && w.label === labelText,
      );
      if (!wt) return;
      if (reduced) {
        repo.wts = repo.wts.filter((w) => w !== wt);
        draw();
      } else {
        wt.mergeStart = now();
        pulses.push({
          repo: repoIndex,
          wt: 0,
          start: now() + MERGE_SECS * 0.6,
        });
      }
    },
    reset() {
      repos = [];
      relations = [];
      pulses = [];
      draw();
    },
    destroy() {
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      themeObserver.disconnect();
      resizeObserver.disconnect();
      visibilityObserver.disconnect();
    },
  };
}

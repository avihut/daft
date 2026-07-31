/**
 * The daft verb layer — layer 3 of the diagram language.
 *
 * Acts are the atoms; daft verbs are the standard molecules. Each documented
 * verb expands here into its canonical beats — the terminal lines and scene
 * acts bound together as one unit — so every diagram renders a verb the same
 * way and the shell can never drift from the graph. A scenario is a list of
 * `VerbInvocation`s replayed by `buildScenario` over a world model that
 * tracks what exists: it generates truthful output (`daft list` rows, sync's
 * prune summary), places repos and fits the camera as the story widens, and
 * lets the composer offer only valid targets. Invalid invocations (their
 * prerequisites deleted) skip cleanly instead of corrupting the scene.
 *
 * The grammar's own rules live in the expansions: a cross-repo `start`
 * relates the entering repo to every carrier of the branch and arcs to the
 * latest one; `ship` merges only where the branch lives; `sync` prunes only
 * merged shells. Hand-written `StepDef[]` scripts stay valid (the hero keeps
 * its tuned pacing), but new scripts should be verb invocations first.
 */

import type { Beat, CamRect, StepDef } from "./engine";

/* --------------------------------- world --------------------------------- */

export interface WorldWt {
  branch: string;
  port?: string;
  agent: boolean;
  merged: boolean;
  removed: boolean;
}

export interface WorldRepo {
  name: string;
  x: number;
  y: number;
  nextPort: number;
  wts: WorldWt[];
}

export interface World {
  repos: WorldRepo[];
  rels: [string, string][];
}

export function emptyWorld(): World {
  return { repos: [], rels: [] };
}

/** Deterministic placement: the first three spots match the hero exactly. */
const REPO_SPOTS = [
  { x: 0, y: 0 },
  { x: 330, y: -150 },
  { x: 300, y: 200 },
  { x: -330, y: 170 },
  { x: -310, y: -180 },
  { x: 560, y: 40 },
];

const NAME_POOL = ["web", "orders", "payments", "billing", "search", "assets"];
const BRANCH_POOL = [
  "checkout",
  "bugfix/cart-total",
  "spike/faster-cart",
  "feature/login",
  "perf/cache",
];
const AGENT_TASKS = [
  "agent · building",
  "agent · testing",
  "agent · refactoring",
];

function findRepo(world: World, name: string): WorldRepo | undefined {
  return world.repos.find((r) => r.name === name);
}

function activeWts(repo: WorldRepo): WorldWt[] {
  return repo.wts.filter((w) => !w.removed);
}

function findWt(
  world: World,
  repoName: string,
  branch: string,
): WorldWt | undefined {
  const repo = findRepo(world, repoName);
  return repo ? activeWts(repo).find((w) => w.branch === branch) : undefined;
}

/** Repos (other than `exclude`) carrying an active worktree for `branch`. */
function carriers(world: World, branch: string, exclude: string): WorldRepo[] {
  return world.repos.filter(
    (r) => r.name !== exclude && activeWts(r).some((w) => w.branch === branch),
  );
}

function related(world: World, a: string, b: string): boolean {
  return world.rels.some(
    ([x, y]) => (x === a && y === b) || (x === b && y === a),
  );
}

function nextRepoName(world: World): string {
  return (
    NAME_POOL.find((n) => !findRepo(world, n)) ?? `svc${world.repos.length + 1}`
  );
}

function nextBranch(world: World): string {
  const used = (b: string): boolean =>
    world.repos.some((r) => activeWts(r).some((w) => w.branch === b));
  const total = world.repos.reduce((n, r) => n + r.wts.length, 0);
  return BRANCH_POOL.find((b) => !used(b)) ?? `feature/f${total + 1}`;
}

function shortBranch(branch: string): string {
  return branch.split("/").pop() ?? branch;
}

function str(args: VerbArgs, key: string, fallback: string): string {
  const v = args[key];
  return typeof v === "string" && v.trim() ? v.trim() : fallback;
}

function addRepo(world: World, name: string): WorldRepo {
  const i = world.repos.length;
  const spot = REPO_SPOTS[i % REPO_SPOTS.length];
  const base = 3000 + i * 1000;
  const repo: WorldRepo = {
    name,
    x: spot.x,
    y: spot.y,
    nextPort: base + 1,
    wts: [
      {
        branch: "main",
        port: `:${base}`,
        agent: false,
        merged: false,
        removed: false,
      },
    ],
  };
  world.repos.push(repo);
  return repo;
}

/** Non-main active worktrees as "repo:branch" targets. */
function removable(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.branch !== "main") out.push(`${r.name}:${w.branch}`);
  return out;
}

/** Branches that can still be shipped: active, non-main, not yet merged. */
function shippable(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.branch !== "main" && !w.merged && !out.includes(w.branch))
        out.push(w.branch);
  return out;
}

function agentCandidates(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.branch !== "main" && !w.agent && !w.merged)
        out.push(`${r.name}:${w.branch}`);
  return out;
}

/**
 * Fit-rect over every repo's orbit — the cameras the hero hand-tuned,
 * generalized: bbox center, hero-derived padding, hero-sized floor.
 */
export function camFor(world: World): CamRect {
  if (!world.repos.length) return { x: 0, y: 0, w: 460, h: 400 };
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  for (const r of world.repos) {
    minX = Math.min(minX, r.x);
    maxX = Math.max(maxX, r.x);
    minY = Math.min(minY, r.y);
    maxY = Math.max(maxY, r.y);
  }
  return {
    x: (minX + maxX) / 2,
    y: (minY + maxY) / 2,
    w: Math.max(maxX - minX + 360, 460),
    h: Math.max(maxY - minY + 320, 400),
  };
}

/* --------------------------------- verbs --------------------------------- */

export type VerbArgs = Record<string, unknown>;

export interface FieldSpec {
  key: string;
  label: string;
  kind: "text" | "choice";
  choices?: string[];
  /** World-aware default. */
  value: string;
}

export interface VerbSpec {
  id: string;
  /** Short palette label. */
  label: string;
  /** How the command is written — shown in the catalog and composer. */
  syntax: string;
  summary: string;
  /** Can this verb do anything in this world? Gates the composer palette. */
  available(world: World): boolean;
  /** Composer form fields, with world-aware defaults and choices. */
  fields(world: World): FieldSpec[];
  /** Expand into one step, mutating the world. */
  run(world: World, args: VerbArgs): StepDef;
}

const clone: VerbSpec = {
  id: "clone",
  label: "clone",
  syntax: "daft clone <url>",
  summary:
    "One repo, one directory per branch — main/ is ready to run the moment the clone lands.",
  available: () => true,
  fields(world) {
    return [
      { key: "name", label: "Repo", kind: "text", value: nextRepoName(world) },
    ];
  },
  run(world, args) {
    let name = str(args, "name", nextRepoName(world));
    if (findRepo(world, name)) name = nextRepoName(world);
    const repo = addRepo(world, name);
    const port = repo.wts[0].port ?? ":3000";
    const beats: Beat[] = [
      { cmd: `daft clone git@github.com:acme/${name}.git` },
      { act: { kind: "repo", repo: name, x: repo.x, y: repo.y } },
      { act: { kind: "wt", repo: name, wt: "main" } },
      { out: `cloned ${name} · worktree main/ ready` },
      { act: { kind: "port", repo: name, wt: "main", port } },
      { out: `dev server → ${port}` },
    ];
    return { title: `Clone ${name}`, cam: camFor(world), beats };
  },
};

const start: VerbSpec = {
  id: "start",
  label: "start",
  syntax: "daft start <branch> — or daft start <repo> <branch>",
  summary:
    "A dedicated worktree for the branch: booted by your hooks, on its own port. Name another repo and the same feature spans both.",
  available: (world) => world.repos.length > 0,
  fields(world) {
    return [
      {
        key: "branch",
        label: "Branch",
        kind: "text",
        value: nextBranch(world),
      },
      {
        key: "repo",
        label: "In repo",
        kind: "choice",
        choices: [...world.repos.map((r) => r.name), nextRepoName(world)],
        value: world.repos[0]?.name ?? "web",
      },
    ];
  },
  run(world, args) {
    const home = world.repos[0];
    const repoName = str(args, "repo", home.name);
    let branch = str(args, "branch", nextBranch(world));
    const cross = repoName !== home.name;
    let target = findRepo(world, repoName);
    const beats: Beat[] = [
      {
        cmd: cross
          ? `daft start ${repoName} ${branch}`
          : `daft start ${branch}`,
      },
    ];
    if (!target) {
      // The repo enters the story: disc, main, its own port.
      target = addRepo(world, repoName);
      beats.push(
        { act: { kind: "repo", repo: repoName, x: target.x, y: target.y } },
        { act: { kind: "wt", repo: repoName, wt: "main" } },
        {
          act: {
            kind: "port",
            repo: repoName,
            wt: "main",
            port: target.wts[0].port ?? ":4000",
          },
        },
      );
    }
    if (findWt(world, repoName, branch)) branch = nextBranch(world);
    // A feature spanning repos ties them together: relate to every carrier,
    // arc to the latest one — exactly the hero's cross-repo grammar.
    const links = carriers(world, branch, repoName);
    for (const c of links) {
      if (!related(world, c.name, repoName)) {
        world.rels.push([c.name, repoName]);
        beats.push({ act: { kind: "relate", a: c.name, b: repoName } });
      }
    }
    beats.push({
      out: cross
        ? `${repoName}: worktree ${branch}/ ready`
        : `worktree ${branch}/ created`,
    });
    const port = `:${target.nextPort++}`;
    target.wts.push({
      branch,
      port,
      agent: false,
      merged: false,
      removed: false,
    });
    beats.push(
      { act: { kind: "wt", repo: repoName, wt: branch } },
      { act: { kind: "boot", repo: repoName, wt: branch, secs: 1.6 } },
    );
    if (!cross)
      beats.push({ out: "✓ install  ✓ .envrc  ✓ build cache", tone: "ok" });
    if (links.length) {
      const from = links[links.length - 1];
      beats.push({
        act: { kind: "arc", a: [from.name, branch], b: [repoName, branch] },
      });
    }
    beats.push(
      { pause: 1 },
      { act: { kind: "port", repo: repoName, wt: branch, port } },
      { out: `dev server → ${port}` },
    );
    return {
      title: cross ? `Start in ${repoName}` : `Start ${shortBranch(branch)}`,
      cam: camFor(world),
      beats,
    };
  },
};

const list: VerbSpec = {
  id: "list",
  label: "list",
  syntax: "daft list",
  summary:
    "Every worktree, its port, and who's working in it — humans and agents side by side.",
  available: (world) => world.repos.length > 0,
  fields(world) {
    const candidates = agentCandidates(world);
    return [
      {
        key: "agent",
        label: "Agent joins",
        kind: "choice",
        choices: ["none", ...candidates],
        value: candidates[0] ?? "none",
      },
    ];
  },
  run(world, args) {
    const marks = new Set<string>(
      Array.isArray(args.agents)
        ? (args.agents as string[])
        : typeof args.agent === "string" && args.agent !== "none"
          ? [args.agent]
          : [],
    );
    const beats: Beat[] = [{ cmd: "daft list" }];
    const multi = world.repos.length > 1;
    interface Row {
      repo: string;
      wt: WorldWt;
      label: string;
      isNew: boolean;
    }
    const rows: Row[] = [];
    for (const r of world.repos) {
      for (const wt of activeWts(r)) {
        const isNew =
          marks.has(`${r.name}:${wt.branch}`) &&
          !wt.agent &&
          wt.branch !== "main";
        if (isNew) wt.agent = true;
        rows.push({
          repo: r.name,
          wt,
          label: multi ? `${r.name}/${wt.branch}` : wt.branch,
          isNew,
        });
      }
    }
    const width = rows.reduce((n, row) => Math.max(n, row.label.length), 0);
    let agentIdx = 0;
    for (const row of rows) {
      const status = row.wt.agent
        ? AGENT_TASKS[agentIdx++ % AGENT_TASKS.length]
        : row.wt.merged
          ? "merged"
          : row.wt.branch === "main"
            ? "clean"
            : "you";
      if (row.isNew)
        beats.push({
          act: { kind: "agent", repo: row.repo, wt: row.wt.branch },
        });
      const text = `${row.label.padEnd(width)}  ${(row.wt.port ?? "").padEnd(5)}  ${status}`;
      beats.push(row.wt.agent ? { out: text, tone: "agent" } : { out: text });
    }
    beats.push({ pause: 1.2 });
    return {
      title: marks.size ? "Agents" : "List",
      cam: camFor(world),
      beats,
    };
  },
};

const exec: VerbSpec = {
  id: "exec",
  label: "exec",
  syntax: "daft exec --related -- <command>",
  summary:
    "Run one command across every related worktree — test the whole feature in one line.",
  available: (world) => world.repos.length > 0,
  fields: () => [
    { key: "command", label: "Command", kind: "text", value: "pnpm test" },
  ],
  run(world, args) {
    const command = str(args, "command", "pnpm test");
    const names = world.repos.map((r) => r.name);
    return {
      title: "Exec",
      cam: camFor(world),
      beats: [
        { cmd: `daft exec --related -- ${command}` },
        { out: `✓ ${names.join("   ✓ ")}`, tone: "ok" },
        { pause: 0.8 },
      ],
    };
  },
};

const ship: VerbSpec = {
  id: "ship",
  label: "ship",
  syntax: "daft exec --related -- git push",
  summary:
    "Push the feature everywhere it lives; the forge merges, and each payload travels home to its repo.",
  available: (world) => shippable(world).length > 0,
  fields(world) {
    const pool = shippable(world);
    return [
      {
        key: "branch",
        label: "Branch",
        kind: "choice",
        choices: pool,
        value: pool[0] ?? "",
      },
    ];
  },
  run(world, args) {
    const pool = shippable(world);
    let branch = str(args, "branch", pool[0] ?? "");
    if (!pool.includes(branch)) branch = pool[0] ?? branch;
    const targets = world.repos.filter((r) =>
      activeWts(r).some((w) => w.branch === branch && !w.merged),
    );
    const beats: Beat[] = [
      { cmd: "daft exec --related -- git push" },
      { out: `${branch} → origin in ${targets.map((r) => r.name).join(", ")}` },
      { pause: 0.5 },
      { out: "# PRs reviewed → merged on the forge", tone: "dim" },
    ];
    targets.forEach((r, i) => {
      const wt = activeWts(r).find((w) => w.branch === branch);
      if (wt) wt.merged = true;
      beats.push({ act: { kind: "merged", repo: r.name, wt: branch } });
      if (i < targets.length - 1) beats.push({ pause: 0.25 });
    });
    beats.push({ pause: 0.9 });
    return { title: `Ship ${shortBranch(branch)}`, cam: camFor(world), beats };
  },
};

const remove: VerbSpec = {
  id: "remove",
  label: "remove",
  syntax: "daft remove <branch>",
  summary:
    "Tear a worktree down — branch, directory, and its claimed resources released in one verb.",
  available: (world) => removable(world).length > 0,
  fields(world) {
    const pool = removable(world);
    return [
      {
        key: "target",
        label: "Worktree",
        kind: "choice",
        choices: pool,
        value: pool[0] ?? "",
      },
    ];
  },
  run(world, args) {
    const pool = removable(world);
    let target = str(args, "target", pool[0] ?? "");
    if (!pool.includes(target)) target = pool[0] ?? target;
    const cut = target.indexOf(":");
    const repoName = target.slice(0, cut);
    const branch = target.slice(cut + 1);
    const wt = findWt(world, repoName, branch);
    if (wt) wt.removed = true;
    const cross = repoName !== world.repos[0]?.name;
    const beats: Beat[] = [
      {
        cmd: cross
          ? `daft remove --repo ${repoName} ${branch}`
          : `daft remove ${branch}`,
      },
      { act: { kind: "remove", repo: repoName, wt: branch } },
      {
        out: wt?.port
          ? `removed ${branch} · ${wt.port} released`
          : `removed ${branch}`,
        tone: "rust",
      },
      { pause: 0.8 },
    ];
    return {
      title: `Remove ${shortBranch(branch)}`,
      cam: camFor(world),
      beats,
    };
  },
};

const sync: VerbSpec = {
  id: "sync",
  label: "sync",
  syntax: "daft sync",
  summary:
    "Every main pulls fresh, every worktree gets the signal — and merged shells are pruned away.",
  available: (world) => world.repos.length > 0,
  fields: () => [],
  run(world) {
    const beats: Beat[] = [{ cmd: "daft sync" }];
    for (const r of world.repos)
      beats.push({ act: { kind: "sync", repo: r.name } });
    const n = world.repos.length;
    beats.push({
      out: `updated ${n} main${n === 1 ? "" : "s"} · bases refreshed`,
    });
    const pruned: { repo: string; branch: string }[] = [];
    for (const r of world.repos) {
      for (const wt of activeWts(r)) {
        if (wt.merged) {
          wt.removed = true;
          pruned.push({ repo: r.name, branch: wt.branch });
        }
      }
    }
    if (pruned.length) {
      const branches = [...new Set(pruned.map((p) => p.branch))].join(", ");
      const repos = [...new Set(pruned.map((p) => p.repo))].join(", ");
      beats.push(
        { pause: 0.5 },
        { out: `pruned ${branches} in ${repos}`, tone: "rust" },
      );
      pruned.forEach((p, i) => {
        beats.push({ act: { kind: "remove", repo: p.repo, wt: p.branch } });
        if (i < pruned.length - 1) beats.push({ pause: 0.3 });
      });
    }
    beats.push({ pause: 1.4 });
    return { title: "Sync", cam: camFor(world), beats };
  },
};

export const VERBS: VerbSpec[] = [clone, start, list, exec, ship, remove, sync];

/* ------------------------------- scenarios ------------------------------- */

export interface VerbInvocation {
  verb: string;
  args: VerbArgs;
}

export interface Scenario {
  steps: StepDef[];
  world: World;
  /** Invocation index → built step index; -1 when skipped (unavailable). */
  mapping: number[];
}

export function buildScenario(invocations: VerbInvocation[]): Scenario {
  const world = emptyWorld();
  const steps: StepDef[] = [];
  const mapping: number[] = [];
  for (const inv of invocations) {
    const spec = VERBS.find((v) => v.id === inv.verb);
    if (!spec?.available(world)) {
      mapping.push(-1);
      continue;
    }
    mapping.push(steps.length);
    steps.push(spec.run(world, inv.args));
  }
  return { steps, world, mapping };
}

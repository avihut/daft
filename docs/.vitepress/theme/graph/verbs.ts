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
 * latest one; the forge merges only where a branch was pushed; `sync` prunes
 * only merged shells. Hand-written `StepDef[]` scripts stay valid (the hero keeps
 * its tuned pacing), but new scripts should be verb invocations first.
 */

import type {
  Beat as BeatOf,
  CamRect,
  FieldSpec as LangFieldSpec,
  VerbArgs as LangVerbArgs,
  OpSpecOf,
  StepDef as StepDefOf,
} from "@dumbshow/core";
import type { Act } from "./render";

/** The engine's shapes bound to daft acts — the module-local spellings of
 * the aliases pack.ts exports. */
type Beat = BeatOf<Act>;
type StepDef = StepDefOf<Act>;

/* --------------------------------- world --------------------------------- */

export interface WorldWt {
  branch: string;
  port?: string;
  agent: boolean;
  merged: boolean;
  removed: boolean;
  /** Pushed to the forge — what `forge merges` is allowed to land. */
  pushed?: boolean;
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
  /** Where the shell stands in the Files tree, e.g. "~/web/checkout". */
  cwd?: string;
  /** Author-pinned repo spots (composer placements); addRepo consults it. */
  spots?: Record<string, { x: number; y: number }>;
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

export function nextRepoName(world: World): string {
  return (
    NAME_POOL.find((n) => !findRepo(world, n)) ?? `svc${world.repos.length + 1}`
  );
}

export function nextBranch(world: World): string {
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

export function addRepo(world: World, name: string): WorldRepo {
  const i = world.repos.length;
  const spot =
    world.spots?.[name] ??
    REPO_SPOTS.find(
      (s) => !world.repos.some((r) => r.x === s.x && r.y === s.y),
    ) ??
    REPO_SPOTS[i % REPO_SPOTS.length];
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

/** Worktrees that can be pushed: active, non-main, unmerged, not pushed. */
function pushable(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.branch !== "main" && !w.merged && !w.pushed)
        out.push(`${r.name}:${w.branch}`);
  return out;
}

/** Branches the forge could merge: pushed somewhere, not yet merged there. */
function forgeable(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.pushed && !w.merged && !out.includes(w.branch)) out.push(w.branch);
  return out;
}

/** Ordered repo pairs with no relation yet — what repo link can join. */
function unrelatedPairs(world: World): [string, string][] {
  const out: [string, string][] = [];
  for (const a of world.repos)
    for (const b of world.repos)
      if (a.name < b.name && !related(world, a.name, b.name))
        out.push([a.name, b.name]);
  return out;
}

/** Every active worktree as a "repo:branch" target. */
function goTargets(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r)) out.push(`${r.name}:${w.branch}`);
  return out;
}

/** Active, non-main, unmerged worktrees — daft merge's source pool. */
function mergeSources(world: World): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.branch !== "main" && !w.merged) out.push(`${r.name}:${w.branch}`);
  return out;
}

/** Merged shells still standing — what prune tears down. */
function prunable(world: World): boolean {
  return world.repos.some((r) => activeWts(r).some((w) => w.merged));
}

/**
 * Walk the Files tree the way a shell would: absolute-from-~ or relative
 * paths, `..` steps, branch names with slashes. Returns the new cwd or an
 * error naming what does not exist.
 */
export function resolveCd(
  world: World,
  cwd: string,
  arg: string,
): { cwd: string } | { error: string } {
  const target = arg.trim() || "~";
  const start = target.startsWith("~")
    ? []
    : cwd.split("/").filter((p) => p !== "" && p !== "~");
  const parts = [...start];
  for (const piece of target.split("/")) {
    if (piece === "" || piece === "~" || piece === ".") continue;
    if (piece === "..") {
      parts.pop();
      continue;
    }
    parts.push(piece);
  }
  if (parts.length === 0) return { cwd: "~" };
  const repo = findRepo(world, parts[0]);
  if (!repo) return { error: `no such directory: ${parts[0]}` };
  if (parts.length === 1) return { cwd: `~/${repo.name}` };
  // Branch directories may contain slashes — the rest of the path is one.
  const branch = parts.slice(1).join("/");
  const wt = activeWts(repo).find((w) => w.branch === branch);
  if (!wt) return { error: `no such directory: ${parts[0]}/${branch}` };
  return { cwd: `~/${repo.name}/${branch}` };
}

/** Keep cwd pointing at something that exists: fall back to the repo's
 * main when the worktree died, to home when the repo did. */
function clampCwd(world: World): void {
  if (!world.cwd) return;
  const parts = world.cwd.split("/");
  if (parts.length < 2) return;
  const repoName = parts[1];
  const branch = parts.slice(2).join("/");
  const repo = findRepo(world, repoName);
  if (!repo) {
    world.cwd = "~";
    return;
  }
  if (branch && !activeWts(repo).some((w) => w.branch === branch))
    world.cwd = activeWts(repo).some((w) => w.branch === "main")
      ? `~/${repoName}/main`
      : "~";
}

/**
 * Tear down every merged shell: mutates the world and returns the beats
 * (rust summary + staggered remove acts). Shared by `prune` and `sync`.
 */
function pruneBeats(world: World): Beat[] {
  const beats: Beat[] = [];
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
  return beats;
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

/**
 * The op shapes are the language-pack contract's (language.ts),
 * instantiated for daft's world and steps: daft verbs print a command and
 * act; events (an agent joining, the forge merging) act without a command
 * — they are world happenings, not things you type, so the verbs stay
 * honest to daft's real surface.
 */
export type VerbArgs = LangVerbArgs;
export type FieldSpec = LangFieldSpec;
export type OpSpec = OpSpecOf<World, StepDef>;

function resolveCloneName(world: World, args: VerbArgs): string {
  const name = str(args, "name", nextRepoName(world));
  return findRepo(world, name) ? nextRepoName(world) : name;
}

const cloneCommand = (world: World, args: VerbArgs): string =>
  `daft clone git@github.com:acme/${resolveCloneName(world, args)}.git`;

const clone: OpSpec = {
  id: "clone",
  kind: "verb",
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
  command: cloneCommand,
  run(world, args) {
    const cmd = cloneCommand(world, args);
    const name = resolveCloneName(world, args);
    const repo = addRepo(world, name);
    const port = repo.wts[0].port ?? ":3000";
    const beats: Beat[] = [
      { cmd },
      { act: { kind: "repo", repo: name, x: repo.x, y: repo.y } },
      { act: { kind: "wt", repo: name, wt: "main" } },
      { out: `cloned ${name} · worktree main/ ready` },
      { act: { kind: "port", repo: name, wt: "main", port } },
      { out: `dev server → ${port}` },
    ];
    world.cwd = `~/${name}/main`;
    return { title: `Clone ${name}`, cam: camFor(world), beats };
  },
};

function startParams(
  world: World,
  args: VerbArgs,
): { repoName: string; branch: string; cross: boolean } {
  const homeName = world.repos[0]?.name ?? "web";
  const repoName = str(args, "repo", homeName);
  const branch = str(args, "branch", nextBranch(world));
  return { repoName, branch, cross: repoName !== homeName };
}

const startCommand = (world: World, args: VerbArgs): string => {
  const p = startParams(world, args);
  return p.cross
    ? `daft start ${p.repoName} ${p.branch}`
    : `daft start ${p.branch}`;
};

const listCommand = (): string => "daft list";

const execCommand = (_world: World, args: VerbArgs): string =>
  `daft exec --related -- ${str(args, "command", "pnpm test")}`;

function resolvePushTarget(
  world: World,
  args: VerbArgs,
): { repoName: string; branch: string } {
  const pool = pushable(world);
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const cut = target.indexOf(":");
  return { repoName: target.slice(0, cut), branch: target.slice(cut + 1) };
}

const pushCommand = (world: World, args: VerbArgs): string =>
  `daft push ${resolvePushTarget(world, args).branch}`;

function resolveGoTarget(
  world: World,
  args: VerbArgs,
): { repoName: string; branch: string; cross: boolean } {
  const pool = goTargets(world);
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const cut = target.indexOf(":");
  const repoName = target.slice(0, cut);
  return {
    repoName,
    branch: target.slice(cut + 1),
    cross: repoName !== world.repos[0]?.name,
  };
}

const goCommand = (world: World, args: VerbArgs): string => {
  const t = resolveGoTarget(world, args);
  if (!t.cross) return `daft go ${t.branch}`;
  return t.branch === "main"
    ? `daft go ${t.repoName}`
    : `daft go ${t.repoName} ${t.branch}`;
};

function resolveMergeSource(
  world: World,
  args: VerbArgs,
): { repoName: string; branch: string } {
  const pool = mergeSources(world);
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const cut = target.indexOf(":");
  return { repoName: target.slice(0, cut), branch: target.slice(cut + 1) };
}

const mergeCommand = (world: World, args: VerbArgs): string =>
  `daft merge ${resolveMergeSource(world, args).branch}`;

const pruneCommand = (): string => "daft prune";

const updateCommand = (): string => "daft update";

function resolveCarry(
  world: World,
  args: VerbArgs,
): { from: string; to: string; copy: boolean } {
  const pool = goTargets(world);
  let from = str(args, "from", pool[0] ?? "");
  if (!pool.includes(from)) from = pool[0] ?? from;
  let to = str(args, "to", pool.find((p) => p !== from) ?? "");
  if (!pool.includes(to) || to === from)
    to = pool.find((p) => p !== from) ?? to;
  return { from, to, copy: str(args, "mode", "move") === "copy" };
}

const carryCommand = (world: World, args: VerbArgs): string => {
  const c = resolveCarry(world, args);
  const branch = c.to.slice(c.to.indexOf(":") + 1);
  return `daft carry ${branch}${c.copy ? " --copy" : ""}`;
};

function resolveRename(
  world: World,
  args: VerbArgs,
): { repoName: string; branch: string; to: string } {
  const pool = removable(world);
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const cut = target.indexOf(":");
  const branch = target.slice(cut + 1);
  return {
    repoName: target.slice(0, cut),
    branch,
    to: str(args, "to", nextBranch(world)),
  };
}

const renameCommand = (world: World, args: VerbArgs): string => {
  const r = resolveRename(world, args);
  return `daft rename ${r.branch} ${r.to}`;
};

const repoAddCommand = (world: World, args: VerbArgs): string =>
  `daft repo add ~/${resolveCloneName(world, args)}`;

function resolveRepoName(world: World, args: VerbArgs): string {
  const pool = world.repos.map((r) => r.name);
  let name = str(args, "name", pool[0] ?? "");
  if (!pool.includes(name)) name = pool[0] ?? name;
  return name;
}

const repoRemoveCommand = (world: World, args: VerbArgs): string =>
  `daft repo remove ${resolveRepoName(world, args)}`;

function resolveLinkPair(world: World, args: VerbArgs): [string, string] {
  const pool = unrelatedPairs(world);
  const a = str(args, "a", pool[0]?.[0] ?? "");
  const b = str(args, "b", pool[0]?.[1] ?? "");
  const ok = pool.some(
    ([x, y]) => (x === a && y === b) || (x === b && y === a),
  );
  return ok ? [a, b] : (pool[0] ?? [a, b]);
}

const repoLinkCommand = (world: World, args: VerbArgs): string =>
  `daft repo link ${resolveLinkPair(world, args)[1]}`;

function resolveUnlinkPair(world: World, args: VerbArgs): [string, string] {
  const pool = world.rels.map(([a, b]) => `${a} ↔ ${b}`);
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const [a, b] = target.split(" ↔ ");
  return [a ?? "", b ?? ""];
}

const repoUnlinkCommand = (world: World, args: VerbArgs): string =>
  `daft repo unlink ${resolveUnlinkPair(world, args)[1]}`;

const syncCommand = (_world: World, args: VerbArgs): string =>
  `daft sync${str(args, "push", "no") === "yes" ? " --push" : ""}`;

const start: OpSpec = {
  id: "start",
  kind: "verb",
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
  command: startCommand,
  run(world, args) {
    const cmd = startCommand(world, args);
    const p = startParams(world, args);
    const repoName = p.repoName;
    let branch = p.branch;
    const cross = p.cross;
    let target = findRepo(world, repoName);
    const beats: Beat[] = [{ cmd }];
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
    world.cwd = `~/${repoName}/${branch}`;
    return {
      title: cross ? `Start in ${repoName}` : `Start ${shortBranch(branch)}`,
      cam: camFor(world),
      beats,
    };
  },
};

const list: OpSpec = {
  id: "list",
  kind: "verb",
  label: "list",
  syntax: "daft list",
  summary:
    "Every worktree, its port, and who's working in it — humans and agents side by side.",
  available: (world) => world.repos.length > 0,
  fields: () => [],
  command: listCommand,
  // Read-only by design: the rows report what exists, the world is never
  // touched. Agents arrive through events, not through listing them.
  run(world) {
    const beats: Beat[] = [{ cmd: listCommand() }];
    const multi = world.repos.length > 1;
    interface Row {
      wt: WorldWt;
      label: string;
    }
    const rows: Row[] = [];
    for (const r of world.repos) {
      for (const wt of activeWts(r)) {
        rows.push({
          wt,
          label: multi ? `${r.name}/${wt.branch}` : wt.branch,
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
      const text = `${row.label.padEnd(width)}  ${(row.wt.port ?? "").padEnd(5)}  ${status}`;
      beats.push(row.wt.agent ? { out: text, tone: "agent" } : { out: text });
    }
    beats.push({ pause: 1.2 });
    return { title: "List", cam: camFor(world), beats };
  },
};

const exec: OpSpec = {
  id: "exec",
  kind: "verb",
  label: "exec",
  syntax: "daft exec --related -- <command>",
  summary:
    "Run one command across every related worktree — test the whole feature in one line.",
  available: (world) => world.repos.length > 0,
  fields: () => [
    { key: "command", label: "Command", kind: "text", value: "pnpm test" },
  ],
  command: execCommand,
  run(world, args) {
    const names = world.repos.map((r) => r.name);
    return {
      title: "Exec",
      cam: camFor(world),
      beats: [
        { cmd: execCommand(world, args) },
        { out: `✓ ${names.join("   ✓ ")}`, tone: "ok" },
        { pause: 0.8 },
      ],
    };
  },
};

const push: OpSpec = {
  id: "push",
  kind: "verb",
  label: "push",
  syntax: "daft push [<branch>]",
  summary:
    "Push a branch from its own worktree — pre-push hooks run against the tree they guard; upstream set.",
  available: (world) => pushable(world).length > 0,
  fields(world) {
    const pool = pushable(world);
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
  command: pushCommand,
  run(world, args) {
    const cmd = pushCommand(world, args);
    const { repoName, branch } = resolvePushTarget(world, args);
    const wt = findWt(world, repoName, branch);
    if (wt) wt.pushed = true;
    return {
      title: `Push ${shortBranch(branch)}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { out: `${repoName}: ${branch} → origin · upstream set` },
        { pause: 0.8 },
      ],
    };
  },
};

function resolveRemoveTarget(
  world: World,
  args: VerbArgs,
): { repoName: string; branch: string; cross: boolean } {
  const pool = removable(world);
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const cut = target.indexOf(":");
  const repoName = target.slice(0, cut);
  return {
    repoName,
    branch: target.slice(cut + 1),
    cross: repoName !== world.repos[0]?.name,
  };
}

const removeCommand = (world: World, args: VerbArgs): string => {
  const t = resolveRemoveTarget(world, args);
  return t.cross
    ? `daft remove --repo ${t.repoName} ${t.branch}`
    : `daft remove ${t.branch}`;
};

const remove: OpSpec = {
  id: "remove",
  kind: "verb",
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
  command: removeCommand,
  run(world, args) {
    const cmd = removeCommand(world, args);
    const { repoName, branch } = resolveRemoveTarget(world, args);
    const wt = findWt(world, repoName, branch);
    if (wt) wt.removed = true;
    clampCwd(world);
    const beats: Beat[] = [
      { cmd },
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

const sync: OpSpec = {
  id: "sync",
  kind: "verb",
  label: "sync",
  syntax: "daft sync [--push]",
  summary:
    "Every main pulls fresh, every worktree gets the signal, merged shells are pruned — and with --push, branches leave for the forge too.",
  available: (world) => world.repos.length > 0,
  fields: () => [
    {
      key: "push",
      label: "Also push (--push)",
      kind: "choice",
      choices: ["no", "yes"],
      value: "no",
    },
  ],
  command: syncCommand,
  run(world, args) {
    const beats: Beat[] = [{ cmd: syncCommand(world, args) }];
    for (const r of world.repos)
      beats.push({ act: { kind: "sync", repo: r.name } });
    const n = world.repos.length;
    beats.push({
      out: `updated ${n} main${n === 1 ? "" : "s"} · bases refreshed`,
    });
    beats.push(...pruneBeats(world));
    clampCwd(world);
    // Pushing is opt-in (daft sync --push): survivors leave for the forge.
    if (str(args, "push", "no") === "yes") {
      let pushed = 0;
      for (const r of world.repos)
        for (const wt of activeWts(r))
          if (wt.branch !== "main" && !wt.merged && !wt.pushed) {
            wt.pushed = true;
            pushed++;
          }
      if (pushed)
        beats.push(
          { pause: 0.4 },
          {
            out: `pushed ${pushed} branch${pushed === 1 ? "" : "es"} → origin`,
          },
        );
    }
    beats.push({ pause: 1.4 });
    return { title: "Sync", cam: camFor(world), beats };
  },
};

const go: OpSpec = {
  id: "go",
  kind: "verb",
  label: "go",
  syntax: "daft go <branch> — or daft go <repo> [<branch>]",
  summary:
    "Jump to a worktree — in this repo or any cataloged one. Read-only navigation; a wrong guess is harmless.",
  available: (world) => goTargets(world).length > 0,
  fields(world) {
    const pool = goTargets(world);
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
  command: goCommand,
  run(world, args) {
    const cmd = goCommand(world, args);
    const { repoName, branch } = resolveGoTarget(world, args);
    world.cwd = `~/${repoName}/${branch}`;
    return {
      title: `Go to ${shortBranch(branch)}`,
      cam: camFor(world),
      beats: [{ cmd }, { out: `→ ~/${repoName}/${branch}/` }, { pause: 0.8 }],
    };
  },
};

const merge: OpSpec = {
  id: "merge",
  kind: "verb",
  label: "merge",
  syntax: "daft merge <branch>",
  summary:
    "Land a branch into main through the quality gates, then the worktree comes down — the payload travels home.",
  available: (world) => mergeSources(world).length > 0,
  fields(world) {
    const pool = mergeSources(world);
    return [
      {
        key: "target",
        label: "Branch",
        kind: "choice",
        choices: pool,
        value: pool[0] ?? "",
      },
    ];
  },
  command: mergeCommand,
  run(world, args) {
    const cmd = mergeCommand(world, args);
    const { repoName, branch } = resolveMergeSource(world, args);
    const wt = findWt(world, repoName, branch);
    if (wt) wt.merged = true;
    const beats: Beat[] = [
      { cmd },
      { out: "✓ fmt  ✓ clippy  ✓ tests", tone: "ok" },
      { act: { kind: "merged", repo: repoName, wt: branch } },
      { out: `merged into main · worktree removed` },
      { pause: 0.6 },
    ];
    if (wt) wt.removed = true;
    clampCwd(world);
    beats.push(
      { act: { kind: "remove", repo: repoName, wt: branch } },
      { pause: 1 },
    );
    return {
      title: `Merge ${shortBranch(branch)}`,
      cam: camFor(world),
      beats,
    };
  },
};

const prune: OpSpec = {
  id: "prune",
  kind: "verb",
  label: "prune",
  syntax: "daft prune",
  summary:
    "Tear down every merged shell — their work already traveled home; rust rings finish the story.",
  available: prunable,
  fields: () => [],
  command: pruneCommand,
  run(world) {
    const beats: Beat[] = [{ cmd: pruneCommand() }];
    beats.push(...pruneBeats(world));
    clampCwd(world);
    beats.push({ pause: 1.2 });
    return { title: "Prune", cam: camFor(world), beats };
  },
};

const update: OpSpec = {
  id: "update",
  kind: "verb",
  label: "update",
  syntax: "daft update",
  summary:
    "Pull fresh bases and rebase every worktree — the teal signal fans out; nothing is pruned, nothing is pushed.",
  available: (world) => world.repos.length > 0,
  fields: () => [],
  command: updateCommand,
  run(world) {
    const beats: Beat[] = [{ cmd: updateCommand() }];
    for (const r of world.repos)
      beats.push({ act: { kind: "sync", repo: r.name } });
    const n = world.repos.reduce((sum, r) => sum + activeWts(r).length, 0);
    beats.push(
      { out: `updated ${n} worktree${n === 1 ? "" : "s"} · bases refreshed` },
      { pause: 1.2 },
    );
    return { title: "Update", cam: camFor(world), beats };
  },
};

const carry: OpSpec = {
  id: "carry",
  kind: "verb",
  label: "carry",
  syntax: "daft carry <branch> [--copy]",
  summary:
    "Move uncommitted changes to the worktree they belong in — a gold payload flies across; --copy leaves the source intact.",
  available: (world) => goTargets(world).length > 1,
  fields(world) {
    const pool = goTargets(world);
    return [
      {
        key: "from",
        label: "From",
        kind: "choice",
        choices: pool,
        value: pool[1] ?? pool[0] ?? "",
      },
      {
        key: "to",
        label: "To",
        kind: "choice",
        choices: pool,
        value: pool[0] ?? "",
      },
      {
        key: "mode",
        label: "Mode",
        kind: "choice",
        choices: ["move", "copy"],
        value: "move",
      },
    ];
  },
  command: carryCommand,
  run(world, args) {
    const cmd = carryCommand(world, args);
    const { from, to, copy } = resolveCarry(world, args);
    const cutA = from.indexOf(":");
    const cutB = to.indexOf(":");
    const a: [string, string] = [from.slice(0, cutA), from.slice(cutA + 1)];
    const b: [string, string] = [to.slice(0, cutB), to.slice(cutB + 1)];
    return {
      title: `Carry to ${shortBranch(b[1])}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { act: { kind: "carry", a, b, ...(copy ? { copy: true } : {}) } },
        {
          out: copy ? `changes copied to ${to}` : `changes moved to ${to}`,
        },
        { pause: 1.2 },
      ],
    };
  },
};

const rename: OpSpec = {
  id: "rename",
  kind: "verb",
  label: "rename",
  syntax: "daft rename <source> <new-branch>",
  summary:
    "Rename a branch and its worktree directory together — the label cross-fades under a gold attention ring.",
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
      {
        key: "to",
        label: "New branch",
        kind: "text",
        value: nextBranch(world),
      },
    ];
  },
  command: renameCommand,
  run(world, args) {
    const cmd = renameCommand(world, args);
    const { repoName, branch, to } = resolveRename(world, args);
    const wt = findWt(world, repoName, branch);
    if (wt) wt.branch = to;
    return {
      title: `Rename to ${shortBranch(to)}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { act: { kind: "rename", repo: repoName, wt: branch, to } },
        { out: `renamed ${branch} → ${to} · branch + directory` },
        { pause: 1.2 },
      ],
    };
  },
};

const repoAdd: OpSpec = {
  id: "repo-add",
  kind: "verb",
  label: "repo add",
  syntax: "daft repo add [<path>] [--name <name>]",
  summary:
    "Register an existing repository in daft's catalog — it joins the story with its main worktree.",
  available: () => true,
  fields(world) {
    return [
      { key: "name", label: "Repo", kind: "text", value: nextRepoName(world) },
    ];
  },
  command: repoAddCommand,
  run(world, args) {
    const cmd = repoAddCommand(world, args);
    const name = resolveCloneName(world, args);
    const repo = addRepo(world, name);
    return {
      title: `Add ${name}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { act: { kind: "repo", repo: name, x: repo.x, y: repo.y } },
        { act: { kind: "wt", repo: name, wt: "main" } },
        { out: `registered ${name} · 1 worktree` },
        { pause: 1 },
      ],
    };
  },
};

const repoRemove: OpSpec = {
  id: "repo-remove",
  kind: "verb",
  label: "repo remove",
  syntax: "daft repo remove <name>",
  summary:
    "Unregister a repository — it leaves daft's world whole: worktrees tear down staggered, links fade with it.",
  available: (world) => world.repos.length > 0,
  fields(world) {
    const pool = world.repos.map((r) => r.name);
    return [
      {
        key: "name",
        label: "Repo",
        kind: "choice",
        choices: pool,
        value: pool[pool.length - 1] ?? "",
      },
    ];
  },
  command: repoRemoveCommand,
  run(world, args) {
    const cmd = repoRemoveCommand(world, args);
    const name = resolveRepoName(world, args);
    world.repos = world.repos.filter((r) => r.name !== name);
    world.rels = world.rels.filter(([a, b]) => a !== name && b !== name);
    clampCwd(world);
    return {
      title: `Remove ${name}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { act: { kind: "repo-remove", repo: name } },
        { out: `removed ${name} from the catalog`, tone: "rust" },
        { pause: 1.6 },
      ],
    };
  },
};

const repoLink: OpSpec = {
  id: "repo-link",
  kind: "verb",
  label: "repo link",
  syntax: "daft repo link <target>",
  summary:
    "Relate two repositories — the dashed teal line that lets features span them.",
  available: (world) => unrelatedPairs(world).length > 0,
  fields(world) {
    const names = world.repos.map((r) => r.name);
    const first = unrelatedPairs(world)[0];
    return [
      {
        key: "a",
        label: "From",
        kind: "choice",
        choices: names,
        value: first?.[0] ?? "",
      },
      {
        key: "b",
        label: "To",
        kind: "choice",
        choices: names,
        value: first?.[1] ?? "",
      },
    ];
  },
  command: repoLinkCommand,
  run(world, args) {
    const cmd = repoLinkCommand(world, args);
    const [a, b] = resolveLinkPair(world, args);
    if (!related(world, a, b)) world.rels.push([a, b]);
    return {
      title: `Link ${a} ↔ ${b}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { act: { kind: "relate", a, b } },
        { out: `linked ${a} ↔ ${b}` },
        { pause: 1 },
      ],
    };
  },
};

const repoUnlink: OpSpec = {
  id: "repo-unlink",
  kind: "verb",
  label: "repo unlink",
  syntax: "daft repo unlink <target>",
  summary:
    "Cut a relation — the dashed line retracts and dies in rust; the repos stay.",
  available: (world) => world.rels.length > 0,
  fields(world) {
    const pool = world.rels.map(([a, b]) => `${a} ↔ ${b}`);
    return [
      {
        key: "target",
        label: "Relation",
        kind: "choice",
        choices: pool,
        value: pool[0] ?? "",
      },
    ];
  },
  command: repoUnlinkCommand,
  run(world, args) {
    const cmd = repoUnlinkCommand(world, args);
    const [a, b] = resolveUnlinkPair(world, args);
    world.rels = world.rels.filter(
      ([x, y]) => !((x === a && y === b) || (x === b && y === a)),
    );
    return {
      title: `Unlink ${a} ↔ ${b}`,
      cam: camFor(world),
      beats: [
        { cmd },
        { act: { kind: "unrelate", a, b } },
        { out: `unlinked ${a} ↔ ${b}`, tone: "rust" },
        { pause: 1.2 },
      ],
    };
  },
};

const cdCommand = (_world: World, args: VerbArgs): string =>
  `cd ${str(args, "path", "~")}`;

const cd: OpSpec = {
  id: "cd",
  kind: "verb",
  typedOnly: true,
  label: "cd",
  syntax: "cd <path>",
  summary:
    "Walk the Files tree — the shell's position is where worktree-scoped verbs run.",
  available: (world) => world.repos.length > 0,
  fields(world) {
    const paths = ["~"];
    for (const r of world.repos) {
      paths.push(`~/${r.name}`);
      for (const w of activeWts(r)) paths.push(`~/${r.name}/${w.branch}`);
    }
    return [
      {
        key: "path",
        label: "Path",
        kind: "choice",
        choices: paths,
        value: world.cwd ?? "~",
      },
    ];
  },
  command: cdCommand,
  run(world, args) {
    const cmd = cdCommand(world, args);
    const outcome = resolveCd(world, world.cwd ?? "~", str(args, "path", "~"));
    const beats: Beat[] = [{ cmd }];
    if ("cwd" in outcome) {
      world.cwd = outcome.cwd;
    } else {
      // The target vanished under a later edit — the projection tells it.
      beats.push({ out: `cd: ${outcome.error}`, tone: "rust" });
    }
    beats.push({ pause: 0.5 });
    return {
      title: `cd ${str(args, "path", "~")}`,
      cam: camFor(world),
      beats,
    };
  },
};

/* --------------------------------- events -------------------------------- */

/** Non-main, unmerged worktrees by agent presence — event target pools. */
function agentCandidates(world: World, joined: boolean): string[] {
  const out: string[] = [];
  for (const r of world.repos)
    for (const w of activeWts(r))
      if (w.branch !== "main" && w.agent === joined && !w.merged)
        out.push(`${r.name}:${w.branch}`);
  return out;
}

function resolveEventTarget(
  pool: string[],
  args: VerbArgs,
): { target: string; repoName: string; branch: string } {
  let target = str(args, "target", pool[0] ?? "");
  if (!pool.includes(target)) target = pool[0] ?? target;
  const cut = target.indexOf(":");
  return {
    target,
    repoName: target.slice(0, cut),
    branch: target.slice(cut + 1),
  };
}

const agentJoins: OpSpec = {
  id: "agent-joins",
  kind: "event",
  label: "agent joins",
  syntax: "an agent picks up a worktree",
  summary:
    "An AI coding agent docks on a worktree and machines it — purple belongs to agents alone.",
  available: (world) => agentCandidates(world, false).length > 0,
  fields(world) {
    const pool = agentCandidates(world, false);
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
  command: () => null,
  run(world, args) {
    const { target, repoName, branch } = resolveEventTarget(
      agentCandidates(world, false),
      args,
    );
    const wt = findWt(world, repoName, branch);
    if (wt) wt.agent = true;
    return {
      title: `Agent joins ${shortBranch(branch)}`,
      cam: camFor(world),
      beats: [
        { out: `# agent picks up ${target}`, tone: "agent" },
        { act: { kind: "agent", repo: repoName, wt: branch } },
        { pause: 1.6 },
      ],
    };
  },
};

const agentLeaves: OpSpec = {
  id: "agent-leaves",
  kind: "event",
  label: "agent leaves",
  syntax: "an agent finishes and leaves",
  summary:
    "The agent undocks: the bot drifts off, the purple decays, and the node is yours again.",
  available: (world) => agentCandidates(world, true).length > 0,
  fields(world) {
    const pool = agentCandidates(world, true);
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
  command: () => null,
  run(world, args) {
    const { target, repoName, branch } = resolveEventTarget(
      agentCandidates(world, true),
      args,
    );
    const wt = findWt(world, repoName, branch);
    if (wt) wt.agent = false;
    return {
      title: `Agent leaves ${shortBranch(branch)}`,
      cam: camFor(world),
      beats: [
        { out: `# agent done with ${target}`, tone: "agent" },
        { act: { kind: "agent-leave", repo: repoName, wt: branch } },
        { pause: 1.6 },
      ],
    };
  },
};

const forgeMerges: OpSpec = {
  id: "forge-merges",
  kind: "event",
  label: "forge merges",
  syntax: "the forge reviews and merges a pushed branch",
  summary:
    "PRs approved: every pushed worktree of the branch merges, and each gold payload travels home.",
  available: (world) => forgeable(world).length > 0,
  fields(world) {
    const pool = forgeable(world);
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
  command: () => null,
  run(world, args) {
    const pool = forgeable(world);
    let branch = str(args, "branch", pool[0] ?? "");
    if (!pool.includes(branch)) branch = pool[0] ?? branch;
    const targets = world.repos.filter((r) =>
      activeWts(r).some((w) => w.branch === branch && w.pushed && !w.merged),
    );
    const beats: Beat[] = [
      {
        out: `# forge: ${branch} approved → merged in ${targets
          .map((r) => r.name)
          .join(", ")}`,
        tone: "dim",
      },
    ];
    targets.forEach((r, i) => {
      const wt = activeWts(r).find((w) => w.branch === branch);
      if (wt) wt.merged = true;
      beats.push({ act: { kind: "merged", repo: r.name, wt: branch } });
      if (i < targets.length - 1) beats.push({ pause: 0.25 });
    });
    beats.push({ pause: 0.9 });
    return {
      title: `Forge merges ${shortBranch(branch)}`,
      cam: camFor(world),
      beats,
    };
  },
};

/* ------------------------------- registry -------------------------------- */

export const OPS: OpSpec[] = [
  clone,
  start,
  go,
  list,
  exec,
  push,
  merge,
  carry,
  rename,
  update,
  prune,
  remove,
  sync,
  cd,
  repoAdd,
  repoRemove,
  repoLink,
  repoUnlink,
  agentJoins,
  agentLeaves,
  forgeMerges,
];

/** Verbs only — the part of the registry the shell can spell. */
export const VERBS: OpSpec[] = OPS.filter((o) => o.kind === "verb");

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

/** Authoring geometry threaded into a build (shape-compatible with the
 * composer document's Placements). */
export interface BuildOptions {
  placements?: {
    repos?: Record<string, { x: number; y: number }>;
    wts?: Record<string, { ang: number; dist: number }>;
  };
}

/**
 * Stamp author-pinned polar placements onto a step's `wt` acts (keyed
 * `"repo:branch"`) so the renderer seats those worktrees exactly where the
 * author dragged them.
 */
export function patchWtPlacements(
  beats: Beat[],
  wts: Record<string, { ang: number; dist: number }> | undefined,
): void {
  if (!wts) return;
  for (const beat of beats) {
    if (!("act" in beat) || beat.act.kind !== "wt") continue;
    const pin = wts[`${beat.act.repo}:${beat.act.wt}`];
    if (pin) {
      beat.act.ang = pin.ang;
      beat.act.dist = pin.dist;
    }
  }
}

export function buildScenario(
  invocations: VerbInvocation[],
  opts?: BuildOptions,
): Scenario {
  const world = emptyWorld();
  if (opts?.placements?.repos) world.spots = opts.placements.repos;
  const steps: StepDef[] = [];
  const mapping: number[] = [];
  for (const inv of invocations) {
    const spec = OPS.find((o) => o.id === inv.verb);
    if (!spec?.available(world)) {
      mapping.push(-1);
      continue;
    }
    mapping.push(steps.length);
    const step = spec.run(world, inv.args);
    patchWtPlacements(step.beats, opts?.placements?.wts);
    steps.push(step);
  }
  return { steps, world, mapping };
}

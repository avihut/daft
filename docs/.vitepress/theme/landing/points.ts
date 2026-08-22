/**
 * The five landing points — the page's argument, in workflow order: start,
 * switch, span, agents, ship. Each point is a claim, two or three sentences
 * of mechanism (what daft does, in git terms), a link into the docs, and a
 * demonstration. Demonstrations are built from the verb registry
 * (`buildScenario` over invocations), never hand-authored, so a point cannot
 * show something the verbs do not do; their compiled digests are goldens
 * (tests/golden/compiled.test.ts). Rules for changing this list live in
 * landing/CLAUDE.md — five points, swap rather than append.
 */

import type { StepDef } from "../graph/pack";
import { buildScenario, type VerbInvocation } from "../graph/verbs";

export type PointDemo =
  | { kind: "scene"; invocations: VerbInvocation[] }
  /** The before/after transcript panes (HomeLanding.vue) — the one point
   * where the proof is the loop you stop typing, not a graph change. */
  | { kind: "switch" };

export interface LandingPoint {
  /** Stable id: the section anchor and the dev player handle suffix. */
  id: string;
  title: string;
  /** Short paragraphs; `code` spans are written in backticks. */
  body: string[];
  link: { text: string; href: string };
  demo: PointDemo;
}

function clone(name: string): VerbInvocation {
  return { verb: "clone", args: { name } };
}
function start(branch: string, repo?: string): VerbInvocation {
  return { verb: "start", args: repo ? { branch, repo } : { branch } };
}

export const LANDING_POINTS: LandingPoint[] = [
  {
    id: "directory",
    title: "Every branch gets its own directory.",
    body: [
      "`daft start checkout` creates the branch and checks it out into a directory of its own, then runs the hooks you wrote for a new worktree — install, env, build cache — so it is ready to work in before you arrive.",
      "Start as many as you need. Each keeps its own dependencies, its own build, and its own dev server port.",
    ],
    link: { text: "Worktrees", href: "/worktrees/" },
    demo: {
      kind: "scene",
      invocations: [
        clone("web"),
        start("checkout"),
        start("bugfix/cart-total"),
        { verb: "go", args: { target: "web:checkout" } },
      ],
    },
  },
  {
    id: "switch",
    title: "Switching costs nothing.",
    body: [
      "Another branch is another directory, so there is nothing to stash and nothing to rebuild. `daft go feature-b` moves your shell there; what you installed and built is still warm.",
      "Come back with `daft go -` and your editor, your server, and your uncommitted changes are exactly where you left them.",
    ],
    link: { text: "Why daft", href: "/about/why-daft" },
    demo: { kind: "switch" },
  },
  {
    id: "span",
    title: "One feature across many repos — still one thing.",
    body: [
      "When a change crosses repos, `daft start orders checkout` opens the same branch in the related repo and links the two worktrees. `daft exec --related -- pnpm test` then runs across every worktree the feature touches, and `daft sync` keeps all of them current.",
      "The repo graph — daft's catalog of where your repos live and how they relate — is what makes that one command instead of four.",
    ],
    link: { text: "The repo graph", href: "/graph/" },
    demo: {
      kind: "scene",
      invocations: [
        clone("web"),
        start("checkout"),
        start("checkout", "orders"),
        { verb: "exec", args: { command: "pnpm test" } },
      ],
    },
  },
  {
    id: "agents",
    title: "Agents work in parallel, each in its own worktree.",
    body: [
      "Give every coding agent a worktree and they stop stepping on each other: separate directories, separate ports, separate branches, one repo. The daft agent skill teaches them the verbs, so an agent starts its own worktree the way you would.",
      "`daft list` shows who is where.",
    ],
    link: { text: "The agent skill", href: "/reference/agent-skill" },
    demo: {
      kind: "scene",
      invocations: [
        clone("web"),
        start("checkout"),
        start("bugfix/cart-total"),
        { verb: "agent-joins", args: { target: "web:checkout" } },
        { verb: "agent-joins", args: { target: "web:bugfix/cart-total" } },
        { verb: "list", args: {} },
      ],
    },
  },
  {
    id: "ship",
    title: "Ship and clean up from anywhere.",
    body: [
      "`daft merge checkout` runs from whichever directory you are standing in: your pre-merge hooks gate it — fmt, clippy, tests, whatever you wire up — the merge lands in main's worktree, and with `-r` the source worktree comes down. No checkout, no losing your place.",
      "When the forge merges the rest, `daft sync` refreshes every base and prunes the branches that already landed.",
    ],
    link: { text: "Merging", href: "/worktrees/merging" },
    demo: {
      kind: "scene",
      invocations: [
        clone("web"),
        start("checkout"),
        start("bugfix/cart-total"),
        { verb: "merge", args: { target: "web:checkout" } },
        { verb: "push", args: { target: "web:bugfix/cart-total" } },
        { verb: "forge-merges", args: { branch: "bugfix/cart-total" } },
        { verb: "sync", args: {} },
      ],
    },
  },
];

/** The point's playable script, or null for the static demonstrations. */
export function pointScript(point: LandingPoint): StepDef[] | null {
  return point.demo.kind === "scene"
    ? buildScenario(point.demo.invocations).steps
    : null;
}

export interface Segment {
  text: string;
  code: boolean;
}

/** Split a body paragraph into text and backticked code spans. */
export function segments(text: string): Segment[] {
  const out: Segment[] = [];
  let last = 0;
  for (const m of text.matchAll(/`([^`]+)`/g)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ text: text.slice(last, at), code: false });
    out.push({ text: m[1], code: true });
    last = at + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last), code: false });
  return out;
}

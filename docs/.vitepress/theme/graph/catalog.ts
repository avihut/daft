/**
 * The verb catalog: every daft verb with a demo scenario showing its
 * canonical animation. Each entry's `focus` names the invocation that IS
 * the verb — the catalog seeks straight to its checkpoint (context built
 * instantly by event replay, no preamble to sit through) and plays the verb
 * from there. Grow this file in the same change that grows `verbs.ts`.
 */

import type { VerbInvocation } from "./verbs";

function clone(name: string): VerbInvocation {
  return { verb: "clone", args: { name } };
}

function start(branch: string, repo?: string): VerbInvocation {
  return { verb: "start", args: repo ? { branch, repo } : { branch } };
}

export interface CatalogEntry {
  id: string;
  title: string;
  syntax: string;
  blurb: string;
  /** The demo scenario; `focus` indexes the invocation that IS this verb. */
  demo: VerbInvocation[];
  focus: number;
}

export const CATALOG: CatalogEntry[] = [
  {
    id: "clone",
    title: "clone",
    syntax: "daft clone <url>",
    blurb:
      "A repo enters as an ink disc; main/ arrives as a ready worktree with its own dev port.",
    demo: [clone("web")],
    focus: 0,
  },
  {
    id: "start",
    title: "start",
    syntax: "daft start <branch>",
    blurb:
      "A worktree orbits out on its edge, boots under the teal setup ring, and claims the next port.",
    demo: [clone("web"), start("checkout")],
    focus: 1,
  },
  {
    id: "start-repo",
    title: "start <repo>",
    syntax: "daft start <repo> <branch>",
    blurb:
      "The same feature spans repos: the second repo joins the story, related in teal, the branches arced in gold.",
    demo: [clone("web"), start("checkout"), start("checkout", "orders")],
    focus: 2,
  },
  {
    id: "list",
    title: "list",
    syntax: "daft list",
    blurb:
      "Worktrees, ports, and status at a glance — a read-only mirror of the graph; nothing moves.",
    demo: [
      clone("web"),
      start("checkout"),
      start("bugfix/cart-total"),
      { verb: "agent-joins", args: { target: "web:bugfix/cart-total" } },
      { verb: "list", args: {} },
    ],
    focus: 4,
  },
  {
    id: "agent-joins",
    title: "agent joins",
    syntax: "event · an agent picks up a worktree",
    blurb:
      "An AI agent docks in purple and machines the node — an event in the world, not a daft verb.",
    demo: [
      clone("web"),
      start("checkout"),
      {
        verb: "agent-joins",
        args: { target: "web:checkout" },
      },
    ],
    focus: 2,
  },
  {
    id: "agent-leaves",
    title: "agent leaves",
    syntax: "event · the agent finishes and leaves",
    blurb:
      "The bot drifts off, the purple decays, and a purple ring collapses inward — the node is yours again.",
    demo: [
      clone("web"),
      start("checkout"),
      { verb: "agent-joins", args: { target: "web:checkout" } },
      { verb: "agent-leaves", args: { target: "web:checkout" } },
    ],
    focus: 3,
  },
  {
    id: "exec",
    title: "exec",
    syntax: "daft exec --related -- <command>",
    blurb:
      "One command across the whole feature — no graph change, just reach.",
    demo: [
      clone("web"),
      start("checkout"),
      { verb: "exec", args: { command: "pnpm test" } },
    ],
    focus: 2,
  },
  {
    id: "go",
    title: "go",
    syntax: "daft go <branch> — or daft go <repo> [<branch>]",
    blurb:
      "Jump to any worktree in any cataloged repo — pure navigation, nothing in the graph moves.",
    demo: [
      clone("web"),
      start("checkout"),
      { verb: "go", args: { target: "web:checkout" } },
    ],
    focus: 2,
  },
  {
    id: "merge",
    title: "merge",
    syntax: "daft merge <branch>",
    blurb:
      "Quality gates pass, the payload travels home into main, and the worktree comes down in rust.",
    demo: [
      clone("web"),
      start("checkout"),
      { verb: "merge", args: { target: "web:checkout" } },
    ],
    focus: 2,
  },
  {
    id: "carry",
    title: "carry",
    syntax: "daft carry <branch> [--copy]",
    blurb:
      "Uncommitted changes fly as a gold payload from one worktree to another; on a move the source dims in flight.",
    demo: [
      clone("web"),
      start("checkout"),
      start("bugfix/cart-total"),
      {
        verb: "carry",
        args: {
          from: "web:bugfix/cart-total",
          to: "web:checkout",
          mode: "move",
        },
      },
    ],
    focus: 3,
  },
  {
    id: "rename",
    title: "rename",
    syntax: "daft rename <source> <new-branch>",
    blurb:
      "Branch and directory take the new name together — the label cross-fades under a gold attention ring.",
    demo: [
      clone("web"),
      start("checkout"),
      {
        verb: "rename",
        args: { target: "web:checkout", to: "feature/login" },
      },
    ],
    focus: 2,
  },
  {
    id: "update",
    title: "update",
    syntax: "daft update",
    blurb:
      "Bases pull fresh and the teal signal fans out to every worktree — no pruning, no pushing.",
    demo: [clone("web"), start("checkout"), { verb: "update", args: {} }],
    focus: 2,
  },
  {
    id: "prune",
    title: "prune",
    syntax: "daft prune",
    blurb:
      "Merged shells come down under rust rings — their work already traveled home.",
    demo: [
      clone("web"),
      start("checkout"),
      { verb: "push", args: { target: "web:checkout" } },
      { verb: "forge-merges", args: { branch: "checkout" } },
      { verb: "prune", args: {} },
    ],
    focus: 4,
  },
  {
    id: "push",
    title: "push",
    syntax: "daft push [<branch>]",
    blurb:
      "The branch leaves for the forge from its own worktree — hooks run against the tree they guard.",
    demo: [
      clone("web"),
      start("checkout"),
      { verb: "push", args: { target: "web:checkout" } },
    ],
    focus: 2,
  },
  {
    id: "forge-merges",
    title: "forge merges",
    syntax: "event · the forge reviews and merges",
    blurb:
      "PRs approved: each pushed worktree merges, gold payloads slide home, and the shells hollow out.",
    demo: [
      clone("web"),
      start("checkout"),
      start("checkout", "orders"),
      { verb: "push", args: { target: "web:checkout" } },
      { verb: "push", args: { target: "orders:checkout" } },
      { verb: "forge-merges", args: { branch: "checkout" } },
    ],
    focus: 5,
  },
  {
    id: "remove",
    title: "remove",
    syntax: "daft remove <branch>",
    blurb:
      "The rust ring spins the setup ring backwards — worktree, branch, and port released.",
    demo: [
      clone("web"),
      start("spike/faster-cart"),
      { verb: "remove", args: { target: "web:spike/faster-cart" } },
    ],
    focus: 2,
  },
  {
    id: "repo-add",
    title: "repo add",
    syntax: "daft repo add [<path>]",
    blurb:
      "An existing repository registers into the catalog and joins the story with its main.",
    demo: [clone("web"), { verb: "repo-add", args: { name: "orders" } }],
    focus: 1,
  },
  {
    id: "repo-link",
    title: "repo link",
    syntax: "daft repo link <target>",
    blurb:
      "The dashed teal relation appears — the connective tissue features span.",
    demo: [
      clone("web"),
      clone("orders"),
      { verb: "repo-link", args: { a: "web", b: "orders" } },
    ],
    focus: 2,
  },
  {
    id: "repo-unlink",
    title: "repo unlink",
    syntax: "daft repo unlink <target>",
    blurb:
      "The relation retracts toward its owner, the crawl reverses, and the line dies in rust.",
    demo: [
      clone("web"),
      clone("orders"),
      { verb: "repo-link", args: { a: "web", b: "orders" } },
      { verb: "repo-unlink", args: { target: "web ↔ orders" } },
    ],
    focus: 3,
  },
  {
    id: "repo-remove",
    title: "repo remove",
    syntax: "daft repo remove <name>",
    blurb:
      "A repository leaves whole: worktrees tear down staggered, the disc shrinks under a collapsing rust ring, links fade with it.",
    demo: [
      clone("web"),
      clone("orders"),
      { verb: "repo-link", args: { a: "web", b: "orders" } },
      start("checkout", "orders"),
      { verb: "repo-remove", args: { name: "orders" } },
    ],
    focus: 4,
  },
  {
    id: "sync",
    title: "sync",
    syntax: "daft sync",
    blurb:
      "Teal signals fan out from every repo — bases refreshed — and merged shells are pruned.",
    demo: [
      clone("web"),
      start("checkout"),
      start("checkout", "orders"),
      { verb: "push", args: { target: "web:checkout" } },
      { verb: "push", args: { target: "orders:checkout" } },
      { verb: "forge-merges", args: { branch: "checkout" } },
      { verb: "sync", args: {} },
    ],
    focus: 6,
  },
];

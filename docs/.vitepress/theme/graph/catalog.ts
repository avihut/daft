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
    syntax: "daft start <branch> [<base>]",
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
      "Worktrees, ports, and who's working in each — agents dock in purple and machine their nodes.",
    demo: [
      clone("web"),
      start("checkout"),
      start("bugfix/cart-total"),
      {
        verb: "list",
        args: { agents: ["web:checkout", "web:bugfix/cart-total"] },
      },
    ],
    focus: 3,
  },
  {
    id: "exec",
    title: "exec",
    syntax: "daft exec --related -- <cmd>",
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
    id: "ship",
    title: "ship",
    syntax: "daft exec --related -- git push",
    blurb:
      "Push the branch everywhere it lives; the forge merges, each gold payload slides home, and the worktrees hollow out.",
    demo: [
      clone("web"),
      start("checkout"),
      start("checkout", "orders"),
      { verb: "ship", args: { branch: "checkout" } },
    ],
    focus: 3,
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
    id: "sync",
    title: "sync",
    syntax: "daft sync",
    blurb:
      "Teal signals fan out from every repo — bases refreshed — and merged shells are pruned.",
    demo: [
      clone("web"),
      start("checkout"),
      start("checkout", "orders"),
      { verb: "ship", args: { branch: "checkout" } },
      { verb: "sync", args: {} },
    ],
    focus: 4,
  },
];

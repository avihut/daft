/**
 * The landing-page story: one product (`acme` shop), three repos — the
 * storefront and the two services a real checkout feature must cross.
 * Every command is the documented daft grammar (see docs/graph/); the
 * camera starts inside `web` and widens only when the story forces it.
 */

import type { CamRect, StepDef } from "./engine";

const WEB = { x: 0, y: 0 };
const ORDERS = { x: 330, y: -150 };
const PAYMENTS = { x: 300, y: 200 };

const CAM_WEB: CamRect = { x: 20, y: 10, w: 460, h: 400 };
const CAM_TWO: CamRect = { x: 165, y: -65, w: 700, h: 540 };
const CAM_ALL: CamRect = { x: 165, y: 25, w: 690, h: 650 };

export const HERO_SCRIPT: StepDef[] = [
  {
    title: "Clone",
    cam: CAM_WEB,
    beats: [
      { cmd: "daft clone git@github.com:acme/web.git" },
      { act: { kind: "repo", repo: "web", ...WEB } },
      { act: { kind: "wt", repo: "web", wt: "main" } },
      { out: "cloned web · worktree main/ ready" },
      { act: { kind: "port", repo: "web", wt: "main", port: ":3000" } },
      { out: "dev server → :3000" },
    ],
  },
  {
    title: "Boot",
    cam: CAM_WEB,
    beats: [
      { cmd: "daft start checkout" },
      { act: { kind: "wt", repo: "web", wt: "checkout" } },
      { out: "worktree checkout/ created" },
      { act: { kind: "boot", repo: "web", wt: "checkout", secs: 2.4 } },
      { out: "✓ pnpm install  ✓ .envrc  ✓ build cache", tone: "ok" },
      { pause: 0.6 },
      { act: { kind: "port", repo: "web", wt: "checkout", port: ":3001" } },
      { out: "dev server → :3001" },
    ],
  },
  {
    title: "Parallel",
    cam: CAM_WEB,
    beats: [
      { cmd: "daft start bugfix/cart-total" },
      { act: { kind: "wt", repo: "web", wt: "bugfix/cart-total" } },
      {
        act: { kind: "boot", repo: "web", wt: "bugfix/cart-total", secs: 1.6 },
      },
      { pause: 1.1 },
      {
        act: {
          kind: "port",
          repo: "web",
          wt: "bugfix/cart-total",
          port: ":3002",
        },
      },
      { out: "ready → :3002" },
      { cmd: "daft start spike/faster-cart" },
      { act: { kind: "wt", repo: "web", wt: "spike/faster-cart" } },
      { pause: 0.9 },
      {
        act: {
          kind: "port",
          repo: "web",
          wt: "spike/faster-cart",
          port: ":3003",
        },
      },
      { out: "ready → :3003" },
    ],
  },
  {
    title: "Agents",
    cam: CAM_WEB,
    beats: [
      { cmd: "daft list" },
      { out: "main               :3000  clean" },
      { act: { kind: "agent", repo: "web", wt: "checkout" } },
      { out: "checkout           :3001  agent · building", tone: "agent" },
      { act: { kind: "agent", repo: "web", wt: "bugfix/cart-total" } },
      { out: "bugfix/cart-total  :3002  agent · testing", tone: "agent" },
      { out: "spike/faster-cart  :3003  you" },
      { pause: 1.2 },
    ],
  },
  {
    title: "Span repos",
    cam: CAM_TWO,
    beats: [
      { cmd: "daft start orders checkout" },
      { act: { kind: "repo", repo: "orders", ...ORDERS } },
      { act: { kind: "wt", repo: "orders", wt: "main" } },
      { act: { kind: "port", repo: "orders", wt: "main", port: ":4000" } },
      { act: { kind: "relate", a: "web", b: "orders" } },
      { out: "orders: worktree checkout/ ready" },
      { act: { kind: "wt", repo: "orders", wt: "checkout" } },
      { act: { kind: "boot", repo: "orders", wt: "checkout", secs: 1.3 } },
      {
        act: {
          kind: "arc",
          a: ["web", "checkout"],
          b: ["orders", "checkout"],
        },
      },
      { pause: 0.8 },
      { act: { kind: "port", repo: "orders", wt: "checkout", port: ":4001" } },
      { out: "dev server → :4001" },
      { cam: CAM_ALL },
      { cmd: "daft start payments checkout" },
      { act: { kind: "repo", repo: "payments", ...PAYMENTS } },
      { act: { kind: "wt", repo: "payments", wt: "main" } },
      { act: { kind: "port", repo: "payments", wt: "main", port: ":5000" } },
      { act: { kind: "relate", a: "orders", b: "payments" } },
      { act: { kind: "relate", a: "web", b: "payments" } },
      { out: "payments: worktree checkout/ ready" },
      { act: { kind: "wt", repo: "payments", wt: "checkout" } },
      { act: { kind: "boot", repo: "payments", wt: "checkout", secs: 1.3 } },
      {
        act: {
          kind: "arc",
          a: ["orders", "checkout"],
          b: ["payments", "checkout"],
        },
      },
      { pause: 0.8 },
      {
        act: { kind: "port", repo: "payments", wt: "checkout", port: ":5001" },
      },
      { out: "dev server → :5001" },
    ],
  },
  {
    title: "Ship",
    cam: CAM_ALL,
    beats: [
      { cmd: "daft exec --related -- pnpm test" },
      { out: "✓ web   ✓ orders   ✓ payments", tone: "ok" },
      { cmd: "daft exec --related -- git push" },
      { out: "checkout → origin in web, orders, payments" },
      { pause: 0.6 },
      { out: "# PRs reviewed → merged on the forge", tone: "dim" },
      { act: { kind: "merged", repo: "web", wt: "checkout" } },
      { pause: 0.25 },
      { act: { kind: "merged", repo: "orders", wt: "checkout" } },
      { pause: 0.25 },
      { act: { kind: "merged", repo: "payments", wt: "checkout" } },
      { pause: 0.8 },
    ],
  },
  {
    title: "Clean up",
    cam: CAM_ALL,
    beats: [
      { cmd: "daft remove spike/faster-cart" },
      { act: { kind: "remove", repo: "web", wt: "spike/faster-cart" } },
      { out: "removed spike/faster-cart · :3003 released", tone: "rust" },
      { cmd: "daft sync" },
      { act: { kind: "sync", repo: "web" } },
      { act: { kind: "sync", repo: "orders" } },
      { act: { kind: "sync", repo: "payments" } },
      { out: "updated 3 mains · bases refreshed" },
      { pause: 0.6 },
      { out: "pruned checkout in web, orders, payments", tone: "rust" },
      { act: { kind: "remove", repo: "web", wt: "checkout" } },
      { pause: 0.3 },
      { act: { kind: "remove", repo: "orders", wt: "checkout" } },
      { pause: 0.3 },
      { act: { kind: "remove", repo: "payments", wt: "checkout" } },
      { pause: 3.4 },
    ],
  },
];

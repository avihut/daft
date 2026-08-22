/**
 * Scripts the playground can replay. The hero story ships first; the
 * vocabulary tour exists so every act kind has a place to be seen (and
 * broken) in isolation — extend it in the same change that grows the
 * language, so the playground always demonstrates the full grammar.
 */

import { HERO_SCRIPT } from "./hero-script";
import type { StepDef } from "./pack";
import { buildScenario } from "./verbs";

const APP = { x: 0, y: 0 };
const SVC = { x: 320, y: -40 };

const CAM_ONE = { x: 30, y: 0, w: 480, h: 420 };
const CAM_TWO = { x: 150, y: -10, w: 700, h: 520 };

const VOCABULARY_SCRIPT: StepDef[] = [
  {
    title: "Repo",
    cam: CAM_ONE,
    beats: [
      { cmd: "daft clone git@github.com:acme/app.git" },
      { act: { kind: "repo", repo: "app", ...APP } },
      { act: { kind: "wt", repo: "app", wt: "main" } },
      { out: "cloned app · worktree main/ ready" },
      { act: { kind: "port", repo: "app", wt: "main", port: ":3000" } },
      { out: "dev server → :3000" },
    ],
  },
  {
    title: "Worktree",
    cam: CAM_ONE,
    beats: [
      { cmd: "daft start feature/login" },
      { act: { kind: "wt", repo: "app", wt: "feature/login" } },
      { act: { kind: "boot", repo: "app", wt: "feature/login", secs: 1.8 } },
      { out: "✓ install  ✓ .envrc  ✓ build cache", tone: "ok" },
      { pause: 1 },
      {
        act: { kind: "port", repo: "app", wt: "feature/login", port: ":3001" },
      },
      { out: "ready → :3001" },
    ],
  },
  {
    title: "Agent joins & leaves",
    cam: CAM_ONE,
    beats: [
      { out: "# an agent picks up app:feature/login", tone: "agent" },
      { act: { kind: "agent", repo: "app", wt: "feature/login" } },
      { pause: 2.4 },
      { out: "# agent done with app:feature/login", tone: "agent" },
      { act: { kind: "agent-leave", repo: "app", wt: "feature/login" } },
      { pause: 1.4 },
    ],
  },
  {
    title: "Carry",
    cam: CAM_ONE,
    beats: [
      { cmd: "daft start spike/faster-cart" },
      { act: { kind: "wt", repo: "app", wt: "spike/faster-cart" } },
      { pause: 0.8 },
      { cmd: "daft carry feature/login" },
      {
        act: {
          kind: "carry",
          a: ["app", "spike/faster-cart"],
          b: ["app", "feature/login"],
        },
      },
      { out: "changes moved to app:feature/login" },
      { pause: 1.2 },
    ],
  },
  {
    title: "Rename",
    cam: CAM_ONE,
    beats: [
      { cmd: "daft rename spike/faster-cart perf/cache" },
      {
        act: {
          kind: "rename",
          repo: "app",
          wt: "spike/faster-cart",
          to: "perf/cache",
        },
      },
      { out: "renamed spike/faster-cart → perf/cache · branch + directory" },
      { pause: 1.6 },
    ],
  },
  {
    title: "Relations",
    cam: CAM_TWO,
    beats: [
      { cmd: "daft start svc feature/login" },
      { act: { kind: "repo", repo: "svc", ...SVC } },
      { act: { kind: "wt", repo: "svc", wt: "main" } },
      { act: { kind: "relate", a: "app", b: "svc" } },
      { out: "svc: worktree feature/login/ ready" },
      { act: { kind: "wt", repo: "svc", wt: "feature/login" } },
      { act: { kind: "boot", repo: "svc", wt: "feature/login", secs: 1.2 } },
      {
        act: {
          kind: "arc",
          a: ["app", "feature/login"],
          b: ["svc", "feature/login"],
        },
      },
      { pause: 1 },
    ],
  },
  {
    title: "Push & the forge",
    cam: CAM_TWO,
    beats: [
      { cmd: "daft push feature/login" },
      { out: "app: feature/login → origin · upstream set" },
      { pause: 0.3 },
      { cmd: "daft push feature/login" },
      { out: "svc: feature/login → origin · upstream set" },
      {
        out: "# forge: feature/login approved → merged in app, svc",
        tone: "dim",
      },
      { act: { kind: "merged", repo: "app", wt: "feature/login" } },
      { pause: 0.3 },
      { act: { kind: "merged", repo: "svc", wt: "feature/login" } },
      { pause: 1.2 },
    ],
  },
  {
    title: "Unlink & repo remove",
    cam: CAM_TWO,
    beats: [
      { cmd: "daft repo unlink svc" },
      { act: { kind: "unrelate", a: "app", b: "svc" } },
      { out: "unlinked app ↔ svc", tone: "rust" },
      { pause: 1.1 },
      { cmd: "daft repo remove svc" },
      { act: { kind: "repo-remove", repo: "svc" } },
      { out: "removed svc from the catalog", tone: "rust" },
      { pause: 1.8 },
    ],
  },
  {
    title: "Sync & teardown",
    cam: CAM_ONE,
    beats: [
      { cmd: "daft sync" },
      { act: { kind: "sync", repo: "app" } },
      { out: "updated 1 main · bases refreshed" },
      { pause: 0.5 },
      { out: "pruned feature/login in app", tone: "rust" },
      { act: { kind: "remove", repo: "app", wt: "feature/login" } },
      { pause: 2.4 },
    ],
  },
];

/**
 * The full lifecycle, spoken entirely in registry ops — what `ship` used
 * to fake as one verb, told honestly: build, verify, push each worktree,
 * the forge merges, sync cleans up.
 */
const SHIP_A_FEATURE: StepDef[] = buildScenario([
  { verb: "clone", args: { name: "web" } },
  { verb: "start", args: { branch: "checkout" } },
  { verb: "agent-joins", args: { target: "web:checkout" } },
  { verb: "agent-leaves", args: { target: "web:checkout" } },
  { verb: "start", args: { branch: "checkout", repo: "orders" } },
  { verb: "exec", args: { command: "pnpm test" } },
  { verb: "push", args: { target: "web:checkout" } },
  { verb: "push", args: { target: "orders:checkout" } },
  { verb: "forge-merges", args: { branch: "checkout" } },
  { verb: "sync", args: {} },
]).steps;

export interface GalleryEntry {
  id: string;
  label: string;
  script: StepDef[];
}

export const GALLERY: GalleryEntry[] = [
  { id: "hero", label: "Landing story", script: HERO_SCRIPT },
  { id: "vocabulary", label: "Vocabulary tour", script: VOCABULARY_SCRIPT },
  { id: "ship-a-feature", label: "Ship a feature", script: SHIP_A_FEATURE },
];

/**
 * Scripts the playground can replay. The hero story ships first; the
 * vocabulary tour exists so every act kind has a place to be seen (and
 * broken) in isolation — extend it in the same change that grows the
 * language, so the playground always demonstrates the full grammar.
 */

import type { StepDef } from "./engine";
import { HERO_SCRIPT } from "./hero-script";

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
    title: "Agent",
    cam: CAM_ONE,
    beats: [
      { cmd: "daft list" },
      { out: "main           :3000  clean" },
      { act: { kind: "agent", repo: "app", wt: "feature/login" } },
      { out: "feature/login  :3001  agent · building", tone: "agent" },
      { pause: 1.4 },
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
    title: "Merge",
    cam: CAM_TWO,
    beats: [
      { cmd: "daft exec --related -- git push" },
      { out: "feature/login → origin in app, svc" },
      { out: "# PRs merged on the forge", tone: "dim" },
      { act: { kind: "merged", repo: "app", wt: "feature/login" } },
      { pause: 0.3 },
      { act: { kind: "merged", repo: "svc", wt: "feature/login" } },
      { pause: 1.2 },
    ],
  },
  {
    title: "Sync & teardown",
    cam: CAM_TWO,
    beats: [
      { cmd: "daft sync" },
      { act: { kind: "sync", repo: "app" } },
      { act: { kind: "sync", repo: "svc" } },
      { out: "updated 2 mains · bases refreshed" },
      { pause: 0.5 },
      { out: "pruned feature/login in app, svc", tone: "rust" },
      { act: { kind: "remove", repo: "app", wt: "feature/login" } },
      { pause: 0.3 },
      { act: { kind: "remove", repo: "svc", wt: "feature/login" } },
      { pause: 2.4 },
    ],
  },
];

export interface GalleryEntry {
  id: string;
  label: string;
  script: StepDef[];
}

export const GALLERY: GalleryEntry[] = [
  { id: "hero", label: "Landing story", script: HERO_SCRIPT },
  { id: "vocabulary", label: "Vocabulary tour", script: VOCABULARY_SCRIPT },
];

import { expect, test } from "bun:test";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  compile,
  derive,
  emptyDoc,
  insertItem,
  setArgs,
  setChapterTitle,
  setSilent,
} from "@dumbshow/core";
import { CATALOG } from "../../.vitepress/theme/graph/catalog";
import { GALLERY } from "../../.vitepress/theme/graph/gallery";
import { type Compiled, DAFT_PACK } from "../../.vitepress/theme/graph/pack";
import { renameEntity } from "../../.vitepress/theme/graph/seed";
import { buildScenario } from "../../.vitepress/theme/graph/verbs";
import {
  LANDING_POINTS,
  pointScript,
} from "../../.vitepress/theme/landing/points";

/**
 * Committed compiled-output goldens — the extraction tripwire. Compile and
 * derive are pure math, so these digests are byte-stable across machines;
 * any refactor that changes them changed behavior. Regenerate deliberately
 * with UPDATE_GOLDENS=1 and review the diff like code.
 */

const DIR = join(dirname(fileURLToPath(import.meta.url)), "__goldens__");

function checkGolden(name: string, value: unknown): void {
  const path = join(DIR, `${name}.json`);
  const json = `${JSON.stringify(value, null, 2)}\n`;
  if (process.env.UPDATE_GOLDENS === "1" || !existsSync(path)) {
    mkdirSync(DIR, { recursive: true });
    writeFileSync(path, json);
    return;
  }
  expect(json).toBe(readFileSync(path, "utf8"));
}

function digest(compiled: Compiled): Record<string, unknown> {
  const actCounts: Record<string, number> = {};
  for (const e of compiled.events)
    actCounts[e.act.kind] = (actCounts[e.act.kind] ?? 0) + 1;
  return {
    duration: compiled.duration,
    steps: compiled.steps.map((s) => ({
      at: s.at,
      end: s.end,
      cue: s.cue,
      title: s.title,
      ...(s.silent ? { silent: true } : {}),
    })),
    termCount: compiled.term.length,
    commands: compiled.term.filter((l) => l.kind === "cmd").map((l) => l.text),
    actCounts,
    eventTimeline: compiled.events.map(
      (e) => `${e.at.toFixed(3)} ${e.act.kind}`,
    ),
    camCount: compiled.cams.length,
  };
}

test("gallery scripts compile to their recorded digests", () => {
  for (const entry of GALLERY)
    checkGolden(`gallery-${entry.id}`, digest(compile(entry.script)));
});

test("catalog demos compile to their recorded digests", () => {
  const all: Record<string, unknown> = {};
  for (const entry of CATALOG)
    all[entry.id] = digest(compile(buildScenario(entry.demo).steps));
  checkGolden("catalog-demos", all);
});

test("landing point scenes compile to their recorded digests", () => {
  // The five points demonstrate from the verb registry; their digests are
  // the tripwire that a registry change silently rewrote the landing.
  const all: Record<string, unknown> = {};
  for (const point of LANDING_POINTS) {
    const script = pointScript(point);
    if (script) all[point.id] = digest(compile(script));
  }
  checkGolden("landing-points", all);
});

test("a mutated composer document derives to its recorded shape", () => {
  let doc = emptyDoc(DAFT_PACK);
  doc = insertItem(doc, 0, { kind: "op", op: "clone", args: { name: "web" } });
  doc = insertItem(doc, 1, {
    kind: "op",
    op: "start",
    args: { repo: "web", branch: "checkout" },
  });
  doc = insertItem(doc, 1, { kind: "chapter", title: "Chapter" });
  doc = insertItem(doc, 3, { kind: "beat", secs: 1.5 });
  doc = insertItem(doc, 4, {
    kind: "op",
    op: "push",
    args: { target: "web:checkout" },
  });
  // Re-resolution, silence, chapter naming, a doc-wide rename, and one
  // honestly-impossible op (prune with nothing hollow) for the -1 mapping.
  doc = setArgs(doc, 2, { repo: "web", branch: "perf/cache" });
  doc = setSilent(doc, 4, true);
  doc = setChapterTitle(doc, 1, "Act I — build");
  doc = insertItem(doc, 5, { kind: "op", op: "prune", args: {} });
  doc = renameEntity(doc, "repo", "web", "store");
  const derived = derive(doc, DAFT_PACK);
  checkGolden("composer-doc", {
    doc,
    mapping: derived.mapping,
    chapters: derived.chapters,
    worldRepos: derived.world.repos.map((r) => r.name),
    digest: digest(derived.compiled),
  });
});

import { describe, expect, test } from "bun:test";
import { CATALOG } from "../../.vitepress/theme/graph/catalog";
import { GALLERY } from "../../.vitepress/theme/graph/gallery";
import {
  buildScenario,
  emptyWorld,
  OPS,
} from "../../.vitepress/theme/graph/verbs";

/** CMP-L2/L3 — the catalog builds clean and command() never lies. */

const ACT_KINDS = [
  "agent",
  "agent-leave",
  "arc",
  "boot",
  "carry",
  "merged",
  "port",
  "relate",
  "remove",
  "rename",
  "repo",
  "repo-remove",
  "sync",
  "unrelate",
  "wt",
];

describe("CMP-L2 catalog demos build clean", () => {
  for (const entry of CATALOG) {
    test(`${entry.id} builds every invocation`, () => {
      const scenario = buildScenario(entry.demo);
      expect(scenario.mapping).not.toContain(-1);
      expect(scenario.mapping[entry.focus]).toBeGreaterThanOrEqual(0);
      expect(scenario.steps).toHaveLength(entry.demo.length);
    });
  }

  test("the vocabulary tour covers all 15 act kinds", () => {
    const tour = GALLERY.find((g) => g.id === "vocabulary");
    if (!tour) throw new Error("no vocabulary tour in the gallery");
    const kinds = new Set<string>();
    for (const step of tour.script)
      for (const beat of step.beats)
        if ("act" in beat) kinds.add(beat.act.kind);
    expect([...kinds].sort()).toEqual(ACT_KINDS);
  });

  test("the ship-a-feature story built whole", () => {
    const ship = GALLERY.find((g) => g.id === "ship-a-feature");
    expect(ship?.script).toHaveLength(10);
  });
});

describe("CMP-L3 command() agrees with run()", () => {
  for (const entry of CATALOG) {
    test(`${entry.id}: every printed command matches its step`, () => {
      const world = emptyWorld();
      for (const inv of entry.demo) {
        const spec = OPS.find((o) => o.id === inv.verb);
        if (!spec) throw new Error(`unknown op ${inv.verb}`);
        expect(spec.available(world)).toBe(true);
        // The command resolves against the world BEFORE the op runs —
        // the same order every surface uses.
        const cmd = spec.command(world, inv.args);
        const step = spec.run(world, inv.args);
        const cmdBeat = step.beats.find((b) => "cmd" in b);
        if (cmd === null) expect(cmdBeat).toBeUndefined();
        else expect(cmdBeat && "cmd" in cmdBeat && cmdBeat.cmd).toBe(cmd);
      }
    });
  }
});

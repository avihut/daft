import { describe, expect, test } from "bun:test";
import { derive, emptyDoc } from "@dumbshow/core";
import {
  canvasDrop,
  dropElement,
} from "../../.vitepress/theme/graph/composer/elements";
import { DAFT_PACK } from "../../.vitepress/theme/graph/pack";
import {
  addSeedRepo,
  addSeedWt,
  placementsOf,
} from "../../.vitepress/theme/graph/seed";

/**
 * CMP-D5 (module half) — dropElement branches the pointer specs cannot
 * reach: they need exact node positions for entities whose slots are
 * hash-derived, so the pure semantics are pinned here instead.
 */

function wtHit(repo: string, wt: string) {
  return { kind: "wt" as const, repo, wt, sx: 0, sy: 0, r: 10 };
}

describe("CMP-D5 element-drop semantics", () => {
  test("agent on a timeline-born worktree points at the event", () => {
    let doc = emptyDoc(DAFT_PACK);
    doc = addSeedRepo(doc, { name: "web", wts: [{ branch: "main" }] });
    doc = {
      ...doc,
      timeline: [
        { kind: "op", op: "start", args: { repo: "web", branch: "checkout" } },
      ],
    };
    const world = derive(doc, DAFT_PACK).world;
    const out = dropElement(
      doc,
      world,
      "agent",
      0,
      0,
      wtHit("web", "checkout"),
    );
    expect("error" in out && out.error).toContain("agent joins event");
  });

  test("agent on main is refused", () => {
    let doc = emptyDoc(DAFT_PACK);
    doc = addSeedRepo(doc, { name: "web", wts: [{ branch: "main" }] });
    const world = derive(doc, DAFT_PACK).world;
    const out = dropElement(doc, world, "agent", 0, 0, wtHit("web", "main"));
    expect("error" in out && out.error).toBe(
      "Agents work on feature worktrees, not main.",
    );
  });

  test("worktree on a timeline-created repo defers to daft start", () => {
    const doc = {
      ...emptyDoc(DAFT_PACK),
      timeline: [{ kind: "op" as const, op: "clone", args: { name: "web" } }],
    };
    const world = derive(doc, DAFT_PACK).world;
    const out = dropElement(doc, world, "worktree", 0, 0, {
      kind: "repo",
      repo: "web",
      sx: 0,
      sy: 0,
      r: 21,
    });
    expect("error" in out && out.error).toContain(
      "created by the timeline — grow it with daft start",
    );
  });

  test("repo onto repo refuses when the timeline already related them", () => {
    // The seed would carry a second copy of a line `daft repo link` made:
    // drawn twice, two hits under `pick`, one relation.
    const doc = {
      ...emptyDoc(DAFT_PACK),
      timeline: [
        { kind: "op" as const, op: "clone", args: { name: "web" } },
        { kind: "op" as const, op: "clone", args: { name: "orders" } },
        {
          kind: "op" as const,
          op: "repo-link",
          args: { a: "web", b: "orders" },
        },
      ],
    };
    const derived = derive(doc, DAFT_PACK);
    const repoHit = (repo: string) => ({
      kind: "repo" as const,
      repo,
      sx: 0,
      sy: 0,
      r: 21,
    });
    const out = canvasDrop({
      doc,
      world: derived.world,
      compiled: derived.compiled,
      source: { kind: "node", hit: repoHit("web") },
      wx: 0,
      wy: 0,
      over: repoHit("orders"),
    });
    expect(out && "error" in out && out.error).toBe(
      "web and orders are already related.",
    );
  });

  test("the relation chip explains the repo-onto-repo gesture", () => {
    const doc = emptyDoc(DAFT_PACK);
    const out = dropElement(
      doc,
      derive(doc, DAFT_PACK).world,
      "relation",
      0,
      0,
      null,
    );
    expect("error" in out && out.error).toBe(
      "Drag one repo onto another on the canvas to relate them.",
    );
  });

  test("a seed worktree drop pins the polar slot under the pointer", () => {
    let doc = emptyDoc(DAFT_PACK);
    doc = addSeedRepo(doc, { name: "web", wts: [{ branch: "main" }] });
    doc = addSeedWt(doc, "web", { branch: "checkout" });
    const world = derive(doc, DAFT_PACK).world;
    const repo = world.repos[0];
    const out = dropElement(doc, world, "worktree", repo.x, repo.y - 90, {
      kind: "repo",
      repo: "web",
      sx: 0,
      sy: 0,
      r: 21,
    });
    if ("error" in out) throw new Error(out.error);
    const wts = placementsOf(out.doc).wts;
    const key = Object.keys(wts).find((k) => k.startsWith("web:"));
    expect(key).toBeDefined();
    const slot = wts[key as string];
    expect(slot.dist).toBe(90);
    expect(slot.ang).toBeCloseTo(-Math.PI / 2, 2);
  });
});

import { describe, expect, test } from "bun:test";
import { type ComposerDoc, derive, emptyDoc } from "@dumbshow/core";
import { DAFT_PACK } from "../../.vitepress/theme/graph/pack";
import {
  addSeedRepo,
  addSeedWt,
  placementsOf,
  renameEntity,
  seedOf,
  setWtPlacement,
} from "../../.vitepress/theme/graph/seed";

/**
 * The entity-rename law (seed.ts) — a name rewritten everywhere it is
 * referenced, and nowhere it is not. Every case here is a document the
 * composer can build by hand, so a regression reaches real authors.
 */

type Op = { op: string; args: Record<string, unknown> };

function docOf(ops: Op[]): ComposerDoc {
  return {
    ...emptyDoc(DAFT_PACK),
    timeline: ops.map((o) => ({ kind: "op" as const, ...o })),
  };
}

const args = (doc: ComposerDoc, i: number): Record<string, unknown> => {
  const item = doc.timeline[i];
  if (item.kind !== "op") throw new Error(`item ${i} is not an op`);
  return item.args;
};

describe("entity renames", () => {
  test("a repo rename rewrites the whole timeline, past a branch rename", () => {
    // Nothing in the language renames a repo, so no op can end the rewrite —
    // a branch rename in the middle used to stop it, stranding later ops on
    // the old repo name (which then grew a phantom second repo on replay).
    const doc = docOf([
      { op: "clone", args: { name: "web" } },
      { op: "start", args: { repo: "web", branch: "checkout" } },
      { op: "rename", args: { target: "web:checkout", to: "login" } },
      { op: "start", args: { repo: "web", branch: "perf/cache" } },
    ]);
    const next = renameEntity(doc, { kind: "repo", name: "web" }, "shop");
    expect(args(next, 0).name).toBe("shop");
    expect(args(next, 1).repo).toBe("shop");
    expect(args(next, 2).target).toBe("shop:checkout");
    expect(args(next, 3).repo).toBe("shop");
    const world = derive(next, DAFT_PACK).world;
    expect(world.repos.map((r) => r.name)).toEqual(["shop"]);
  });

  test("a branch rename stops at the op that renames it away", () => {
    const doc = docOf([
      { op: "clone", args: { name: "web" } },
      { op: "start", args: { repo: "web", branch: "checkout" } },
      { op: "rename", args: { target: "web:checkout", to: "login" } },
      { op: "push", args: { target: "web:login" } },
    ]);
    const next = renameEntity(
      doc,
      { kind: "branch", repo: "web", branch: "checkout" },
      "cart",
    );
    expect(args(next, 1).branch).toBe("cart");
    expect(args(next, 2).target).toBe("web:cart");
    // The name that op introduces is untouched, and so is everything after.
    expect(args(next, 2).to).toBe("login");
    expect(args(next, 3).target).toBe("web:login");
  });

  test("the op that introduces the name is rewritten with it", () => {
    const doc = docOf([
      { op: "clone", args: { name: "web" } },
      { op: "start", args: { repo: "web", branch: "spike/faster-cart" } },
      {
        op: "rename",
        args: { target: "web:spike/faster-cart", to: "checkout" },
      },
      { op: "push", args: { target: "web:checkout" } },
    ]);
    const next = renameEntity(
      doc,
      { kind: "branch", repo: "web", branch: "checkout" },
      "cart",
    );
    expect(args(next, 2).to).toBe("cart");
    expect(args(next, 3).target).toBe("web:cart");
  });

  test("a branch rename leaves the same branch in a sibling repo alone", () => {
    // web:checkout and orders:checkout are two worktrees of one feature.
    let doc = emptyDoc(DAFT_PACK);
    doc = addSeedRepo(doc, { name: "web", wts: [{ branch: "main" }] });
    doc = addSeedWt(doc, "web", { branch: "checkout" });
    doc = addSeedRepo(doc, { name: "orders", wts: [{ branch: "main" }] });
    doc = addSeedWt(doc, "orders", { branch: "checkout" });
    doc = setWtPlacement(doc, "web:checkout", { ang: 0.5, dist: 90 });
    doc = setWtPlacement(doc, "orders:checkout", { ang: 1.5, dist: 90 });
    doc = {
      ...doc,
      timeline: [
        { kind: "op", op: "push", args: { target: "web:checkout" } },
        {
          kind: "op",
          op: "start",
          args: { repo: "orders", branch: "checkout" },
        },
      ],
    };

    const next = renameEntity(
      doc,
      { kind: "branch", repo: "orders", branch: "checkout" },
      "login",
    );
    const seed = seedOf(next);
    const branches = (name: string) =>
      seed.repos.find((r) => r.name === name)?.wts.map((w) => w.branch);
    expect(branches("web")).toEqual(["main", "checkout"]);
    expect(branches("orders")).toEqual(["main", "login"]);
    expect(Object.keys(placementsOf(next).wts).sort()).toEqual([
      "orders:login",
      "web:checkout",
    ]);
    // Composite targets rewrite on their own repo only; a bare branch
    // argument follows the repo its own item names.
    expect(args(next, 0).target).toBe("web:checkout");
    expect(args(next, 1).branch).toBe("login");
  });

  test("a rename in one repo does not stop the rewrite in another", () => {
    const doc = docOf([
      { op: "clone", args: { name: "web" } },
      { op: "start", args: { repo: "web", branch: "checkout" } },
      { op: "start", args: { repo: "orders", branch: "checkout" } },
      { op: "rename", args: { target: "web:checkout", to: "login" } },
      { op: "push", args: { target: "orders:checkout" } },
    ]);
    const next = renameEntity(
      doc,
      { kind: "branch", repo: "orders", branch: "checkout" },
      "cart",
    );
    expect(args(next, 1).branch).toBe("checkout");
    expect(args(next, 2).branch).toBe("cart");
    expect(args(next, 3).target).toBe("web:checkout");
    expect(args(next, 4).target).toBe("orders:cart");
  });
});

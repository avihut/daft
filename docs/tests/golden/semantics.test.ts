import { describe, expect, test } from "bun:test";
import { compile } from "@dumbshow/core";
import { createSceneCursor } from "../../.vitepress/theme/graph/render";
import {
  buildScenario,
  OPS,
  type VerbInvocation,
  type World,
} from "../../.vitepress/theme/graph/verbs";

/** CMP-L4 — verb semantics at the world and act level, no browser. */

function build(invocations: VerbInvocation[]): {
  world: World;
  steps: ReturnType<typeof buildScenario>["steps"];
  mapping: number[];
} {
  return buildScenario(invocations);
}

const clone = (name: string): VerbInvocation => ({
  verb: "clone",
  args: { name },
});
const start = (branch: string, repo?: string): VerbInvocation => ({
  verb: "start",
  args: repo ? { branch, repo } : { branch },
});

function wt(world: World, repo: string, branch: string) {
  return world.repos
    .find((r) => r.name === repo)
    ?.wts.find((w) => w.branch === branch);
}

function available(id: string, world: World): boolean {
  const spec = OPS.find((o) => o.id === id);
  if (!spec) throw new Error(`unknown op ${id}`);
  return spec.available(world);
}

describe("CMP-L4 verb semantics", () => {
  test("push marks pushed and exhausts its pool", () => {
    const before = build([clone("web"), start("checkout")]).world;
    expect(wt(before, "web", "checkout")?.pushed).toBeFalsy();
    expect(available("push", before)).toBe(true);
    const after = build([
      clone("web"),
      start("checkout"),
      { verb: "push", args: { target: "web:checkout" } },
    ]).world;
    expect(wt(after, "web", "checkout")?.pushed).toBe(true);
    expect(available("push", after)).toBe(false);
  });

  test("the forge is gated on a push and merges only pushed worktrees", () => {
    const unpushed = build([clone("web"), start("checkout")]).world;
    expect(available("forge-merges", unpushed)).toBe(false);
    const world = build([
      clone("web"),
      start("checkout"),
      start("bugfix/cart-total"),
      { verb: "push", args: { target: "web:checkout" } },
      { verb: "forge-merges", args: { branch: "checkout" } },
    ]).world;
    expect(wt(world, "web", "checkout")?.merged).toBe(true);
    expect(wt(world, "web", "bugfix/cart-total")?.merged).toBeFalsy();
  });

  test("merge lands the work and removes the worktree", () => {
    const { world, steps, mapping } = build([
      clone("web"),
      start("checkout"),
      { verb: "merge", args: { target: "web:checkout" } },
    ]);
    expect(wt(world, "web", "checkout")?.removed).toBe(true);
    const kinds = steps[mapping[2]].beats
      .filter((b) => "act" in b)
      .map((b) => ("act" in b ? b.act.kind : ""));
    expect(kinds).toContain("remove");
  });

  test("prune waits for hollow shells, then clears them", () => {
    expect(
      available("prune", build([clone("web"), start("checkout")]).world),
    ).toBe(false);
    const world = build([
      clone("web"),
      start("checkout"),
      { verb: "push", args: { target: "web:checkout" } },
      { verb: "forge-merges", args: { branch: "checkout" } },
      { verb: "prune", args: {} },
    ]).world;
    expect(wt(world, "web", "checkout")?.removed).toBe(true);
  });

  test("sync --push pushes the survivors it syncs", () => {
    const world = build([
      clone("web"),
      start("checkout"),
      { verb: "sync", args: { push: "yes" } },
    ]).world;
    expect(wt(world, "web", "checkout")?.pushed).toBe(true);
  });

  test("rename renames in place — no teleport, references rewritten", () => {
    const { world, steps, mapping } = build([
      clone("web"),
      start("checkout"),
      { verb: "rename", args: { target: "web:checkout", to: "feature/login" } },
      { verb: "go", args: { target: "web:feature/login" } },
    ]);
    expect(mapping).not.toContain(-1);
    const repo = world.repos.find((r) => r.name === "web");
    // Same slot in the worktree list: the orbit index — and so the
    // hash-derived angle — is untouched by the rename.
    expect(repo?.wts.map((w) => w.branch)).toEqual(["main", "feature/login"]);
    const renameActs = steps[mapping[2]].beats.filter(
      (b) => "act" in b && b.act.kind === "rename",
    );
    expect(renameActs).toHaveLength(1);
  });

  test("repo-remove staggers its teardown 0.12s apart", () => {
    const scenario = build([
      clone("web"),
      clone("orders"),
      { verb: "repo-link", args: { a: "web", b: "orders" } },
      start("checkout", "orders"),
      { verb: "repo-remove", args: { name: "orders" } },
    ]);
    const orders = scenario.world.repos.find((r) => r.name === "orders") as
      | (World["repos"][number] & { removed?: boolean })
      | undefined;
    expect(
      !orders || orders.removed === true || orders.wts.every((w) => w.removed),
    ).toBe(true);
    // The stagger is scene state: applyAct stamps each live worktree
    // 0.12s after the previous one, and fades touching relations.
    const compiled = compile(scenario.steps);
    const at = compiled.events.find((e) => e.act.kind === "repo-remove")?.at;
    if (at === undefined) throw new Error("no repo-remove event");
    const cursor = createSceneCursor(compiled.events);
    cursor.sync(at + 0.01);
    const scene = cursor.scene() as unknown as {
      repos: {
        label: string;
        removed?: number;
        wts: { removed?: number }[];
      }[];
      rels: { a: string; b: string; removed?: number }[];
    };
    const sceneOrders = scene.repos.find((r) => r.label === "orders");
    if (!sceneOrders) throw new Error("no orders repo in the scene");
    expect(sceneOrders.removed).toBeCloseTo(at, 5);
    const stamps = sceneOrders.wts
      .map((w) => w.removed)
      .filter((v): v is number => v !== undefined)
      .sort((a, b) => a - b);
    expect(stamps.length).toBeGreaterThanOrEqual(2);
    for (let i = 1; i < stamps.length; i++)
      expect(stamps[i] - stamps[i - 1]).toBeCloseTo(0.12, 5);
    const touching = scene.rels.find(
      (r) => r.a === "orders" || r.b === "orders",
    );
    expect(touching?.removed).toBeCloseTo(at, 5);
  });

  test("arcs fade on endpoint removal — stamped, never popped", () => {
    const scenario = build([
      clone("web"),
      start("checkout"),
      start("checkout", "orders"),
      { verb: "remove", args: { target: "web:checkout" } },
    ]);
    const compiled = compile(scenario.steps);
    const removeAt = compiled.events.find(
      (e) =>
        e.act.kind === "remove" &&
        "repo" in e.act &&
        e.act.repo === "web" &&
        "wt" in e.act &&
        e.act.wt === "checkout",
    )?.at;
    if (removeAt === undefined) throw new Error("no remove event");
    const cursor = createSceneCursor(compiled.events);
    cursor.sync(removeAt - 0.01);
    expect(cursor.scene().arcs).toHaveLength(1);
    expect(cursor.scene().arcs[0].removed).toBeUndefined();
    cursor.sync(removeAt + 0.01);
    // The arc is still there, stamped with its removal time — the
    // renderer fades it out instead of popping it.
    expect(cursor.scene().arcs).toHaveLength(1);
    expect(cursor.scene().arcs[0].removed).toBeCloseTo(removeAt, 5);
  });
});

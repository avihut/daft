import { describe, expect, test } from "bun:test";
import { emptyWorld, OPS, VERBS } from "../../.vitepress/theme/graph/verbs";

/**
 * CMP-L1 — registry shape. Pure module-level invariants, no browser: the
 * op registry is the composer's vocabulary, and every surface (palette,
 * shell, attributes, derive) resolves through it.
 */

describe("CMP-L1 registry shape", () => {
  test("21 ops: 18 verbs (incl. typed-only cd) + 3 events", () => {
    expect(OPS).toHaveLength(21);
    expect(OPS.filter((op) => op.kind === "verb")).toHaveLength(18);
    expect(OPS.filter((op) => op.kind === "event")).toHaveLength(3);
    expect(new Set(OPS.map((op) => op.id)).size).toBe(OPS.length);
  });

  test("cd is a typed-only verb — reachable from the shell, not the palette", () => {
    const cd = OPS.find((op) => op.id === "cd");
    expect(cd?.kind).toBe("verb");
    expect(cd?.typedOnly).toBe(true);
  });

  test("events print no command", () => {
    const world = emptyWorld();
    for (const op of OPS.filter((o) => o.kind === "event")) {
      expect(op.command(world, {})).toBeNull();
    }
  });

  test("VERBS is the verb-only view of OPS", () => {
    expect(VERBS).toHaveLength(18);
    expect(VERBS.every((op) => op.kind === "verb")).toBe(true);
  });
});

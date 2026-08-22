import { describe, expect, test } from "bun:test";
import { derive, emptyDoc } from "@dumbshow/core";
import { DAFT_PACK } from "../../.vitepress/theme/graph/pack";
import { parseCommand } from "../../.vitepress/theme/graph/shell";
import type { World } from "../../.vitepress/theme/graph/verbs";

/**
 * The shell grammar as a table — parseCommand is pure (line + world + cwd
 * in, outcome out), so the cwd-aware arms that make daft's grammar honest
 * are pinned here without a browser.
 */

function worldAfter(
  ops: { op: string; args: Record<string, unknown> }[],
): World {
  return derive(
    {
      ...emptyDoc(DAFT_PACK),
      timeline: ops.map((o) => ({
        kind: "op" as const,
        op: o.op,
        args: o.args,
      })),
    },
    DAFT_PACK,
  ).world;
}

const empty = worldAfter([]);
const cloned = worldAfter([
  { op: "clone", args: { name: "web" } },
  { op: "start", args: { repo: "web", branch: "checkout" } },
]);
const two = worldAfter([
  { op: "clone", args: { name: "web" } },
  { op: "clone", args: { name: "orders" } },
]);

describe("shell grammar table", () => {
  test("refusals", () => {
    const cases: [string, World, string, string][] = [
      [
        "daft go billing",
        empty,
        "~",
        'error: unknown repo "billing" — nothing cloned or added by that name',
      ],
      [
        "ls",
        empty,
        "~",
        "this shell tells daft stories — try a daft command, or cd",
      ],
      [
        "daft clone git@github.com:acme/web.git",
        cloned,
        "~/web/main",
        "daft clone: web is already in the story",
      ],
      [
        "daft start checkout",
        cloned,
        "~/web/main",
        "daft start: checkout already exists in web",
      ],
      // Local-first two-name reading: the second name resolving locally
      // wins over treating the first as another repo.
      [
        "daft start payments checkout",
        cloned,
        "~/web/main",
        "daft start: checkout already exists in web",
      ],
      // Both names known: the branch is already there, whichever name
      // leads. Accepting it made `start` create a different branch than the
      // line said.
      [
        "daft start web checkout",
        cloned,
        "~/web/main",
        "daft start: checkout already exists in web",
      ],
      [
        "daft push",
        cloned,
        "~/web/main",
        "daft push: nothing to push from web/main",
      ],
      // Standing at ~ names no repo — linking would otherwise silently
      // pick the first one in the story.
      [
        "daft repo link orders",
        two,
        "~",
        "daft repo link: cd into the repo to link from",
      ],
      [
        "cd main",
        cloned,
        "~/web/checkout",
        "cd: no such directory: web/checkout/main",
      ],
      ["daft remove main", cloned, "~/web/main", "daft remove: main stays"],
      [
        "daft carry checkout",
        cloned,
        "~",
        "daft carry: cd into the worktree holding the changes first",
      ],
    ];
    for (const [line, world, cwd, error] of cases) {
      expect(parseCommand(line, world, cwd)).toEqual({ ok: false, error });
    }
  });

  test("resolutions", () => {
    expect(
      parseCommand("daft clone git@github.com:acme/web.git", empty, "~"),
    ).toEqual({ ok: true, op: "clone", args: { name: "web" } });
    expect(parseCommand("daft push", cloned, "~/web/checkout")).toEqual({
      ok: true,
      op: "push",
      args: { target: "web:checkout" },
    });
    expect(parseCommand("daft push checkout", cloned, "~/web/main")).toEqual({
      ok: true,
      op: "push",
      args: { target: "web:checkout" },
    });
    expect(parseCommand("cd ../checkout", cloned, "~/web/main")).toEqual({
      ok: true,
      op: "cd",
      args: { path: "../checkout" },
    });
    expect(
      parseCommand("daft exec -- pnpm test", cloned, "~/web/main"),
    ).toEqual({ ok: true, op: "exec", args: { command: "pnpm test" } });
    expect(parseCommand("daft go checkout", cloned, "~/web/main")).toEqual({
      ok: true,
      op: "go",
      args: { target: "web:checkout" },
    });
    expect(parseCommand("daft sync --push", cloned, "~/web/main").ok).toBe(
      true,
    );
  });
});

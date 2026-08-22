import { expect, test } from "@playwright/test";
import {
  canvasPoint,
  chip,
  cmdLines,
  expectParked,
  lastRepoActPos,
  openComposer,
  playerState,
  rowAction,
  settled,
  stageCanvas,
  synthDrag,
  synthTapAt,
  tapChip,
} from "./helpers";

/** CMP-E — the Attributes form (except E4 rename, in pack.rename). */

test("CMP-E1 op fields re-resolve; a vanished target reads (gone)", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "push");
  // push is selected; its pool offers exactly the one unpushed worktree.
  const target = page.locator("#dx-f-target");
  await expect(target.locator("option")).toHaveCount(1);
  await expect(target).toHaveValue("web:checkout");
  // Delete start: the pool empties and the kept value reads (gone).
  await rowAction(page, 1, "Remove");
  await expect(page.locator(".dx-vrow").nth(1)).toHaveClass(/\bdead\b/);
  await expect(target.locator("option").first()).toHaveText(
    "web:checkout (gone)",
  );
});

test("CMP-E2 an arg edit rewrites the projection and lands there", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await page.locator("#dx-f-branch").fill("perf/cache");
  await page.locator("#dx-f-branch").blur();
  await settled(page);
  const cmds = await cmdLines(page);
  expect(cmds[cmds.length - 1]).toContain("daft start perf/cache");
  await expect(
    page.locator(".dx-vrow").nth(1).locator(".dx-vrest"),
  ).toContainText("perf/cache");
  const snap = await expectParked(page);
  expect(snap.step).toBe(1);
});

test("CMP-E3 the silent switch flips visibility, never timing", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  const before = await playerState(page);
  const sw = page.getByRole("switch", { name: "Visible in the terminal" });
  await expect(sw).toHaveAttribute("aria-checked", "true");
  await sw.click();
  await settled(page);
  await expect(sw).toHaveAttribute("aria-checked", "false");
  const after = await playerState(page);
  expect(after?.duration).toBe(before?.duration);
  // The editor dims the step's lines — command and outputs — not just one,
  // and touches nothing of the other step's.
  await expect(
    page.locator('.dx-tline.hiddenop:has-text("daft start")'),
  ).toHaveCount(1);
  await expect(
    page.locator('.dx-tline.hiddenop:has-text("daft clone")'),
  ).toHaveCount(0);
  expect(
    await page.locator(".dx-tline.hiddenop").count(),
  ).toBeGreaterThanOrEqual(1);
});

test("CMP-E5 seed fields persist; verb-born entities are read-only", async ({
  page,
}) => {
  await openComposer(page);
  const canvas = stageCanvas(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  await synthDrag(chip(page, "worktree"), await canvasPoint(page, 0, -12));
  await settled(page);
  // Port and the merged flag stick, and survive a reselect.
  await page.locator("#dx-f-eport").fill(":4100");
  await page.locator("#dx-f-eport").blur();
  await settled(page);
  const merged = page.getByRole("switch", { name: "Merged" });
  await merged.click();
  await settled(page);
  await expect(merged).toHaveAttribute("aria-checked", "true");
  await synthTapAt(canvas, await canvasPoint(page, 0, 0));
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
  await synthTapAt(canvas, await canvasPoint(page, 0, -48));
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("checkout");
  await expect(page.locator("#dx-f-eport")).toHaveValue(":4100");
  // Delete the seed worktree from the scene: selection clears.
  await page.getByRole("button", { name: "Delete from the scene" }).click();
  await settled(page);
  await expect(page.locator(".dx-attrs-hint")).toBeVisible();
  // A verb-born entity shows read-only state pointing at its step.
  await tapChip(page, "clone");
  const pos = await lastRepoActPos(page, "orders");
  if (!pos) throw new Error("no repo act for orders");
  await synthTapAt(canvas, await canvasPoint(page, pos.x, pos.y));
  await settled(page);
  await expect(page.locator(".dx-attrs .dx-insp-head b")).toHaveText("orders");
  await expect(page.locator(".dx-attrs")).toContainText(
    "Born from the timeline",
  );
});

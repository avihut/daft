import { expect, test } from "@playwright/test";
import {
  canvasPoint,
  center,
  chip,
  expectParked,
  lastRepoActPos,
  openComposer,
  rowVerbs,
  settled,
  stageCanvas,
  synthDrag,
  synthDragCancel,
  synthDragEnd,
  synthDragMove,
  synthDragStart,
  tapChip,
} from "./helpers";

/** CMP-D — drag and drop: threshold, timeline drops, elements, nodes. */

test("CMP-D1 threshold splits tap from drag; cancel drops nothing", async ({
  page,
}) => {
  await openComposer(page);
  const exec = chip(page, "exec");
  // Past 4px: the ghost engages and follows.
  await synthDragStart(exec, await center(exec));
  await expect(page.locator(".dx-ghost")).toBeVisible();
  await expect(page.locator(".dx-ghost b")).toHaveText("exec");
  // pointercancel aborts without dropping.
  await synthDragCancel(exec);
  await expect(page.locator(".dx-ghost")).toHaveCount(0);
  await expect(page.locator(".dx-vrow")).toHaveCount(0);
  // Under 4px the release is the tap behavior: the chip appends.
  await tapChip(page, "exec");
  expect(await rowVerbs(page)).toEqual(["exec"]);
});

test("CMP-D2 chip to timeline shows the insertion line and lands there", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  const exec = chip(page, "exec");
  const row1 = page.locator(".dx-vrow").nth(1);
  const box = await row1.boundingBox();
  if (!box) throw new Error("no row box");
  // Hover the top half of row 1: the gold line marks index 1.
  const over = { x: box.x + box.width / 2, y: box.y + 4 };
  await synthDragStart(exec, await center(exec));
  await synthDragMove(exec, over);
  await expect(page.locator(".dx-drop-line")).toBeVisible();
  await synthDragEnd(exec, over);
  await settled(page);
  expect(await rowVerbs(page)).toEqual(["clone", "exec", "start"]);
  await expect(page.locator(".dx-vrow").nth(1)).toHaveClass(/\bsel\b/);
  await expectParked(page);
});

test("CMP-D3 an op dropped above its prerequisites is skipped honestly", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const row0 = page.locator(".dx-vrow").nth(0);
  const box = await row0.boundingBox();
  if (!box) throw new Error("no row box");
  await synthDrag(chip(page, "push"), {
    x: box.x + box.width / 2,
    y: box.y + 3,
  });
  await settled(page);
  expect(await rowVerbs(page)).toEqual(["push", "clone"]);
  await expect(page.locator(".dx-vrow").nth(0)).toHaveClass(/\bdead\b/);
  await expect(page.locator(".dx-vrow .dx-skipped")).toHaveText("skipped");
});

test("CMP-D4 rows reorder by drag with after-removal semantics", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "list");
  const rows = page.locator(".dx-vrow");
  // Drag clone (0) below the last row: index 3, after-removal → position 2.
  const last = await rows.nth(2).boundingBox();
  if (!last) throw new Error("no row box");
  await synthDrag(rows.nth(0), {
    x: last.x + last.width / 2,
    y: last.y + last.height + 6,
  });
  await settled(page);
  expect(await rowVerbs(page)).toEqual(["start", "list", "clone"]);
  await expect(page.locator(".dx-vrow").nth(0)).toHaveClass(/\bdead\b/);
  // Drag clone (2) back above start (0): everything derives again.
  const first = await rows.nth(0).boundingBox();
  if (!first) throw new Error("no row box");
  await synthDrag(rows.nth(2), {
    x: first.x + first.width / 2,
    y: first.y + 3,
  });
  await settled(page);
  expect(await rowVerbs(page)).toEqual(["clone", "start", "list"]);
  await expect(page.locator(".dx-vrow.dead")).toHaveCount(0);
});

test("CMP-D5 element drops build the seed; invalid targets explain", async ({
  page,
}) => {
  await openComposer(page);
  // A repo lands where dropped — the empty canvas maps through the
  // documented default camera.
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
  await expect(page.locator(".dx-canvas-empty")).toHaveCount(0);
  await expectParked(page);
  // A worktree dropped on the disc takes the polar slot under the pointer.
  await synthDrag(chip(page, "worktree"), await canvasPoint(page, 0, -12));
  await settled(page);
  await expect(page.locator(".dx-attrs .dx-insp-head b")).toHaveText(
    "web · checkout",
  );
  await expect(page.locator("#dx-f-ename")).toHaveValue("checkout");
  // The slot: ang −π/2 (straight up), dist clamped to the 48 minimum.
  const wtPoint = await canvasPoint(page, 0, -48);
  // An agent docks on that seed feature worktree.
  await synthDrag(chip(page, "agent"), wtPoint);
  await settled(page);
  await expect(
    page.getByRole("switch", { name: "Agent working here" }),
  ).toHaveAttribute("aria-checked", "true");
  // Invalid: a worktree needs a repo under the pointer.
  await synthDrag(chip(page, "worktree"), await canvasPoint(page, 90, 60));
  await expect(page.locator(".dx-notice")).toHaveText(
    "Drop a worktree onto a repo to give it a home.",
  );
  // Invalid: an agent needs a worktree, not a repo disc.
  await synthDrag(chip(page, "agent"), await canvasPoint(page, 0, 0));
  await expect(page.locator(".dx-notice")).toHaveText(
    "Drop the agent onto a worktree.",
  );
  // Invalid: element chips mean nothing on the timeline.
  const tl = await page.locator(".dx-vtl").boundingBox();
  if (!tl) throw new Error("no timeline box");
  await synthDrag(chip(page, "relation"), {
    x: tl.x + tl.width / 2,
    y: tl.y + 30,
  });
  await expect(page.locator(".dx-notice")).toHaveText(
    "Elements drop on the canvas, not the timeline.",
  );
  // Nothing was stored by the refusals: still one repo, one extra worktree.
  await expect(page.locator(".dx-vrow")).toHaveCount(0);
});

test("CMP-D6 node drags re-place, relate, and freeze siblings", async ({
  page,
}) => {
  await openComposer(page);
  const canvas = stageCanvas(page);
  // Seed two repos and a pinned worktree by dropping elements.
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  await synthDrag(chip(page, "worktree"), await canvasPoint(page, 0, -12));
  await settled(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 150, 40));
  await settled(page);
  // Drag the web disc to a new spot: its placement — and the scene — follow.
  await synthDragStart(canvas, await canvasPoint(page, 0, 0));
  await synthDragMove(canvas, await canvasPoint(page, -60, 80));
  await synthDragEnd(canvas, await canvasPoint(page, -60, 80));
  await settled(page);
  expect(await lastRepoActPos(page, "web")).toEqual({ x: -60, y: 80 });
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
  // Repo onto repo relates them; doing it again is refused as duplicate.
  await synthDragStart(canvas, await canvasPoint(page, -60, 80));
  await synthDragMove(canvas, await canvasPoint(page, 150, 40));
  await synthDragEnd(canvas, await canvasPoint(page, 150, 40));
  await settled(page);
  await synthDragStart(canvas, await canvasPoint(page, -60, 80));
  await synthDragMove(canvas, await canvasPoint(page, 150, 40));
  await synthDragEnd(canvas, await canvasPoint(page, 150, 40));
  await expect(page.locator(".dx-notice")).toHaveText(
    "web and orders are already related.",
  );
  // Re-seat the worktree: every sibling freezes first (main included).
  await synthDragStart(canvas, await canvasPoint(page, -60, 80 - 48));
  await synthDragMove(canvas, await canvasPoint(page, 0, 40));
  await synthDragEnd(canvas, await canvasPoint(page, 0, 40));
  await settled(page);
  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  const download = await downloadPromise;
  const { readFileSync } = await import("node:fs");
  const saved = JSON.parse(
    readFileSync((await download.path()) as string, "utf8"),
  );
  expect(Object.keys(saved.placements.wts).sort()).toEqual([
    "web:checkout",
    "web:main",
  ]);
  const slot = saved.placements.wts["web:checkout"];
  expect(slot.dist).toBeGreaterThanOrEqual(48);
  expect(slot.dist).toBeLessThanOrEqual(150);
  // A worktree dragged onto another repo is refused.
  const checkoutPt = await canvasPoint(
    page,
    -60 + slot.dist * Math.cos(slot.ang),
    80 + slot.dist * Math.sin(slot.ang),
  );
  await synthDragStart(canvas, checkoutPt);
  await synthDragMove(canvas, await canvasPoint(page, 150, 40));
  await synthDragEnd(canvas, await canvasPoint(page, 150, 40));
  await expect(page.locator(".dx-notice")).toHaveText(
    "Worktrees stay with their repo — carry moves the changes.",
  );
});

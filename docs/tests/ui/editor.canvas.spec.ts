import { expect, test } from "@playwright/test";
import {
  canvasHash,
  canvasPoint,
  canvasWrap,
  chip,
  cursorOf,
  lastRepoActPos,
  openComposer,
  playerState,
  settled,
  settleStep,
  stageCanvas,
  synthDrag,
  synthDragEnd,
  synthDragMove,
  synthDragStart,
  synthTapAt,
  tapChip,
} from "./helpers";

/** CMP-F — canvas selection, the overlay markers, and the pointer:
 * hover affordance, live drag preview, cancel. */

test("CMP-F1 picking selects with a ring; empty space clears", async ({
  page,
}) => {
  await openComposer(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  const canvas = stageCanvas(page);
  // Deselect the freshly dropped repo first: tap empty space.
  await synthTapAt(canvas, await canvasPoint(page, 110, 80));
  await settled(page);
  const bare = await canvasHash(page);
  await synthTapAt(canvas, await canvasPoint(page, 0, 0));
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
  const ringed = await canvasHash(page);
  expect(ringed).not.toBe(bare);
  // Empty space clears both the form and the ring.
  await synthTapAt(canvas, await canvasPoint(page, 110, 80));
  await settled(page);
  await expect(page.locator(".dx-attrs-hint")).toBeVisible();
  expect(await canvasHash(page)).toBe(bare);
});

test("CMP-F2 the ring survives rebuilds and dies with its entity", async ({
  page,
}) => {
  await openComposer(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  const canvas = stageCanvas(page);
  // Re-place the selected disc: the drop rebuilds the player, the
  // selection (and its ring) carries over to the fresh frame.
  await synthDragStart(canvas, await canvasPoint(page, 0, 0));
  await synthDragMove(canvas, await canvasPoint(page, 70, -50));
  await synthDragEnd(canvas, await canvasPoint(page, 70, -50));
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
  const withRing = await canvasHash(page);
  await synthTapAt(canvas, await canvasPoint(page, -80, 80));
  await settled(page);
  await expect(page.locator(".dx-attrs-hint")).toBeVisible();
  expect(await canvasHash(page)).not.toBe(withRing);
  // Deleting the entity leaves no ring and an honest empty stage.
  await synthTapAt(canvas, await canvasPoint(page, 70, -50));
  await settled(page);
  await page.getByRole("button", { name: "Delete from the scene" }).click();
  await settled(page);
  await expect(page.locator(".dx-attrs-hint")).toBeVisible();
  await expect(page.locator(".dx-canvas-empty")).toBeVisible();
});

test("CMP-F3 hovering an entity points and paints; empty canvas does neither", async ({
  page,
}) => {
  await openComposer(page);
  const canvas = stageCanvas(page);
  const wrap = canvasWrap(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  // Off the disc first: no flag, default cursor, a baseline frame.
  const off = await canvasPoint(page, 0, -150);
  await page.mouse.move(off.x, off.y);
  await expect(wrap).not.toHaveAttribute("data-hover");
  expect(await cursorOf(canvas)).toBe("default");
  const plain = await canvasHash(page);
  // Over the disc: the wrap flags hover, the cursor points, the pack's
  // hover marker paints.
  const on = await canvasPoint(page, 0, 0);
  await page.mouse.move(on.x, on.y);
  await expect(wrap).toHaveAttribute("data-hover", "true");
  expect(await cursorOf(canvas)).toBe("pointer");
  expect(await canvasHash(page)).not.toBe(plain);
  // And back off: the marker goes with the flag.
  await page.mouse.move(off.x, off.y);
  await expect(wrap).not.toHaveAttribute("data-hover");
  expect(await canvasHash(page)).toBe(plain);
});

test("CMP-F4 a node drag previews live and commits on release, keeping the playhead", async ({
  page,
}) => {
  await openComposer(page);
  const canvas = stageCanvas(page);
  const wrap = canvasWrap(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  // A second step, so "keeps the playhead" can mean something — then park
  // on the seed step, not where the edit landed.
  await tapChip(page, "clone");
  await settleStep(page, 0);
  expect((await playerState(page))?.step).toBe(0);
  const from = await canvasPoint(page, 0, 0);
  const to = await canvasPoint(page, -60, 80);
  await synthDragStart(canvas, from);
  await synthDragMove(canvas, to);
  // Mid-flight: the scene already shows the move — the live player IS the
  // preview — the node is flagged as dragged, the cursor grabs, and no DOM
  // ghost floats around.
  expect(await lastRepoActPos(page, "web")).toEqual({ x: -60, y: 80 });
  await expect(wrap).toHaveAttribute("data-dragging", "true");
  expect(await cursorOf(canvas)).toBe("grabbing");
  await expect(page.locator(".dx-ghost")).toHaveCount(0);
  // Release: the commit is exactly what was previewed; the playhead stays.
  await synthDragEnd(canvas, to);
  await settled(page);
  expect(await lastRepoActPos(page, "web")).toEqual({ x: -60, y: 80 });
  await expect(wrap).not.toHaveAttribute("data-dragging");
  expect((await playerState(page))?.step).toBe(0);
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
});

test("CMP-F5 Escape abandons a drag: the base document comes back, nothing drops", async ({
  page,
}) => {
  await openComposer(page);
  const canvas = stageCanvas(page);
  const wrap = canvasWrap(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 0, 0));
  await settled(page);
  const from = await canvasPoint(page, 0, 0);
  const to = await canvasPoint(page, -60, 80);
  await synthDragStart(canvas, from);
  await synthDragMove(canvas, to);
  expect(await lastRepoActPos(page, "web")).toEqual({ x: -60, y: 80 });
  await page.keyboard.press("Escape");
  await settled(page);
  expect(await lastRepoActPos(page, "web")).toEqual({ x: 0, y: 0 });
  await expect(wrap).not.toHaveAttribute("data-dragging");
  // A release after the cancel is just a release.
  await synthDragEnd(canvas, to);
  await settled(page);
  expect(await lastRepoActPos(page, "web")).toEqual({ x: 0, y: 0 });
});

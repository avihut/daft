import { expect, test } from "@playwright/test";
import {
  canvasHash,
  canvasPoint,
  chip,
  openComposer,
  settled,
  stageCanvas,
  synthDrag,
  synthDragEnd,
  synthDragMove,
  synthDragStart,
  synthTapAt,
} from "./helpers";

/** CMP-F — canvas selection and the overlay ring. */

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

import { expect, test } from "@playwright/test";
import {
  canvasHash,
  canvasPoint,
  canvasWrap,
  chip,
  cursorOf,
  lastRepoActPos,
  openComposer,
  settled,
  stageCanvas,
  synthDrag,
  synthDragEnd,
  synthDragMove,
  synthDragStart,
  synthTapAt,
} from "./helpers";

/** CMP-M — relations as entities (daft pack): the line between two repos
 * is hoverable, selectable, never draggable, and seed relations unrelate
 * from the Attributes card. */

test("CMP-M1 a relation hovers, selects, refuses to move, and unrelates", async ({
  page,
}) => {
  await openComposer(page);
  const canvas = stageCanvas(page);
  const wrap = canvasWrap(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, -120, 0));
  await settled(page);
  await synthDrag(chip(page, "repo"), await canvasPoint(page, 120, 0));
  await settled(page);
  // Relate by dragging web onto orders.
  await synthDragStart(canvas, await canvasPoint(page, -120, 0));
  await synthDragMove(canvas, await canvasPoint(page, 120, 0));
  await synthDragEnd(canvas, await canvasPoint(page, 120, 0));
  await settled(page);
  await expect(page.locator(".dx-rel-line")).toHaveText("web ↔ orders");
  // The line's midpoint is a hit: hover points, the marker paints.
  const mid = await canvasPoint(page, 0, 0);
  const far = await canvasPoint(page, 0, -140);
  await page.mouse.move(far.x, far.y);
  const plain = await canvasHash(page);
  await page.mouse.move(mid.x, mid.y);
  await expect(wrap).toHaveAttribute("data-hover", "true");
  expect(await cursorOf(canvas)).toBe("pointer");
  expect(await canvasHash(page)).not.toBe(plain);
  // A tap selects it: the relation card, and the selection marker along
  // the line.
  await page.mouse.move(far.x, far.y);
  await synthTapAt(canvas, mid);
  await expect(page.locator(".dx-attrs .dx-tag")).toHaveText("relation");
  await expect(page.locator(".dx-attrs .dx-insp-head b")).toHaveText(
    "web ↔ orders",
  );
  expect(await canvasHash(page)).not.toBe(plain);
  await expect(page.locator(".dx-rel-line")).toHaveClass(/sel/);
  // Dragging the line only selects — relations follow their repos.
  await synthDragStart(canvas, mid);
  await synthDragMove(canvas, await canvasPoint(page, 0, 80));
  await synthDragEnd(canvas, await canvasPoint(page, 0, 80));
  await settled(page);
  expect(await lastRepoActPos(page, "web")).toEqual({ x: -120, y: 0 });
  expect(await lastRepoActPos(page, "orders")).toEqual({ x: 120, y: 0 });
  await expect(page.locator(".dx-rel-line")).toHaveCount(1);
  // The World view's relation row selects it too; Unrelate cuts it.
  await synthTapAt(canvas, far);
  await expect(page.locator(".dx-rel-line")).not.toHaveClass(/sel/);
  await page.locator(".dx-rel-line").click();
  await expect(page.locator(".dx-attrs .dx-tag")).toHaveText("relation");
  await page.getByRole("button", { name: "Unrelate" }).click();
  await settled(page);
  await expect(page.locator(".dx-rel-line")).toHaveCount(0);
  await expect(page.getByText("web ↔ orders")).toHaveCount(0);
});

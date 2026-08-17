import { expect, test } from "@playwright/test";
import {
  expectParked,
  openComposer,
  playerState,
  rowAction,
  rowVerbs,
  settled,
  tapChip,
} from "./helpers";

/** CMP-C — timeline editing through rows, buttons, and annotations. */

test("CMP-C1 selecting a row parks; reselecting releases without moving", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  // Adding start left it selected; select the clone row instead.
  await page.locator(".dx-vrow b", { hasText: "clone" }).click();
  await settled(page);
  await expect(page.locator(".dx-vrow").nth(0)).toHaveClass(/\bsel\b/);
  const parked = await expectParked(page);
  expect(parked.step).toBe(0);
  // Clicking the selected row again deselects but keeps the playhead.
  await page.locator(".dx-vrow b", { hasText: "clone" }).click();
  await settled(page);
  await expect(page.locator(".dx-vrow.sel")).toHaveCount(0);
  const after = await playerState(page);
  expect(after?.t).toBe(parked.t);
});

test("CMP-C2 reordering above prerequisites marks the row skipped", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await rowAction(page, 1, "Move up");
  expect(await rowVerbs(page)).toEqual(["start", "clone"]);
  await expect(page.locator(".dx-vrow").nth(0)).toHaveClass(/\bdead\b/);
  await expect(page.locator(".dx-vrow .dx-skipped")).toHaveText("skipped");
  await rowAction(page, 0, "Move down");
  expect(await rowVerbs(page)).toEqual(["clone", "start"]);
  await expect(page.locator(".dx-vrow.dead")).toHaveCount(0);
});

test("CMP-C3 deleting a row shifts selection honestly", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  // Adding list selected it (index 2); delete start (index 1) below it:
  // the selection follows list down to index 1.
  await tapChip(page, "list");
  await rowAction(page, 1, "Remove");
  expect(await rowVerbs(page)).toEqual(["clone", "list"]);
  await expect(page.locator(".dx-vrow").nth(1)).toHaveClass(/\bsel\b/);
  // Deleting the selected row clears the selection.
  await rowAction(page, 1, "Remove");
  expect(await rowVerbs(page)).toEqual(["clone"]);
  await expect(page.locator(".dx-vrow.sel")).toHaveCount(0);
});

test("CMP-C4 chapters are rail notches, selectable and renameable", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "chapter");
  // A chapter is a notch on the rail, not a full row.
  await expect(page.locator(".dx-vrow")).toHaveCount(1);
  await expect(page.locator(".dx-chap .dx-notch")).toHaveCount(1);
  // The chip tap selected it; move selection away, then notch-click back.
  await page.locator(".dx-vrow b", { hasText: "clone" }).click();
  await settled(page);
  await expect(page.locator(".dx-chap.sel")).toHaveCount(0);
  await page.locator(".dx-chap .dx-notch").click();
  await settled(page);
  await expect(page.locator(".dx-chap")).toHaveClass(/\bsel\b/);
  // Rename through the Attributes form; the bloom label follows.
  await page.locator("#dx-f-chapter").fill("Act I");
  await page.locator("#dx-f-chapter").blur();
  await settled(page);
  await expect(page.locator(".dx-chap-name")).toHaveText("Act I");
});

test("CMP-C5 beats stretch the preceding step", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const before = await expectParked(page);
  await tapChip(page, "beat");
  await expect(page.locator(".dx-vrow.beat")).toHaveCount(1);
  const withBeat = await expectParked(page);
  expect(withBeat.duration).toBeCloseTo(before.duration + 1, 1);
  // Stretch it further through the Attributes form.
  await page.locator("#dx-f-beat").fill("2.5");
  await page.locator("#dx-f-beat").blur();
  await settled(page);
  const stretched = await playerState(page);
  expect(stretched?.duration).toBeCloseTo(before.duration + 2.5, 1);
});

test("CMP-C5b a beat with nothing before it maps to no step", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "beat");
  await expect(page.locator(".dx-vrow.beat")).toHaveCount(1);
  // No steps derive from a lone beat: the stage stays empty, no player.
  expect(await playerState(page)).toBeNull();
  await expect(page.locator(".dx-canvas-empty")).toBeVisible();
});

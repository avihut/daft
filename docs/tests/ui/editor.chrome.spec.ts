import { expect, test } from "@playwright/test";
import { canvasHash, openComposer, tapChip } from "./helpers";

/** CMP-J — panes and chrome: toggles, the minimize matrix, theme. */

test("CMP-J1 the scrubber toggle hides only the player bar", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await expect(page.locator(".dx-playbar")).toBeVisible();
  await page.getByRole("button", { name: "Scrubber" }).click();
  await expect(page.locator(".dx-playbar")).toHaveCount(0);
  await expect(page.locator(".dx-seq")).toBeVisible();
  await expect(page.locator(".dx-term")).toBeVisible();
  await expect(page.locator(".dx-canvas-wrap")).toBeVisible();
  await page.getByRole("button", { name: "Scrubber" }).click();
  await expect(page.locator(".dx-playbar")).toBeVisible();
});

test("CMP-J2 the minimize matrix folds and restores honestly", async ({
  page,
}) => {
  await openComposer(page);
  // Timeline chevron: section folds to an in-pane flap.
  await page.getByRole("button", { name: "Minimize the timeline" }).click();
  await expect(page.locator(".dx-seq")).toHaveCount(0);
  const inPaneFlap = page.locator(".dx-flap.in-pane", { hasText: "Timeline" });
  await expect(inPaneFlap).toBeVisible();
  await inPaneFlap.click();
  await expect(page.locator(".dx-seq")).toBeVisible();
  // Catalog chevron folds its body, head stays.
  await page.getByRole("button", { name: "Minimize the catalog" }).click();
  await expect(page.locator(".dx-cat-body")).toHaveCount(0);
  await expect(page.locator(".dx-cat-search input")).toBeVisible();
  // Both minimized: the whole sidepane folds to the flap rail.
  await page.getByRole("button", { name: "Minimize the timeline" }).click();
  await expect(page.locator(".dx-body")).toHaveClass(/\bside-min\b/);
  await expect(page.locator(".dx-flap")).toHaveCount(2);
  // Either flap restores its section and unfolds the pane.
  await page.locator(".dx-flap", { hasText: "Catalog" }).click();
  await expect(page.locator(".dx-body")).not.toHaveClass(/\bside-min\b/);
  await expect(page.locator(".dx-cat-body")).toBeVisible();
  // The timeline is still folded from before — restore it to get its
  // header back (the « control lives there), then collapse directly.
  await page.locator(".dx-flap.in-pane", { hasText: "Timeline" }).click();
  await expect(page.locator(".dx-seq")).toBeVisible();
  await page.getByRole("button", { name: "Collapse the sidepane" }).click();
  await expect(page.locator(".dx-body")).toHaveClass(/\bside-min\b/);
  await page.locator(".dx-flap", { hasText: "Timeline" }).click();
  await expect(page.locator(".dx-body")).not.toHaveClass(/\bside-min\b/);
  await expect(page.locator(".dx-seq")).toBeVisible();
});

test("CMP-J3 the terminal minimizes to a bottom flap", async ({ page }) => {
  await openComposer(page);
  await page.getByRole("button", { name: "Minimize the terminal" }).click();
  await expect(page.locator(".dx-term")).toHaveCount(0);
  const flap = page.locator(".dx-term-flap", { hasText: "Terminal" });
  await expect(flap).toBeVisible();
  await flap.click();
  await expect(page.locator(".dx-term")).toBeVisible();
});

test("CMP-J4 the theme toggle repaints the canvas palette", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const startedDark = await page.evaluate(() =>
    document.documentElement.classList.contains("dark"),
  );
  const before = await canvasHash(page);
  await page
    .getByRole("button", { name: /Switch to (light|dark) theme/ })
    .click();
  await page.waitForTimeout(400);
  expect(
    await page.evaluate(() =>
      document.documentElement.classList.contains("dark"),
    ),
  ).toBe(!startedDark);
  const flipped = await canvasHash(page);
  expect(flipped).not.toBe(before);
  await page
    .getByRole("button", { name: /Switch to (light|dark) theme/ })
    .click();
  await page.waitForTimeout(400);
  expect(
    await page.evaluate(() =>
      document.documentElement.classList.contains("dark"),
    ),
  ).toBe(startedDark);
  expect(await canvasHash(page)).toBe(before);
});

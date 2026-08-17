import { expect, test } from "@playwright/test";
import { chip, expectParked, openComposer, rowVerbs, tapChip } from "./helpers";

/** CMP-B — building documents from the catalog. */

test("CMP-B1 a chip tap appends at the insertion point and parks", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  expect(await rowVerbs(page)).toEqual(["clone"]);
  await expect(page.locator(".dx-vrow.sel")).toHaveCount(1);
  const snap = await expectParked(page);
  expect(snap.step).toBe(0);
  expect(snap.t).toBeGreaterThan(0);
  expect(snap.duration).toBeGreaterThan(0);
});

test("CMP-B2 chip seeding is world-aware", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  const rest = await page.locator(".dx-vrow .dx-vrest").last().textContent();
  expect(rest).toContain("checkout");
  expect(rest).toContain("web");
});

test("CMP-B3 groups derive from the registry", async ({ page }) => {
  await openComposer(page);
  // cd is typed-only: reachable from the shell, absent from the palette.
  await expect(chip(page, "cd")).toHaveCount(0);
  await expect(
    page.locator('h4:text-is("Elements") + .dx-chips .dx-chip'),
  ).toHaveCount(4);
  // Every non-typed-only verb: 18 verbs minus cd.
  await expect(
    page.locator('h4:text-is("Verbs") + .dx-chips .dx-chip'),
  ).toHaveCount(17);
  // Events render dashed, agent ones carry the purple dot.
  await expect(page.locator(".dx-chip.ev")).toHaveCount(3);
  await expect(page.locator(".dx-chip .dx-evdot.agent")).toHaveCount(2);
  for (const label of ["agent joins", "agent leaves", "forge merges"]) {
    await expect(chip(page, label)).toHaveClass(/\bev\b/);
  }
  const meta = page.locator('h4:text-is("Meta") + .dx-chips .dx-chip');
  await expect(meta).toHaveText(["chapter", "beat"]);
});

test("CMP-B4 search filters across groups and restores on clear", async ({
  page,
}) => {
  await openComposer(page);
  const search = page.locator(".dx-cat-search input");
  await search.fill("carr");
  await expect(page.locator(".dx-cat-body .dx-chip")).toHaveText(["carry"]);
  await expect(page.locator(".dx-cat-body h4")).toHaveCount(1);
  await search.fill("zzzz");
  await expect(page.locator(".dx-cat-body .dx-chip")).toHaveCount(0);
  await expect(page.locator(".dx-cat-body .dx-empty")).toContainText(
    'Nothing matches "zzzz"',
  );
  await search.fill("");
  await expect(page.locator(".dx-cat-body h4")).toHaveCount(4);
});

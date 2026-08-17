import { expect, test } from "@playwright/test";
import { openComposer, settled, tapChip } from "./helpers";

/** CMP-G — the World | daft | Files views at the playhead. */

test("CMP-G1 world rows carry the markers and select entities", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  const rows = page.locator(".dx-trow");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(1).locator(".dx-dot")).toHaveClass(/\bgold\b/);
  await expect(rows.nth(1)).toContainText("main");
  await expect(rows.nth(2)).toContainText("checkout");
  await expect(rows.nth(2).locator(".dx-port")).toHaveText(":3001");
  // A row click selects the entity; the selection highlights the row.
  await rows.nth(2).click();
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("checkout");
  await expect(rows.nth(2)).toHaveClass(/\bsel\b/);
});

test("CMP-G2 the daft registry lists repos, paths, and relations", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "clone");
  await tapChip(page, "repo link");
  await page.getByRole("tab", { name: "daft" }).click();
  const crows = page.locator(".dx-crow");
  await expect(crows).toHaveCount(2);
  await expect(crows.nth(0)).toContainText("web");
  await expect(crows.nth(0).locator(".dx-path")).toHaveText("~/web");
  await expect(crows.nth(1).locator(".dx-path")).toHaveText("~/orders");
  await expect(page.locator(".dx-view .dx-rel-line")).toHaveText(
    /(web ↔ orders|orders ↔ web)/,
  );
});

test("CMP-G3 files mirror the layout; hollow folders wait for prune", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "push");
  await tapChip(page, "forge merges");
  await page.getByRole("tab", { name: "Files" }).click();
  const frows = page.locator(".dx-frow");
  // ~, web, main, checkout — the merged (hollow) folder is still there.
  await expect(frows).toHaveCount(4);
  await expect(frows.nth(3)).toContainText("checkout");
  // The cwd marker sits where the story last cd'd (start moved it).
  await expect(page.locator(".dx-frow.cwd")).toContainText("checkout");
  await tapChip(page, "prune");
  await expect(frows).toHaveCount(3);
  await expect(page.locator(".dx-ftree")).not.toContainText("checkout");
  // cwd clamps to the nearest surviving worktree, not a bare repo dir.
  await expect(page.locator(".dx-frow.cwd")).toHaveCount(1);
  await expect(page.locator(".dx-frow.cwd")).toContainText("main");
});

test("CMP-G4 all three views trim to the playhead", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "agent joins");
  // Parked at the end: checkout exists and carries the agent.
  await expect(page.locator(".dx-trow")).toHaveCount(3);
  await expect(
    page.locator(".dx-trow", { hasText: "checkout" }).locator(".dx-agent-chip"),
  ).toHaveCount(1);
  // Park on clone: the later steps' world is gone from every view.
  await page.locator(".dx-vrow b", { hasText: "clone" }).click();
  await settled(page);
  await expect(page.locator(".dx-trow")).toHaveCount(2);
  await page.getByRole("tab", { name: "Files" }).click();
  await expect(page.locator(".dx-frow")).toHaveCount(3);
  await expect(page.locator(".dx-ftree")).not.toContainText("checkout");
  await page.getByRole("tab", { name: "daft" }).click();
  await expect(page.locator(".dx-crow")).toHaveCount(1);
});

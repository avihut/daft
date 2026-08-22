import { expect, test } from "@playwright/test";
import {
  canvasPoint,
  cmdLines,
  lastRepoActPos,
  openComposer,
  rowVerbs,
  settled,
  stageCanvas,
  synthTapAt,
  tapChip,
} from "./helpers";

/**
 * CMP-E4 — entity rename is doc-wide (daft-pack semantics: the transcript
 * is a projection, so renaming rewrites every past command; geometry is
 * frozen first so nothing teleports).
 */

test("CMP-E4 renaming a repo rewrites history with frozen geometry", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  const before = await lastRepoActPos(page, "web");
  if (!before) throw new Error("no repo act for web");
  await synthTapAt(
    stageCanvas(page),
    await canvasPoint(page, before.x, before.y),
  );
  await settled(page);
  await expect(page.locator("#dx-f-ename")).toHaveValue("web");
  await page.locator("#dx-f-ename").fill("store");
  await page.locator("#dx-f-ename").press("Enter");
  await settled(page);
  await expect(page.locator(".dx-notice")).toHaveText(
    "Renamed web → store everywhere",
  );
  // The past clone command now tells the new name.
  const cmds = await cmdLines(page);
  expect(cmds[0]).toContain("daft clone git@github.com:acme/store.git");
  expect(cmds[0]).not.toContain("web");
  // start's short form is unchanged, but its args were rewritten.
  expect(cmds[1]).toContain("daft start checkout");
  await expect(
    page.locator(".dx-vrow").nth(1).locator(".dx-vrest"),
  ).toContainText("store");
  expect(await rowVerbs(page)).toEqual(["clone", "start"]);
  // Selection followed the rename; the disc never moved.
  await expect(page.locator("#dx-f-ename")).toHaveValue("store");
  expect(await lastRepoActPos(page, "store")).toEqual(before);
  expect(await lastRepoActPos(page, "web")).toBeNull();
});

import { readFileSync } from "node:fs";
import { DOC_VERSION, parseDoc, serializeDoc } from "@dumbshow/core";
import { expect, test } from "@playwright/test";
import {
  expectParked,
  openComposer,
  playerState,
  rowVerbs,
  tapChip,
} from "./helpers";

/**
 * CMP-A — document lifecycle. The `editor.` prefix marks specs that test
 * generic editor mechanics (they move to the extracted package's harness
 * eventually); `pack.` specs cover daft-language behavior and stay here.
 */

const DRAFT_KEY = `daft-composer-draft-v${DOC_VERSION}`;

test("CMP-A1 empty state offers nothing that needs a document", async ({
  page,
}) => {
  await openComposer(page);
  await expect(page.locator(".dx-vrow")).toHaveCount(0);
  await expect(page.locator(".dx-canvas-empty")).toBeVisible();
  await expect(
    page.locator(".dx-chrome-btn.primary", { hasText: "Export" }),
  ).toBeDisabled();
  await expect(page.locator(".dx-ttime")).toHaveText(/0:00\.0\s*\/\s*0:00\.0/);
  for (const name of [
    "Jump to start",
    "Previous step",
    "Play",
    "Next step",
    "Jump to end",
  ]) {
    await expect(
      page.getByRole("button", { name, exact: true }),
    ).toBeDisabled();
  }
});

test("CMP-A2 draft autosaves and restores across a reload", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  expect(await rowVerbs(page)).toEqual(["clone", "start"]);
  // Outlast the 800ms debounce so the final state is on disk.
  await page.waitForTimeout(950);
  await page.reload();
  await page.waitForSelector(".dx-app");
  // The restore notice auto-dismisses after 4s — assert it promptly.
  await expect(page.locator(".dx-notice")).toHaveText("Draft restored");
  expect(await rowVerbs(page)).toEqual(["clone", "start"]);
  await expectParked(page);
});

test("CMP-A3 save downloads the document and parseDoc roundtrips it", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const downloadPromise = page.waitForEvent("download");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("untitled-scenario.daft.json");
  const text = readFileSync((await download.path()) as string, "utf8");
  const raw = JSON.parse(text);
  expect(Object.keys(raw).sort()).toEqual([
    "placements",
    "seed",
    "timeline",
    "title",
    "version",
  ]);
  expect(raw.version).toBe(DOC_VERSION);
  expect(raw.timeline).toHaveLength(1);
  // The serializer and parser agree byte for byte.
  expect(serializeDoc(parseDoc(text))).toBe(text);
});

test("CMP-A4 open validates and never clobbers the current document", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const input = page.locator('input[type="file"]');
  await input.setInputFiles({
    name: "future.daft.json",
    mimeType: "application/json",
    buffer: Buffer.from('{"version": 99}'),
  });
  await expect(page.locator(".dx-notice")).toContainText("newer composer");
  expect(await rowVerbs(page)).toEqual(["clone"]);
  await input.setInputFiles({
    name: "garbage.daft.json",
    mimeType: "application/json",
    buffer: Buffer.from("this is not json"),
  });
  await expect(page.locator(".dx-notice")).toContainText("not valid JSON");
  expect(await rowVerbs(page)).toEqual(["clone"]);
});

test("CMP-A5 a corrupt draft is discarded, not restored", async ({ page }) => {
  await openComposer(page);
  await page.evaluate(
    (key) => localStorage.setItem(key, "{{{ definitely not a doc"),
    DRAFT_KEY,
  );
  await page.reload();
  await page.waitForSelector(".dx-app");
  await expect(page.locator(".dx-vrow")).toHaveCount(0);
  expect(
    await page.evaluate((key) => localStorage.getItem(key), DRAFT_KEY),
  ).toBeNull();
});

test("CMP-A6 retitling updates the filename without rebuilding the player", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const before = await expectParked(page);
  // Tag the live player object: a rebuild would produce a fresh one.
  await page.evaluate(() => {
    (
      window as unknown as { __daftPlayer: { __probe?: number } }
    ).__daftPlayer.__probe = 7;
  });
  await page.locator(".dx-title").fill("Ship a feature");
  await page.locator(".dx-title").press("Enter");
  await expect(page.locator(".dx-filename")).toHaveText(
    "ship-a-feature.daft.json",
  );
  const probe = await page.evaluate(
    () =>
      (window as unknown as { __daftPlayer: { __probe?: number } }).__daftPlayer
        .__probe,
  );
  expect(probe).toBe(7);
  const after = await playerState(page);
  expect(after?.t).toBe(before.t);
  expect(after?.step).toBe(before.step);
});

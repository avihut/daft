import { expect, type Page, test } from "@playwright/test";
import {
  expectParked,
  openComposer,
  playerState,
  rowAction,
  settled,
  synthTapAt,
  tapChip,
} from "./helpers";

/** CMP-I — the player: no autoplay, transport, scrubber, speed, loop. */

/* The evaluate callbacks are serialized into the page, so each one casts
 * the dev handle inline — no shared closures survive the boundary. */

async function stepTime(
  page: Page,
  index: number,
  field: "at" | "cue",
): Promise<number> {
  return page.evaluate(
    ([i, f]) => {
      const p = (
        window as unknown as {
          __daftPlayer: { compiled: { steps: { at: number; cue: number }[] } };
        }
      ).__daftPlayer;
      return p.compiled.steps[i as number][f as "at" | "cue"];
    },
    [index, field] as const,
  );
}

async function seekTo(page: Page, t: number): Promise<void> {
  await page.evaluate((target) => {
    (
      window as unknown as { __daftPlayer: { seek(t: number): void } }
    ).__daftPlayer.seek(target);
  }, t);
}

test("CMP-I1 no autoplay, ever, on any mutation path", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await expectParked(page);
  await tapChip(page, "start");
  await expectParked(page);
  await rowAction(page, 1, "Move up");
  await expectParked(page);
  await rowAction(page, 0, "Move down");
  await expectParked(page);
  await page.locator("#dx-f-branch").fill("perf/cache");
  await page.locator("#dx-f-branch").blur();
  await settled(page);
  await expectParked(page);
  const shell = page.getByRole("textbox", {
    name: "Type a daft command — it lands on the timeline",
  });
  await shell.fill("daft list");
  await shell.press("Enter");
  await settled(page);
  await expectParked(page);
  await rowAction(page, 2, "Remove");
  await expectParked(page);
  // Draft restore parks too.
  await page.waitForTimeout(950);
  await page.reload();
  await page.waitForSelector(".dx-app");
  await expectParked(page);
});

test("CMP-I2 transport: toggle, step, jump, restart from the end", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await page.getByRole("button", { name: "Play", exact: true }).click();
  await page.waitForTimeout(200);
  expect((await playerState(page))?.playing).toBe(true);
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  expect((await playerState(page))?.playing).toBe(false);
  // Step transport.
  await page
    .getByRole("button", { name: "Previous step", exact: true })
    .click();
  await settled(page);
  expect((await playerState(page))?.step).toBe(0);
  await page.getByRole("button", { name: "Next step", exact: true }).click();
  await settled(page);
  expect((await playerState(page))?.step).toBe(1);
  await page
    .getByRole("button", { name: "Jump to start", exact: true })
    .click();
  await settled(page);
  expect((await playerState(page))?.t).toBe(0);
  await page.getByRole("button", { name: "Jump to end", exact: true }).click();
  await settled(page);
  const atEnd = await playerState(page);
  expect(atEnd?.t).toBeCloseTo(atEnd?.duration ?? -1, 3);
  // Play at the end restarts from zero.
  await page.getByRole("button", { name: "Play", exact: true }).click();
  await page.waitForTimeout(250);
  const restarted = await playerState(page);
  expect(restarted?.playing).toBe(true);
  expect(restarted?.t).toBeLessThan(2);
  await page.getByRole("button", { name: "Pause", exact: true }).click();
});

test("CMP-I3 the scrubber seeks; chapter notches sit and seek", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "chapter");
  await tapChip(page, "start");
  const scrub = page.locator(".dx-scrub");
  const box = await scrub.boundingBox();
  if (!box) throw new Error("no scrubber box");
  // Pointer-down at 30% seeks to 30% of the duration.
  await synthTapAt(scrub, { x: box.x + box.width * 0.3, y: box.y + 4 });
  await settled(page);
  const snap = await playerState(page);
  expect(snap && Math.abs(snap.t - snap.duration * 0.3)).toBeLessThan(0.2);
  // The chapter's notch sits at its bound step's start and seeks there.
  const stepAt = await stepTime(page, 1, "at");
  await page.locator(".dx-snotch").click();
  await settled(page);
  expect((await playerState(page))?.t).toBeCloseTo(stepAt, 3);
  // The chapter name blooms on bar hover.
  await scrub.hover();
  await page.waitForTimeout(350);
  const opacity = await page
    .locator(".dx-schap")
    .evaluate((el) => getComputedStyle(el).opacity);
  expect(Number(opacity)).toBeGreaterThan(0.5);
});

test("CMP-I4 speed and loop wire through to the clock", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  // 2× advances roughly twice as far in the same wall time.
  await page
    .getByRole("button", { name: "Jump to start", exact: true })
    .click();
  await page.locator(".dx-speed").selectOption("2");
  await page.getByRole("button", { name: "Play", exact: true }).click();
  await page.waitForTimeout(500);
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  const fast = await playerState(page);
  expect(fast?.t).toBeGreaterThan(0.7);
  await page.locator(".dx-speed").selectOption("1");
  // Loop: playing across the end wraps instead of stopping.
  await page.getByRole("button", { name: "Loop off", exact: true }).click();
  const duration = (await playerState(page))?.duration ?? 0;
  await seekTo(page, duration - 0.15);
  await page.getByRole("button", { name: "Play", exact: true }).click();
  await page.waitForTimeout(600);
  const wrapped = await playerState(page);
  expect(wrapped?.playing).toBe(true);
  expect(wrapped?.t).toBeLessThan(duration - 0.2);
  await page.getByRole("button", { name: "Pause", exact: true }).click();
  // Loop off: the end pauses the clock at the end.
  await page.getByRole("button", { name: "Loop on", exact: true }).click();
  await seekTo(page, duration - 0.15);
  await page.getByRole("button", { name: "Play", exact: true }).click();
  await page.waitForTimeout(600);
  const ended = await playerState(page);
  expect(ended?.playing).toBe(false);
  expect(ended?.t).toBeCloseTo(duration, 2);
});

test("CMP-I5 a terminal click parks on the checkpoint and selects", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "list");
  await page
    .locator(".dx-tline.is-cmd", { hasText: "daft start" })
    .first()
    .click();
  await settled(page);
  const cue = await stepTime(page, 1, "cue");
  const snap = await playerState(page);
  expect(snap?.playing).toBe(false);
  expect(snap?.step).toBe(1);
  expect(snap && Math.abs(snap.t - cue)).toBeLessThan(0.1);
  await expect(page.locator(".dx-vrow").nth(1)).toHaveClass(/\bsel\b/);
});

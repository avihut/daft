import { expect, test } from "@playwright/test";

/**
 * The landing page — the install line, the five points, and the players
 * behind the scenes (see .vitepress/theme/landing/CLAUDE.md). These specs
 * pin the page's shape, not its pixels: the hero anchor (pack.hero.spec)
 * owns the canvas.
 */

interface PlayerLike {
  playing(): boolean;
  current(): number;
  clock(): number;
  compiled: { steps: unknown[] };
}

async function openLanding(page: import("@playwright/test").Page) {
  await page.goto("/");
  await page.waitForSelector(".VPHero .dl-install");
}

test("the install line switches per platform and copies the command", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openLanding(page);
  const hero = page.locator(".VPHero");
  const cmd = hero.locator(".dl-install-cmd");
  await hero.getByRole("tab", { name: "macOS" }).click();
  await expect(cmd).toContainText("brew install avihut/tap/daft");
  await hero.getByRole("tab", { name: "Linux" }).click();
  await expect(cmd).toContainText("daft-installer.sh | sh");
  await hero.getByRole("tab", { name: "Windows" }).click();
  await expect(cmd).toContainText("daft-installer.ps1 | iex");
  await hero.getByRole("tab", { name: "macOS" }).click();
  await hero.locator(".dl-install-copy").click();
  await expect(hero.locator(".dl-install-copy")).toHaveText("Copied");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "brew install avihut/tap/daft",
  );
  // The meta line carries the two ways onward.
  await expect(
    hero.locator('.dl-install-meta a[href="/getting-started/installation"]'),
  ).toBeVisible();
  await expect(
    hero.locator('.dl-install-meta a[href="/getting-started/quick-start"]'),
  ).toBeVisible();
});

test("five points in workflow order, four of them replaying a scene", async ({
  page,
}) => {
  await openLanding(page);
  const points = page.locator(".dl-point");
  await expect(points).toHaveCount(5);
  await expect(points.locator("h2")).toHaveText([
    "Every branch gets its own directory.",
    "Switching costs nothing.",
    "One feature across many repos — still one thing.",
    "Agents work in parallel, each in its own worktree.",
    "Ship and clean up from anywhere.",
  ]);
  await expect(points.locator(".dl-point-n")).toHaveText([
    "01",
    "02",
    "03",
    "04",
    "05",
  ]);
  await expect(page.locator(".dl-point canvas")).toHaveCount(4);
  await expect(page.locator("#point-switch .dl-ba")).toBeVisible();
  // Every point links onward into the docs.
  await expect(points.locator(".dl-point-link")).toHaveCount(5);
  // The page's first canvas is the hero's — pack.hero.spec hashes it.
  expect(
    await page
      .locator("canvas")
      .first()
      .evaluate((el) => el.closest(".dl-hero-stage") !== null),
  ).toBe(true);
});

test("each scene runs its own player; a command click lands that scene only", async ({
  page,
}) => {
  await openLanding(page);
  await page.waitForFunction(() => {
    const w = window as unknown as Record<string, unknown>;
    return Boolean(w.__daftPlayer) && Boolean(w.__daftPoint_directory);
  });
  const shape = await page.evaluate(() => {
    const w = window as unknown as Record<string, PlayerLike>;
    return {
      distinct: w.__daftPlayer !== w.__daftPoint_directory,
      heroSteps: w.__daftPlayer.compiled.steps.length,
      pointSteps: w.__daftPoint_directory.compiled.steps.length,
    };
  });
  expect(shape.distinct).toBe(true);
  expect(shape.heroSteps).toBe(7);
  expect(shape.pointSteps).toBe(4);

  await page.locator("#point-directory").scrollIntoViewIfNeeded();
  // Freeze the hero (it may still peek into the viewport and keep playing)
  // so "untouched" is a clock comparison, not a race.
  const heroClock = await page.evaluate(() => {
    const p = (
      window as unknown as Record<string, PlayerLike & { pause(): void }>
    ).__daftPlayer;
    p.pause();
    return p.clock();
  });
  // Let the scene type its second command, then click it: the scene lands
  // on that step's checkpoint, paused; the hero is untouched. Committed
  // commands carry role="button" — the line being typed has no handler.
  const second = page
    .locator('#point-directory .dl-ln.is-cmd[role="button"]')
    .nth(1);
  await expect(second).toBeVisible({ timeout: 15_000 });
  await second.click();
  const after = await page.evaluate(() => {
    const w = window as unknown as Record<string, PlayerLike>;
    return {
      pointPlaying: w.__daftPoint_directory.playing(),
      pointStep: w.__daftPoint_directory.current(),
      heroClock: w.__daftPlayer.clock(),
      heroPlaying: w.__daftPlayer.playing(),
    };
  });
  expect(after.pointPlaying).toBe(false);
  expect(after.pointStep).toBe(1);
  expect(after.heroClock).toBe(heroClock);
  expect(after.heroPlaying).toBe(false);
});

test("under reduced motion the hero and the scenes settle on their last step, paused", async ({
  page,
}) => {
  // Emulated on the page before navigation: the players read
  // prefers-reduced-motion once, when they are created.
  await page.emulateMedia({ reducedMotion: "reduce" });
  await openLanding(page);
  await page.waitForFunction(() => {
    const w = window as unknown as Record<string, unknown>;
    return Boolean(w.__daftPlayer) && Boolean(w.__daftPoint_ship);
  });
  const state = await page.evaluate(() => {
    const w = window as unknown as Record<string, PlayerLike>;
    const at = (p: PlayerLike) => ({
      step: p.current(),
      last: p.compiled.steps.length - 1,
      playing: p.playing(),
    });
    return { hero: at(w.__daftPlayer), ship: at(w.__daftPoint_ship) };
  });
  expect(state.hero.step).toBe(state.hero.last);
  expect(state.hero.playing).toBe(false);
  expect(state.ship.step).toBe(state.ship.last);
  expect(state.ship.playing).toBe(false);
});

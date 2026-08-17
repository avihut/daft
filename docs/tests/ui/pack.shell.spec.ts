import { expect, type Locator, type Page, test } from "@playwright/test";
import {
  cmdLines,
  expectParked,
  openComposer,
  rowVerbs,
  settled,
  tapChip,
} from "./helpers";

/** CMP-H — the shell as an editor (daft grammar, cwd, edit-in-place). */

async function typeLine(page: Page, line: string): Promise<void> {
  const input = page.getByRole("textbox", {
    name: "Type a daft command — it lands on the timeline",
  });
  await input.fill(line);
  await input.press("Enter");
  await settled(page);
}

function cmdLine(page: Page, text: string): Locator {
  return page.locator(".dx-tline.is-cmd", { hasText: text }).first();
}

test("CMP-H1 a typed command lands as a row and re-renders", async ({
  page,
}) => {
  await openComposer(page);
  await typeLine(page, "daft clone git@github.com:acme/web.git");
  expect(await rowVerbs(page)).toEqual(["clone"]);
  expect((await cmdLines(page))[0]).toContain(
    "daft clone git@github.com:acme/web.git",
  );
  await expect(
    page.getByRole("textbox", {
      name: "Type a daft command — it lands on the timeline",
    }),
  ).toHaveValue("");
  await expectParked(page);
});

test("CMP-H2 invalid lines print ephemerally and never persist", async ({
  page,
}) => {
  await openComposer(page);
  await typeLine(page, "daft go billing");
  await expect(page.locator(".dx-tline.is-rust")).toHaveText(
    'error: unknown repo "billing" — nothing cloned or added by that name',
  );
  await expect(page.locator(".dx-vrow")).toHaveCount(0);
  // The next valid line clears the error.
  await typeLine(page, "daft clone git@github.com:acme/web.git");
  await expect(page.locator(".dx-tline.is-rust")).toHaveCount(0);
  // A rebuild clears it too.
  await typeLine(page, "ls");
  await expect(page.locator(".dx-tline.is-rust")).toHaveText(
    "this shell tells daft stories — try a daft command, or cd",
  );
  await tapChip(page, "start");
  await expect(page.locator(".dx-tline.is-rust")).toHaveCount(0);
});

test("CMP-H3 cd walks the files tree like a real shell", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  // start moved cwd into the new worktree; walk back to main.
  await typeLine(page, "cd ~/web/main");
  expect(await rowVerbs(page)).toEqual(["clone", "start", "cd"]);
  await page.getByRole("tab", { name: "Files" }).click();
  await expect(page.locator(".dx-frow.cwd")).toContainText("main");
  await typeLine(page, "cd ../checkout");
  await expect(page.locator(".dx-frow.cwd")).toContainText("checkout");
  // Relative walks fail like a filesystem, not like a search.
  await typeLine(page, "cd main");
  await expect(page.locator(".dx-tline.is-rust")).toHaveText(
    "cd: no such directory: web/checkout/main",
  );
  // Errors accumulate until a valid line or a rebuild clears them.
  await typeLine(page, "cd ~/nope");
  await expect(page.locator(".dx-tline.is-rust")).toHaveCount(2);
  await expect(page.locator(".dx-tline.is-rust").last()).toHaveText(
    "cd: no such directory: nope",
  );
  expect(await rowVerbs(page)).toEqual(["clone", "start", "cd", "cd"]);
});

test("CMP-H4 bare push validates the cwd worktree itself", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await typeLine(page, "cd ~/web/main");
  await typeLine(page, "daft push");
  await expect(page.locator(".dx-tline.is-rust")).toHaveText(
    "daft push: nothing to push from web/main",
  );
  await typeLine(page, "cd ../checkout");
  await typeLine(page, "daft push");
  const cmds = await cmdLines(page);
  expect(cmds[cmds.length - 1]).toContain("daft push checkout");
  await expectParked(page);
});

test("CMP-H5 typed commands insert at the insertion point", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "list");
  // Park on clone: the next typed line lands right after it.
  await page.locator(".dx-vrow b", { hasText: "clone" }).click();
  await settled(page);
  await typeLine(page, "daft exec -- pnpm test");
  expect(await rowVerbs(page)).toEqual(["clone", "exec", "start", "list"]);
  const cmds = await cmdLines(page);
  expect(cmds[1]).toContain("daft exec --related -- pnpm test");
});

test("CMP-H6 edit in place: update, replace, refuse, escape, jump away", async ({
  page,
}) => {
  await openComposer(page);
  // Three steps, parked at the end: the transcript is a projection at the
  // playhead, so a mid-story line is only clickable from later in time.
  await tapChip(page, "clone");
  await tapChip(page, "start");
  await tapChip(page, "list");
  // First click on a non-parked command jumps and selects…
  await cmdLine(page, "daft start").click();
  await settled(page);
  await expect(page.locator(".dx-vrow").nth(1)).toHaveClass(/\bsel\b/);
  // …and the second click swaps the line for a prefilled input.
  await cmdLine(page, "daft start").click();
  const editBox = page.getByRole("textbox", { name: "Edit this command" });
  await expect(editBox).toHaveValue("daft start checkout");
  // Same verb: args update, the projection rewrites.
  await editBox.fill("daft start perf/cache");
  await editBox.press("Enter");
  await settled(page);
  expect((await cmdLines(page))[1]).toContain("daft start perf/cache");
  // A different available verb replaces the op, with a notice.
  await cmdLine(page, "daft start").click();
  await page
    .getByRole("textbox", { name: "Edit this command" })
    .fill("daft update");
  await page.getByRole("textbox", { name: "Edit this command" }).press("Enter");
  await settled(page);
  await expect(page.locator(".dx-notice")).toHaveText("start became update");
  expect(await rowVerbs(page)).toEqual(["clone", "update", "list"]);
  // An invalid line is refused ephemerally; the editor stays open.
  await cmdLine(page, "daft update").click();
  const editAgain = page.getByRole("textbox", { name: "Edit this command" });
  await editAgain.fill("daft frobnicate");
  await editAgain.press("Enter");
  await expect(page.locator(".dx-tline.is-rust")).toHaveText(
    "unknown daft command: frobnicate",
  );
  await expect(editAgain).toBeVisible();
  expect(await rowVerbs(page)).toEqual(["clone", "update", "list"]);
  // Escape cancels; clicking another command closes and jumps.
  await editAgain.press("Escape");
  await expect(editAgain).toHaveCount(0);
  await cmdLine(page, "daft update").click();
  await expect(
    page.getByRole("textbox", { name: "Edit this command" }),
  ).toBeVisible();
  await cmdLine(page, "daft clone").click();
  await settled(page);
  await expect(
    page.getByRole("textbox", { name: "Edit this command" }),
  ).toHaveCount(0);
  await expect(page.locator(".dx-vrow").nth(0)).toHaveClass(/\bsel\b/);
});

test("CMP-H7 the eye hides a step from exports, dimmed here", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  const clone = cmdLine(page, "daft clone");
  await clone.hover();
  await clone.locator(".dx-eye").click();
  await settled(page);
  await expect(
    page.locator('.dx-tline.hiddenop:has-text("daft clone")'),
  ).toHaveCount(1);
  await expect(
    page.locator('.dx-tline.hiddenop:has-text("daft start")'),
  ).toHaveCount(0);
  // The editor keeps the line (dimmed); the eye now offers to show it.
  await clone.hover();
  await expect(clone.locator(".dx-eye")).toHaveAttribute(
    "aria-label",
    "Show in exports",
  );
  await clone.locator(".dx-eye").click();
  await settled(page);
  await expect(page.locator(".dx-tline.hiddenop")).toHaveCount(0);
});

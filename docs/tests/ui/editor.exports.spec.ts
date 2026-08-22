import { readFileSync } from "node:fs";
import { expect, type Page, test } from "@playwright/test";
import { DUMBSHOW_FS, GRAPH_FS, openComposer, tapChip } from "./helpers";

/** CMP-K — exports: menu gating, PNG, GIF, webm, script and embed. */

async function openExportMenu(page: Page): Promise<void> {
  await page.locator(".dx-chrome-btn.primary", { hasText: "Export" }).click();
  await expect(page.locator(".dx-export-pop")).toBeVisible();
}

/** Does this browser support the webm recorder? (Same detection the app
 * uses — headless shells often lack MediaRecorder.) */
async function webmSupported(page: Page): Promise<boolean> {
  return page.evaluate(async (dumbshowFs) => {
    const mod = (await import(dumbshowFs)) as {
      webmMimeType(): string | null;
    };
    return mod.webmMimeType() !== null;
  }, DUMBSHOW_FS);
}

async function exportDownload(
  page: Page,
  entry: string,
): Promise<{ name: string; bytes: Buffer }> {
  await openExportMenu(page);
  const downloadPromise = page.waitForEvent("download", { timeout: 120_000 });
  await page.locator(".dx-export-pop button", { hasText: entry }).click();
  const download = await downloadPromise;
  return {
    name: download.suggestedFilename(),
    bytes: readFileSync((await download.path()) as string),
  };
}

test("CMP-K1 the menu is gated by content and grows with support", async ({
  page,
}) => {
  await openComposer(page);
  const exportBtn = page.locator(".dx-chrome-btn.primary", {
    hasText: "Export",
  });
  await expect(exportBtn).toBeDisabled();
  await tapChip(page, "clone");
  await expect(exportBtn).toBeEnabled();
  await openExportMenu(page);
  const labels = await page.locator(".dx-export-pop button").allTextContents();
  const expected = ["PNG still", "GIF"];
  if (await webmSupported(page)) expected.push("Video · webm");
  expected.push("Compiled script", "Docs embed");
  expect(labels.map((l) => l.split(/(?=[a-z] [A-Z])/)[0])).toHaveLength(
    expected.length,
  );
  for (const [i, label] of expected.entries())
    expect(labels[i]).toContain(label);
});

test("CMP-K2 PNG is 2x the stage frame and render-deterministic", async ({
  page,
}) => {
  await openComposer(page);
  await tapChip(page, "clone");
  const first = await exportDownload(page, "PNG still");
  expect(first.name).toBe("untitled-scenario.png");
  // PNG magic + IHDR dimensions at exactly 2x the canvas CSS size.
  expect(first.bytes.subarray(0, 8)).toEqual(
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  );
  const size = await page.locator(".dx-canvas-wrap canvas").evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { w: Math.round(r.width), h: Math.round(r.height) };
  });
  expect(first.bytes.readUInt32BE(16)).toBe(size.w * 2);
  expect(first.bytes.readUInt32BE(20)).toBe(size.h * 2);
  // A second render of the same clock is byte-identical.
  const second = await exportDownload(page, "PNG still");
  expect(second.bytes.equals(first.bytes)).toBe(true);
});

test("CMP-K3 GIF carries the magic, the frame size, and the cap", async ({
  page,
}) => {
  test.slow();
  await openComposer(page);
  await tapChip(page, "clone");
  const gif = await exportDownload(page, "GIF");
  expect(gif.name).toBe("untitled-scenario.gif");
  expect(gif.bytes.subarray(0, 6).toString("ascii")).toBe("GIF89a");
  const size = await page.locator(".dx-canvas-wrap canvas").evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { w: Math.round(r.width), h: Math.round(r.height) };
  });
  const scale = Math.min(1, 900 / Math.max(size.w, size.h));
  expect(gif.bytes.readUInt16LE(6)).toBe(Math.round(size.w * scale));
  expect(gif.bytes.readUInt16LE(8)).toBe(Math.round(size.h * scale));
  await expect(page.locator(".dx-notice")).toHaveText("GIF exported");
  // The long-edge cap, exercised directly: a 2000px request caps at 900.
  const capped = await page.evaluate(
    async ({ graphFs, dumbshowFs }) => {
      const mod = (await import(dumbshowFs)) as {
        renderGifBlob(
          scene: unknown,
          compiled: unknown,
          opts: { width: number; height: number },
        ): Promise<Blob>;
      };
      const pack = (await import(`${graphFs}/pack.ts`)) as {
        DAFT_PACK: { scene: unknown };
      };
      const w = window as unknown as { __daftPlayer: { compiled: unknown } };
      const blob = await mod.renderGifBlob(
        pack.DAFT_PACK.scene,
        w.__daftPlayer.compiled,
        {
          width: 2000,
          height: 800,
        },
      );
      const bytes = new Uint8Array(await blob.arrayBuffer());
      return { w: bytes[6] + bytes[7] * 256, h: bytes[8] + bytes[9] * 256 };
    },
    { graphFs: GRAPH_FS, dumbshowFs: DUMBSHOW_FS },
  );
  expect(capped.w).toBe(900);
  expect(capped.h).toBe(360);
});

test("CMP-K4 webm records in real time where supported", async ({ page }) => {
  test.slow();
  await openComposer(page);
  await tapChip(page, "clone");
  test.skip(
    !(await webmSupported(page)),
    "MediaRecorder unavailable in this browser build",
  );
  // One op only: the recording runs as long as the document does.
  const webm = await exportDownload(page, "Video · webm");
  expect(webm.name).toBe("untitled-scenario.webm");
  expect(webm.bytes.subarray(0, 4)).toEqual(
    Buffer.from([0x1a, 0x45, 0xdf, 0xa3]),
  );
  await expect(page.locator(".dx-notice")).toHaveText("webm exported");
});

test("CMP-K5 script and embed leave as engine language", async ({ page }) => {
  await openComposer(page);
  await tapChip(page, "clone");
  await tapChip(page, "start");
  // Headless clipboard is blocked: the entry falls back to a download and
  // says so either way — assert the notice, take the file.
  const script = await exportDownload(page, "Compiled script");
  await expect(page.locator(".dx-notice")).toHaveText(
    /(Script copied|Clipboard unavailable — downloaded instead)/,
  );
  const steps = JSON.parse(script.bytes.toString("utf8"));
  expect(Array.isArray(steps)).toBe(true);
  expect(steps).toHaveLength(2);
  // The exported script recompiles to the live document's exact clock.
  const durations = await page.evaluate(
    async ({ dumbshowFs, script: stepDefs }) => {
      const engine = (await import(dumbshowFs)) as {
        compile(steps: unknown[]): { duration: number };
      };
      const w = window as unknown as {
        __daftPlayer: { compiled: { duration: number } };
      };
      return {
        exported: engine.compile(stepDefs as unknown[]).duration,
        live: w.__daftPlayer.compiled.duration,
      };
    },
    { dumbshowFs: DUMBSHOW_FS, script: steps },
  );
  expect(durations.exported).toBe(durations.live);
  const embed = await exportDownload(page, "Docs embed");
  const snippet = embed.bytes.toString("utf8");
  expect(snippet).toContain('<RepoDiagram :script="SCRIPT" />');
  expect(snippet).toContain('<RepoTerminal :script="SCRIPT" />');
  expect(snippet).toContain("Static frame instead");
});

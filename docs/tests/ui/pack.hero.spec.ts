import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import { DUMBSHOW_FS, GRAPH_FS } from "./helpers";

/**
 * CMP-L5/L6 — the hero as regression anchor.
 *
 * L5 compares six-point canvas fingerprints against a LOCAL, gitignored
 * baseline: pixel output differs across machines and browser builds, so
 * absolute values are never committed. The first run (or a browser-version
 * change, or DAFT_PIXEL_BASELINE=capture) writes the baseline; later runs
 * on the same machine must match it exactly — the pre/post-extraction
 * tripwire. L6 needs no baseline at all: offline replay must agree with
 * itself, fresh vs incremental vs rewound.
 */

const BASELINE_PATH = "tests/ui/.pixel-baseline.json";
const SEEK_TS = [1.027, 7.702, 15.404, 25.674, 35.944, 46.213];

test("CMP-L5 hero six-point fingerprint matches the local baseline", async ({
  page,
  browser,
}) => {
  await page.goto("/");
  await page.waitForSelector("canvas");
  await page.waitForFunction(
    () => (window as unknown as { __daftPlayer?: unknown }).__daftPlayer,
  );
  const hashes: number[] = [];
  for (const t of SEEK_TS) {
    await page.evaluate((target) => {
      const p = (
        window as unknown as {
          __daftPlayer: { pause(): void; seek(t: number): void };
        }
      ).__daftPlayer;
      p.pause();
      p.seek(target);
    }, t);
    await page.waitForTimeout(150);
    hashes.push(
      await page
        .locator("canvas")
        .first()
        .evaluate((el) => {
          const s = (el as HTMLCanvasElement).toDataURL();
          let h = 5381;
          for (let i = 0; i < s.length; i++)
            h = ((h * 33) ^ s.charCodeAt(i)) | 0;
          return h;
        }),
    );
  }
  const stamp = { browser: browser.version(), ts: SEEK_TS, hashes };
  const capture =
    process.env.DAFT_PIXEL_BASELINE === "capture" || !existsSync(BASELINE_PATH);
  if (!capture) {
    const baseline = JSON.parse(readFileSync(BASELINE_PATH, "utf8")) as {
      browser: string;
      hashes: number[];
    };
    if (baseline.browser === stamp.browser) {
      expect(
        hashes,
        "hero pixels drifted — if intentional, rerun with DAFT_PIXEL_BASELINE=capture",
      ).toEqual(baseline.hashes);
      return;
    }
  }
  writeFileSync(BASELINE_PATH, `${JSON.stringify(stamp, null, 2)}\n`);
});

test("CMP-L6 offline replay is deterministic in every direction", async ({
  page,
}) => {
  await page.goto("/");
  const verdict = await page.evaluate(
    async ({ graphFs, dumbshowFs, samples }) => {
      const engine = (await import(dumbshowFs)) as {
        compile(steps: unknown[]): { duration: number };
      };
      const hero = (await import(`${graphFs}/hero-script.ts`)) as Record<
        string,
        unknown
      >;
      const offline = (await import(dumbshowFs)) as {
        createOfflineRenderer(
          scene: unknown,
          compiled: unknown,
          opts: { width: number; height: number; palette?: unknown },
        ): { canvas: HTMLCanvasElement; renderAt(t: number): void };
      };
      const pack = (await import(`${graphFs}/pack.ts`)) as {
        DAFT_PACK: { scene: unknown };
      };
      const render = (await import(`${graphFs}/render.ts`)) as {
        readPalette(): unknown;
      };
      const script = (hero.HERO_SCRIPT ?? hero.default) as unknown[];
      const compiled = engine.compile(script);
      const palette = render.readPalette();
      const opts = { width: 800, height: 500, palette };
      const shot = (r: { canvas: HTMLCanvasElement }): string =>
        r.canvas.toDataURL();
      // Incremental: one renderer walks the samples in order.
      const inc = offline.createOfflineRenderer(
        pack.DAFT_PACK.scene,
        compiled,
        opts,
      );
      const incremental = samples.map((t) => {
        inc.renderAt(t);
        return shot(inc);
      });
      // Fresh: a new renderer per sample.
      const fresh = samples.map((t) => {
        const r = offline.createOfflineRenderer(
          pack.DAFT_PACK.scene,
          compiled,
          opts,
        );
        r.renderAt(t);
        return shot(r);
      });
      // Rewound: render the end first, then rewind to each sample.
      const rew = offline.createOfflineRenderer(
        pack.DAFT_PACK.scene,
        compiled,
        opts,
      );
      rew.renderAt(samples[samples.length - 1]);
      const rewound = samples.map((t) => {
        rew.renderAt(t);
        return shot(rew);
      });
      return samples.map((t, i) => ({
        t,
        freshMatches: incremental[i] === fresh[i],
        rewoundMatches: incremental[i] === rewound[i],
      }));
    },
    {
      graphFs: GRAPH_FS,
      dumbshowFs: DUMBSHOW_FS,
      samples: [1.027, 15.404, 35.944, 46.213],
    },
  );
  for (const point of verdict) {
    expect(point.freshMatches, `fresh render diverged at t=${point.t}`).toBe(
      true,
    );
    expect(
      point.rewoundMatches,
      `post-rewind render diverged at t=${point.t}`,
    ).toBe(true);
  }
});

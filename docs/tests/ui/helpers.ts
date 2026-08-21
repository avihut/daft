import path from "node:path";
import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Shared harness for the composer UI specs — the executable form of the
 * "Harness notes" in .vitepress/theme/graph/composer/test-scenarios.md.
 * Every trap documented there has its countermeasure here: the draft
 * debounce reset protocol, synthetic pointers dispatched on the pressed
 * element, and the settle-after-edit timing.
 */

/** Absolute /@fs URL of the graph layer, for in-page module imports. */
export const GRAPH_FS = `/@fs${path.resolve(process.cwd(), ".vitepress/theme/graph")}`;

/** Absolute /@fs URL of the dumbshow machinery module (engine, offline
 * renderer, media exports) for in-page imports — bare specifiers don't
 * resolve in page.evaluate. Default: the installed package's built dist.
 * Under DUMBSHOW_SRC (the source link, see .vitepress/config.ts) it is the
 * linked checkout's source entry, so specs import the same module the page
 * runs. */
export const DUMBSHOW_FS = `/@fs${
  process.env.DUMBSHOW_SRC
    ? path.resolve(process.env.DUMBSHOW_SRC, "src/index.ts")
    : path.resolve(
        process.cwd(),
        "node_modules/@avihut/dumbshow/dist/dumbshow.js",
      )
}`;

export async function openComposer(page: Page): Promise<void> {
  await page.goto("/composer");
  await page.waitForSelector(".dx-app", { timeout: 15_000 });
}

/**
 * The catalog reset protocol: the draft autosaves on an 800ms debounce, so
 * a single clear races a pending write. Clear, outlast the debounce, clear
 * again, reload. Only needed for within-test reloads — each test gets a
 * fresh browser context, so the first load is already clean.
 */
export async function resetDraft(page: Page): Promise<void> {
  await page.evaluate(() => localStorage.clear());
  await page.waitForTimeout(950);
  await page.evaluate(() => localStorage.clear());
  await page.reload();
  await page.waitForSelector(".dx-app", { timeout: 15_000 });
}

/** Let Vue flush, the player rebuild, and the landing settle after an edit. */
export async function settled(page: Page): Promise<void> {
  await page.waitForTimeout(350);
}

export interface Pt {
  x: number;
  y: number;
}

export async function center(target: Locator): Promise<Pt> {
  const box = await target.boundingBox();
  if (!box) throw new Error("no bounding box for locator");
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

/**
 * Synthetic tap: pointerdown+up on the element with travel under the 4px
 * threshold. Events are dispatched on the element itself — the dnd
 * controller listens on the pressed element (pointer capture is try/caught
 * for synthetic pointers).
 */
export async function synthTap(target: Locator): Promise<void> {
  await target.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const init: PointerEventInit = {
      bubbles: true,
      cancelable: true,
      button: 0,
      pointerId: 1,
      clientX: r.left + r.width / 2,
      clientY: r.top + r.height / 2,
    };
    el.dispatchEvent(new PointerEvent("pointerdown", init));
    el.dispatchEvent(new PointerEvent("pointerup", init));
  });
}

/**
 * Synthetic drags, in phases so specs can assert mid-flight (the ghost,
 * the insertion line). Every event is dispatched on the pressed element —
 * the dnd controller listens there (pointer capture is try/caught for
 * synthetic pointers) — and drop zones resolve by coordinates, never by
 * event target. `from`/`to` are client coordinates.
 */
type SynthPhase =
  | { phase: "start"; from: Pt }
  | { phase: "move"; to: Pt; steps: number }
  | { phase: "end"; at: Pt }
  | { phase: "cancel" }
  | { phase: "tap"; at: Pt };

async function synthPhase(el: Locator, arg: SynthPhase): Promise<void> {
  await el.evaluate(async (node, a) => {
    const fire = (type: string, x: number, y: number): void => {
      node.dispatchEvent(
        new PointerEvent(type, {
          bubbles: true,
          cancelable: true,
          button: 0,
          pointerId: 1,
          clientX: x,
          clientY: y,
        }),
      );
    };
    const tick = (): Promise<void> =>
      new Promise((resolve) => setTimeout(resolve, 25));
    if (a.phase === "start") {
      fire("pointerdown", a.from.x, a.from.y);
      fire("pointermove", a.from.x + 6, a.from.y + 6);
      await tick();
      return;
    }
    if (a.phase === "move") {
      for (let i = 1; i <= a.steps; i++) {
        fire("pointermove", a.to.x, a.to.y);
        await tick();
      }
      return;
    }
    if (a.phase === "end") {
      fire("pointermove", a.at.x, a.at.y);
      await tick();
      fire("pointerup", a.at.x, a.at.y);
      return;
    }
    if (a.phase === "tap") {
      fire("pointerdown", a.at.x, a.at.y);
      fire("pointerup", a.at.x, a.at.y);
      return;
    }
    fire("pointercancel", 0, 0);
  }, arg);
}

/** Tap (down+up, zero travel) at a specific client point on the element. */
export function synthTapAt(el: Locator, at: Pt): Promise<void> {
  return synthPhase(el, { phase: "tap", at });
}

/** Press at `from` and travel just past the 4px threshold — drag engaged. */
export function synthDragStart(el: Locator, from: Pt): Promise<void> {
  return synthPhase(el, { phase: "start", from });
}

/** Glide the engaged drag to `to` (repeated moves let zones resolve). */
export function synthDragMove(el: Locator, to: Pt, steps = 2): Promise<void> {
  return synthPhase(el, { phase: "move", to, steps });
}

/** Finish the engaged drag: final move + release at `at`. */
export function synthDragEnd(el: Locator, at: Pt): Promise<void> {
  return synthPhase(el, { phase: "end", at });
}

/** Abort the engaged drag without dropping. */
export function synthDragCancel(el: Locator): Promise<void> {
  return synthPhase(el, { phase: "cancel" });
}

/** A whole drag from the element's center to a client point. */
export async function synthDrag(source: Locator, to: Pt): Promise<void> {
  const from = await center(source);
  await synthDragStart(source, from);
  await synthDragMove(source, to);
  await synthDragEnd(source, to);
}

/** A catalog chip by its exact label (icons are aria-hidden). */
export function chip(page: Page, label: string): Locator {
  return page
    .locator(".dx-cat")
    .getByRole("button", { name: label, exact: true });
}

/** Tap a chip and wait out the rebuild — the standard way specs build docs. */
export async function tapChip(page: Page, label: string): Promise<void> {
  await synthTap(chip(page, label));
  await settled(page);
}

/** The timeline's op/beat rows' verbs, in order. */
export function rowVerbs(page: Page): Promise<string[]> {
  return page.locator(".dx-vrow b").allTextContents();
}

/**
 * Click a row's hover-revealed action button ("Move up", "Move down",
 * "Remove") — the controls are hidden until the row is hovered, so hover
 * first like a real user.
 */
export async function rowAction(
  page: Page,
  rowIndex: number,
  label: string,
): Promise<void> {
  const row = page.locator(".dx-vrow").nth(rowIndex);
  await row.hover();
  await row.locator(`button[aria-label="${label}"]`).click();
  await settled(page);
}

/** The stage canvas element. */
export function stageCanvas(page: Page): Locator {
  return page.locator(".dx-canvas-wrap canvas");
}

/** The stage wrap — carries the pointer-state attributes
 * (`data-hover`, `data-dragging`) the cursor rules key off. */
export function canvasWrap(page: Page): Locator {
  return page.locator(".dx-canvas-wrap");
}

/** The computed CSS cursor of an element — what the pointer shows over it. */
export function cursorOf(target: Locator): Promise<string> {
  return target.evaluate((el) => getComputedStyle(el).cursor);
}

/** Park the dev player settled on a step — a module-level seek, so specs
 * can put the playhead somewhere other than where the last edit landed. */
export async function settleStep(page: Page, step: number): Promise<void> {
  await page.evaluate((i) => {
    const w = window as unknown as {
      __daftPlayer?: { settle(i: number): void };
    };
    w.__daftPlayer?.settle(i);
  }, step);
  await settled(page);
}

/**
 * World → client coordinates through the live camera — positions are
 * computed, never scanned (repo discs slip through probe grids). Uses the
 * same camRectAt/makeView the renderer uses, via /@fs dev-server imports;
 * with no player yet, the stage's documented default camera applies.
 */
export async function canvasPoint(
  page: Page,
  wx: number,
  wy: number,
): Promise<Pt> {
  return page.evaluate(
    async (arg) => {
      const canvas = document.querySelector(
        ".dx-canvas-wrap canvas",
      ) as HTMLCanvasElement;
      const r = canvas.getBoundingClientRect();
      const render = (await import(`${arg.graphFs}/render.ts`)) as {
        camRectAt(
          keys: unknown[],
          t: number,
          reduced: boolean,
        ): { x: number; y: number; w: number; h: number };
        makeView(
          rect: { x: number; y: number; w: number; h: number },
          width: number,
          height: number,
        ): { sx(wx: number): number; sy(wy: number): number };
      };
      const w = window as unknown as {
        __daftPlayer?: {
          clock(): number;
          reducedMotion: boolean;
          compiled: { cams: unknown[]; steps: unknown[] };
        };
      };
      const p = w.__daftPlayer;
      const rect = p?.compiled.steps.length
        ? render.camRectAt(p.compiled.cams, p.clock(), p.reducedMotion)
        : { x: 0, y: 0, w: 460, h: 400 };
      const view = render.makeView(rect, r.width, r.height);
      return { x: r.left + view.sx(arg.wx), y: r.top + view.sy(arg.wy) };
    },
    { graphFs: GRAPH_FS, wx, wy },
  );
}

/** A stable hash of the stage canvas pixels — compare within one run
 * only (rendering differs across machines and browser builds). */
export async function canvasHash(page: Page): Promise<number> {
  return page.evaluate(() => {
    const c = document.querySelector(
      ".dx-canvas-wrap canvas",
    ) as HTMLCanvasElement;
    const s = c.toDataURL();
    let h = 5381;
    for (let i = 0; i < s.length; i++) h = ((h * 33) ^ s.charCodeAt(i)) | 0;
    return h;
  });
}

/** The transcript's command lines (excluding the typing ghost), in order. */
export function cmdLines(page: Page): Promise<string[]> {
  return page
    .locator(".dx-tline.is-cmd:not(:has(.dx-caret))")
    .allTextContents();
}

/** The last compiled repo act's world position for `name` — where the
 * scene actually put the disc. */
export async function lastRepoActPos(
  page: Page,
  name: string,
): Promise<Pt | null> {
  return page.evaluate((repoName) => {
    const w = window as unknown as {
      __daftPlayer?: {
        compiled: {
          events: {
            act: { kind: string; repo?: string; x?: number; y?: number };
          }[];
        };
      };
    };
    const events = w.__daftPlayer?.compiled.events ?? [];
    let found: { x: number; y: number } | null = null;
    for (const e of events) {
      if (e.act.kind === "repo" && e.act.repo === repoName)
        found = { x: e.act.x as number, y: e.act.y as number };
    }
    return found;
  }, name);
}

export interface PlayerSnap {
  playing: boolean;
  t: number;
  duration: number;
  step: number;
}

/** Read the dev player handle — re-grab after every edit (last-writer-wins). */
export async function playerState(page: Page): Promise<PlayerSnap | null> {
  return page.evaluate(() => {
    const w = window as unknown as {
      __daftPlayer?: {
        playing(): boolean;
        clock(): number;
        current(): number;
        compiled: { duration: number };
      };
    };
    const p = w.__daftPlayer;
    if (!p) return null;
    return {
      playing: p.playing(),
      t: p.clock(),
      duration: p.compiled.duration,
      step: p.current(),
    };
  });
}

/** The no-autoplay invariant: after any mutation the player sits paused. */
export async function expectParked(page: Page): Promise<PlayerSnap> {
  const snap = await playerState(page);
  expect(snap, "expected a live player handle").not.toBeNull();
  expect(snap?.playing).toBe(false);
  return snap as PlayerSnap;
}

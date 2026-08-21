import { defineConfig, devices } from "@playwright/test";

/**
 * Composer UI specs (tests/ui) against the VitePress dev server. Dev mode
 * is load-bearing: specs import graph modules in-page through /@fs URLs,
 * which only the dev server serves — never point this at a built site.
 * Run via `mise run docs:test:ui` (cwd must be docs/, the task ensures it).
 */
/** The dev-server port. DOCS_PORT runs the suite beside another docs server
 * (a sibling worktree's) instead of silently reusing it — the config reuses
 * whatever already answers at the URL. */
const port = Number(process.env.DOCS_PORT ?? 5173);

export default defineConfig({
  testDir: "./tests/ui",
  fullyParallel: true,
  retries: 0,
  reporter: [["list"]],
  timeout: 45_000,
  expect: { timeout: 7_000 },
  use: {
    baseURL: `http://localhost:${port}`,
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1440, height: 900 },
      },
    },
  ],
  webServer: {
    command: `bunx vitepress dev --port ${port} --strictPort`,
    url: `http://localhost:${port}/`,
    reuseExistingServer: true,
    timeout: 90_000,
  },
});

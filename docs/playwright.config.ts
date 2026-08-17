import { defineConfig, devices } from "@playwright/test";

/**
 * Composer UI specs (tests/ui) against the VitePress dev server. Dev mode
 * is load-bearing: specs import graph modules in-page through /@fs URLs,
 * which only the dev server serves — never point this at a built site.
 * Run via `mise run docs:test:ui` (cwd must be docs/, the task ensures it).
 */
export default defineConfig({
  testDir: "./tests/ui",
  fullyParallel: true,
  retries: 0,
  reporter: [["list"]],
  timeout: 45_000,
  expect: { timeout: 7_000 },
  use: {
    baseURL: "http://localhost:5173",
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
    command: "bunx vitepress dev",
    url: "http://localhost:5173/",
    reuseExistingServer: true,
    timeout: 90_000,
  },
});

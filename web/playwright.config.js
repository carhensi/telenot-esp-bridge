import { defineConfig } from "@playwright/test";

// Harness against a running `telenot-sim serve`. Three projects:
//   --project=e2e        → functional security suite (*.e2e.spec.js, headless, video off, fast)
//   --project=landscape  → demo recording 16:10 desktop (speedrun only, video on)
//   --project=portrait   → demo recording 9:16 mobile UI (speedrun only, video on)
// Demo projects must be run INDIVIDUALLY (fresh sim per run). The e2e suite isolates itself
// via POST /sim/reset in beforeEach, so it runs end-to-end in one go:
//   npm run e2e:sec                     # security suite
//   npx playwright test --project=landscape   # demo
export default defineConfig({
  testDir: "./tests/e2e",
  outputDir: "./test-results",
  timeout: 180_000,
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: process.env.SIM_URL || "http://127.0.0.1:8443",
    actionTimeout: 20_000,
    navigationTimeout: 20_000,
  },
  projects: [
    {
      name: "e2e",
      testMatch: /\.e2e\.spec\.js$/,
      timeout: 30_000,
      use: {
        // Dedicated sim on 8444, pre-loaded with fixture config (configured=true) →
        // dashboard + DiagModal (backup/update panels) reachable without the full wizard.
        baseURL: "http://127.0.0.1:8444",
        browserName: "chromium",
        viewport: { width: 1280, height: 900 },
        video: { mode: "off" },
      },
    },
    {
      name: "landscape",
      testMatch: /speedrun\.spec\.js$/,
      use: {
        browserName: "chromium",
        viewport: { width: 1280, height: 900 },
        video: { mode: "on", size: { width: 1280, height: 900 } },
      },
    },
    {
      name: "portrait",
      testMatch: /speedrun\.spec\.js$/,
      use: {
        browserName: "chromium",
        viewport: { width: 402, height: 874 },
        deviceScaleFactor: 2,
        isMobile: true,
        hasTouch: true,
        video: { mode: "on", size: { width: 402, height: 874 } },
      },
    },
  ],
  // Two sims: 8443 fresh for demo recordings, 8444 with fixture config (configured)
  // for the functional e2e suite. Both always start — negligible overhead.
  webServer: [
    {
      command: "../target/release/telenot-sim serve --ema mock --web dist/index.html --bind 127.0.0.1:8443",
      url: "http://127.0.0.1:8443",
      reuseExistingServer: false,
      timeout: 30_000,
      stdout: "pipe",
      stderr: "pipe",
    },
    {
      command: "../target/release/telenot-sim serve --ema mock --config tests/e2e/fixture-config.json --web dist/index.html --bind 127.0.0.1:8444",
      url: "http://127.0.0.1:8444",
      reuseExistingServer: false,
      timeout: 30_000,
      stdout: "pipe",
      stderr: "pipe",
    },
  ],
});

import { defineConfig } from "@playwright/test";

// UI E2E with the in-browser mock backend (Phase 5, L6).
// `vite build --mode test` turns on the PREP-06 `__DUALPAL_TEST__` ready signal.
export default defineConfig({
  testDir: "ui/tests/e2e",
  testMatch: "**/*.spec.ts",
  fullyParallel: true,
  retries: 0,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:4173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "webkit", use: { browserName: "webkit" } },
  ],
  webServer: {
    command: "npm run build -- --mode test && npx vite preview --port 4173 --strictPort",
    url: "http://localhost:4173",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});

import type { Options } from "@wdio/types";

// Full-stack desktop E2E (Phase 6, L7): WebdriverIO drives the real app binary
// through tauri-driver. See docs/testing/AUTOMATION_TEST_PLAN.md §8.15 and §9.2.

const binary = process.env.DUALPAL_E2E_BINARY || "target/debug/dualpal";

export const config: Options.Testrunner = {
  runner: "local",
  specs: ["./specs/**/*.ts"],
  maxInstances: 1,
  capabilities: [
    {
      // @ts-expect-error tauri-driver capability
      "tauri:options": { application: binary },
      "webdriver:enyo:options": {},
    } as unknown as Options.WebdriverIO,
  ],
  logLevel: "warn",
  bail: 0,
  waitforTimeout: 10_000,
  connectionRetryTimeout: 60_000,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 60_000,
  },
};

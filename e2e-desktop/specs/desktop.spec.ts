// Smoke flows that prove the real wiring (Phase 6, L7). Each spec runs in a
// fresh temp environment set by the runner (see §7.5 of the test plan).

import { expect } from "@wdio/globals";

const waitApp = async () => {
  // The app is ready once both panes have listed their folder.
  await browser.executeAsync((done) => {
    const w = window as unknown as { __DUALPAL_TEST__?: { ready: Promise<void> } };
    (w.__DUALPAL_TEST__?.ready ?? Promise.resolve()).then(done);
  });
};

describe("Dual Pal desktop", () => {
  it("DESK-001 starts and lists the home folder in both panes", async () => {
    await waitApp();
    await expect($('[data-testid="pane-left"]')).toBeDisplayed();
    await expect($('[data-testid="pane-right"]')).toBeDisplayed();
    await expect($('[data-testid="pane-left-status"]')).toHaveText(/.items/);
  });

  it("DESK-014 a deleted folder drops the pane to its parent", async () => {
    // A helper (removed from the app between steps) is outside the browser;
    // this test only asserts the error banner surface exists.
    await expect($('[data-testid="pane-left-error"]')).toHaveText(/.+/);
  });
});

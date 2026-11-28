import { expect, test } from "@playwright/test";
import { calls, emit, openApp, ops, point, row } from "./pom/app";

// Transfer queue controls in the Operations panel (§8.14 UI-E2E).

const restored = {
  id: "op-restored",
  kind: "copy",
  summary: "Copy 2 items to /home/user/docs",
  status: "paused",
  progress: { filesTotal: 2, filesDone: 1, bytesTotal: 100, bytesDone: 40, skipped: 0, current: null },
  error: null,
  queuedMs: 1,
  startedMs: 1,
  finishedMs: null,
  places: ["local"],
  options: { keepGoing: false, verify: false, limit: null },
  failed: 0,
  waitingFor: null,
  restored: true,
};

test.describe("UI-E2E transfer queue", () => {
  test("UI-E2E-080 new copies use the panel's Verify and Don't stop on errors choices", async ({ page }) => {
    await openApp(page);
    await ops(page).getByRole("checkbox", { name: "Verify copies" }).check();
    await ops(page).getByRole("checkbox", { name: "Don't stop on errors" }).check();
    await expect
      .poll(async () => (await calls(page, "save_settings")).at(-1)?.settings)
      .toMatchObject({ verifyCopies: true, keepGoing: true });
    await row(page, "right", "docs").dblclick();
    await point(page, "left", "a.txt");
    await page.keyboard.press("F5");
    await expect
      .poll(async () => (await calls(page, "enqueue_operation")).at(-1)?.options)
      .toEqual({ verify: true, keepGoing: true });
  });

  test("UI-E2E-081 bandwidth limit and Pause all reach the backend", async ({ page }) => {
    await openApp(page);
    await ops(page).getByLabel("Limit unit").selectOption("MB/s");
    await ops(page).getByLabel("Bandwidth limit").fill("2");
    await ops(page).getByLabel("Bandwidth limit").press("Enter");
    await expect
      .poll(async () => (await calls(page, "set_bandwidth_limit")).at(-1))
      .toEqual({ bytesPerSec: 2 * 1024 * 1024 });
    await ops(page).getByRole("button", { name: "Pause all" }).click();
    await expect.poll(async () => (await calls(page, "set_queue_paused")).at(-1)).toEqual({ paused: true });
    await expect(ops(page).getByRole("button", { name: "Resume all" })).toBeVisible();
  });

  test("UI-E2E-082 a copy kept from the last run offers Resume and Discard", async ({ page }) => {
    await openApp(page);
    await emit(page, "operation-updated", restored);
    await expect(ops(page).getByText("Not finished when Dual Pal closed")).toBeVisible();
    await ops(page).getByRole("button", { name: "Resume" }).click();
    await expect.poll(async () => (await calls(page, "resume_operation")).at(-1)).toEqual({ id: "op-restored" });
    await ops(page).getByRole("button", { name: "Discard" }).click();
    await expect.poll(async () => (await calls(page, "cancel_operation")).at(-1)).toEqual({ id: "op-restored" });
  });
});

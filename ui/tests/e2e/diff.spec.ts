import { expect, test } from "@playwright/test";
import { calls, clearCalls, defaultScenario, openApp, point, row } from "./pom/app";

// The built-in file compare view, opened from the keyboard, the command
// palette and Compare & Sync, with the mock backend.

const MOD = "ControlOrMeta";

const scenario = {
  ...defaultScenario,
  files: {
    ...defaultScenario.files,
    "/home/user/old.txt": "one\ntwo\nthree\nfour\n",
    "/home/user/new.txt": "one\nTWO\nthree\nfour\nfive\n",
  },
};

/** Mark two files in the left pane. */
async function markPair(page: import("@playwright/test").Page) {
  // Insert marks and moves down; old.txt follows new.txt.
  await point(page, "left", "new.txt");
  await page.keyboard.press("Insert");
  await page.keyboard.press("Insert");
}

test.describe("file compare view", () => {
  test("UI-KEY-045 Alt ↓ / Alt ↑, F7 / Shift F7 and Alt → / Alt ← in the compare view", async ({ page }) => {
    await openApp(page, scenario);
    await markPair(page);
    await clearCalls(page);
    await page.keyboard.press(`${MOD}+Shift+d`);
    const status = page.getByTestId("diff-status");
    await expect(status).toHaveText("2 changes");
    await page.keyboard.press("Alt+ArrowDown");
    await expect(status).toHaveText("Change 1 of 2");
    await page.keyboard.press("F7");
    await expect(status).toHaveText("Change 2 of 2");
    await page.keyboard.press("Shift+F7");
    await expect(status).toHaveText("Change 1 of 2");
    await page.keyboard.press("Alt+ArrowDown");
    await page.keyboard.press("Alt+ArrowUp");
    await expect(status).toHaveText("Change 1 of 2");
    // Alt → copies the current change to the right side.
    await page.keyboard.press("Alt+ArrowRight");
    await expect(status).toHaveText("Change 1 of 1");
    expect((await calls(page, "diff_apply")).at(-1)).toMatchObject({ hunk: 0, to: "right" });
    await page.keyboard.press("Alt+ArrowLeft");
    await expect(status).toHaveText("The files are identical");
    expect((await calls(page, "diff_apply")).at(-1)).toMatchObject({ hunk: 0, to: "left" });
    // Unsaved changes: closing asks first.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("diff-unsaved")).toBeVisible();
    await page.getByTestId("diff-unsaved").getByRole("button", { name: "Save", exact: true }).click();
    await expect(page.getByTestId("diff-view")).toBeHidden();
    expect((await calls(page, "diff_save")).map((c) => c.side).sort()).toEqual(["left", "right"]);
  });

  test("UI-E2E-050 double-clicking a changed file in Compare & Sync opens the compare view", async ({ page }) => {
    const side = { dir: false, size: 3, modifiedMs: 1_700_000_000_000 };
    await openApp(page, {
      ...scenario,
      results: {
        sync_plan: {
          id: "plan-1",
          mode: "update",
          comparison: {
            left: { fs: "local", path: "/home/user" },
            right: { fs: "local", path: "/home/user/docs" },
            options: {},
            items: [{ rel: "notes.txt", name: "notes.txt", depth: 0, dir: false, left: side, right: { ...side, size: 4 }, state: "different" }],
            summary: { same: 0, leftOnly: 0, rightOnly: 0, leftNewer: 0, rightNewer: 0, different: 1, kindMismatch: 0 },
          },
          steps: [],
          hasHistory: false,
        },
      },
    });
    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await point(page, "left", "a.txt");
    await page.keyboard.press(`${MOD}+Shift+y`);
    const compare = page.getByTestId("compare-view");
    await expect(compare).toBeVisible();
    await clearCalls(page);
    await compare.getByRole("row").filter({ hasText: "notes.txt" }).dblclick();
    await expect(page.getByTestId("diff-view")).toBeVisible();
    expect(await calls(page, "diff_open")).toEqual([
      expect.objectContaining({ left: { fs: "local", path: "/home/user/notes.txt" }, right: { fs: "local", path: "/home/user/docs/notes.txt" } }),
    ]);
    // Esc closes the compare view only; Compare & Sync stays.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("diff-view")).toBeHidden();
    await expect(compare).toBeVisible();
  });

  test("UI-E2E-051 the palette opens Compare Files and can switch to the external compare app", async ({ page }) => {
    await openApp(page, scenario);
    await markPair(page);
    await page.keyboard.press(`${MOD}+Shift+p`);
    await page.getByTestId("palette").locator("input").fill("Compare Files");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("diff-view")).toBeVisible();
    // new.txt is on the left: its changed and extra lines are tinted, the changed word marked.
    await expect(page.getByTestId("diff-left").locator(".row.removed")).toHaveCount(2);
    await expect(page.getByTestId("diff-left").locator("mark")).toHaveText(["TWO"]);
    await page.keyboard.press("Escape");

    await page.keyboard.press(`${MOD}+Shift+p`);
    await page.getByTestId("palette").locator("input").fill("Use External Compare App");
    await page.keyboard.press("Enter");
    await expect.poll(async () => (await calls(page, "save_settings")).at(-1)?.settings.externalCompare).toBe(true);
    await clearCalls(page);
    await page.keyboard.press(`${MOD}+Shift+d`);
    await expect.poll(() => calls(page, "compare_files")).toEqual([
      { left: { fs: "local", path: "/home/user/new.txt" }, right: { fs: "local", path: "/home/user/old.txt" } },
    ]);
    await expect(page.getByTestId("diff-view")).toBeHidden();
  });
});

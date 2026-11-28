import { expect, test } from "@playwright/test";
import { calls, clearCalls, defaultScenario, openApp, pane, point, row } from "./pom/app";

// The Settings screen (README "Settings"): changes apply at once and key
// bindings replace the defaults.

const MOD = "ControlOrMeta";

test.describe("UI-E2E settings", () => {
  test("UI-E2E-034 a command rebound in Settings runs from its new key, and the palette shows it", async ({ page }) => {
    await openApp(page);
    await point(page, "left", "a.txt");
    // Open Settings from the command palette.
    await page.keyboard.press(`${MOD}+Shift+p`);
    await page.getByTestId("palette").locator("input").fill("Settings");
    await page.keyboard.press("Enter");
    const settings = page.getByTestId("settings-dialog");
    await expect(settings).toBeVisible();
    await settings.getByTestId("settings-tab-keyboard").click();

    // Refresh: Ctrl/⌘ R becomes F9.
    const refresh = settings.locator('[data-testid="key-row"][data-command="refresh"]');
    await refresh.getByRole("button", { name: /^(Ctrl\+R|⌘R)$/ }).click();
    await expect(refresh).toContainText("Press keys…");
    await page.keyboard.press("F9");
    await expect(refresh.getByRole("button", { name: "F9", exact: true })).toBeVisible();
    await expect.poll(async () => (await calls(page, "update_settings")).at(-1)?.patch).toEqual({ keyBindings: { refresh: ["F9"] } });
    await page.keyboard.press("Escape");
    await expect(settings).toBeHidden();

    await point(page, "left", "a.txt");
    await clearCalls(page);
    await page.keyboard.press("F9");
    await expect.poll(async () => (await calls(page, "list_dir")).map((a) => a.location.path)).toContain("/home/user");
    // The old key does nothing now.
    await clearCalls(page);
    await page.keyboard.press(`${MOD}+r`);
    // A key that still works, so the one before has been handled by now.
    await page.keyboard.press(`${MOD}+Shift+t`);
    await expect.poll(() => calls(page, "open_terminal")).toHaveLength(1);
    expect(await calls(page, "list_dir")).toEqual([]);

    // The palette lists the new key.
    await page.keyboard.press(`${MOD}+Shift+p`);
    await page.getByTestId("palette").locator("input").fill("Refresh");
    await expect(page.getByTestId("palette").locator("li").first()).toContainText("F9");
  });

  test("UI-E2E-035 appearance and pane settings apply at once and are read at start", async ({ page }) => {
    await openApp(page, { ...defaultScenario, settings: { theme: "dark", hiddenColumns: ["modified"] } });
    const root = page.locator("html");
    await expect(root).toHaveAttribute("data-theme", "dark");
    await expect(pane(page, "left").locator(".columns")).not.toContainText("Date Modified");

    await page.keyboard.press(`${MOD}+,`);
    const settings = page.getByTestId("settings-dialog");
    await settings.getByLabel("Light").check();
    await expect(root).toHaveAttribute("data-theme", "light");
    await settings.getByLabel("Large").check();
    await expect(root).toHaveAttribute("data-font-size", "large");
    await settings.getByTestId("settings-tab-panes").click();
    await settings.getByLabel("Date Modified").check();
    await settings.getByLabel("Size").uncheck();
    await expect(row(page, "left", ".hidden")).toHaveCount(0);
    await settings.getByTestId("settings-show-hidden").check();
    await settings.getByRole("button", { name: "Close" }).click();
    await expect(row(page, "left", ".hidden")).toBeVisible();
    await expect(row(page, "right", ".hidden")).toBeVisible();
    const columns = pane(page, "left").locator(".columns");
    await expect(columns).toContainText("Date Modified");
    await expect(columns).not.toContainText("Size");
  });

  test("UI-E2E-036 a pane that leaves its last folder and comes back at once saves nothing", async ({ page }) => {
    const last = { mode: "last", folder: null };
    await openApp(page, { ...defaultScenario, settings: { paneStart: [last, last], lastFolders: ["/home/user", "/home/user"] } });
    await expect(row(page, "left", "a.txt")).toBeVisible();
    await clearCalls(page);
    await point(page, "left", "docs");
    await page.keyboard.press("Enter");
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await page.keyboard.press("Backspace");
    await expect(row(page, "left", "a.txt")).toBeVisible();
    // Longer than the wait before a folder is saved.
    await page.waitForTimeout(1500);
    expect((await calls(page, "update_settings")).map((a) => a.patch)).toEqual([]);
  });
});

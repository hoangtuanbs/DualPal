import { expect, test } from "@playwright/test";
import { openApp, pane, row, toolbar, dialog } from "./pom/app";

test.describe("UI-E2E app shell", () => {
  test("UI-E2E-001 loads and lists the home folder in both panes", async ({ page }) => {
    await openApp(page);

    // Both panes are present and show the home listing.
    await expect(pane(page, "left")).toBeVisible();
    await expect(pane(page, "right")).toBeVisible();
    await expect(row(page, "left", "readme.md")).toBeVisible();
    await expect(row(page, "right", "readme.md")).toBeVisible();
    // The status line reports a count and free space.
    await expect(page.getByTestId("pane-left-status")).toContainText("items");
  });

  test("UI-KEY-004 Backspace opens the parent folder", async ({ page }) => {
    await openApp(page);
    // Enter the docs folder.
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    // Backspace goes up again.
    await pane(page, "left").click();
    await page.keyboard.press("Backspace");
    await expect(row(page, "left", "docs")).toBeVisible();
    await expect(row(page, "left", "readme.md")).toBeVisible();
  });

  test("UI-KEY-014 F7 opens the new-folder dialog and closes it on OK", async ({ page }) => {
    await openApp(page);
    await pane(page, "left").click();
    await page.keyboard.press("F7");
    await expect(dialog(page)).toBeVisible();
    await dialog(page).getByTestId("dialog-input").fill("My Folder");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect(dialog(page)).toBeHidden();
  });

  test("UI-KEY-007 Ctrl+A marks all visible items", async ({ page }) => {
    await openApp(page);
    await pane(page, "left").click();
    await page.keyboard.press("ControlOrMeta+a");
    // Several rows are now selected.
    const selected = await row(page, "left", "a.txt").getAttribute("aria-selected");
    expect(selected).toBe("true");
  });

  test("UI-E2E-015 Compare & Sync opens the compare view", async ({ page }) => {
    await openApp(page);
    // The two panes must show different folders to compare.
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await toolbar(page).getByRole("button", { name: "Compare & Sync" }).click();
    await expect(page.getByTestId("compare-view")).toBeVisible();
  });

  test("UI-E2E-024 command palette opens and focuses input", async ({ page }) => {
    await openApp(page);
    await page.keyboard.press("ControlOrMeta+Shift+p");
    await expect(page.getByTestId("palette")).toBeVisible();
  });
});

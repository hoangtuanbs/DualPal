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

test.describe("UI-E2E group by date", () => {
  test("groups rows under date headers that fold away and back", async ({ page }) => {
    await openApp(page);
    const rows = page.getByTestId("pane-left-row");
    const count = await rows.count();
    expect(count).toBeGreaterThan(0);

    await pane(page, "left").click();
    await toolbar(page).getByRole("button", { name: "Group by date" }).click();
    const header = page.getByTestId("pane-left-group");
    // The mock's files all date from 2023.
    await expect(header).toHaveCount(1);
    await expect(header).toContainText("A long time ago");
    await expect(header).toHaveAttribute("aria-expanded", "true");

    await header.click();
    await expect(header).toHaveAttribute("aria-expanded", "false");
    await expect(rows).toHaveCount(0);
    // The status line still counts the folded items.
    await expect(page.getByTestId("pane-left-status")).toContainText(`${count} item`);

    await header.click();
    await expect(rows).toHaveCount(count);

    await toolbar(page).getByRole("button", { name: "Group by date" }).click();
    await expect(header).toHaveCount(0);
  });
});

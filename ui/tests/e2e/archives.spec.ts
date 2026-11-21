import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, cursorIndex, defaultScenario, enqueued, openApp, pane, point, row } from "./pom/app";
import type { BrowserScenario } from "./browser-mock";

// Archives opened as folders (README "Archives").

const scenario: BrowserScenario = {
  ...defaultScenario,
  files: { ...defaultScenario.files, "/home/user/pack.zip": "zip-bytes" },
  archives: {
    "/home/user/pack.zip": { "/docs/": "", "/docs/inner.txt": "inside", "/top.txt": "top" },
  },
};

function menuItem(page: Page, label: string | RegExp) {
  return page.getByTestId("context-menu").getByRole("menuitem", { name: label });
}

async function crumbs(page: Page, side: "left" | "right") {
  return page.getByTestId(`pane-${side}-crumbs`).locator(".crumb").allTextContents();
}

test.describe("archives", () => {
  test("UI-E2E-032 Enter opens an archive as a folder, Backspace leaves it", async ({ page }) => {
    await openApp(page, scenario);
    await point(page, "left", "pack.zip");
    await clearCalls(page);
    await page.keyboard.press("Enter");
    await expect(row(page, "left", "top.txt")).toBeVisible();
    expect(await calls(page, "open_file")).toHaveLength(0);
    const [open] = await calls(page, "open_archive");
    expect(open.location).toEqual({ fs: "local", path: "/home/user/pack.zip" });
    expect(await crumbs(page, "left")).toEqual(["/", "home", "user", "pack.zip"]);

    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "inner.txt")).toBeVisible();
    expect(await crumbs(page, "left")).toEqual(["/", "home", "user", "pack.zip", "docs"]);

    await page.keyboard.press("Backspace");
    await expect(row(page, "left", "top.txt")).toBeVisible();
    await page.keyboard.press("Backspace");
    await expect(row(page, "left", "readme.md")).toBeVisible();
    const cursor = await cursorIndex(page, "left");
    await expect(pane(page, "left").locator(`[data-index="${cursor}"]`)).toHaveAttribute("data-name", "pack.zip");
    await expect.poll(() => calls(page, "close_archive")).toHaveLength(1);
  });

  test("UI-E2E-033 copying out of an archive and Extract Here / Extract to", async ({ page }) => {
    await openApp(page, scenario);
    await point(page, "left", "pack.zip");
    await page.keyboard.press("Enter");
    await expect(row(page, "left", "top.txt")).toBeVisible();
    await point(page, "left", "top.txt");
    await clearCalls(page);
    // F7 inside an archive: nothing to create, the read-only notice instead.
    await page.keyboard.press("F7");
    await expect(page.getByText(/Archives are read-only/)).toBeVisible();
    // F5 copies out to the other pane.
    await page.keyboard.press("F5");
    await expect.poll(async () => (await enqueued(page)).length).toBe(1);
    const [copy] = await enqueued(page);
    expect(copy.type).toBe("copy");
    expect(copy.sources[0].fs).toMatch(/^archive:/);
    expect(copy.sources[0].path).toBe("/top.txt");
    expect(copy.destination).toEqual({ fs: "local", path: "/home/user" });

    // Back out, then extract from the context menu.
    await page.keyboard.press("Backspace");
    await expect(row(page, "left", "pack.zip")).toBeVisible();
    await clearCalls(page);
    await row(page, "left", "pack.zip").click({ button: "right" });
    await menuItem(page, "Extract Here").click();
    await expect.poll(async () => (await enqueued(page)).length).toBe(1);
    const [here] = await enqueued(page);
    expect(here.sources[0]).toEqual({ fs: expect.stringMatching(/^archive:/), path: "/" });
    expect(here.destination).toEqual({ fs: "local", path: "/home/user" });
    await expect.poll(() => calls(page, "close_archive")).toHaveLength(1);

    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await clearCalls(page);
    await row(page, "left", "pack.zip").click({ button: "right" });
    await menuItem(page, /^Extract to “docs”/).click();
    await expect.poll(async () => (await enqueued(page)).length).toBe(1);
    expect((await enqueued(page))[0].destination).toEqual({ fs: "local", path: "/home/user/docs" });

    // "Open in Default App" still opens the file itself.
    await row(page, "left", "pack.zip").click({ button: "right" });
    await menuItem(page, "Open in Default App").click();
    await expect.poll(() => calls(page, "open_file")).toHaveLength(1);
  });
});

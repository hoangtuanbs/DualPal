import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { calls, clearCalls, defaultScenario, openApp, point, row } from "./pom/app";

// The F3 viewer (README: Keyboard, "Viewing files").

const MOD = "ControlOrMeta";

const scenario = {
  ...defaultScenario,
  files: {
    ...defaultScenario.files,
    "/home/user/log.txt": Array.from({ length: 5000 }, (_, i) => (i === 4000 ? "found the Needle\n" : `entry ${i + 1}\n`)).join(""),
  },
};

const viewer = (page: Page) => page.getByTestId("viewer");
const textRows = (page: Page) => page.getByTestId("text-view").locator(".row .tx");

test.describe("UI-KEY viewer", () => {
  test("UI-KEY-040 F3 views the file under the cursor; Esc or F3 closes it", async ({ page }) => {
    await openApp(page, scenario);
    await point(page, "left", "readme.md");
    await clearCalls(page);
    await page.keyboard.press("F3");
    await expect(viewer(page)).toBeVisible();
    await expect(viewer(page).getByRole("heading", { name: "readme.md" })).toBeVisible();
    await expect(textRows(page).first()).toHaveText("# Readme");
    const [open] = await calls(page, "viewer_open");
    expect(open.location).toEqual({ fs: "local", path: "/home/user/readme.md" });
    await page.keyboard.press("Escape");
    await expect(viewer(page)).toBeHidden();

    // F3 also closes it, and the pane keys work again afterwards.
    await page.keyboard.press("F3");
    await expect(viewer(page)).toBeVisible();
    await page.keyboard.press("F3");
    await expect(viewer(page)).toBeHidden();
    await expect(calls(page, "viewer_close")).resolves.toHaveLength(2);

    // A folder under the cursor opens nothing.
    await point(page, "left", "docs");
    await page.keyboard.press("F3");
    await expect(viewer(page)).toBeHidden();
  });

  test("UI-KEY-041 in the viewer, 1 / 2 / 3 switch the view, Ctrl/Cmd F finds, F3 / Shift F3 step through matches", async ({ page }) => {
    await openApp(page, scenario);
    await point(page, "left", "log.txt");
    await page.keyboard.press("F3");
    await expect(textRows(page).first()).toHaveText("entry 1");
    await page.keyboard.press("2");
    await expect(page.getByTestId("hex-view")).toBeVisible();
    await expect(page.getByTestId("hex-view").locator(".row").first()).toContainText("65 6E 74 72 79");
    await page.keyboard.press("3"); // no image view for text: stays in hex
    await expect(page.getByTestId("hex-view")).toBeVisible();
    await page.keyboard.press("1");
    await expect(page.getByTestId("text-view")).toBeVisible();

    await page.keyboard.press(`${MOD}+f`);
    const field = page.getByTestId("viewer-search");
    await expect(field).toBeFocused();
    await field.fill("needle");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("text-match")).toHaveText("Needle");
    await expect(textRows(page).filter({ hasText: "found the Needle" })).toBeVisible();
    await page.keyboard.press("F3");
    await expect(page.getByTestId("viewer-search-status")).toContainText("No more matches below");
    await page.keyboard.press("Shift+F3");
    await expect(page.getByTestId("text-match")).toHaveText("Needle");
    const searches = await calls(page, "viewer_search");
    expect(searches.map((s) => s.backward)).toEqual([false, false, true]);
    await page.keyboard.press("Escape"); // closes find
    await expect(field).toBeHidden();
    await expect(viewer(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(viewer(page)).toBeHidden();
  });
});

test.describe("UI-E2E viewer", () => {
  test("UI-E2E-040 View in the item menu and the command palette opens the viewer", async ({ page }) => {
    await openApp(page, scenario);
    await row(page, "left", "a.txt").click({ button: "right" });
    await page.getByTestId("context-menu").getByRole("menuitem", { name: /^View/ }).click();
    await expect(viewer(page)).toBeVisible();
    await expect(textRows(page).first()).toHaveText("aaa");
    await viewer(page).getByRole("button", { name: "Close", exact: true }).click();
    await expect(viewer(page)).toBeHidden();

    await point(page, "left", "b.txt");
    await page.keyboard.press(`${MOD}+Shift+p`);
    await page.keyboard.type("View");
    await page.keyboard.press("Enter");
    await expect(viewer(page)).toBeVisible();
    await expect(textRows(page).first()).toHaveText("bbb");
  });
});

test("UI-E2E-041 the viewer has no serious accessibility problems in text and hex view", async ({ page }) => {
  const here = dirname(fileURLToPath(import.meta.url));
  const allowed = new Set<string>(JSON.parse(readFileSync(join(here, "a11y-allowlist.json"), "utf8")).allow["*"] ?? []);
  await openApp(page, scenario);
  await point(page, "left", "log.txt");
  await page.keyboard.press("F3");
  await expect(textRows(page).first()).toHaveText("entry 1");
  await page.keyboard.press(`${MOD}+f`);
  for (const key of ["1", "2"]) {
    await page.getByTestId("text-view").or(page.getByTestId("hex-view")).focus();
    await page.keyboard.press(key);
    const result = await new AxeBuilder({ page }).include("[data-testid=viewer]").analyze();
    const bad = result.violations
      .filter((v) => (v.impact === "serious" || v.impact === "critical") && !allowed.has(v.id))
      .map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).slice(0, 3).join(", ")}`);
    expect(bad, `view ${key}`).toEqual([]);
  }
});

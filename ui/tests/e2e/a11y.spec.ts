import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { dialog, openApp, pane, point, row, toolbar } from "./pom/app";

// Accessibility (§8.17 A11Y). Known violations are listed in
// a11y-allowlist.json with the screen they occur on, so the scan catches new
// ones while the existing ones are burned down.

const here = dirname(fileURLToPath(import.meta.url));
const allow: Record<string, string[]> = JSON.parse(readFileSync(join(here, "a11y-allowlist.json"), "utf8")).allow;

/** Serious and critical axe violations on the page, minus the allowed ones. */
async function violations(page: Page, screenName: string) {
  const result = await new AxeBuilder({ page }).analyze();
  const allowed = new Set([...(allow["*"] ?? []), ...(allow[screenName] ?? [])]);
  return result.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .filter((v) => !allowed.has(v.id))
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 3).join(", ")}`);
}

const MOD = "ControlOrMeta";

/** Toolbar buttons with no palette command and no shortcut yet (BUG, see
 *  the known-gap test below). "Commands" opens the palette itself. */
const NO_KEYBOARD_WAY = ["Auto-sync left to right", "Auto-sync right to left", "Auto-sync both ways"];

/** Labels of toolbar buttons that the palette or a shortcut does not reach. */
async function unreachable(page: Page) {
  const buttons = await toolbar(page)
    .getByRole("button")
    .evaluateAll((els) =>
      els
        .map((e) => ({ label: e.getAttribute("aria-label") ?? "", title: e.getAttribute("title") ?? "" }))
        .filter((b) => b.label && b.label !== "Commands"),
    );
  expect(buttons.length).toBeGreaterThan(10);
  await page.keyboard.press(`${MOD}+Shift+p`);
  const palette = page.getByTestId("palette");
  const input = palette.locator("input");
  const missing: string[] = [];
  for (const { label, title } of buttons) {
    // A shortcut in the tooltip, e.g. "(F5)" or "(Ctrl/⌘ Shift L)", counts.
    if (/((Fd+|Ctrl|⌘|Shift|Alt)[^)]*)/.test(title) || /⇧|⌘/.test(title)) continue;
    await input.fill(label);
    const titles = await palette.locator("li .title").allTextContents();
    const words = label.toLowerCase().split(/s+/);
    if (!titles.some((t) => words.every((w) => t.toLowerCase().includes(w)))) missing.push(label);
  }
  await page.keyboard.press("Escape");
  return missing;
}

test.describe("A11Y", () => {
  test("A11Y-001 axe finds no new serious violations on each screen", async ({ page }) => {
    await openApp(page);
    await point(page, "left", "a.txt");
    expect.soft(await violations(page, "main"), "main window").toEqual([]);

    await page.keyboard.press("F7");
    await expect(dialog(page)).toBeVisible();
    expect.soft(await violations(page, "dialog"), "new-folder dialog").toEqual([]);
    await page.keyboard.press("Escape");

    await page.keyboard.press("Shift+F8");
    await expect(dialog(page)).toBeVisible();
    expect.soft(await violations(page, "dialog"), "delete confirmation").toEqual([]);
    await page.keyboard.press("Escape");

    await page.keyboard.press(`${MOD}+Shift+p`);
    await expect(page.getByTestId("palette")).toBeVisible();
    expect.soft(await violations(page, "palette"), "command palette").toEqual([]);
    await page.keyboard.press("Escape");

    await page.keyboard.press(" ");
    await page.keyboard.press(" ");
    await page.keyboard.press("Shift+F2");
    await expect(page.getByTestId("multi-rename")).toBeVisible();
    expect.soft(await violations(page, "multi-rename"), "Multi Rename").toEqual([]);
    await page.keyboard.press("Escape");

    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await toolbar(page).getByRole("button", { name: "Compare & Sync" }).click();
    await expect(page.getByTestId("compare-view")).toBeVisible();
    expect.soft(await violations(page, "compare"), "compare view").toEqual([]);
    await page.keyboard.press("Escape");

    await page.getByTestId("sidebar").getByRole("button", { name: "Add server" }).click();
    await expect(page.getByTestId("connection-dialog")).toBeVisible();
    expect.soft(await violations(page, "connection-dialog"), "add server dialog").toEqual([]);
  });

  test("A11Y-002 every toolbar action can be reached without the mouse", async ({ page }) => {
    // Through a shortcut or the command palette (Ctrl/Cmd Shift P).
    await openApp(page);
    const missing = (await unreachable(page)).filter((l) => !NO_KEYBOARD_WAY.includes(l));
    expect(missing, "toolbar buttons with no shortcut and no palette command").toEqual([]);
  });

  test.fixme("A11Y-002 auto-sync can be started without the mouse", async ({ page }) => {
    // BUG: the three auto-sync buttons have no shortcut and no palette command.
    await openApp(page);
    expect(await unreachable(page)).toEqual([]);
  });

  test("A11Y-003 focus is visible in dialogs and comes back to the pane after they close", async ({ page }) => {
    await openApp(page);
    await point(page, "left", "a.txt");
    await page.keyboard.press("F2");
    const input = dialog(page).getByTestId("dialog-input");
    await expect(input).toBeFocused();
    const outline = await input.evaluate((el) => {
      const s = getComputedStyle(el);
      return { style: s.outlineStyle, width: parseFloat(s.outlineWidth), shadow: s.boxShadow };
    });
    expect(outline.style !== "none" && outline.width > 0 || outline.shadow !== "none", JSON.stringify(outline)).toBe(true);

    await page.keyboard.press("Escape");
    await expect(dialog(page)).toBeHidden();
    // Keys work on the pane again: the cursor moves.
    const inPane = await page.evaluate(() => !!document.activeElement?.closest('[data-testid="pane-left"]'));
    const body = await page.evaluate(() => document.activeElement === document.body);
    expect(inPane || body, "focus is in the pane (or nowhere that eats keys)").toBe(true);
    await page.keyboard.press("ArrowDown");
    await expect(pane(page, "left").locator(".cursor")).toHaveAttribute("data-name", "b.txt");
  });
});

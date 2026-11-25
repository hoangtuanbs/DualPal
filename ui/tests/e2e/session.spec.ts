import { expect, test } from "@playwright/test";
import { calls, defaultScenario, openApp, pane, row } from "./pom/app";

const tab = (path: string, extra: Record<string, unknown> = {}) => ({
  location: { fs: "local", path },
  view: "list",
  showHidden: false,
  sortKey: "name",
  sortAsc: true,
  ...extra,
});

test.describe("UI-E2E last session", () => {
  test("UI-E2E-060 restores the saved tabs, folders and layout", async ({ page }) => {
    await openApp(page, {
      ...defaultScenario,
      results: {
        session: {
          version: 1,
          split: 0.3,
          showInspector: false,
          activeSide: 1,
          sides: [
            { tabs: [tab("/home/user"), tab("/home/user/docs", { showHidden: true })], index: 1 },
            { tabs: [tab("/home/user/gone/away")], index: 0 },
          ],
        },
      },
    });
    // Left: two tabs, the docs one in front.
    await expect(pane(page, "left").getByTestId("pane-left-tab")).toHaveCount(2);
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    // Right: the saved folder is gone, so its nearest existing parent opens.
    await expect(row(page, "right", "docs")).toBeVisible();
    await expect(page.getByTestId("inspector")).toHaveCount(0);
    // The left pane takes about 30% of the panes' width.
    const left = await pane(page, "left").boundingBox();
    const right = await pane(page, "right").boundingBox();
    expect(left!.width / (left!.width + right!.width)).toBeCloseTo(0.3, 1);
  });

  test("UI-E2E-061 saves the session after a change", async ({ page }) => {
    await openApp(page);
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await expect
      .poll(async () => (await calls(page, "save_session")).at(-1)?.session?.sides?.[0]?.tabs?.[0]?.location?.path)
      .toBe("/home/user/docs");
    const last = (await calls(page, "save_session")).at(-1).session;
    expect(last.version).toBe(1);
    expect(last.sides[1].tabs[0].location.path).toBe("/home/user");
  });

  test("UI-E2E-062 starts in the home folder when the saved session is unreadable", async ({ page }) => {
    await openApp(page, { ...defaultScenario, results: { session: { version: 1, sides: "broken" } } });
    await expect(row(page, "left", "readme.md")).toBeVisible();
    await expect(row(page, "right", "readme.md")).toBeVisible();
  });
});

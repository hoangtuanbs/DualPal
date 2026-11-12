import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  clearCalls,
  control,
  cursorIndex,
  dialog,
  emit,
  enqueued,
  marked,
  openApp,
  pane,
  point,
  row,
} from "./pom/app";

// Keyboard table of the README (§8.14 UI-KEY). Each test drives the real app
// in a browser with the mock backend and checks the backend calls it makes.

const MOD = "ControlOrMeta";

/** The rows of a pane, in display order. */
async function names(page: Page, side: "left" | "right") {
  return pane(page, side)
    .locator("[data-index][data-name]")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.name!));
}

async function start(page: Page) {
  await openApp(page);
  await point(page, "left", "a.txt");
  await clearCalls(page);
}

test.describe("UI-KEY keyboard", () => {
  test("UI-KEY-001 Tab switches the active pane", async ({ page }) => {
    await start(page);
    await expect(pane(page, "left")).toHaveClass(/active/);
    await page.keyboard.press("Tab");
    await expect(pane(page, "right")).toHaveClass(/active/);
    await expect(pane(page, "left")).not.toHaveClass(/active/);
    await page.keyboard.press("Tab");
    await expect(pane(page, "left")).toHaveClass(/active/);
  });

  test("UI-KEY-002 arrows, Home, End and pages move the cursor and clamp", async ({ page }) => {
    await start(page);
    const count = (await names(page, "left")).length;
    await page.keyboard.press("Home");
    expect(await cursorIndex(page, "left")).toBe(0);
    await page.keyboard.press("ArrowUp");
    expect(await cursorIndex(page, "left")).toBe(0); // clamps at the top
    await page.keyboard.press("ArrowDown");
    expect(await cursorIndex(page, "left")).toBe(1);
    await page.keyboard.press("End");
    expect(await cursorIndex(page, "left")).toBe(count - 1);
    await page.keyboard.press("ArrowDown");
    expect(await cursorIndex(page, "left")).toBe(count - 1); // clamps at the end
    await page.keyboard.press("PageUp");
    expect(await cursorIndex(page, "left")).toBe(0);
    await page.keyboard.press("PageDown");
    expect(await cursorIndex(page, "left")).toBe(count - 1);
  });

  test("UI-KEY-003 Enter opens a folder or a file", async ({ page }) => {
    await start(page);
    await point(page, "left", "docs");
    await page.keyboard.press("Enter");
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    expect((await calls(page, "list_dir")).map((a) => a.location.path)).toContain("/home/user/docs");

    await point(page, "left", "notes.txt");
    await page.keyboard.press("Enter");
    await expect.poll(() => calls(page, "open_file")).toHaveLength(1);
    const [open] = await calls(page, "open_file");
    expect(open.location).toEqual({ fs: "local", path: "/home/user/docs/notes.txt" });
    expect(open.editor).toBeFalsy();
  });

  test("UI-KEY-005 Space and Insert toggle the mark and move down", async ({ page }) => {
    await start(page);
    const start_ = await cursorIndex(page, "left");
    await page.keyboard.press(" ");
    expect(await marked(page, "left")).toEqual(["a.txt"]);
    expect(await cursorIndex(page, "left")).toBe(start_ + 1);
    await page.keyboard.press("Insert");
    expect(await marked(page, "left")).toEqual(["a.txt", "b.txt"]);
    await page.keyboard.press("ArrowUp");
    await page.keyboard.press(" ");
    expect(await marked(page, "left")).toEqual(["a.txt"]);
  });

  test("UI-KEY-006 Shift+arrows mark the items passed over", async ({ page }) => {
    await start(page);
    await page.keyboard.press("Shift+ArrowDown");
    await page.keyboard.press("Shift+ArrowDown");
    expect(await marked(page, "left")).toEqual(["a.txt", "b.txt", "empty.txt"]);
    await page.keyboard.press("Shift+ArrowUp");
    expect((await marked(page, "left")).length).toBeGreaterThanOrEqual(2);
  });

  test("UI-KEY-008 Shift+F10 and the Menu key open the context menu for the cursor item", async ({ page }) => {
    await start(page);
    await page.keyboard.press("Shift+F10");
    await expect(page.getByTestId("context-menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("context-menu")).toBeHidden();
    // WebView2 delivers the Menu key only as a `contextmenu` event with no
    // mouse button pressed before it; Chromium would also send a keydown.
    await page.locator('[data-testid="pane-left"] .cursor').first().dispatchEvent("contextmenu");
    await expect(page.getByTestId("context-menu")).toBeVisible();
    await expect(page.getByTestId("context-menu-item").first()).toBeVisible();
  });

  test("UI-KEY-009 F2 renames the cursor item", async ({ page }) => {
    await start(page);
    await page.keyboard.press("F2");
    const input = dialog(page).getByTestId("dialog-input");
    await expect(input).toHaveValue("a.txt");
    await input.fill("renamed.txt");
    await page.keyboard.press("Enter");
    await expect.poll(() => enqueued(page)).toEqual([
      { type: "rename", location: { fs: "local", path: "/home/user/a.txt" }, newName: "renamed.txt" },
    ]);
    await expect(row(page, "left", "renamed.txt")).toBeVisible();
  });

  test("UI-KEY-010 Shift+F2 opens Multi Rename with the marked items", async ({ page }) => {
    await start(page);
    await page.keyboard.press(" ");
    await page.keyboard.press(" ");
    await page.keyboard.press("Shift+F2");
    const mr = page.getByTestId("multi-rename");
    await expect(mr).toBeVisible();
    await expect(mr).toContainText("Rename 2 items");
    await expect(mr).toContainText("0 of 2 names change");
  });

  test("UI-KEY-011 F4 opens the file in the editor", async ({ page }) => {
    await start(page);
    await page.keyboard.press("F4");
    await expect.poll(() => calls(page, "open_file")).toHaveLength(1);
    const [open] = await calls(page, "open_file");
    expect(open.location.path).toBe("/home/user/a.txt");
    expect(open.editor).toBe(true);
  });

  test("UI-KEY-012 F5 copies the marked items to the other pane, with the conflict flow", async ({ page }) => {
    await start(page);
    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await point(page, "left", "a.txt");
    await page.keyboard.press(" ");
    await page.keyboard.press(" ");
    await clearCalls(page);
    await control(page, { nextOpError: { kind: "alreadyExists", message: "already exists: /home/user/docs/a.txt" } });
    await page.keyboard.press("F5");
    await expect.poll(() => enqueued(page)).toEqual([
      {
        type: "copy",
        sources: [
          { fs: "local", path: "/home/user/a.txt" },
          { fs: "local", path: "/home/user/b.txt" },
        ],
        destination: { fs: "local", path: "/home/user/docs" },
        conflict: "fail",
      },
    ]);
    // The failure offers to replace or skip; Replace re-enqueues with that policy.
    const panel = page.getByTestId("ops-panel");
    await expect(panel.getByRole("button", { name: "Replace existing" })).toBeVisible();
    await panel.getByRole("button", { name: "Replace existing" }).click();
    await expect.poll(async () => (await enqueued(page)).map((o) => o.conflict)).toEqual(["fail", "overwrite"]);
  });

  test("UI-KEY-013 F6 moves the marked items to the other pane", async ({ page }) => {
    await start(page);
    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await point(page, "left", "b.txt");
    await clearCalls(page);
    await page.keyboard.press("F6");
    await expect.poll(() => enqueued(page)).toEqual([
      {
        type: "move",
        sources: [{ fs: "local", path: "/home/user/b.txt" }],
        destination: { fs: "local", path: "/home/user/docs" },
        conflict: "fail",
      },
    ]);
    await expect(row(page, "left", "b.txt")).toBeHidden();
    await expect(row(page, "right", "b.txt")).toBeVisible();
  });

  test("UI-KEY-015 F8 and Delete trash without asking", async ({ page }) => {
    await start(page);
    await page.keyboard.press("F8");
    await expect.poll(() => enqueued(page)).toEqual([
      { type: "trash", targets: [{ fs: "local", path: "/home/user/a.txt" }] },
    ]);
    await expect(dialog(page)).toBeHidden();
    await point(page, "left", "b.txt");
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await enqueued(page)).length).toBe(2);
    expect((await enqueued(page))[1]).toEqual({ type: "trash", targets: [{ fs: "local", path: "/home/user/b.txt" }] });
  });

  test("UI-KEY-016 Shift+F8 and Shift+Delete ask before deleting", async ({ page }) => {
    await start(page);
    await page.keyboard.press("Shift+F8");
    await expect(dialog(page)).toBeVisible();
    await expect(dialog(page)).toContainText("a.txt");
    await dialog(page).getByTestId("dialog-cancel").click();
    await expect(dialog(page)).toBeHidden();
    expect(await enqueued(page)).toEqual([]);

    await page.keyboard.press("Shift+Delete");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(() => enqueued(page)).toEqual([
      { type: "delete", targets: [{ fs: "local", path: "/home/user/a.txt" }] },
    ]);
  });

  test("UI-KEY-017 + and - mark and unmark by pattern", async ({ page }) => {
    await start(page);
    await page.keyboard.press("+");
    await dialog(page).getByTestId("dialog-input").fill("*.txt");
    await dialog(page).getByTestId("dialog-ok").click();
    expect(await marked(page, "left")).toEqual(["a.txt", "b.txt", "empty.txt"]);
    await page.keyboard.press("-");
    await dialog(page).getByTestId("dialog-input").fill("b*");
    await dialog(page).getByTestId("dialog-ok").click();
    expect(await marked(page, "left")).toEqual(["a.txt", "empty.txt"]);
  });

  test("UI-KEY-018 Ctrl/Cmd 1 and 2 switch list and icon view", async ({ page }) => {
    await start(page);
    const list = page.getByTestId("pane-left-list");
    await page.keyboard.press(`${MOD}+2`);
    await expect(list).toHaveClass(/grid/);
    await page.keyboard.press(`${MOD}+1`);
    await expect(list).not.toHaveClass(/grid/);
  });

  test("UI-KEY-019 Ctrl/Cmd T opens a tab, W closes it, the last one stays", async ({ page }) => {
    await start(page);
    // A single tab shows no tab strip.
    const tabs = page.getByTestId("pane-left-tab");
    await expect(tabs).toHaveCount(0);
    await page.keyboard.press(`${MOD}+t`);
    await expect(tabs).toHaveCount(2);
    await page.keyboard.press(`${MOD}+w`);
    await expect(tabs).toHaveCount(0);
    for (let i = 0; i < 3; i++) await page.keyboard.press(`${MOD}+w`);
    await expect(tabs).toHaveCount(0);
    await expect(row(page, "left", "a.txt")).toBeVisible();
  });

  test("UI-KEY-020 Ctrl Tab and Ctrl Shift Tab cycle tabs with wrap", async ({ page }) => {
    await start(page);
    await page.keyboard.press(`${MOD}+t`);
    await page.keyboard.press(`${MOD}+t`);
    const tabs = page.getByTestId("pane-left-tab");
    await expect(tabs).toHaveCount(3);
    const selected = async () => {
      const all = await tabs.evaluateAll((els) => els.map((e) => e.getAttribute("aria-selected")));
      return all.indexOf("true");
    };
    const first = await selected();
    await page.keyboard.press("Control+Tab");
    expect(await selected()).toBe((first + 1) % 3);
    await page.keyboard.press("Control+Shift+Tab");
    expect(await selected()).toBe(first);
    await page.keyboard.press("Control+Shift+Tab");
    expect(await selected()).toBe((first + 2) % 3);
  });

  test("UI-KEY-021 Ctrl/Cmd I toggles the inspector", async ({ page }) => {
    await start(page);
    const inspector = page.getByTestId("inspector");
    const visible = await inspector.isVisible();
    await page.keyboard.press(`${MOD}+i`);
    await expect(inspector).toBeVisible({ visible: !visible });
    await page.keyboard.press(`${MOD}+i`);
    await expect(inspector).toBeVisible({ visible });
  });

  test("UI-KEY-022 Ctrl/Cmd S in the quick editor saves the text", async ({ page }) => {
    await start(page);
    await point(page, "left", "readme.md");
    const inspector = page.getByTestId("inspector");
    if (!(await inspector.isVisible())) await page.keyboard.press(`${MOD}+i`);
    const editor = inspector.locator("textarea");
    await expect(editor).toBeVisible();
    await editor.fill("# Changed\n");
    await page.keyboard.press(`${MOD}+s`);
    await expect.poll(() => calls(page, "write_text")).toEqual([
      { location: { fs: "local", path: "/home/user/readme.md" }, text: "# Changed\n" },
    ]);
  });

  test("UI-KEY-023 Ctrl/Cmd R lists the folder again", async ({ page }) => {
    await start(page);
    await page.keyboard.press(`${MOD}+r`);
    await expect.poll(async () => (await calls(page, "list_dir")).map((a) => a.location.path)).toContain("/home/user");
  });

  test("UI-KEY-024 Ctrl/Cmd Shift . shows hidden files", async ({ page }) => {
    await start(page);
    await expect(row(page, "left", ".hidden")).toHaveCount(0);
    await page.keyboard.press(`${MOD}+Shift+.`);
    await expect(row(page, "left", ".hidden")).toBeVisible();
    await page.keyboard.press(`${MOD}+Shift+.`);
    await expect(row(page, "left", ".hidden")).toHaveCount(0);
  });

  test("UI-KEY-025 Ctrl/Cmd Shift P opens the palette with its input focused", async ({ page }) => {
    await start(page);
    await page.keyboard.press(`${MOD}+Shift+p`);
    await expect(page.getByTestId("palette")).toBeVisible();
    await expect(page.getByTestId("palette").locator("input")).toBeFocused();
  });

  test("UI-KEY-026 Ctrl/Cmd Shift Y compares the two pane folders", async ({ page }) => {
    await start(page);
    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await point(page, "left", "a.txt");
    await clearCalls(page);
    await page.keyboard.press(`${MOD}+Shift+y`);
    await expect(page.getByTestId("compare-view")).toBeVisible();
    await expect.poll(() => calls(page, "sync_plan")).toHaveLength(1);
    const [plan] = await calls(page, "sync_plan");
    expect(plan.left).toEqual({ fs: "local", path: "/home/user" });
    expect(plan.right).toEqual({ fs: "local", path: "/home/user/docs" });
  });

  test("UI-KEY-027 Ctrl/Cmd Shift D compares the two cursor files", async ({ page }) => {
    await start(page);
    // No file of the same name on the right: the right cursor file is used.
    await row(page, "right", "docs").dblclick();
    await point(page, "right", "notes.txt");
    await point(page, "left", "a.txt");
    await clearCalls(page);
    await page.keyboard.press(`${MOD}+Shift+d`);
    await expect.poll(() => calls(page, "compare_files")).toEqual([
      { left: { fs: "local", path: "/home/user/a.txt" }, right: { fs: "local", path: "/home/user/docs/notes.txt" } },
    ]);
    // A file of the same name in the other pane wins over its cursor.
    await page.keyboard.press("Tab");
    await page.keyboard.press("Backspace");
    await expect(row(page, "right", "a.txt")).toBeVisible();
    await point(page, "right", "b.txt");
    await point(page, "left", "a.txt");
    await clearCalls(page);
    await page.keyboard.press(`${MOD}+Shift+d`);
    await expect.poll(() => calls(page, "compare_files")).toEqual([
      { left: { fs: "local", path: "/home/user/a.txt" }, right: { fs: "local", path: "/home/user/a.txt" } },
    ]);
  });

  test("UI-KEY-028 Ctrl/Cmd Shift L links browsing in both panes", async ({ page }) => {
    await start(page);
    await page.keyboard.press(`${MOD}+Shift+l`);
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    // Off again: the other pane stays where it is.
    await page.keyboard.press("Backspace");
    await expect(row(page, "right", "docs")).toBeVisible();
    await page.keyboard.press(`${MOD}+Shift+l`);
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await expect(row(page, "right", "docs")).toBeVisible();
  });

  test("UI-KEY-029 shortcuts are ignored while typing in a field", async ({ page }) => {
    await start(page);
    await page.keyboard.press("F2");
    const input = dialog(page).getByTestId("dialog-input");
    await expect(input).toBeFocused();
    await page.keyboard.press("F5");
    await page.keyboard.press("F8");
    await page.keyboard.press("Delete");
    expect(await enqueued(page)).toEqual([]);
    await expect(dialog(page)).toBeVisible();
  });

  test("UI-KEY-030 shortcuts make no calls while auto-sync locks the panes", async ({ page }) => {
    await start(page);
    await row(page, "right", "docs").dblclick();
    await expect(row(page, "right", "notes.txt")).toBeVisible();
    await emit(page, "live-sync-updated", {
      from: { fs: "local", path: "/home/user" },
      to: { fs: "local", path: "/home/user/docs" },
      direction: "oneWay",
      state: "watching",
      fromWatched: true,
      toWatched: true,
      lastSyncMs: null,
      itemsSynced: 0,
      syncs: 1,
      error: null,
      conflicts: 0,
    });
    await expect(page.getByTestId("live-bar")).toBeVisible();
    await clearCalls(page);
    for (const key of ["F5", "F6", "F8", "Shift+F8", "F7", "F2"]) await page.keyboard.press(key);
    expect(await enqueued(page)).toEqual([]);
    await expect(dialog(page)).toBeHidden();
  });
});

import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  clearCalls,
  control,
  cursorIndex,
  defaultScenario,
  dialog,
  emit,
  enqueued,
  marked,
  openApp,
  ops,
  pane,
  point,
  row,
  toolbar,
} from "./pom/app";

// End-to-end flows with the mock backend (§8.14 UI-E2E).

const MOD = "ControlOrMeta";

async function rightInDocs(page: Page) {
  await row(page, "right", "docs").dblclick();
  await expect(row(page, "right", "notes.txt")).toBeVisible();
}

function opInfo(id: string, status: string, bytesDone: number, extra: Record<string, unknown> = {}) {
  return {
    id,
    kind: "copy",
    summary: "Copy a.txt",
    status,
    progress: { filesTotal: 4, filesDone: Math.floor(bytesDone / 25), bytesTotal: 100, bytesDone, skipped: 0, current: "/home/user/a.txt" },
    error: null,
    queuedMs: 1,
    startedMs: 1,
    finishedMs: status === "running" ? null : 2,
    places: ["local"],
    ...extra,
  };
}

const liveStatus = (state: "watching" | "syncing" | "failed", error: string | null = null) => ({
  from: { fs: "local", path: "/home/user" },
  to: { fs: "local", path: "/home/user/docs" },
  direction: "oneWay",
  state,
  fromWatched: true,
  toWatched: true,
  lastSyncMs: null,
  itemsSynced: 0,
  syncs: 1,
  error,
  conflicts: 0,
});

test.describe("UI-E2E operations", () => {
  test("UI-E2E-002 the operations panel shows live progress and the final state", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await control(page, { holdOps: true });
    await point(page, "left", "a.txt");
    await page.keyboard.press("F5");
    await expect.poll(async () => (await enqueued(page)).length).toBe(1);

    const bar = ops(page).locator(".bar > div");
    // The bar's inline style is `width: N%`.
    const width = async () => parseFloat(((await bar.getAttribute("style")) ?? "").replace(/[^\d.]/g, ""));
    for (const done of [25, 50, 75]) {
      await emit(page, "operation-updated", opInfo("op-1", "running", done));
      await expect(ops(page)).toContainText(`${done / 25} of 4 items`);
      await expect.poll(width).toBeCloseTo(done, 0);
    }
    await page.evaluate(() => (window as any).__mock.finishOp("op-1"));
    await expect(ops(page).locator(".bar")).toHaveCount(0);
    await ops(page).getByRole("tab", { name: "Activity" }).click();
    await expect(ops(page).locator("tbody tr, tr.completed").first()).toContainText("copy");
  });

  test("UI-E2E-003 a failed operation shows its error and is not retried", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await control(page, { nextOpError: { kind: "permissionDenied", message: "permission denied: /home/user/docs" } });
    await point(page, "left", "a.txt");
    await page.keyboard.press("F5");
    await expect(ops(page)).toContainText("permission denied: /home/user/docs");
    // Give an automatic retry the chance to happen, then check it did not.
    await page.waitForTimeout(300); // a gap, not a wait for a condition
    expect(await enqueued(page)).toHaveLength(1);
    await expect(ops(page).getByRole("button", { name: "Replace existing" })).toHaveCount(0);
  });

  test("UI-E2E-004 conflict: Skip or Replace re-enqueues with that policy", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await point(page, "left", "a.txt");
    for (const [button, policy] of [
      ["Skip existing", "skip"],
      ["Replace existing", "overwrite"],
    ] as const) {
      await clearCalls(page);
      await control(page, { nextOpError: { kind: "alreadyExists", message: "already exists: /home/user/docs/a.txt" } });
      await page.keyboard.press("F5");
      const retry = ops(page).getByRole("button", { name: button }).first();
      await expect(retry).toBeVisible();
      await retry.click();
      await expect.poll(async () => (await enqueued(page)).map((o) => o.conflict)).toEqual(["fail", policy]);
      const [first, second] = await enqueued(page);
      expect(second.sources).toEqual(first.sources);
      expect(second.destination).toEqual(first.destination);
    }
  });

  test("UI-E2E-005 a pane lists its folder again once an operation finishes", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await point(page, "left", "readme.md");
    await page.keyboard.press("F5");
    await expect(row(page, "right", "readme.md")).toBeVisible();
    await expect(row(page, "left", "readme.md")).toBeVisible();
  });

  test("UI-E2E-006 a folder that disappears sends the pane to the nearest parent", async ({ page }) => {
    await openApp(page, {
      ...defaultScenario,
      files: { ...defaultScenario.files, "/home/user/docs/deep/": "", "/home/user/docs/deep/x.txt": "x" },
    });
    await row(page, "left", "docs").dblclick();
    await row(page, "left", "deep").dblclick();
    await expect(row(page, "left", "x.txt")).toBeVisible();
    await page.evaluate(() => (window as any).__mock.remove("/home/user/docs"));
    await page.keyboard.press(`${MOD}+r`);
    await expect(row(page, "left", "readme.md")).toBeVisible();
    await expect(page.getByTestId("pane-left-error")).toHaveCount(0);
  });

  test.fixme("UI-E2E-006 the move to the parent is explained with a notice", async ({ page }) => {
    // BUG: the pane moves up silently; the plan asks for a notice.
    await openApp(page);
    await row(page, "left", "docs").dblclick();
    await page.evaluate(() => (window as any).__mock.remove("/home/user/docs"));
    await page.keyboard.press(`${MOD}+r`);
    await expect(page.locator(".notice")).toContainText("docs");
  });

  test("UI-E2E-023 Compress zips the marked items next to them", async ({ page }) => {
    // The product names the archive itself (no name prompt).
    await openApp(page);
    await point(page, "left", "a.txt");
    await row(page, "left", "b.txt").click({ modifiers: ["ControlOrMeta"] });
    await clearCalls(page);
    await toolbar(page).getByRole("button", { name: "Compress" }).click();
    await expect.poll(() => enqueued(page)).toEqual([
      {
        type: "compress",
        sources: [
          { fs: "local", path: "/home/user/a.txt" },
          { fs: "local", path: "/home/user/b.txt" },
        ],
        destination: { fs: "local", path: "/home/user/Archive.zip" },
      },
    ]);
    await expect(row(page, "left", "Archive.zip")).toBeVisible();
    // A second archive does not replace the first.
    await clearCalls(page);
    await toolbar(page).getByRole("button", { name: "Compress" }).click();
    await expect.poll(async () => (await enqueued(page))[0]?.destination.path).toBe("/home/user/Archive 2.zip");
  });
});

test.describe("UI-E2E favorites, selection, editing", () => {
  test("UI-E2E-007 ★ adds the current folder to Favorites, × removes it", async ({ page }) => {
    await openApp(page);
    await row(page, "left", "docs").dblclick();
    await toolbar(page).getByRole("button", { name: "Favorite" }).click();
    const sidebar = page.getByTestId("sidebar");
    const fav = sidebar.getByRole("button", { name: "docs", exact: true });
    await expect(fav).toBeVisible();
    expect((await calls(page, "add_favorite"))[0].favorite.location).toEqual({ fs: "local", path: "/home/user/docs" });
    // The × shows on hover.
    await fav.hover();
    await sidebar.locator('[aria-label="Remove docs from Favorites"]').click();
    await expect(fav).toHaveCount(0);
    expect((await calls(page, "remove_favorite"))[0].location).toEqual({ fs: "local", path: "/home/user/docs" });
  });

  test.fixme("UI-E2E-007 removing a favorite asks first", async ({ page }) => {
    // BUG: the sidebar removes a favorite at once; the plan asks for a confirmation.
    await openApp(page);
    await row(page, "left", "docs").dblclick();
    await toolbar(page).getByRole("button", { name: "Favorite" }).click();
    await page.getByTestId("sidebar").getByRole("button", { name: "docs", exact: true }).hover();
    await page.getByTestId("sidebar").locator('[aria-label="Remove docs from Favorites"]').click();
    await expect(dialog(page)).toBeVisible();
  });

  test("UI-E2E-021 select by pattern marks matching names", async ({ page }) => {
    await openApp(page);
    await toolbar(page).getByRole("button", { name: "Select" }).click();
    const input = dialog(page).getByTestId("dialog-input");
    await input.fill("*.md; e*");
    await dialog(page).getByTestId("dialog-ok").click();
    expect(await marked(page, "left")).toEqual(["empty.txt", "readme.md"]);
  });

  test("UI-E2E-022 quick edit: dirty marker, Ctrl/Cmd S saves, Revert", async ({ page }) => {
    await openApp(page);
    await point(page, "left", "readme.md");
    const inspector = page.getByTestId("inspector");
    if (!(await inspector.isVisible())) await page.keyboard.press(`${MOD}+i`);
    const editor = inspector.getByRole("textbox", { name: "Edit readme.md" });
    await expect(editor).toHaveValue("# Readme\n");
    await expect(inspector).toContainText("Quick edit");
    await editor.fill("# Readme\nmore\n");
    await expect(inspector).toContainText("Edited");
    await page.keyboard.press(`${MOD}+s`);
    await expect(inspector).toContainText("Saved");
    expect(await calls(page, "write_text")).toEqual([
      { location: { fs: "local", path: "/home/user/readme.md" }, text: "# Readme\nmore\n" },
    ]);
    await editor.fill("throw away");
    await inspector.getByRole("button", { name: "Revert" }).click();
    await expect(editor).toHaveValue("# Readme\nmore\n");
  });
});

test.describe("UI-E2E Multi Rename", () => {
  test("UI-E2E-020 Multi Rename end to end, with a saved preset", async ({ page }) => {
    await openApp(page);
    await point(page, "left", "a.txt");
    await row(page, "left", "b.txt").click({ modifiers: ["ControlOrMeta"] });
    await toolbar(page).getByRole("button", { name: "Multi Rename" }).click();
    const mr = page.getByTestId("multi-rename");
    await expect(mr).toContainText("Rename 2 items");

    await mr.getByLabel("Find").fill(".txt");
    await mr.getByLabel("Replace with").fill(".md");
    await expect(mr.getByLabel("Preview")).toContainText("a.md");
    await expect(mr).toContainText("2 of 2 names change");

    // A clash is reported and blocks Rename.
    await mr.getByLabel("Replace with").fill("");
    await mr.getByLabel("Find").fill("a.txt");
    await mr.getByLabel("Replace with").fill("b.txt");
    await expect(mr).toContainText("name needs fixing");
    await expect(mr.getByRole("button", { name: "Rename", exact: true })).toBeDisabled();

    // Save the working rule as a preset, then load it again.
    await mr.getByLabel("Find").fill(".txt");
    await mr.getByLabel("Replace with").fill(".md");
    await mr.getByRole("button", { name: "Save Preset…" }).click();
    await mr.getByPlaceholder("Preset name").fill("To markdown");
    await mr.getByRole("button", { name: "Save", exact: true }).click();
    expect((await calls(page, "save_rename_preset"))[0].preset.name).toBe("To markdown");
    await mr.getByLabel("Find").fill("zzz");
    await mr.locator("label.preset select").selectOption("To markdown");
    await expect(mr.getByLabel("Find")).toHaveValue(".txt");

    await clearCalls(page);
    await mr.getByRole("button", { name: "Rename", exact: true }).click();
    await expect(mr).toBeHidden();
    await expect.poll(() => enqueued(page)).toEqual([
      {
        type: "batchRename",
        items: [
          { location: { fs: "local", path: "/home/user/a.txt" }, newName: "a.md" },
          { location: { fs: "local", path: "/home/user/b.txt" }, newName: "b.md" },
        ],
      },
    ]);
  });
});

test.describe("UI-E2E auto-sync", () => {
  test("UI-E2E-018 auto-sync asks, starts, locks the panes and stops", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    const live = page.getByTestId("live-controls");

    await clearCalls(page);
    await live.getByRole("button", { name: "Auto-sync left to right" }).click();
    await expect(dialog(page)).toContainText("/home/user");
    await expect(dialog(page)).toContainText("/home/user/docs");
    await dialog(page).getByTestId("dialog-cancel").click();
    expect(await calls(page, "live_sync_start")).toEqual([]);

    await live.getByRole("button", { name: "Auto-sync left to right" }).click();
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(() => calls(page, "live_sync_start")).toEqual([
      { from: { fs: "local", path: "/home/user" }, to: { fs: "local", path: "/home/user/docs" }, direction: "oneWay" },
    ]);
    await expect(page.getByTestId("live-bar")).toContainText("Auto-sync");
    await expect(page.locator(".middle")).toHaveAttribute("inert", "");

    await page.getByTestId("live-stop").click();
    await expect.poll(async () => (await calls(page, "live_sync_stop")).length).toBe(1);
    await emit(page, "live-sync-updated", null);
    await expect(page.locator(".middle")).not.toHaveAttribute("inert", "");
    await expect(page.getByTestId("live-stop")).toHaveCount(0);
  });

  test("UI-E2E-019 an auto-sync failure shows in the bar", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await emit(page, "live-sync-updated", liveStatus("watching"));
    await expect(page.getByTestId("live-bar")).toBeVisible();
    await emit(page, "live-sync-updated", liveStatus("failed", "the source folder is gone"));
    await expect(page.getByTestId("live-bar")).toHaveClass(/failed/);
    await expect(page.getByTestId("live-bar")).toContainText(/gone|failed/i);
  });
});

test.describe("UI-E2E layout and scale", () => {
  test("UI-E2E-028 the smallest window keeps names readable without sideways scrolling", async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 480 });
    await openApp(page);
    await expect(row(page, "left", "readme.md")).toBeVisible();
    for (const side of ["left", "right"] as const) {
      const list = page.getByTestId(`pane-${side}-list`);
      const overflow = await list.evaluate((el) => el.scrollWidth - el.clientWidth);
      expect(overflow, `${side} pane scrolls sideways`).toBeLessThanOrEqual(1);
      const name = row(page, side, "readme.md").locator(".name, span").first();
      expect((await name.boundingBox())!.width).toBeGreaterThan(30);
    }
    const bar = (await toolbar(page).boundingBox())!;
    expect(bar.x + bar.width).toBeLessThanOrEqual(800 + 1);
    const doc = await page.evaluate(() => document.documentElement.scrollWidth);
    expect(doc).toBeLessThanOrEqual(800);
  });

  test("UI-E2E-030 5 000 entries render, scroll and End reaches the last", async ({ page }) => {
    const files: Record<string, string> = { "/home/user/": "" };
    for (let i = 0; i < 5000; i++) files[`/home/user/file-${String(i).padStart(4, "0")}.txt`] = "x";
    await openApp(page, { home: "/home/user", files });
    await expect(page.getByTestId("pane-left-status")).toContainText("5000 items");
    await point(page, "left", "file-0000.txt");
    await page.keyboard.press("End");
    expect(await cursorIndex(page, "left")).toBe(4999);
    await expect(row(page, "left", "file-4999.txt")).toBeInViewport();
    await page.keyboard.press("Home");
    await expect(row(page, "left", "file-0000.txt")).toBeInViewport();
  });
});

test.describe("UI-E2E compare", () => {
  test("UI-E2E-016 compare options: by content, ignore patterns, hidden items plan again", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await toolbar(page).getByRole("button", { name: "Compare & Sync" }).click();
    const view = page.getByTestId("compare-view");
    await expect(view).toBeVisible();
    await expect.poll(async () => (await calls(page, "sync_plan")).length).toBe(1);
    const last = async () => (await calls(page, "sync_plan")).at(-1).options;

    await view.getByLabel("Compare by").selectOption("content");
    await expect.poll(async () => (await last()).by).toBe("content");

    await view.getByLabel("Ignore").fill("*.tmp; .git, node_modules");
    await view.getByLabel("Ignore").press("Enter");
    await expect.poll(async () => (await last()).exclude).toEqual(["*.tmp", ".git", "node_modules"]);

    const before = (await calls(page, "sync_plan")).length;
    // Hidden items are compared by default; turning that off plans again.
    await expect(view.getByLabel("Hidden items")).toBeChecked();
    await view.getByLabel("Hidden items").uncheck();
    await expect.poll(async () => (await calls(page, "sync_plan")).length).toBe(before + 1);
    expect(await last()).toMatchObject({ by: "content", includeHidden: false, exclude: ["*.tmp", ".git", "node_modules"] });

    await view.getByRole("button", { name: "Compare again" }).click();
    await expect.poll(async () => (await calls(page, "sync_plan")).length).toBe(before + 2);
  });
});

import { expect, test, type Page } from "@playwright/test";
import {
  calls,
  clearCalls,
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

// Mouse interactions (§8.14 UI-MOUSE), checked through the backend calls.

const MOD = process.platform === "darwin" ? "Meta" : "Control";

async function names(page: Page, side: "left" | "right") {
  return pane(page, side)
    .locator("[data-index][data-name]")
    .evaluateAll((els) => els.map((e) => (e as HTMLElement).dataset.name!));
}

function menuItem(page: Page, label: string | RegExp) {
  return page.getByTestId("context-menu").getByRole("menuitem", { name: label });
}

/** Open `docs` in the right pane so the panes show different folders. */
async function rightInDocs(page: Page) {
  await row(page, "right", "docs").dblclick();
  await expect(row(page, "right", "notes.txt")).toBeVisible();
}

/** Press on a row and move far enough to start a drag, then over `to`. */
async function dragTo(page: Page, from: ReturnType<typeof row>, to: { x: number; y: number }) {
  const box = (await from.boundingBox())!;
  await page.mouse.move(box.x + 20, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 40, box.y + box.height / 2 + 10, { steps: 3 });
  await page.mouse.move(to.x, to.y, { steps: 5 });
}

async function emptySpot(page: Page, side: "left" | "right") {
  const box = (await page.getByTestId(`pane-${side}-list`).boundingBox())!;
  return { x: box.x + box.width / 2, y: box.y + box.height - 10 };
}

test.describe("UI-MOUSE mouse", () => {
  test("UI-MOUSE-001 click sets the cursor, Ctrl/Cmd-click marks, Shift-click marks a range", async ({ page }) => {
    await openApp(page);
    const list = await names(page, "left");
    await point(page, "left", "a.txt");
    expect(await cursorIndex(page, "left")).toBe(list.indexOf("a.txt"));
    expect(await marked(page, "left")).toEqual([]);

    await row(page, "left", "empty.txt").click({ modifiers: [MOD] });
    // Like Explorer: the item clicked just before joins in.
    expect(await marked(page, "left")).toEqual(["a.txt", "empty.txt"]);
    await row(page, "left", "empty.txt").click({ modifiers: [MOD] });
    expect(await marked(page, "left")).toEqual(["a.txt"]);

    await point(page, "left", "a.txt");
    await row(page, "left", "readme.md").click({ modifiers: ["Shift"] });
    expect(await marked(page, "left")).toEqual(list.slice(list.indexOf("a.txt"), list.indexOf("readme.md") + 1));

    await point(page, "left", "b.txt");
    expect(await marked(page, "left")).toEqual([]);
  });

  test("UI-MOUSE-002 double-click opens a folder or a file", async ({ page }) => {
    await openApp(page);
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await clearCalls(page);
    await row(page, "left", "notes.txt").dblclick();
    await expect.poll(() => calls(page, "open_file")).toHaveLength(1);
    expect((await calls(page, "open_file"))[0].location.path).toBe("/home/user/docs/notes.txt");
  });

  test("UI-MOUSE-003 each item of the item menu makes its call", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    const open = async (name: string) => {
      await row(page, "left", name).click({ button: "right" });
      await expect(page.getByTestId("context-menu")).toBeVisible();
    };
    const choose = async (name: string, label: string | RegExp) => {
      await open(name);
      await menuItem(page, label).click();
      await expect(page.getByTestId("context-menu")).toBeHidden();
    };

    await clearCalls(page);
    await choose("a.txt", "Open");
    await expect.poll(async () => (await calls(page, "open_file")).map((a) => [a.location.path, !!a.editor])).toEqual([
      ["/home/user/a.txt", false],
    ]);

    await clearCalls(page);
    await choose("a.txt", /Edit in Editor/);
    await expect.poll(async () => (await calls(page, "open_file")).map((a) => a.editor)).toEqual([true]);

    await clearCalls(page);
    await choose("a.txt", /^Copy to “/);
    await expect.poll(async () => (await enqueued(page)).map((o) => [o.type, o.destination.path])).toEqual([
      ["copy", "/home/user/docs"],
    ]);

    await clearCalls(page);
    await choose("b.txt", /^Move to “/);
    await expect.poll(async () => (await enqueued(page)).map((o) => [o.type, o.sources[0].path])).toEqual([
      ["move", "/home/user/b.txt"],
    ]);

    await clearCalls(page);
    await choose("a.txt", /Rename/);
    await dialog(page).getByTestId("dialog-input").fill("c.txt");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(async () => (await enqueued(page)).map((o) => [o.type, o.newName])).toEqual([["rename", "c.txt"]]);

    await clearCalls(page);
    await choose("c.txt", "Compress into ZIP");
    await expect.poll(async () => (await enqueued(page)).map((o) => [o.type, o.destination.path])).toEqual([
      ["compress", "/home/user/c.zip"],
    ]);

    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await choose("readme.md", "Copy Path");
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("/home/user/readme.md");

    await clearCalls(page);
    await choose("readme.md", /Move to Trash/);
    await expect.poll(async () => (await enqueued(page)).map((o) => o.type)).toEqual(["trash"]);

    await clearCalls(page);
    await choose("empty.txt", /Delete Permanently/);
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(async () => (await enqueued(page)).map((o) => o.type)).toEqual(["delete"]);

    // Compare Files appears for two marked files.
    await point(page, "left", "c.zip");
    await row(page, "left", "c.txt").click({ modifiers: [MOD] });
    await clearCalls(page);
    await choose("c.txt", "Compare Files");
    await expect.poll(() => calls(page, "compare_files")).toHaveLength(1);
  });

  test("UI-MOUSE-004 the menu of a marked item acts on all marked items", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await point(page, "left", "a.txt");
    await row(page, "left", "b.txt").click({ modifiers: [MOD] });
    await clearCalls(page);
    await row(page, "left", "b.txt").click({ button: "right" });
    await menuItem(page, /^Copy 2 Items to “/).click();
    await expect.poll(async () => (await enqueued(page))[0]?.sources.map((s: { path: string }) => s.path)).toEqual([
      "/home/user/a.txt",
      "/home/user/b.txt",
    ]);
    // Right-clicking an unmarked item acts on that item only.
    await clearCalls(page);
    await row(page, "left", "readme.md").click({ button: "right" });
    await menuItem(page, /^Copy to “/).click();
    await expect.poll(async () => (await enqueued(page))[0]?.sources.map((s: { path: string }) => s.path)).toEqual([
      "/home/user/readme.md",
    ]);
  });

  test("UI-MOUSE-005 the menu of empty space: new folder, refresh, select all", async ({ page }) => {
    await openApp(page);
    const spot = await emptySpot(page, "left");
    const openMenu = async () => {
      await page.mouse.click(spot.x, spot.y, { button: "right" });
      await expect(page.getByTestId("context-menu")).toBeVisible();
    };

    await openMenu();
    await menuItem(page, "New Folder…").click();
    await dialog(page).getByTestId("dialog-input").fill("Fresh");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(() => enqueued(page)).toEqual([
      { type: "createDir", location: { fs: "local", path: "/home/user/Fresh" } },
    ]);
    await expect(row(page, "left", "Fresh")).toBeVisible();

    await clearCalls(page);
    await openMenu();
    await menuItem(page, "Refresh").click();
    await expect.poll(async () => (await calls(page, "list_dir")).length).toBeGreaterThan(0);

    await openMenu();
    await menuItem(page, "Select All").click();
    expect(await marked(page, "left")).toEqual(await names(page, "left"));
  });

  test("UI-MOUSE-006 dragging to the other pane copies, Shift moves, Esc cancels", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await clearCalls(page);

    await dragTo(page, row(page, "left", "a.txt"), await emptySpot(page, "right"));
    await page.mouse.up();
    await expect.poll(() => enqueued(page)).toEqual([
      {
        type: "copy",
        sources: [{ fs: "local", path: "/home/user/a.txt" }],
        destination: { fs: "local", path: "/home/user/docs" },
        conflict: "fail",
      },
    ]);

    await clearCalls(page);
    await dragTo(page, row(page, "left", "b.txt"), await emptySpot(page, "right"));
    await page.keyboard.down("Shift");
    await page.mouse.up();
    await page.keyboard.up("Shift");
    await expect.poll(async () => (await enqueued(page)).map((o) => [o.type, o.sources[0].path])).toEqual([
      ["move", "/home/user/b.txt"],
    ]);

    await clearCalls(page);
    await dragTo(page, row(page, "left", "readme.md"), await emptySpot(page, "right"));
    await page.keyboard.press("Escape");
    await page.mouse.up();
    expect(await enqueued(page)).toEqual([]);
  });

  test("UI-MOUSE-007 dragging onto a folder in the same pane copies into it; onto itself does nothing", async ({ page }) => {
    await openApp(page);
    await clearCalls(page);
    const docs = (await row(page, "left", "docs").boundingBox())!;
    await dragTo(page, row(page, "left", "a.txt"), { x: docs.x + 30, y: docs.y + docs.height / 2 });
    await page.mouse.up();
    await expect.poll(async () => (await enqueued(page)).map((o) => [o.type, o.destination.path])).toEqual([
      ["copy", "/home/user/docs"],
    ]);

    await clearCalls(page);
    const self = (await row(page, "left", "docs").boundingBox())!;
    await dragTo(page, row(page, "left", "docs"), { x: self.x + 60, y: self.y + self.height / 2 });
    await page.mouse.up();
    // Back in its own folder too: nothing happens.
    await dragTo(page, row(page, "left", "b.txt"), await emptySpot(page, "left"));
    await page.mouse.up();
    expect(await enqueued(page)).toEqual([]);
  });

  test("UI-MOUSE-008 files dropped from the system are copied where they land", async ({ page }) => {
    await openApp(page);
    await rightInDocs(page);
    await clearCalls(page);
    const spot = await emptySpot(page, "right");
    const dpr = await page.evaluate(() => devicePixelRatio);
    const position = { x: spot.x * dpr, y: spot.y * dpr };
    await emit(page, "tauri://drag-enter", { paths: ["/outside/photo.jpg"], position });
    await emit(page, "tauri://drag-drop", { paths: ["/outside/photo.jpg", "/outside/doc.pdf"], position });
    await expect.poll(() => enqueued(page)).toEqual([
      {
        type: "copy",
        sources: [
          { fs: "local", path: "/outside/photo.jpg" },
          { fs: "local", path: "/outside/doc.pdf" },
        ],
        destination: { fs: "local", path: "/home/user/docs" },
        conflict: "fail",
      },
    ]);
  });

  test("UI-MOUSE-009 a column header sorts, a second click reverses", async ({ page }) => {
    await openApp(page);
    const header = pane(page, "left").getByRole("button", { name: /^Size/ });
    await header.click();
    const files = async () => (await names(page, "left")).filter((n) => n.includes("."));
    const asc = await files();
    // empty.txt (0) < a.txt = b.txt (3) < readme.md (9)
    expect(asc[0]).toBe("empty.txt");
    expect(asc.at(-1)).toBe("readme.md");
    await header.click();
    const desc = await files();
    expect(desc[0]).toBe("readme.md");
    expect(desc.at(-1)).toBe("empty.txt");
  });

  test("UI-MOUSE-010 a breadcrumb click opens that folder", async ({ page }) => {
    await openApp(page);
    await row(page, "left", "docs").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    const crumbs = page.getByTestId("pane-left-crumbs");
    await crumbs.getByRole("button", { name: "user" }).click();
    await expect(row(page, "left", "docs")).toBeVisible();
    await expect(row(page, "left", "readme.md")).toBeVisible();
    await crumbs.getByRole("button", { name: "home" }).click();
    await expect(row(page, "left", "user")).toBeVisible();
  });
});

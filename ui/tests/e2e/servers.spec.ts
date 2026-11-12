import { expect, test, type Page } from "@playwright/test";
import type { ServerScenario } from "./browser-mock";
import { calls, clearCalls, control, defaultScenario, dialog, emit, openApp, pane, point, row, toolbar } from "./pom/app";

// Servers, remote editing and saved syncs with the mock backend (§8.14 UI-E2E).

const FP = "SHA256:abc123fingerprint";

function sftp(extra: Partial<ServerScenario> = {}): ServerScenario {
  return {
    connection: {
      id: "build",
      name: "Build box",
      endpoint: { type: "sftp", host: "build.example", port: 22, user: "deploy", auth: { method: "password" } },
      initialPath: null,
      favorites: [],
    },
    files: { "/srv/": "", "/srv/app/": "", "/srv/app/readme.txt": "remote readme", "/srv/logo.png": "png" },
    start: "/srv",
    ...extra,
  };
}

async function withServers(page: Page, ...servers: ServerScenario[]) {
  await openApp(page, {
    ...defaultScenario,
    servers: Object.fromEntries(servers.map((s) => [s.connection.id, s])),
  });
}

const sidebar = (page: Page) => page.getByTestId("sidebar");
const serverItem = (page: Page, name: string) => sidebar(page).locator(".item", { hasText: name }).first();

test.describe("UI-E2E servers", () => {
  test("UI-E2E-008 add an SFTP server, trust its key, and browse it", async ({ page }) => {
    await openApp(page);
    await control(page, { newServer: { fingerprint: FP, files: { "/": "", "/etc/": "", "/home/": "" } } });
    await sidebar(page).getByRole("button", { name: "Add server" }).click();
    const form = page.getByTestId("connection-dialog");
    await form.getByLabel("Name").fill("Pi");
    await form.getByPlaceholder("example.com").fill("pi@raspberry.local");
    await form.getByRole("button", { name: "Connect" }).click();

    // The host is unknown: its fingerprint is shown to check before trusting.
    await expect(dialog(page)).toContainText(FP);
    expect((await calls(page, "save_connection"))[0].connection.endpoint).toMatchObject({
      type: "sftp",
      host: "raspberry.local",
      user: "pi",
    });
    await dialog(page).getByTestId("dialog-ok").click();
    await expect(row(page, "left", "etc")).toBeVisible();
    await expect(serverItem(page, "Pi")).toBeVisible();
    await expect(serverItem(page, "Pi").getByLabel("Connected")).toBeVisible();
    const connects = await calls(page, "connect");
    expect(connects.map((c) => c.trust)).toEqual([null, FP]);
  });

  test("UI-E2E-009 a wrong password asks again; the right one with Remember connects", async ({ page }) => {
    await withServers(page, sftp({ password: "hunter2" }));
    await serverItem(page, "Build box").click();
    await expect(dialog(page)).toContainText("Enter the password for deploy@build.example");
    const input = dialog(page).getByTestId("dialog-input");
    await expect(input).toHaveAttribute("type", "password");
    await input.fill("wrong");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect(dialog(page)).toContainText("That password did not work");
    await dialog(page).getByTestId("dialog-input").fill("hunter2");
    await dialog(page).getByRole("checkbox").check();
    await dialog(page).getByTestId("dialog-ok").click();
    await expect(row(page, "left", "app")).toBeVisible();
    const last = (await calls(page, "connect")).at(-1);
    expect(last).toEqual({ id: "build", secret: "hunter2", remember: true, trust: null });
  });

  test("UI-E2E-010 a changed host key blocks the connection with no way to trust it", async ({ page }) => {
    await withServers(page, sftp({ hostKeyChanged: true, fingerprint: FP }));
    await serverItem(page, "Build box").click();
    await expect(page.locator(".notice")).toContainText("could be an attack");
    await expect(dialog(page)).toBeHidden();
    await expect(page.getByRole("button", { name: /Trust/ })).toHaveCount(0);
    expect((await calls(page, "connect")).every((c) => c.trust === null)).toBe(true);
    await expect(row(page, "left", "readme.md")).toBeVisible(); // still home
  });

  test("UI-E2E-011 an FTPS server's self-signed certificate is shown before trusting it", async ({ page }) => {
    await withServers(
      page,
      sftp({
        fingerprint: FP,
        connection: {
          id: "nas",
          name: "NAS",
          endpoint: { type: "ftp", host: "nas.local", port: 21, user: "me", security: "explicit" },
          initialPath: null,
          favorites: [],
        },
      }),
    );
    await serverItem(page, "NAS").click();
    await expect(dialog(page)).toContainText("certificate");
    await expect(dialog(page)).toContainText(FP);
    await dialog(page).getByTestId("dialog-ok").click();
    await expect(row(page, "left", "app")).toBeVisible();
    expect((await calls(page, "connect")).at(-1).trust).toBe(FP);
  });

  test("UI-E2E-012 S3 and WebDAV servers round-trip through the dialog", async ({ page }) => {
    await openApp(page);
    const form = page.getByTestId("connection-dialog");

    await sidebar(page).getByRole("button", { name: "Add server" }).click();
    await form.getByRole("radio", { name: "S3" }).click();
    await form.getByLabel("Name").fill("Photos bucket");
    await form.getByLabel("Bucket").fill("photos");
    await form.getByLabel("Endpoint").fill("http://127.0.0.1:9000");
    await form.getByLabel("Access key ID").fill("AKIA1");
    await form.getByRole("checkbox").check();
    await form.getByRole("button", { name: "Save" }).click();
    await expect(serverItem(page, "Photos bucket")).toBeVisible();

    await sidebar(page).getByRole("button", { name: "Add server" }).click();
    await form.getByRole("radio", { name: "WebDAV" }).click();
    await form.getByLabel("Address").fill("https://dav.example/remote.php/dav");
    await form.getByLabel("User").fill("me");
    await form.getByRole("button", { name: "Save" }).click();
    await expect(serverItem(page, "dav.example")).toBeVisible();

    // Editing shows what was saved, and saving again keeps it.
    await serverItem(page, "Photos bucket").hover();
    await sidebar(page).getByRole("button", { name: "Edit Photos bucket", exact: true }).click();
    await expect(form.getByLabel("Bucket")).toHaveValue("photos");
    await expect(form.getByLabel("Endpoint")).toHaveValue("http://127.0.0.1:9000");
    await expect(form.getByRole("checkbox")).toBeChecked();
    await clearCalls(page);
    await form.getByRole("button", { name: "Save" }).click();
    const saved = (await calls(page, "save_connection"))[0];
    expect(saved.connection.endpoint).toEqual({
      type: "s3",
      bucket: "photos",
      region: "us-east-1",
      endpoint: "http://127.0.0.1:9000",
      accessKeyId: "AKIA1",
      pathStyle: true,
    });
    expect(saved.secret).toBeNull(); // the saved secret is kept

    await serverItem(page, "dav.example").hover();
    await sidebar(page).getByRole("button", { name: "Edit dav.example", exact: true }).click();
    await expect(form.getByLabel("Address")).toHaveValue("https://dav.example/remote.php/dav");
    await expect(form.getByLabel("User")).toHaveValue("me");
  });

  test("a click on a server shows it connecting, and a second click does not connect again", async ({ page }) => {
    await withServers(page, sftp());
    await control(page, { holdConnect: true });
    await serverItem(page, "Build box").click();
    await expect(serverItem(page, "Build box")).toHaveAttribute("aria-busy", "true");
    await expect(serverItem(page, "Build box").getByRole("status", { name: "Connecting" })).toBeVisible();
    await serverItem(page, "Build box").click();
    expect(await calls(page, "connect")).toHaveLength(1);

    await page.evaluate(() => (window as any).__mock.releaseConnects());
    await expect(row(page, "left", "app")).toBeVisible();
    await expect(serverItem(page, "Build box")).not.toHaveAttribute("aria-busy");
    await expect(serverItem(page, "Build box").getByLabel("Connected")).toBeVisible();
  });

  test("UI-E2E-013 ★ on a server folder saves it under the server; disconnect sends panes home", async ({ page }) => {
    await withServers(page, sftp());
    await serverItem(page, "Build box").click();
    await row(page, "left", "app").dblclick();
    await expect(row(page, "left", "readme.txt")).toBeVisible();
    await toolbar(page).getByRole("button", { name: "Favorite" }).click();
    await expect.poll(() => calls(page, "add_connection_favorite")).toEqual([
      { id: "build", favorite: { name: "app", path: "/srv/app" } },
    ]);
    await expect(sidebar(page).locator(".item.nested", { hasText: "app" })).toBeVisible();

    await serverItem(page, "Build box").hover();
    await sidebar(page).getByRole("button", { name: "Disconnect Build box", exact: true }).click();
    await expect(row(page, "left", "readme.md")).toBeVisible();
    expect(await calls(page, "disconnect")).toEqual([{ id: "build" }]);
    await expect(serverItem(page, "Build box").getByLabel("Connected")).toHaveCount(0);
  });

  test("UI-E2E-014 F4 on a server file shows the editing bar and follows its status", async ({ page }) => {
    await withServers(page, sftp());
    await serverItem(page, "Build box").click();
    await row(page, "left", "app").dblclick();
    await point(page, "left", "readme.txt");
    await page.keyboard.press("F4");
    const bar = page.getByTestId("edit-sessions");
    await expect(bar).toContainText("readme.txt");
    await expect(bar).toContainText("Uploads when you save");
    const [session] = await page.evaluate(() => (window as any).__mock.handlers.edit_sessions());

    await emit(page, "edit-session-updated", { ...session, status: "uploading" });
    await expect(bar).toContainText("Uploading…");
    await emit(page, "edit-session-updated", { ...session, status: "uploaded", uploads: 1, lastUploadMs: Date.now() });
    await expect(bar).toContainText("Uploaded");
    await emit(page, "edit-session-updated", { ...session, status: "failed", error: "connection lost" });
    await expect(bar).toContainText("Upload failed: connection lost");
    await bar.getByRole("button", { name: "Upload" }).click();
    await expect.poll(() => calls(page, "upload_edit_session")).toEqual([{ id: session.id }]);

    // Stopping a session with unsent changes asks first.
    await emit(page, "edit-session-updated", { ...session, status: "failed", error: "connection lost" });
    await bar.getByRole("button", { name: "Stop editing readme.txt" }).click();
    await expect(dialog(page)).toContainText("Discard unsent changes?");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(() => calls(page, "stop_edit_session")).toEqual([{ id: session.id }]);
  });

  test("UI-E2E-017 a sync saved as a favorite reconnects its server and opens Compare", async ({ page }) => {
    await withServers(page, sftp());
    await serverItem(page, "Build box").click();
    await expect(row(page, "left", "app")).toBeVisible();
    await page.keyboard.press("Tab"); // right pane stays local
    await toolbar(page).getByRole("button", { name: "Compare & Sync" }).click();
    const view = page.getByTestId("compare-view");
    await view.getByRole("button", { name: "Save as Favorite…" }).click();
    await view.getByLabel("Favorite name").fill("Deploy");
    await view.getByRole("button", { name: "Save", exact: true }).click();
    await expect.poll(async () => (await calls(page, "save_sync_favorite"))[0]?.favorite).toMatchObject({
      name: "Deploy",
      left: { fs: "conn:build", path: "/srv" },
      right: { fs: "local", path: "/home/user" },
    });
    await page.keyboard.press("Escape");
    await expect(view).toBeHidden();

    // Disconnect, then open the saved sync: it connects and compares again.
    await serverItem(page, "Build box").hover();
    await sidebar(page).getByRole("button", { name: "Disconnect Build box", exact: true }).click();
    await clearCalls(page);
    await sidebar(page).locator(".item", { hasText: "Deploy" }).click();
    await expect(view).toBeVisible();
    expect((await calls(page, "connect")).map((c) => c.id)).toEqual(["build"]);
    await expect.poll(async () => (await calls(page, "sync_plan"))[0]?.left).toEqual({ fs: "conn:build", path: "/srv" });
  });
});

test.describe("UI-E2E Windows paths", () => {
  test("UI-E2E-029 C:\\ paths: breadcrumbs, parent navigation and joined names", async ({ page }) => {
    await openApp(page, {
      home: "C:\\Users\\me",
      files: {
        "C:\\Users\\me\\": "",
        "C:\\Users\\me\\notes.txt": "n",
        "C:\\Users\\me\\Documents\\": "",
        "C:\\Users\\me\\Documents\\cv.docx": "cv",
        "C:\\Users\\other\\": "",
      },
    });
    await expect(row(page, "left", "Documents")).toBeVisible();
    const crumbs = page.getByTestId("pane-left-crumbs");
    await expect(crumbs).toContainText("Users");
    await expect(crumbs).toContainText("me");

    await row(page, "left", "Documents").dblclick();
    await expect(row(page, "left", "cv.docx")).toBeVisible();
    await crumbs.getByRole("button", { name: "me", exact: true }).click();
    await expect(row(page, "left", "notes.txt")).toBeVisible();
    await page.keyboard.press("Backspace");
    await expect(row(page, "left", "other")).toBeVisible();
    // Back down to the home folder for the rest of the test.
    await row(page, "left", "me").dblclick();
    await expect(row(page, "left", "notes.txt")).toBeVisible();

    // New names are joined with a backslash.
    await pane(page, "left").click();
    await page.keyboard.press("F7");
    await dialog(page).getByTestId("dialog-input").fill("Projects");
    await dialog(page).getByTestId("dialog-ok").click();
    await expect.poll(async () => (await calls(page, "enqueue_operation"))[0]?.operation).toEqual({
      type: "createDir",
      location: { fs: "local", path: "C:\\Users\\me\\Projects" },
    });
    await expect(row(page, "left", "Projects")).toBeVisible();
  });
});

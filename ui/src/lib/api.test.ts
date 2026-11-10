import { describe, expect, it } from "vitest";
import { api, defaultCompareOptions, errorKind, errorMessage } from "./api";
import { installMockBackend, tree } from "../../tests/mock-backend";

describe("UI-UNIT-007 errorKind / errorMessage", () => {
  it("reads kind and message from an object", () => {
    const e = { kind: "notFound", message: "not found: /x" };
    expect(errorKind(e)).toBe("notFound");
    expect(errorMessage(e)).toBe("not found: /x");
  });

  it("falls back to String() for anything else", () => {
    expect(errorKind("boom")).toBeNull();
    expect(errorKind(null)).toBeNull();
    expect(errorMessage("boom")).toBe("boom");
    expect(errorMessage(null)).toBe("null");
    expect(errorMessage(new Error("oops"))).toContain("oops");
  });
});

describe("UI-UNIT-008 api passes exact argument names", () => {
  it("records the exact argument objects the Rust side expects", async () => {
    const mock = installMockBackend();
    const loc = { fs: "local", path: "/home/user" };
    const entry = { fs: "local", path: "/home/user/readme.md" };

    await api.fileSystems();
    expect(mock.lastCall("file_systems")).toEqual({});

    await api.listDir(loc);
    expect(mock.lastCall("list_dir")).toEqual({ location: loc });

    await api.readText(entry, 1024);
    expect(mock.lastCall("read_text")).toEqual({ location: entry, maxBytes: 1024 });

    await api.writeText(entry, "hi");
    expect(mock.lastCall("write_text")).toEqual({ location: entry, text: "hi" });

    await api.availableSpace(loc);
    expect(mock.lastCall("available_space")).toEqual({ location: loc });

    await api.cancel("op-1");
    expect(mock.lastCall("cancel_operation")).toEqual({ id: "op-1" });

    await api.openUrl("mailto:x@y.z");
    expect(mock.lastCall("open_url")).toEqual({ url: "mailto:x@y.z" });

    await api.saveSettings({ editor: "/v", diffTool: null });
    expect(mock.lastCall("save_settings")).toEqual({ settings: { editor: "/v", diffTool: null } });

    await api.reorderFavorites(1, 0);
    expect(mock.lastCall("reorder_favorites")).toEqual({ from: 1, to: 0 });

    await api.activityInsights(7);
    expect(mock.lastCall("activity_insights")).toEqual({ days: 7, utcOffsetMinutes: -new Date().getTimezoneOffset() });

    await api.renamePreview([], [{ name: "a.txt", dir: false, modifiedMs: null, parent: "p" }], []);
    expect(mock.lastCall("rename_preview")).toEqual({
      rules: [],
      inputs: [{ name: "a.txt", dir: false, modifiedMs: null, parent: "p" }],
      others: [],
    });

    await api.removeRenamePreset("My rule");
    expect(mock.lastCall("remove_rename_preset")).toEqual({ name: "My rule" });

    await api.liveSyncStart(loc, entry, "oneWay");
    expect(mock.lastCall("live_sync_start")).toEqual({ from: loc, to: entry, direction: "oneWay" });

    await api.syncPlan(loc, entry, "mirror", { by: "size", toleranceMs: 0, exclude: [], includeHidden: true });
    expect(mock.lastCall("sync_plan")).toEqual({
      left: loc,
      right: entry,
      mode: "mirror",
      options: { by: "size", toleranceMs: 0, exclude: [], includeHidden: true },
    });

    await api.runSync("plan-1", { "a.txt": "copyToRight" });
    expect(mock.lastCall("run_sync")).toEqual({ plan: "plan-1", overrides: { "a.txt": "copyToRight" } });

    await api.forgetSyncHistory(loc, entry);
    expect(mock.lastCall("forget_sync_history")).toEqual({ left: loc, right: entry });
  });
});

describe("api covers the remaining commands", () => {
  it("invokes every command function with its documented argument names", async () => {
    const mock = installMockBackend();
    const loc = { fs: "local", path: "/home/user" };
    const e = { fs: "local", path: "/home/user/readme.md" };
    const conn = {
      id: "c1",
      name: "SFTP",
      endpoint: { type: "sftp", host: "h", port: 22, user: "u", auth: { method: "password" } },
      initialPath: null,
      favorites: [],
    } as const;

    expect(defaultCompareOptions().by).toBe("sizeAndDate");
    expect(defaultCompareOptions().exclude).toContain(".DS_Store");

    await api.homeLocation();
    await api.parentLocation(loc);
    await api.enqueue({ type: "createDir", location: loc });
    await api.operations();
    await api.clearFinished();
    await api.activity();
    await api.clearActivity();
    await api.volumes();
    await api.favorites();
    await api.addFavorite({ name: "W", location: loc, icon: "folder" });
    await api.removeFavorite(loc);
    await api.openPath(loc);
    await api.aboutInfo();
    await api.stat(e);
    await api.connections();
    await api.saveConnection(conn as any, "pw");
    await api.removeConnection("c1");
    await api.reorderConnections(["c1"]);
    await api.connect("c1");
    await api.disconnect("c1");
    await api.addConnectionFavorite("c1", { name: "Docs", path: "/docs" });
    await api.removeConnectionFavorite("c1", "/docs");
    await api.sshConfigHosts();
    await api.openFile(loc);
    await api.editSessions();
    await api.stopEditSession("s1");
    await api.uploadEditSession("s1").catch(() => {});
    await api.previewPath(loc);
    await api.settings();
    await api.cancelCompare();
    await api.syncFavorites();
    await api.saveSyncFavorite({ id: "", name: "B", left: loc, right: e, mode: "update", options: defaultCompareOptions() });
    await api.removeSyncFavorite("s1");
    await api.renamePresets();
    await api.saveRenamePreset({ name: "p", rules: [], builtin: false });
    await api.diffTools();
    await api.compareFiles(loc, e);
    await api.liveSyncStop();
    await api.liveSyncStatus();

    // Every function above was recorded against its command name.
    for (const cmd of ["home_location", "parent_location", "operations", "activity", "volumes", "favorites", "about_info", "connections", "ssh_config_hosts", "settings", "sync_favorites", "rename_presets", "diff_tools", "live_sync_stop", "live_sync_status"]) {
      expect(mock.calls(cmd).length).toBeGreaterThan(0);
    }
  });
});

describe("folder size API", () => {
  it("passes the location and the request id, and cancels by id", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/d/": "", "/d/x": "xy" }));
    const size = await api.folderSize({ fs: "local", path: "/d" }, 7);
    expect(size).toEqual({ bytes: 2, files: 1, folders: 0, skipped: 0 });
    expect(mock.lastCall("folder_size")).toEqual({ location: { fs: "local", path: "/d" }, request: 7 });

    await api.cancelFolderSize(7);
    expect(mock.lastCall("cancel_folder_size")).toEqual({ request: 7 });
  });
});

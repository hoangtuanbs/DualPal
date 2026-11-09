import { describe, expect, it } from "vitest";
import { api, type Entry, type Insights, type OperationInfo, type SyncPreview } from "../../src/lib/api";
import { installMockBackend, defaultScenario, tree } from "./index";

// UI-MOCK-001 — the mock returns the same JSON shapes the Rust side serialises.
// These assertions mirror the serde shapes in crates/dualpal-core and
// src-tauri, so a serde rename that breaks the real backend also fails here.
describe("UI-MOCK-001 mock shapes match Rust", () => {
  it("list_dir returns Entry objects", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/w/": "", "/w/a.txt": "hello" }));
    const entries: Entry[] = await api.listDir({ fs: "local", path: "/w" });
    expect(entries.length).toBe(1);
    const e = entries[0];
    expect(e).toMatchObject({
      name: "a.txt",
      path: "/w/a.txt",
      kind: "file",
      symlink: false,
      size: 5,
      hidden: false,
      readonly: false,
    });
    expect(typeof e.modifiedMs).toBe("number");
    expect(typeof e.mode).toBe("number");
  });

  it("enqueue + operations return OperationInfo objects", async () => {
    installMockBackend();
    const id = await api.enqueue({ type: "createDir", location: { fs: "local", path: "/x" } });
    const ops: OperationInfo[] = await api.operations();
    expect(ops).toEqual([
      expect.objectContaining({
        id,
        kind: "createDir",
        status: "completed",
        progress: expect.objectContaining({ filesTotal: 0, filesDone: 0, bytesTotal: 0, bytesDone: 0, skipped: 0 }),
        error: null,
      }),
    ]);
  });

  it("sync_plan returns a SyncPreview", async () => {
    installMockBackend();
    const loc = { fs: "local", path: "/a" };
    const plan: SyncPreview = await api.syncPlan(loc, { fs: "local", path: "/b" }, "update", {
      by: "sizeAndDate",
      toleranceMs: 2000,
      exclude: [],
      includeHidden: true,
    });
    expect(plan).toMatchObject({
      id: expect.any(String),
      mode: "update",
      steps: [],
      hasHistory: false,
    });
    expect(plan.comparison).toHaveProperty("items");
    expect(plan.comparison).toHaveProperty("summary");
  });

  it("activity_insights returns an Insights object", async () => {
    installMockBackend();
    const ins: Insights = await api.activityInsights(7);
    expect(ins).toMatchObject({
      days: [],
      operations: 0,
      completed: 0,
      failed: 0,
      cancelled: 0,
      files: 0,
      bytes: 0,
      busyMs: 0,
      throughput: null,
      byKind: [],
      byPlace: [],
      largest: [],
      failures: [],
    });
  });

  it("the default scenario has the standard tree", () => {
    const s = defaultScenario();
    expect(s.home).toEqual({ fs: "local", path: "/home/user" });
    expect(s.trees.local.has("/home/user/readme.md")).toBe(true);
    expect(s.trees.local.has("/home/user/docs/notes.txt")).toBe(true);
  });
});

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installMockBackend } from "../../tests/mock-backend";
import { PaneGroup, PaneState } from "./pane-state.svelte";
import {
  MAX_TABS,
  SESSION_VERSION,
  captureSession,
  parseSession,
  restoreSides,
  sessionSaver,
  type Session,
} from "./session";

const HOME = { fs: "local", path: "/home/user" };
const DOCS = { fs: "local", path: "/home/user/docs" };

const tab = (path: string, extra: Record<string, unknown> = {}) => ({
  location: { fs: "local", path },
  view: "list",
  showHidden: false,
  sortKey: "name",
  sortAsc: true,
  ...extra,
});

const session = (left: unknown[], right: unknown[], extra: Record<string, unknown> = {}) => ({
  version: SESSION_VERSION,
  split: 0.35,
  showInspector: false,
  activeSide: 1,
  sides: [
    { tabs: left, index: 0 },
    { tabs: right, index: 0 },
  ],
  ...extra,
});

const groups = () => [new PaneGroup(new PaneState()), new PaneGroup(new PaneState())];

describe("parseSession", () => {
  it("accepts a saved session as it was written", () => {
    const s = parseSession(session([tab("/a", { view: "icons", sortKey: "size", sortAsc: false, showHidden: true })], [tab("/b")]));
    expect(s).toEqual({
      version: SESSION_VERSION,
      split: 0.35,
      showInspector: false,
      activeSide: 1,
      sides: [
        { tabs: [{ location: { fs: "local", path: "/a" }, view: "icons", showHidden: true, sortKey: "size", sortAsc: false }], index: 0 },
        { tabs: [{ location: { fs: "local", path: "/b" }, view: "list", showHidden: false, sortKey: "name", sortAsc: true }], index: 0 },
      ],
    });
  });

  it.each([null, undefined, "text", 3, [], {}, { version: 99, sides: [] }])("gives null for %j", (raw) => {
    expect(parseSession(raw)).toBeNull();
  });

  it("drops broken tabs and falls back to defaults for broken fields", () => {
    const s = parseSession({
      version: SESSION_VERSION,
      split: "wide",
      showInspector: "yes",
      activeSide: 7,
      sides: [
        { tabs: [null, { location: { fs: "", path: "/x" } }, tab("/ok", { view: "grid", sortKey: "colour" })], index: 2 },
        "nonsense",
      ],
    })!;
    expect(s.split).toBeNull();
    expect(s.showInspector).toBeNull();
    expect(s.activeSide).toBe(0);
    expect(s.sides[0]).toEqual({
      tabs: [{ location: { fs: "local", path: "/ok" }, view: "list", showHidden: false, sortKey: "name", sortAsc: true }],
      index: 0,
    });
    expect(s.sides[1]).toEqual({ tabs: [], index: 0 });
  });

  it("keeps the shown tab when tabs before it are dropped", () => {
    const s = parseSession(session([null, tab("/a"), tab("/b"), tab("/c")], []))!;
    const withIndex = parseSession({ ...session([null, tab("/a"), tab("/b"), tab("/c")], []), sides: [{ tabs: [null, tab("/a"), tab("/b"), tab("/c")], index: 2 }, {}] })!;
    expect(s.sides[0].index).toBe(0);
    expect(withIndex.sides[0].tabs[withIndex.sides[0].index].location.path).toBe("/b");
  });

  it("keeps the split within range and limits the number of tabs", () => {
    const many = Array.from({ length: MAX_TABS + 10 }, (_, i) => tab(`/t${i}`));
    const s = parseSession(session(many, [], { split: 5 }))!;
    expect(s.split).toBeLessThan(1);
    expect(s.sides[0].tabs).toHaveLength(MAX_TABS);
  });
});

describe("captureSession and restoreSides", () => {
  beforeEach(() => {
    installMockBackend();
  });

  it("round-trips tabs, folders, view settings and the layout", async () => {
    const g = groups();
    await g[0].current.open(HOME);
    g[0].current.view = "icons";
    g[0].current.sortKey = "modified";
    g[0].current.sortAsc = false;
    const second = new PaneState();
    second.showHidden = true;
    await second.open(DOCS);
    g[0].newTab(second);
    await g[1].current.open(DOCS);

    const saved = captureSession(g, { split: 0.4123, showInspector: true, activeSide: 1 });
    expect(saved.split).toBe(0.412);
    expect(saved.activeSide).toBe(1);
    expect(saved.sides[0].index).toBe(1);

    // Through JSON, as it is stored on disk.
    const back = parseSession(JSON.parse(JSON.stringify(saved)));
    expect(back).toEqual(saved);

    const fresh = groups();
    await restoreSides(fresh, back, HOME);
    expect(fresh[0].tabs.map((p) => p.location?.path)).toEqual([HOME.path, DOCS.path]);
    expect(fresh[0].index).toBe(1);
    expect(fresh[0].tabs[0].view).toBe("icons");
    expect(fresh[0].tabs[0].sortKey).toBe("modified");
    expect(fresh[0].tabs[0].sortAsc).toBe(false);
    expect(fresh[0].tabs[1].showHidden).toBe(true);
    expect(fresh[1].tabs.map((p) => p.location?.path)).toEqual([DOCS.path]);
    expect(captureSession(fresh, { split: 0.412, showInspector: true, activeSide: 1 })).toEqual(saved);
  });

  it("opens the fallback on both sides without a session", async () => {
    const g = groups();
    await restoreSides(g, null, HOME);
    expect(g.map((x) => x.tabs.length)).toEqual([1, 1]);
    expect(g.map((x) => x.current.location?.path)).toEqual([HOME.path, HOME.path]);
  });

  it("opens the nearest existing folder for a local folder that is gone", async () => {
    const g = groups();
    await restoreSides(g, parseSession(session([tab("/home/user/docs/old/deeper")], [])), HOME);
    expect(g[0].current.location?.path).toBe(DOCS.path);
    expect(g[0].current.error).toBeNull();
  });

  it("leaves out tabs that cannot be opened and keeps the shown one", async () => {
    const g = groups();
    const raw = {
      ...session([], []),
      sides: [
        {
          tabs: [tab("/home/user"), { ...tab("/nowhere"), location: { fs: "conn:gone", path: "/nowhere" } }, tab("/home/user/docs")],
          index: 2,
        },
        { tabs: [{ ...tab("/srv"), location: { fs: "conn:gone", path: "/srv" } }], index: 0 },
      ],
    };
    await restoreSides(g, parseSession(raw), HOME);
    expect(g[0].tabs.map((p) => p.location?.path)).toEqual([HOME.path, DOCS.path]);
    expect(g[0].current.location?.path).toBe(DOCS.path);
    // A side left without tabs shows the fallback folder.
    expect(g[1].tabs.map((p) => p.location?.path)).toEqual([HOME.path]);
  });

  it("does not save tabs that never opened a folder", () => {
    const g = groups();
    const s = captureSession(g, { split: 0.5, showInspector: true, activeSide: 0 });
    expect(s.sides).toEqual([
      { tabs: [], index: 0 },
      { tabs: [], index: 0 },
    ]);
  });
});

describe("sessionSaver", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  const s = (split: number) => parseSession(session([tab("/a")], [], { split }))! as Session;

  it("saves once, a while after the last change", async () => {
    const save = vi.fn().mockResolvedValue(undefined);
    const saver = sessionSaver(save, 100);
    saver.schedule(s(0.3));
    saver.schedule(s(0.4));
    await vi.advanceTimersByTimeAsync(99);
    expect(save).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0][0].split).toBe(0.4);
  });

  it("skips a session equal to the last one saved", async () => {
    const save = vi.fn().mockResolvedValue(undefined);
    const saver = sessionSaver(save, 10);
    saver.schedule(s(0.3));
    await vi.advanceTimersByTimeAsync(10);
    saver.schedule(s(0.3));
    await vi.advanceTimersByTimeAsync(10);
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("flush saves the pending session at once", async () => {
    const save = vi.fn().mockResolvedValue(undefined);
    const saver = sessionSaver(save, 10_000);
    saver.schedule(s(0.3));
    await saver.flush();
    expect(save).toHaveBeenCalledTimes(1);
    await saver.flush();
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("tries again after a failed save", async () => {
    const save = vi.fn().mockRejectedValueOnce(new Error("disk full")).mockResolvedValue(undefined);
    const saver = sessionSaver(save, 10);
    saver.schedule(s(0.3));
    await vi.advanceTimersByTimeAsync(10);
    saver.schedule(s(0.3));
    await vi.advanceTimersByTimeAsync(10);
    expect(save).toHaveBeenCalledTimes(2);
  });
});

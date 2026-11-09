import { beforeEach, describe, expect, it } from "vitest";
import { PaneState } from "./pane-state.svelte";
import { installMockBackend, tree } from "../../tests/mock-backend";
import type { Entry, Location } from "./api";

const LOC: Location = { fs: "local", path: "/dir" };

const file = (name: string, size = 0, modifiedMs: number | null = null): Entry => ({
  name,
  path: `/dir/${name}`,
  kind: "file",
  symlink: false,
  size,
  modifiedMs,
  createdMs: null,
  hidden: name.startsWith("."),
  readonly: false,
  mode: null,
});

const dir = (name: string): Entry => ({ ...file(name), kind: "dir", size: 0 });

beforeEach(() => {
  installMockBackend();
});

async function pane(entries: Entry[], location = LOC): Promise<PaneState> {
  const p = new PaneState();
  await p.open(location, { entries });
  return p;
}

describe("UI-UNIT-009 PaneState.visible sorting", () => {
  it("sorts folders first, then names naturally and case-insensitively", async () => {
    const p = await pane([file("file2.txt"), dir("z"), file("a.txt"), dir("A"), file("file10.txt")]);
    expect(p.visible.map((e) => e.name)).toEqual(["A", "z", "a.txt", "file2.txt", "file10.txt"]);
  });

  it("sorts by size and modified, ascending and descending", async () => {
    const p = await pane([file("a.txt", 10, 5), file("b.txt", 5, 2), file("c.txt", 20, 8)]);
    p.setSort("size");
    expect(p.visible.map((e) => e.name)).toEqual(["b.txt", "a.txt", "c.txt"]);
    p.setSort("size"); // toggles descending
    expect(p.visible.map((e) => e.name)).toEqual(["c.txt", "a.txt", "b.txt"]);

    p.setSort("modified");
    expect(p.visible.map((e) => e.name)).toEqual(["b.txt", "a.txt", "c.txt"]);
  });

  it("sorts equal names case-insensitively, keeping a stable order", async () => {
    const p = await pane([file("a"), file("A")]);
    expect(p.visible.map((e) => e.name)).toEqual(["a", "A"]);
  });
});

describe("UI-UNIT-010 PaneState hidden filter", () => {
  it("hides dot files until showHidden", async () => {
    const p = await pane([file("shown.txt"), file(".secret")]);
    expect(p.visible.map((e) => e.name)).toEqual(["shown.txt"]);
    p.showHidden = true;
    expect(p.visible.map((e) => e.name)).toEqual([".secret", "shown.txt"]);
  });
});

describe("UI-UNIT-011 PaneState.targets", () => {
  it("is the cursor entry when nothing is marked, otherwise the marked ones", async () => {
    const p = await pane([file("a.txt"), file("b.txt"), file("c.txt")]);
    expect(p.targets.map((e) => e.name)).toEqual(["a.txt"]);

    p.toggle(p.visible[2]);
    expect(p.targets.map((e) => e.name)).toEqual(["c.txt"]);

    p.toggle(p.visible[0]);
    expect(p.targets.map((e) => e.name)).toEqual(["a.txt", "c.txt"]);
  });
});

describe("UI-UNIT-012 PaneState history", () => {
  it("back and forward move through the open history", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/dir/": "", "/dir/sub/": "", "/dir/sub/inside.txt": "i" }));

    const p = new PaneState();
    await p.open(LOC); // lists /dir via the mock
    const sub = { fs: "local", path: "/dir/sub" };
    await p.open(sub);
    expect(p.location).toEqual(sub);
    expect(p.canBack).toBe(true);
    expect(p.canForward).toBe(false);

    await p.back();
    expect(p.location).toEqual(LOC);
    expect(p.canForward).toBe(true);

    await p.forward();
    expect(p.location).toEqual(sub);
  });
});

describe("UI-UNIT-013 PaneState folder disappears", () => {
  it("goes to the nearest existing parent", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/a/": "", "/a/b/": "", "/a/b/c/": "", "/a/keep.txt": "k" }));

    const deep = { fs: "local", path: "/a/b/c" };
    const p = new PaneState();
    await p.open(deep); // lists via the mock, which now has the tree
    expect(p.location).toEqual(deep);

    // The folder vanishes: /a/b/c and /a/b are gone, /a remains.
    mock.setTree("local", tree({ "/a/": "", "/a/keep.txt": "k" }));
    await p.refresh();
    expect(p.location).toEqual({ fs: "local", path: "/a" });
    expect(p.error).toBeNull();
  });
});

describe("UI-UNIT-014 PaneState.navigated hook", () => {
  it("fires after a folder opens", async () => {
    let fired: { from: Location | null; to: Location } | null = null;
    PaneState.navigated = (_pane, from, to) => {
      fired = { from, to };
    };
    try {
      const p = new PaneState();
      await p.open(LOC, { entries: [] });
      expect(fired).toEqual({ from: null, to: LOC });

      const sub = { fs: "local", path: "/dir/sub" };
      await p.open(sub, { entries: [] });
      expect(fired).toEqual({ from: LOC, to: sub });
    } finally {
      PaneState.navigated = null;
    }
  });
});

describe("PaneState helpers", () => {
  it("title is the last path segment", async () => {
    const p = await pane([file("a.txt")], { fs: "local", path: "/dir/sub" });
    expect(p.title).toBe("sub");
  });

  it("moveCursor clamps to the visible range", async () => {
    const p = await pane([file("a.txt"), file("b.txt"), file("c.txt")]);
    expect(p.current?.name).toBe("a.txt");
    p.moveCursor(5);
    expect(p.current?.name).toBe("c.txt");
    p.moveCursor(-10);
    expect(p.current?.name).toBe("a.txt");
  });

  it("selectWhere marks and unmarks matching names", async () => {
    const p = await pane([file("a.jpg"), file("b.jpg"), file("c.txt")]);
    p.selectWhere((n) => n.endsWith(".jpg"), true);
    expect(p.selected.size).toBe(2);
    p.selectWhere((n) => n === "a.jpg", false);
    expect(p.selected.size).toBe(1);
  });

  it("setSort toggles direction for the same key", async () => {
    const p = await pane([file("b.txt"), file("a.txt")]);
    expect(p.visible.map((e) => e.name)).toEqual(["a.txt", "b.txt"]);
    p.setSort("name"); // already "name", so this toggles to descending
    expect(p.visible.map((e) => e.name)).toEqual(["b.txt", "a.txt"]);
    p.setSort("name"); // toggle back to ascending
    expect(p.visible.map((e) => e.name)).toEqual(["a.txt", "b.txt"]);
  });

  it("clone copies the view settings", async () => {
    const p = await pane([file("a.txt")]);
    p.showHidden = true;
    p.setSort("size");
    const c = p.clone();
    expect(c.showHidden).toBe(true);
    expect(c.sortKey).toBe("size");
  });

  it("up navigates to the parent", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/dir/": "", "/dir/sub/": "", "/dir/sub/x.txt": "x" }));
    const p = new PaneState();
    await p.open({ fs: "local", path: "/dir/sub" });
    await p.up();
    expect(p.location).toEqual({ fs: "local", path: "/dir" });
  });

  it("activate opens a folder and a file", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/dir/": "", "/dir/sub/": "", "/dir/a.txt": "a" }));
    const p = new PaneState();
    await p.open({ fs: "local", path: "/dir" });

    await p.activate(p.visible.find((e) => e.name === "sub")!);
    expect(p.location).toEqual({ fs: "local", path: "/dir/sub" });

    // A file is opened with its default app (open_file), not listed.
    await p.up();
    await p.activate(p.visible.find((e) => e.name === "a.txt")!);
    expect(mock.lastCall("open_file")).toEqual({ location: { fs: "local", path: "/dir/a.txt" }, editor: false });
  });
});

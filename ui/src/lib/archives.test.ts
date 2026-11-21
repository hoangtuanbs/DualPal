import { beforeEach, describe, expect, it } from "vitest";
import { PaneState } from "./pane-state.svelte";
import { archiveCrumbs, archiveStem, isArchiveName, openArchives, releaseArchives, setShownLocations } from "./archives.svelte";
import { defaultScenario, installMockBackend, tree, type MockHandle } from "../../tests/mock-backend";
import type { Location } from "./api";

const HOME: Location = { fs: "local", path: "/home/user" };

let mock: MockHandle;
let panes: PaneState[] = [];

beforeEach(() => {
  const s = defaultScenario();
  s.trees.local = tree({
    "/home/user/": "",
    "/home/user/a.txt": "a",
    "/home/user/pack.zip": "zip-bytes",
    "/home/user/broken.zip": "junk",
  });
  s.archives = {
    "/home/user/pack.zip": tree({ "/docs/": "", "/docs/inner.txt": "inside", "/top.txt": "t", "/nested.tar": "tar-bytes" }),
    "/nested.tar": tree({ "/deep.txt": "deep" }),
  };
  mock = installMockBackend(s);
  for (const id of [...openArchives.keys()]) openArchives.delete(id);
  panes = [];
  setShownLocations(() => panes.map((p) => p.location));
});

async function paneAt(loc: Location): Promise<PaneState> {
  const p = new PaneState();
  panes.push(p);
  await p.open(loc);
  return p;
}

const entry = (p: PaneState, name: string) => p.visible.find((e) => e.name === name)!;

describe("UI-UNIT-019 archive names", () => {
  it("knows which files open as folders", () => {
    for (const n of ["a.zip", "B.ZIP", "a.tar", "a.tar.gz", "a.tgz", "a.tar.bz2", "a.tbz2", "a.tar.xz", "a.txz", "a.7z"]) {
      expect(isArchiveName(n), n).toBe(true);
    }
    for (const n of ["a.gz", ".zip", "zip", "a.rar", "a.txt"]) expect(isArchiveName(n), n).toBe(false);
    expect(archiveStem("photos.tar.gz")).toBe("photos");
    expect(archiveStem("x.7z")).toBe("x");
  });
});

describe("UI-UNIT-020 entering and leaving archives", () => {
  it("Enter on an archive opens it as a folder; up at its top goes back onto it", async () => {
    const p = await paneAt(HOME);
    await p.activate(entry(p, "pack.zip"));
    expect(mock.lastCall("open_archive")).toEqual({ location: { fs: "local", path: "/home/user/pack.zip" }, request: expect.any(Number) });
    expect(mock.calls("open_file")).toHaveLength(0);
    expect(p.location?.fs).toMatch(/^archive:/);
    expect(p.location?.path).toBe("/");
    expect(p.inArchive).toBe(true);
    expect(p.visible.map((e) => e.name)).toEqual(["docs", "nested.tar", "top.txt"]);
    expect(p.title).toBe("pack.zip");

    await p.activate(entry(p, "docs"));
    expect(p.location?.path).toBe("/docs");
    await p.up();
    expect(p.location?.path).toBe("/");
    const fsId = p.location!.fs;
    await p.up();
    expect(p.location).toEqual(HOME);
    expect(p.current?.name).toBe("pack.zip");
    // Left behind: the archive is closed and dropped from the history.
    expect(mock.lastCall("close_archive")).toEqual({ fsId });
    expect(openArchives.size).toBe(0);
    expect(p.canBack).toBe(true);
    await p.back();
    expect(p.location).toEqual(HOME);
  });

  it("an archive another pane shows stays open", async () => {
    const left = await paneAt(HOME);
    await left.activate(entry(left, "pack.zip"));
    const right = await paneAt(left.location!);
    await left.up();
    expect(mock.calls("close_archive")).toHaveLength(0);
    right.location = HOME;
    releaseArchives();
    expect(mock.calls("close_archive")).toHaveLength(1);
  });

  it("an archive inside an archive returns into the outer one", async () => {
    const p = await paneAt(HOME);
    await p.activate(entry(p, "pack.zip"));
    const outer = p.location!.fs;
    await p.activate(entry(p, "nested.tar"));
    expect(p.visible.map((e) => e.name)).toEqual(["deep.txt"]);
    expect(openArchives.size).toBe(2);
    await p.up();
    expect(p.location).toEqual({ fs: outer, path: "/" });
    expect(p.current?.name).toBe("nested.tar");
    expect(openArchives.size).toBe(1);
  });

  it("a damaged archive shows an error and stays put", async () => {
    const p = await paneAt(HOME);
    await p.activate(entry(p, "broken.zip"));
    expect(p.location).toEqual(HOME);
    expect(p.error).toContain("damaged");
  });

  it("crumbs lead from the folder holding the archive", async () => {
    const p = await paneAt(HOME);
    await p.activate(entry(p, "pack.zip"));
    const fs = p.location!.fs;
    const outer = (l: Location) =>
      ["/", ...l.path.split("/").filter(Boolean)].map((name, i, all) => ({
        name,
        loc: { fs: l.fs, path: i === 0 ? "/" : "/" + all.slice(1, i + 1).join("/") },
      }));
    const crumbs = archiveCrumbs({ fs, path: "/docs" }, outer)!;
    expect(crumbs.map((c) => c.name)).toEqual(["/", "home", "user", "pack.zip", "docs"]);
    expect(crumbs[2].loc).toEqual({ fs: "local", path: "/home/user" });
    expect(crumbs[3].loc).toEqual({ fs, path: "/" });
    expect(archiveCrumbs(HOME, outer)).toBeNull();
  });
});

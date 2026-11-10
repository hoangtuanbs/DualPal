import { describe, expect, it } from "vitest";
import type { Connection, ImportReport } from "./api";
import {
  allSelected,
  chosen,
  describeEndpoint,
  importButtonLabel,
  passwordNote,
  summary,
  toggled,
} from "./import-selection";

const conn = (name: string, passwordMissing: boolean) => ({
  connection: {
    id: "",
    name,
    initialPath: null,
    favorites: [],
    endpoint: { type: "sftp", host: "example.com", port: 22, user: "alice", auth: { method: "password" } },
  } as Connection,
  secret: passwordMissing ? null : "pw",
  passwordMissing,
});

const report: ImportReport = {
  connections: [conn("A", false), conn("B", true), conn("C", false)],
  skipped: [{ name: "D", reason: "SCP sessions are not supported" }],
};

describe("IMP-UI-001 import selection", () => {
  it("starts with every importable server ticked", () => {
    expect([...allSelected(report)]).toEqual([0, 1, 2]);
  });

  it("toggles one entry without changing the previous selection", () => {
    const start = allSelected(report);
    const off = toggled(start, 1);
    expect([...off].sort()).toEqual([0, 2]);
    expect([...start]).toEqual([0, 1, 2]);
    expect([...toggled(off, 1)].sort()).toEqual([0, 1, 2]);
  });

  it("returns the chosen servers in the order the export listed them", () => {
    const picked = chosen(report, new Set([2, 0]));
    expect(picked.map((c) => c.connection.name)).toEqual(["A", "C"]);
    expect(chosen(report, new Set())).toEqual([]);
  });

  it("labels the import button by count", () => {
    expect(importButtonLabel(0)).toBe("Import");
    expect(importButtonLabel(1)).toBe("Import 1 server");
    expect(importButtonLabel(3)).toBe("Import 3 servers");
  });

  it("summarises the report, including skipped entries", () => {
    expect(summary(report)).toBe("3 servers can be imported, 1 skipped.");
    expect(summary({ connections: [], skipped: [] })).toBe("No servers can be imported.");
    expect(summary({ connections: [conn("A", false)], skipped: [] })).toBe("1 server can be imported.");
  });

  it("notes a server that came without a password", () => {
    expect(passwordNote(conn("A", false))).toBeNull();
    expect(passwordNote(conn("B", true))).toContain("No password in the export");
  });

  it("describes the endpoint of each kind", () => {
    expect(describeEndpoint(conn("A", false).connection)).toBe("SFTP · alice@example.com:22");
    const ftp = { ...conn("F", false).connection, endpoint: { type: "ftp", host: "f.example.com", port: 21, user: "", security: "explicit" } } as Connection;
    expect(describeEndpoint(ftp)).toBe("FTPS · anonymous@f.example.com:21");
    const dav = { ...conn("W", false).connection, endpoint: { type: "webDav", url: "https://dav.example.com/", user: "" } } as Connection;
    expect(describeEndpoint(dav)).toBe("WebDAV · https://dav.example.com/");
  });
});

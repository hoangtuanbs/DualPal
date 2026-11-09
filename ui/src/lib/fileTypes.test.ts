import { describe, expect, it } from "vitest";
import { mockConvertFileSrc, clearMocks } from "@tauri-apps/api/mocks";
import { category, extension, glyph, patternMatcher, thumbnailUrl } from "./fileTypes";
import type { Entry } from "./api";

const entry = (overrides: Partial<Entry>): Entry => ({
  name: "x.txt",
  path: "/x.txt",
  kind: "file",
  symlink: false,
  size: 0,
  modifiedMs: null,
  createdMs: null,
  hidden: false,
  readonly: false,
  mode: null,
  ...overrides,
});

describe("UI-UNIT-003 extension", () => {
  it("returns the last extension, lower-cased", () => {
    expect(extension(".bashrc")).toBe("");
    expect(extension("a.TAR.GZ")).toBe("gz");
    expect(extension("noext")).toBe("");
    expect(extension("file.")).toBe("");
  });
});

describe("UI-UNIT-004 category and glyph", () => {
  it("classifies folders, apps and known extensions", () => {
    expect(category(entry({ kind: "dir", name: "docs" }))).toBe("folder");
    expect(category(entry({ kind: "dir", name: "Photos.app" }))).toBe("app");
    expect(category(entry({ name: "pic.png" }))).toBe("image");
    expect(category(entry({ name: "movie.mp4" }))).toBe("video");
    expect(category(entry({ name: "song.mp3" }))).toBe("audio");
    expect(category(entry({ name: "doc.pdf" }))).toBe("pdf");
    expect(category(entry({ name: "x.zip" }))).toBe("archive");
    expect(category(entry({ name: "main.rs" }))).toBe("code");
    expect(category(entry({ name: "readme.md" }))).toBe("text");
    expect(category(entry({ name: "installer.exe" }))).toBe("app");
    expect(category(entry({ name: "unknown.xyz" }))).toBe("file");
  });

  it("maps a category to a glyph", () => {
    expect(glyph(entry({ kind: "dir", name: "d" }))).toBe("📁");
    expect(glyph(entry({ name: "a.png" }))).toBe("🖼️");
  });
});

describe("UI-UNIT-005 thumbnailUrl", () => {
  it("only returns a URL for local image files up to 50 MB", () => {
    mockConvertFileSrc("windows");
    try {
      const loc = { fs: "local", path: "/dir" };
      const img = entry({ name: "a.png", size: 1000 });
      expect(thumbnailUrl(loc, img)).toBeTruthy();

      // non-local, non-image, too large → null
      expect(thumbnailUrl(null, img)).toBeNull();
      expect(thumbnailUrl({ fs: "remote", path: "/dir" }, img)).toBeNull();
      expect(thumbnailUrl(loc, entry({ name: "a.txt" }))).toBeNull();
      expect(thumbnailUrl(loc, entry({ name: "a.png", size: 60_000_000 }))).toBeNull();
      expect(thumbnailUrl(loc, entry({ name: "a.png", kind: "dir" }))).toBeNull();
    } finally {
      clearMocks();
    }
  });
});

describe("UI-UNIT-006 patternMatcher", () => {
  it("matches wildcard lists, case-insensitively", () => {
    const m = patternMatcher("*.jpg; *.png");
    expect(m("A.JPG")).toBe(true);
    expect(m("photo.png")).toBe(true);
    expect(m("notes.txt")).toBe(false);
  });

  it("treats a ? as a single character", () => {
    const m = patternMatcher("file?.txt");
    expect(m("file1.txt")).toBe(true);
    expect(m("file12.txt")).toBe(false);
  });

  it("treats regex specials literally", () => {
    const m = patternMatcher("a+b.txt; [1].txt");
    expect(m("a+b.txt")).toBe(true);
    expect(m("ab.txt")).toBe(false);
    expect(m("[1].txt")).toBe(true);
  });

  it("an empty pattern matches nothing", () => {
    expect(patternMatcher("")("anything.txt")).toBe(false);
  });
});

import { describe, expect, it } from "vitest";
import { checksumAlgorithmOf, checksumText, defaultChecksumName, folderNameOf, sortVerifyEntries, withChecksumExtension } from "./checksum";

describe("checksum helpers", () => {
  it("reads the algorithm from a checksum extension", () => {
    expect(checksumAlgorithmOf("SUMS.MD5")).toBe("md5");
    expect(checksumAlgorithmOf("a.sha256")).toBe("sha256");
    expect(checksumAlgorithmOf("photo.jpg")).toBeNull();
    expect(checksumAlgorithmOf("md5")).toBeNull();
    expect(checksumAlgorithmOf(".md5")).toBeNull();
  });

  it("replaces or adds the extension of the chosen algorithm", () => {
    expect(withChecksumExtension("photo.jpg", "sha1")).toBe("photo.jpg.sha1");
    expect(withChecksumExtension("SUMS.md5", "xxh128")).toBe("SUMS.xxh128");
    expect(defaultChecksumName(["a.txt"], "Docs", "md5")).toBe("a.txt.md5");
    expect(defaultChecksumName(["a.txt", "b"], "Docs", "md5")).toBe("Docs.md5");
    expect(defaultChecksumName(["a.txt", "b"], "C:", "sha256")).toBe("Checksums.sha256");
    expect(defaultChecksumName(["a", "b"], "", "md5")).toBe("Checksums.md5");
    expect(folderNameOf("/home/u/Docs/")).toBe("Docs");
    expect(folderNameOf("C:\\Users\\u")).toBe("u");
  });

  it("writes the GNU text layout, leaving out entries without a hash", () => {
    expect(
      checksumText([
        { path: "a.txt", hash: "aa", error: null },
        { path: "x", hash: null, error: "denied" },
        { path: "sub/b c", hash: "bb", error: null },
      ]),
    ).toBe("aa  a.txt\nbb  sub/b c\n");
  });

  it("escapes names with backslashes, newlines and carriage returns", () => {
    expect(
      checksumText([
        { path: "a\r", hash: "aa", error: null },
        { path: "b\nc\\d", hash: "bb", error: null },
      ]),
    ).toBe("\\aa  a\\r\n\\bb  b\\nc\\\\d\n");
  });

  it("puts failures first, then missing files, then matches", () => {
    const entry = (path: string, status: "ok" | "failed" | "missing") => ({ path, status, expected: "", actual: null, error: null });
    const sorted = sortVerifyEntries([entry("c", "ok"), entry("b", "missing"), entry("z", "failed"), entry("a", "failed")]);
    expect(sorted.map((e) => e.path)).toEqual(["a", "z", "b", "c"]);
  });
});

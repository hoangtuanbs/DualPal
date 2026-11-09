import { describe, expect, it } from "vitest";
import { ABOUT, emailSupport, openProjectPage, osName, supportMailUrl } from "./about";
import { installMockBackend } from "../../tests/mock-backend";

describe("UI-UNIT-016 about constants", () => {
  it("has the README donate address", () => {
    expect(ABOUT.ethereumAddress).toBe("0xF8352b493e7750C03bde4bdA94A203B010933735");
  });

  it("builds a support mailto with version and OS", () => {
    const url = supportMailUrl({ version: "0.1.0", os: "windows", arch: "x86_64" });
    expect(url.startsWith("mailto:hoangtuanbs@proton.me?")).toBe(true);
    expect(url).toContain(encodeURIComponent("Dual Pal 0.1.0 support"));
  });

  it("names the OS for humans", () => {
    expect(osName({ version: "0.1.0", os: "macos", arch: "aarch64" })).toBe("macOS (aarch64)");
    expect(osName({ version: "0.1.0", os: "windows", arch: "x86_64" })).toBe("Windows (x86_64)");
  });

  it("opens the project page and email via the API", async () => {
    const mock = installMockBackend();
    await openProjectPage();
    expect(mock.lastCall("open_url")).toEqual({ url: ABOUT.projectUrl });

    await emailSupport();
    expect(String(mock.lastCall("open_url")?.url)).toContain("mailto:hoangtuanbs@proton.me?");
  });
});

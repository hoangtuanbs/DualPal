import { describe, expect, it } from "vitest";
// The release script is plain Node with no dependencies.
import {
  assetName,
  buildManifest,
  pairSignatures,
  platformKeysFor,
  updaterBuildConfig,
} from "../../scripts/updater-manifest.mjs";

const RELEASE = "https://github.com/hoangtuanbs/DualPal/releases/download/v0.2.0";

describe("updater manifest", () => {
  it("maps each updater file to its Tauri platform keys", () => {
    expect(platformKeysFor("Dual Pal.app.tar.gz")).toEqual(["darwin-aarch64", "darwin-x86_64"]);
    expect(platformKeysFor("dualpal_0.2.0_amd64.AppImage")).toEqual(["linux-x86_64"]);
    expect(platformKeysFor("Dual Pal_0.2.0_x64-setup.exe")).toEqual(["windows-x86_64"]);
    expect(platformKeysFor("Dual Pal_0.2.0_x64_en-US.msi")).toEqual([]);
    expect(platformKeysFor("dualpal.deb")).toEqual([]);
  });

  it("turns spaces in file names into dots, as GitHub stores them", () => {
    expect(assetName("Dual Pal.app.tar.gz")).toBe("Dual.Pal.app.tar.gz");
  });

  it("pairs only the updater files that have a .sig next to them", () => {
    expect(
      pairSignatures(["Dual Pal.app.tar.gz", "Dual Pal.app.tar.gz.sig", "Dual Pal.dmg", "Dual Pal.dmg.sig.bak"]),
    ).toEqual(["Dual Pal.app.tar.gz"]);
  });

  it("assembles the static JSON the Tauri updater reads", () => {
    const manifest = buildManifest({
      version: "0.2.0",
      notes: "Fixes",
      pubDate: "2026-11-10T12:00:00.000Z",
      releaseUrl: RELEASE,
      entries: [
        { fileName: "Dual Pal.app.tar.gz", signature: "sig-mac\n" },
        { fileName: "dualpal_0.2.0_amd64.AppImage", signature: "sig-linux" },
        { fileName: "Dual Pal_0.2.0_x64-setup.exe", signature: "sig-win" },
      ],
    });

    expect(manifest).toEqual({
      version: "0.2.0",
      notes: "Fixes",
      pub_date: "2026-11-10T12:00:00.000Z",
      platforms: {
        "darwin-aarch64": { signature: "sig-mac", url: `${RELEASE}/Dual.Pal.app.tar.gz` },
        "darwin-x86_64": { signature: "sig-mac", url: `${RELEASE}/Dual.Pal.app.tar.gz` },
        "linux-x86_64": { signature: "sig-linux", url: `${RELEASE}/dualpal_0.2.0_amd64.AppImage` },
        "windows-x86_64": { signature: "sig-win", url: `${RELEASE}/Dual.Pal_0.2.0_x64-setup.exe` },
      },
    });
  });

  it("refuses two updater files for the same platform", () => {
    expect(() =>
      buildManifest({
        version: "0.2.0",
        pubDate: "2026-11-10T12:00:00.000Z",
        releaseUrl: RELEASE,
        entries: [
          { fileName: "a-setup.exe", signature: "x" },
          { fileName: "b-setup.exe", signature: "y" },
        ],
      }),
    ).toThrow(/two updater files for windows-x86_64/);
  });

  it("refuses a release with no updater artifacts", () => {
    expect(() => buildManifest({ version: "0.2.0", pubDate: "x", releaseUrl: RELEASE, entries: [] })).toThrow(
      /no updater artifacts/,
    );
  });
});

describe("updater build config", () => {
  it("turns on updater artifacts with the public key", () => {
    expect(updaterBuildConfig(" RWQabc \n")).toEqual({
      bundle: { createUpdaterArtifacts: true },
      plugins: { updater: { pubkey: "RWQabc" } },
    });
  });

  it("refuses to build without a public key", () => {
    expect(() => updaterBuildConfig("")).toThrow(/TAURI_UPDATER_PUBKEY is empty/);
    expect(() => updaterBuildConfig(undefined)).toThrow(/TAURI_UPDATER_PUBKEY is empty/);
  });
});

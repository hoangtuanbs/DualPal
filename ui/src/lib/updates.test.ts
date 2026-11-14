import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Update } from "@tauri-apps/plugin-updater";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { installMockBackend, type MockHandle } from "../../tests/mock-backend";
import { ABOUT } from "./about";
import {
  activeOperationCount,
  compareVersions,
  isCheckDue,
  isNewerVersion,
  progressPercent,
  releaseNotesUrl,
  UPDATE_CHECK_INTERVAL_MS,
} from "./updates";
import { updates } from "./updates.svelte";

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: vi.fn(async () => {}) }));

const DAY = UPDATE_CHECK_INTERVAL_MS;
const NOW = Date.UTC(2026, 10, 10, 12);

/** A fake update whose download reports 100 bytes in two chunks. */
function fakeUpdate(version = "0.2.0") {
  return {
    version,
    currentVersion: "0.1.0",
    download: vi.fn(async (onEvent?: (e: unknown) => void) => {
      onEvent?.({ event: "Started", data: { contentLength: 100 } });
      onEvent?.({ event: "Progress", data: { chunkLength: 40 } });
      onEvent?.({ event: "Progress", data: { chunkLength: 60 } });
      onEvent?.({ event: "Finished" });
    }),
    install: vi.fn(async () => {}),
  };
}

describe("updates rules", () => {
  it("checks when nothing was recorded, when the record is in the future, or a day has passed", () => {
    expect(isCheckDue(null, NOW)).toBe(true);
    expect(isCheckDue(undefined, NOW)).toBe(true);
    expect(isCheckDue(NOW + 1000, NOW)).toBe(true);
    expect(isCheckDue(NOW - DAY, NOW)).toBe(true);
    expect(isCheckDue(NOW - DAY + 1, NOW)).toBe(false);
    expect(isCheckDue(NOW - 60_000, NOW)).toBe(false);
  });

  it("links the release notes to the version tag", () => {
    expect(releaseNotesUrl("0.2.0")).toBe(`${ABOUT.projectUrl}/releases/tag/v0.2.0`);
  });

  it("gives download progress in whole percent, or null without a size", () => {
    expect(progressPercent(0, 100)).toBe(0);
    expect(progressPercent(33, 100)).toBe(33);
    expect(progressPercent(250, 100)).toBe(100);
    expect(progressPercent(10, null)).toBeNull();
    expect(progressPercent(10, 0)).toBeNull();
  });

  it("compares versions the semver way", () => {
    expect(compareVersions("0.2.0", "0.1.0")).toBe(1);
    expect(compareVersions("v0.2.0", "0.2.0")).toBe(0);
    expect(compareVersions("0.10.0", "0.9.9")).toBe(1);
    expect(compareVersions("1.0", "1.0.0")).toBe(0);
    expect(compareVersions("1.0.0-beta.2", "1.0.0-beta.10")).toBe(-1);
    expect(compareVersions("1.0.0-alpha", "1.0.0-alpha.1")).toBe(-1);
    expect(compareVersions("1.0.0-rc.1", "1.0.0")).toBe(-1);
    expect(compareVersions("1.0.0", "1.0.0-rc.1")).toBe(1);
    expect(compareVersions("1.0.0+build.5", "1.0.0")).toBe(0);
    expect(isNewerVersion("0.3.0", "0.2.0")).toBe(true);
    expect(isNewerVersion("0.2.0", "0.2.0")).toBe(false);
    expect(isNewerVersion("0.1.9", "0.2.0")).toBe(false);
    // Text that is not a version never announces anything.
    expect(isNewerVersion("nightly", "0.2.0")).toBe(false);
  });

  it("counts queued and running operations only", () => {
    expect(activeOperationCount([{ status: "queued" }, { status: "running" }, { status: "completed" }, { status: "failed" }])).toBe(2);
    expect(activeOperationCount([])).toBe(0);
  });
});

describe("updates store", () => {
  let mock: MockHandle;

  beforeEach(() => {
    mock = installMockBackend();
    vi.mocked(check).mockReset();
    vi.mocked(relaunch).mockClear();
    updates.setBusyCheck(() => 0);
    // Leave no offer from an earlier test on screen.
    updates.dismiss();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("does not check at start-up when the last check is recent", async () => {
    mock.setSettings({ editor: null, diffTool: null, lastUpdateCheck: NOW - 60_000 });
    await updates.checkAutomatically(NOW);
    expect(check).not.toHaveBeenCalled();
    expect(mock.lastCall("save_settings")).toBeUndefined();
  });

  it("records the check time and offers a release found at start-up", async () => {
    mock.setSettings({ editor: "/bin/ed", diffTool: null, lastUpdateCheck: NOW - DAY - 1 });
    const update = fakeUpdate();
    vi.mocked(check).mockResolvedValue(update as unknown as Update);

    await updates.checkAutomatically(NOW);

    expect(mock.lastCall("save_settings")).toEqual({
      settings: { editor: "/bin/ed", diffTool: null, lastUpdateCheck: NOW },
    });
    expect(updates.phase).toBe("available");
    expect(updates.update?.version).toBe("0.2.0");
  });

  it("stays silent and logs when the start-up check fails", async () => {
    mock.setSettings({ editor: null, diffTool: null, lastUpdateCheck: null });
    vi.mocked(check).mockRejectedValue(new Error("no network"));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const before = { phase: updates.phase, message: updates.message };

    await expect(updates.checkAutomatically(NOW)).resolves.toBeUndefined();

    expect(warn).toHaveBeenCalled();
    // No error is shown for a start-up check: the state stays as it was.
    expect({ phase: updates.phase, message: updates.message }).toEqual(before);
    expect(updates.phase).not.toBe("error");
  });

  it("reports up to date for a manual check when nothing is newer", async () => {
    vi.mocked(check).mockResolvedValue(null);
    await expect(updates.checkNow()).resolves.toEqual({ kind: "upToDate" });
    expect(updates.phase).toBe("upToDate");
  });

  it("throws from a manual check that fails, with GitHub's reason in the message", async () => {
    vi.mocked(check).mockRejectedValue(new Error("404 Not Found"));
    await expect(updates.checkNow()).rejects.toMatchObject({ message: expect.stringContaining("offline") });
    expect(updates.phase).toBe("error");
    expect(updates.message).toContain("Could not check for updates");
    expect(updates.message).toContain("offline");
  });

  it("returns the version of a release found by a manual check", async () => {
    vi.mocked(check).mockResolvedValue(fakeUpdate("1.0.0") as unknown as Update);
    await expect(updates.checkNow()).resolves.toEqual({ kind: "available", version: "1.0.0", installable: true });
    expect(updates.dismissed).toBe(false);
  });

  it("waits for running file operations until the person confirms", async () => {
    const update = fakeUpdate();
    vi.mocked(check).mockResolvedValue(update as unknown as Update);
    await updates.checkNow();
    updates.setBusyCheck(() => 1);

    await updates.install();

    expect(update.download).not.toHaveBeenCalled();
    expect(update.install).not.toHaveBeenCalled();
    expect(updates.waitingForOperations).toBe(true);
    expect(updates.phase).toBe("available");

    updates.keepWaiting();
    expect(updates.waitingForOperations).toBe(false);
  });

  it("leaves a running install alone when another check is asked for", async () => {
    const update = fakeUpdate();
    let finish = () => {};
    update.download.mockImplementation(() => new Promise<void>((resolve) => (finish = resolve)));
    vi.mocked(check).mockResolvedValue(update as unknown as Update);
    await updates.checkNow();
    updates.setBusyCheck(() => 0);
    const installing = updates.install();
    vi.mocked(check).mockClear();

    expect(await updates.checkNow()).toEqual({ kind: "available", version: "0.2.0", installable: true });
    expect(check).not.toHaveBeenCalled();
    expect(updates.phase).toBe("downloading");

    finish();
    await installing;
    expect(update.install).toHaveBeenCalledOnce();
  });

  it("restarts only after work that began during the install has finished", async () => {
    vi.useFakeTimers();
    try {
      const update = fakeUpdate();
      let busy = 0;
      update.install.mockImplementation(async () => {
        busy = 1;
      });
      vi.mocked(check).mockResolvedValue(update as unknown as Update);
      await updates.checkNow();
      updates.setBusyCheck(() => busy);

      const installing = updates.install();
      await vi.advanceTimersByTimeAsync(2000);
      expect(relaunch).not.toHaveBeenCalled();

      busy = 0;
      await vi.advanceTimersByTimeAsync(600);
      await installing;
      expect(relaunch).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it("downloads with progress, installs and relaunches when no operation is running", async () => {
    const update = fakeUpdate();
    vi.mocked(check).mockResolvedValue(update as unknown as Update);
    await updates.checkNow();
    const progress: (number | null)[] = [];
    // The download callbacks feed the progress the bar shows.
    update.download.mockImplementationOnce(async (onEvent?: (e: unknown) => void) => {
      onEvent?.({ event: "Started", data: { contentLength: 100 } });
      onEvent?.({ event: "Progress", data: { chunkLength: 40 } });
      progress.push(updates.progress);
      onEvent?.({ event: "Progress", data: { chunkLength: 60 } });
      onEvent?.({ event: "Finished" });
    });

    await updates.install();

    expect(progress).toEqual([40]);
    expect(update.download).toHaveBeenCalledOnce();
    expect(update.install).toHaveBeenCalledOnce();
    expect(relaunch).toHaveBeenCalledOnce();
    expect(updates.progress).toBe(100);
  });

  it("installs anyway when the person confirms while operations run", async () => {
    const update = fakeUpdate();
    vi.mocked(check).mockResolvedValue(update as unknown as Update);
    await updates.checkNow();
    updates.setBusyCheck(() => 3);

    await updates.install(true);

    expect(update.install).toHaveBeenCalledOnce();
    expect(relaunch).toHaveBeenCalledOnce();
  });

  it("does not download again when a retry follows a failed install", async () => {
    const update = fakeUpdate();
    update.install.mockRejectedValueOnce(new Error("disk full"));
    vi.mocked(check).mockResolvedValue(update as unknown as Update);
    await updates.checkNow();

    await updates.install();
    expect(updates.phase).toBe("error");
    expect(updates.message).toContain("disk full");

    await updates.install();
    expect(update.download).toHaveBeenCalledOnce();
    expect(update.install).toHaveBeenCalledTimes(2);
    expect(relaunch).toHaveBeenCalledOnce();
  });

  it("hides the offer after Later and shows it again on the next check", async () => {
    vi.mocked(check).mockResolvedValue(fakeUpdate() as unknown as Update);
    await updates.checkNow();
    updates.dismiss();
    expect(updates.dismissed).toBe(true);

    vi.mocked(check).mockResolvedValue(fakeUpdate() as unknown as Update);
    await updates.checkNow();
    expect(updates.dismissed).toBe(false);
  });
});

describe("release notice from GitHub", () => {
  let mock: MockHandle;
  const RELEASE = {
    version: "0.3.0",
    tag: "v0.3.0",
    name: "Dual Pal 0.3.0",
    url: "https://github.com/hoangtuanbs/DualPal/releases/tag/v0.3.0",
    publishedAt: "2026-11-13T10:00:00Z",
  };

  beforeEach(() => {
    mock = installMockBackend();
    vi.mocked(check).mockReset();
    // No signed latest.json: the updater plugin cannot install anything.
    vi.mocked(check).mockRejectedValue(new Error("Could not fetch a valid release JSON from the remote"));
    updates.setBusyCheck(() => 0);
    updates.dismiss();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("offers the release page when GitHub has a newer release the updater cannot install", async () => {
    mock.setLatestRelease(RELEASE);

    await expect(updates.checkNow()).resolves.toEqual({ kind: "available", version: "0.3.0", installable: false });

    expect(updates.phase).toBe("available");
    expect(updates.version).toBe("0.3.0");
    expect(updates.currentVersion).toBe("0.1.0");
    expect(updates.update).toBeNull();
    await updates.openReleasePage();
    expect(mock.lastCall("open_url")).toEqual({ url: RELEASE.url });
  });

  it("offers an in-app install when the updater has the same release", async () => {
    mock.setLatestRelease(RELEASE);
    vi.mocked(check).mockResolvedValue(fakeUpdate("0.3.0") as unknown as Update);

    await expect(updates.checkNow()).resolves.toEqual({ kind: "available", version: "0.3.0", installable: true });
    expect(updates.update?.version).toBe("0.3.0");
  });

  it("is up to date when GitHub's latest release is this version, without asking the updater", async () => {
    mock.setLatestRelease({ ...RELEASE, version: "0.1.0", tag: "v0.1.0" });

    await expect(updates.checkNow()).resolves.toEqual({ kind: "upToDate" });
    expect(check).not.toHaveBeenCalled();
  });

  it("does not check by itself when automatic checks are off", async () => {
    mock.setLatestRelease(RELEASE);
    mock.setSettings({ editor: null, diffTool: null, lastUpdateCheck: null, updateChecks: false });

    await updates.checkAutomatically(NOW);

    expect(updates.autoCheck).toBe(false);
    expect(mock.calls("latest_release")).toHaveLength(0);
    expect(mock.lastCall("save_settings")).toBeUndefined();
  });

  it("announces a newer release found by the automatic check", async () => {
    mock.setLatestRelease(RELEASE);
    mock.setSettings({ editor: null, diffTool: null, lastUpdateCheck: null });

    await updates.checkAutomatically(NOW);

    expect(updates.phase).toBe("available");
    expect(updates.dismissed).toBe(false);
    expect(updates.version).toBe("0.3.0");
  });

  it("stays quiet about a skipped release, but a manual check still shows it", async () => {
    mock.setLatestRelease(RELEASE);
    mock.setSettings({ editor: null, diffTool: null, lastUpdateCheck: null });
    await updates.checkNow();

    await updates.skip();
    expect(updates.dismissed).toBe(true);
    expect(mock.lastCall("save_settings")).toEqual({
      settings: { editor: null, diffTool: null, lastUpdateCheck: null, skippedVersion: "0.3.0" },
    });

    await updates.checkAutomatically(NOW);
    expect(mock.calls("latest_release")).toHaveLength(2);
    expect(updates.dismissed).toBe(true);

    // A newer release than the skipped one is announced again.
    mock.setLatestRelease({ ...RELEASE, version: "0.3.1", tag: "v0.3.1" });
    await updates.checkAutomatically(NOW + UPDATE_CHECK_INTERVAL_MS);
    expect(updates.dismissed).toBe(false);
    expect(updates.version).toBe("0.3.1");

    updates.dismiss();
    mock.setLatestRelease(RELEASE);
    await expect(updates.checkNow()).resolves.toMatchObject({ kind: "available", version: "0.3.0" });
    expect(updates.dismissed).toBe(false);
  });

  it("saves the automatic check switch", async () => {
    mock.setSettings({ editor: "/bin/ed", diffTool: null });

    await updates.setAutoCheck(false);
    expect(updates.autoCheck).toBe(false);
    expect(mock.lastCall("save_settings")).toEqual({ settings: { editor: "/bin/ed", diffTool: null, updateChecks: false } });

    await updates.setAutoCheck(true);
    expect(updates.autoCheck).toBe(true);
  });
});

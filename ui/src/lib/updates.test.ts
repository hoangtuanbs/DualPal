import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Update } from "@tauri-apps/plugin-updater";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { installMockBackend, type MockHandle } from "../../tests/mock-backend";
import { ABOUT } from "./about";
import { activeOperationCount, isCheckDue, progressPercent, releaseNotesUrl, UPDATE_CHECK_INTERVAL_MS } from "./updates";
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
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("does not check at start-up when the last check is recent", async () => {
    mock.setSettings({ editor: null, diffTool: null, lastUpdateCheck: NOW - 60_000 });
    await updates.checkOnStartup(NOW);
    expect(check).not.toHaveBeenCalled();
    expect(mock.lastCall("save_settings")).toBeUndefined();
  });

  it("records the check time and offers a release found at start-up", async () => {
    mock.setSettings({ editor: "/bin/ed", diffTool: null, lastUpdateCheck: NOW - DAY - 1 });
    const update = fakeUpdate();
    vi.mocked(check).mockResolvedValue(update as unknown as Update);

    await updates.checkOnStartup(NOW);

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

    await expect(updates.checkOnStartup(NOW)).resolves.toBeUndefined();

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

  it("throws from a manual check that fails, with the reason in the message", async () => {
    vi.mocked(check).mockRejectedValue(new Error("404 Not Found"));
    await expect(updates.checkNow()).rejects.toThrow("404");
    expect(updates.phase).toBe("error");
    expect(updates.message).toContain("Could not check for updates");
    expect(updates.message).toContain("404");
  });

  it("returns the version of a release found by a manual check", async () => {
    vi.mocked(check).mockResolvedValue(fakeUpdate("1.0.0") as unknown as Update);
    await expect(updates.checkNow()).resolves.toEqual({ kind: "available", version: "1.0.0" });
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

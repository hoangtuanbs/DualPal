import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import OperationsPanel from "./OperationsPanel.svelte";
import type { ActivityRecord, OperationInfo } from "./api";

function op(id: string, status: OperationInfo["status"], extra: Partial<OperationInfo> = {}): OperationInfo {
  return {
    id,
    kind: "copy",
    summary: `Copy ${id}`,
    status,
    progress: { filesTotal: 4, filesDone: 1, bytesTotal: 200, bytesDone: 50, skipped: 0, current: "/a/x.bin" },
    error: null,
    queuedMs: 1,
    startedMs: 1,
    finishedMs: null,
    places: ["local"],
    ...extra,
  };
}

const record: ActivityRecord = {
  id: "old",
  kind: "trash",
  summary: "Trash old.txt",
  status: "completed",
  startedMs: 1_700_000_000_000,
  finishedMs: 1_700_000_001_000,
  filesDone: 1,
  bytesDone: 10,
  skipped: 0,
  error: null,
  places: ["local"],
};

function setup(operations: OperationInfo[], activity: ActivityRecord[] = []) {
  const props = {
    operations,
    activity,
    oncancel: vi.fn(),
    onretry: vi.fn(),
    onclear: vi.fn(),
    onclearactivity: vi.fn(),
    oninsights: vi.fn(),
  };
  render(OperationsPanel, { props });
  return props;
}

describe("UI-CMP-008 OperationsPanel", () => {
  it("shows statuses, a progress bar at the right percentage and the details", () => {
    setup([op("run", "running"), op("done", "completed", { finishedMs: 2 })]);
    expect(screen.getByText("Copy run")).toBeInTheDocument();
    expect(screen.getByText("Copy done")).toBeInTheDocument();
    const bars = document.querySelectorAll<HTMLElement>(".bar > div");
    expect(bars).toHaveLength(1); // only the running one
    expect(bars[0].style.width).toBe("25%");
    expect(screen.getAllByText(/1 of 4 items/).length).toBeGreaterThan(0);
  });

  it("cancels a running operation and clears finished ones", async () => {
    const user = userEvent.setup();
    const p = setup([op("run", "running"), op("done", "completed", { finishedMs: 2 })]);
    const cancels = screen.getAllByRole("button", { name: "Cancel" });
    expect(cancels).toHaveLength(1);
    await user.click(cancels[0]);
    expect(p.oncancel).toHaveBeenCalledWith("run");
    await user.click(screen.getByRole("button", { name: "Clear finished" }));
    expect(p.onclear).toHaveBeenCalled();
  });

  it("disables Clear finished when nothing has finished", () => {
    setup([op("run", "running")]);
    expect(screen.getByRole("button", { name: "Clear finished" })).toBeDisabled();
  });

  it("offers Replace and Skip for a copy that hit an existing file", async () => {
    const user = userEvent.setup();
    const p = setup([
      op("clash", "failed", { error: { kind: "alreadyExists", message: "already exists: /b/x" }, finishedMs: 2 }),
      op("other", "failed", { error: { kind: "permissionDenied", message: "permission denied: /c" }, finishedMs: 2 }),
    ]);
    expect(screen.getByText("already exists: /b/x")).toBeInTheDocument();
    expect(screen.getByText("permission denied: /c")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Replace existing" })).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Replace existing" }));
    expect(p.onretry).toHaveBeenCalledWith("clash", "overwrite");
    await user.click(screen.getByRole("button", { name: "Skip existing" }));
    expect(p.onretry).toHaveBeenCalledWith("clash", "skip");
  });

  it("lists finished work on the Activity tab", async () => {
    const user = userEvent.setup();
    const p = setup([], [record]);
    expect(screen.getByText(/No operations yet/)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Activity" }));
    expect(screen.getByText("Trash old.txt")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Insights…" }));
    expect(p.oninsights).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Clear history" }));
    expect(p.onclearactivity).toHaveBeenCalled();
  });
});

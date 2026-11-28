import { describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import OperationsPanel from "./OperationsPanel.svelte";
import { failureText, formatRate } from "./transfers";
import type { ActivityRecord, FailedItem, OperationInfo } from "./api";
import { fsNames } from "./fs-names.svelte";

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

describe("transfer queue controls", () => {
  function controls(operations: OperationInfo[], extra: Record<string, unknown> = {}) {
    const props = {
      operations,
      activity: [],
      oncancel: vi.fn(),
      onretry: vi.fn(),
      onclear: vi.fn(),
      onclearactivity: vi.fn(),
      oninsights: vi.fn(),
      onpause: vi.fn(),
      onresume: vi.fn(),
      onpauseall: vi.fn(),
      onlimit: vi.fn(),
      onoplimit: vi.fn(),
      ondefaults: vi.fn(),
      loadfailures: vi.fn(async (): Promise<FailedItem[]> => [
        { source: { fs: "local", path: "/a/x.txt" }, target: { fs: "local", path: "/b/x.txt" }, error: { kind: "alreadyExists", message: "already exists: /b/x.txt" } },
        { source: { fs: "local", path: "/a/y.txt" }, target: { fs: "local", path: "/b/y.txt" }, error: { kind: "permissionDenied", message: "permission denied: /a/y.txt" } },
      ]),
      onsavefailures: vi.fn(),
      onretryfailed: vi.fn(),
      ...extra,
    };
    render(OperationsPanel, { props });
    return props;
  }

  it("UI-UNIT-060 pauses running copies, resumes paused ones and discards restored ones", async () => {
    const user = userEvent.setup();
    const p = controls([
      op("run", "running"),
      op("held", "paused"),
      op("old", "paused", { restored: true }),
      op("del", "running", { kind: "delete", summary: "Delete del" }),
    ]);
    // Only copies and moves can be paused.
    const pauses = screen.getAllByRole("button", { name: "Pause" });
    expect(pauses).toHaveLength(1);
    await user.click(pauses[0]);
    expect(p.onpause).toHaveBeenCalledWith("run");
    const resumes = screen.getAllByRole("button", { name: "Resume" });
    expect(resumes).toHaveLength(2);
    await user.click(resumes[0]);
    expect(p.onresume).toHaveBeenCalledWith("old");
    expect(screen.getAllByText("Paused")).toHaveLength(2);
    expect(screen.getByText("Not finished when Dual Pal closed")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Discard" }));
    expect(p.oncancel).toHaveBeenCalledWith("old");
    // A paused copy keeps its progress bar.
    expect(document.querySelectorAll(".bar")).toHaveLength(4);
  });

  it("UI-UNIT-061 pauses and resumes the whole queue; queued items show as paused", async () => {
    const user = userEvent.setup();
    const p = controls([op("wait", "queued")]);
    await user.click(screen.getByRole("button", { name: "Pause all" }));
    expect(p.onpauseall).toHaveBeenCalledWith(true);
    cleanup();
    const q = controls([op("wait", "queued")], { queuePaused: true });
    expect(screen.getByText("Paused")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Resume all" }));
    expect(q.onpauseall).toHaveBeenCalledWith(false);
  });

  it("UI-UNIT-062 sets the global bandwidth limit in KB/s or MB/s", async () => {
    const user = userEvent.setup();
    const p = controls([]);
    const field = screen.getByLabelText("Bandwidth limit");
    expect(field).toHaveValue(null);
    await user.type(field, "500");
    await user.tab();
    expect(p.onlimit).toHaveBeenLastCalledWith(500 * 1024);
    await user.selectOptions(screen.getByLabelText("Limit unit"), "MB/s");
    expect(p.onlimit).toHaveBeenLastCalledWith(500 * 1024 * 1024);
    await user.clear(field);
    await user.tab();
    expect(p.onlimit).toHaveBeenLastCalledWith(0);
    cleanup();
    controls([], { bandwidthLimit: 2 * 1024 * 1024 });
    expect(screen.getByLabelText("Bandwidth limit")).toHaveValue(2);
    expect(screen.getByLabelText("Limit unit")).toHaveValue("MB/s");
  });

  it("UI-UNIT-063 sets the defaults for new copies", async () => {
    const user = userEvent.setup();
    const p = controls([], { defaults: { verify: false, keepGoing: true } });
    expect(screen.getByRole("checkbox", { name: "Don't stop on errors" })).toBeChecked();
    await user.click(screen.getByRole("checkbox", { name: "Verify copies" }));
    expect(p.ondefaults).toHaveBeenCalledWith({ verify: true, keepGoing: true });
  });

  it("UI-UNIT-064 lists the errors of an unattended copy; copy, save and retry them", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const p = controls([op("bulk", "completed", { failed: 2, finishedMs: 2, options: { keepGoing: true } })]);
    expect(screen.getByText("Done with 2 errors")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show errors" }));
    expect(p.loadfailures).toHaveBeenCalledWith("bulk");
    expect(await screen.findByText("/a/x.txt")).toBeInTheDocument();
    expect(screen.getByText("permission denied: /a/y.txt")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Copy list" }));
    expect(writeText).toHaveBeenCalledWith("/a/x.txt: already exists: /b/x.txt\n/a/y.txt: permission denied: /a/y.txt\n");
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Save list…" }));
    expect(p.onsavefailures).toHaveBeenCalledWith("bulk");
    await user.click(screen.getByRole("button", { name: "Retry failed" }));
    expect(p.onretryfailed).toHaveBeenCalledWith("bulk");
    await user.click(screen.getByRole("button", { name: "Hide errors" }));
    expect(screen.queryByTestId("ops-failures")).not.toBeInTheDocument();
  });

  it("UI-UNIT-065 shows the server a restored operation waits for", () => {
    fsNames["sftp-1"] = "My NAS";
    controls([op("w", "queued", { waitingFor: "sftp-1", restored: true }), op("x", "paused", { waitingFor: "gone" })]);
    expect(screen.getByText("Waiting for My NAS")).toBeInTheDocument();
    expect(screen.getByText("Waiting for gone")).toBeInTheDocument();
    delete fsNames["sftp-1"];
  });

  it("UI-UNIT-066 a running copy can get its own limit; its details say so", async () => {
    const user = userEvent.setup();
    const p = controls([
      op("lim", "running", { options: { limit: 512 * 1024, verify: true } }),
      op("free", "running", { options: { limit: 0 } }),
    ]);
    expect(screen.getByText(/limit 512 KB\/s · checked/)).toBeInTheDocument();
    expect(screen.getByText(/no limit/)).toBeInTheDocument();
    await user.click(screen.getAllByRole("button", { name: "Limit…" })[1]);
    expect(p.onoplimit).toHaveBeenCalledWith("lim");
  });

  it("UI-UNIT-067 failure text and rate formatting", () => {
    expect(failureText([])).toBe("");
    expect(
      failureText([{ source: { fs: "s", path: "/p" }, target: { fs: "d", path: "/q" }, error: { kind: "io", message: "boom" } }]),
    ).toBe("/p: boom\n");
    expect(formatRate(512 * 1024)).toBe("512 KB/s");
    expect(formatRate(1536)).toBe("1.5 KB/s");
    expect(formatRate(3 * 1024 * 1024)).toBe("3 MB/s");
    expect(formatRate(1.5 * 1024 * 1024)).toBe("1.5 MB/s");
  });
});

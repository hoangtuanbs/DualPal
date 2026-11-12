import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import CompareView from "./CompareView.svelte";
import type { DiffItem, SyncPreview, SyncStep } from "./api";

const side = (size: number, ms = 1_700_000_000_000) => ({ dir: false, size, modifiedMs: ms });

function item(rel: string, state: DiffItem["state"], l: number | null, r: number | null): DiffItem {
  return { rel, name: rel, depth: 0, dir: false, left: l === null ? null : side(l), right: r === null ? null : side(r), state };
}

function step(rel: string, action: SyncStep["action"], reason: SyncStep["reason"], l: number | null, r: number | null, conflict = false): SyncStep {
  return {
    rel,
    dir: false,
    action,
    reason,
    conflict,
    left: l === null ? null : { files: 1, bytes: l },
    right: r === null ? null : { files: 1, bytes: r },
  };
}

/** A plan in mirror mode: a new file, a changed one, an extra one to delete,
 *  an identical one and a conflict. */
function plan(mode = "mirror"): SyncPreview {
  return {
    id: "plan-1",
    mode: mode as SyncPreview["mode"],
    comparison: {
      left: { fs: "local", path: "/a" },
      right: { fs: "local", path: "/b" },
      options: {} as never,
      items: [
        item("new.txt", "leftOnly", 10, null),
        item("changed.txt", "leftNewer", 20, 15),
        item("extra.txt", "rightOnly", null, 5),
        item("same.txt", "same", 3, 3),
        item("both.txt", "different", 7, 8),
      ],
      summary: { same: 1, leftOnly: 1, rightOnly: 1, leftNewer: 1, rightNewer: 0, different: 1, kindMismatch: 0 },
    },
    steps: [
      step("new.txt", "copyToRight", "new", 10, null),
      step("changed.txt", "copyToRight", "changed", 20, 15),
      step("extra.txt", "deleteRight", "extra", null, 5),
      step("both.txt", "skip", "conflict", 7, 8, true),
    ],
    hasHistory: false,
  };
}

function backend() {
  const calls: { cmd: string; args: any }[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args });
    if (cmd === "sync_plan") return plan((args as any).mode);
    if (cmd === "run_sync") return "op-9";
    if (cmd === "plugin:event|listen") return 1;
    return null;
  });
  return { calls: (c: string) => calls.filter((x) => x.cmd === c).map((x) => x.args) };
}

function setup() {
  const props = {
    left: { fs: "local", path: "/a" },
    right: { fs: "local", path: "/b" },
    nameOf: (fs: string) => fs,
    onclose: vi.fn(),
    onsavefavorite: vi.fn(async (f) => ({ ...f, id: "fav-1" })),
    oncomparefiles: vi.fn(async () => {}),
  };
  render(CompareView, { props });
  return props;
}

const rowOf = (name: string) => screen.getAllByRole("row").find((r) => r.querySelector(".label")?.textContent === name)!;

describe("UI-CMP-007 CompareView", () => {
  it("plans on open and renders each state, hiding identical items under Differences", async () => {
    const be = backend();
    setup();
    await waitFor(() => expect(rowOf("new.txt")).toBeTruthy());
    expect(be.calls("sync_plan")[0]).toMatchObject({ left: { fs: "local", path: "/a" }, right: { fs: "local", path: "/b" } });
    expect(rowOf("new.txt")).toHaveClass("leftOnly");
    expect(rowOf("changed.txt")).toHaveClass("leftNewer");
    expect(rowOf("extra.txt")).toHaveClass("rightOnly");
    expect(rowOf("both.txt")).toHaveClass("conflict");
    expect(rowOf("same.txt")).toBeUndefined();
    await userEvent.setup().click(screen.getByRole("tab", { name: "All items" }));
    expect(rowOf("same.txt")).toHaveClass("same");
  });

  it("changes a row's action and sends it as an override", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    const select = await screen.findByRole("combobox", { name: "Action for both.txt" });
    // Only actions possible for that item are offered.
    expect(within(select).getAllByRole("option").map((o) => (o as HTMLOptionElement).value)).toEqual([
      "copyToRight",
      "copyToLeft",
      "deleteLeft",
      "deleteRight",
      "skip",
    ]);
    const newOnly = screen.getByRole("combobox", { name: "Action for new.txt" });
    expect(within(newOnly).queryByRole("option", { name: /Copy to left/ })).toBeNull();

    await user.selectOptions(select, "copyToLeft");
    // Keep the extra file so no confirmation is needed.
    await user.selectOptions(screen.getByRole("combobox", { name: "Action for extra.txt" }), "skip");
    await user.click(screen.getByRole("button", { name: "Synchronize" }));
    await waitFor(() => expect(be.calls("run_sync")).toHaveLength(1));
    expect(be.calls("run_sync")[0]).toEqual({ plan: "plan-1", overrides: { "both.txt": "copyToLeft", "extra.txt": "skip" } });
  });

  it("asks before a sync that deletes, and Cancel runs nothing", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await screen.findByRole("combobox", { name: "Action for extra.txt" });
    await user.click(screen.getByRole("button", { name: "Synchronize" }));
    expect(screen.getByText(/This deletes 1 item for good/)).toBeInTheDocument();
    expect(be.calls("run_sync")).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByText(/This deletes/)).toBeNull();
    expect(be.calls("run_sync")).toEqual([]);

    await user.click(screen.getByRole("button", { name: "Synchronize" }));
    await user.click(screen.getByRole("button", { name: "Delete and sync" }));
    await waitFor(() => expect(be.calls("run_sync")).toHaveLength(1));
  });

  it("double-clicking a file on both sides opens the compare app", async () => {
    const user = userEvent.setup();
    backend();
    const p = setup();
    await waitFor(() => expect(rowOf("changed.txt")).toBeTruthy());
    await user.dblClick(rowOf("changed.txt"));
    expect(p.oncomparefiles).toHaveBeenCalledWith({ fs: "local", path: "/a/changed.txt" }, { fs: "local", path: "/b/changed.txt" });
    // A file on one side only cannot be compared.
    await user.dblClick(rowOf("new.txt"));
    expect(p.oncomparefiles).toHaveBeenCalledTimes(1);
  });

  it("changing the mode plans again in that mode", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await screen.findByRole("combobox", { name: "Action for new.txt" });
    await user.click(screen.getByRole("button", { name: /Two-way/i }));
    await waitFor(() => expect(be.calls("sync_plan").at(-1).mode).toBe("twoWay"));
  });
});

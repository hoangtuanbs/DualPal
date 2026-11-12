import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import CommandPalette, { fuzzyScore, type Command } from "./CommandPalette.svelte";

function commands(run = vi.fn()): Command[] {
  return [
    { id: "copy", title: "Copy to Other Pane", group: "File", shortcut: "F5", run: () => run("copy") },
    { id: "new-folder", title: "New Folder", group: "File", shortcut: "F7", run: () => run("new-folder") },
    { id: "server", title: "Connect to Build Box", group: "Servers", run: () => run("server") },
    { id: "fav", title: "Open Projects", group: "Favorites", run: () => run("fav") },
    { id: "sync", title: "Sync Photos Backup", group: "Syncs", run: () => run("sync") },
  ];
}

const titles = () => screen.getAllByRole("option").map((o) => o.querySelector(".title")?.textContent);

describe("UI-CMP-003 CommandPalette", () => {
  it("scores subsequences, preferring word starts and runs", () => {
    expect(fuzzyScore("nf", "New Folder")).not.toBeNull();
    expect(fuzzyScore("xyz", "New Folder")).toBeNull();
    expect(fuzzyScore("", "anything")).toBe(0);
    expect(fuzzyScore("nf", "New Folder")!).toBeGreaterThan(fuzzyScore("nf", "Info Dialog")!);
  });

  it("filters fuzzily across servers, favorites and syncs", async () => {
    const user = userEvent.setup();
    render(CommandPalette, { props: { commands: commands(), onclose: () => {} } });
    expect(titles()).toHaveLength(5);
    await user.type(screen.getByRole("combobox"), "bldbx");
    expect(titles()).toEqual(["Connect to Build Box"]);
    await user.clear(screen.getByRole("combobox"));
    await user.type(screen.getByRole("combobox"), "favorites");
    expect(titles()[0]).toBe("Open Projects");
    await user.clear(screen.getByRole("combobox"));
    await user.type(screen.getByRole("combobox"), "qqq");
    expect(screen.getByText("No matching commands")).toBeInTheDocument();
  });

  it("focuses the input, moves with arrows (wrapping) and runs on Enter", async () => {
    const user = userEvent.setup();
    const run = vi.fn();
    const onclose = vi.fn();
    render(CommandPalette, { props: { commands: commands(run), onclose } });
    expect(screen.getByRole("combobox")).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(screen.getAllByRole("option")[2]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{ArrowUp}{ArrowUp}{ArrowUp}");
    expect(screen.getAllByRole("option")[4]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    expect(onclose).toHaveBeenCalled();
    expect(run).toHaveBeenCalledWith("sync");
  });

  it("closes on Escape without running anything", async () => {
    const user = userEvent.setup();
    const run = vi.fn();
    const onclose = vi.fn();
    render(CommandPalette, { props: { commands: commands(run), onclose } });
    await user.keyboard("{Escape}");
    expect(onclose).toHaveBeenCalled();
    expect(run).not.toHaveBeenCalled();
  });
});

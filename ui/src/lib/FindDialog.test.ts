import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import FindDialog from "./FindDialog.svelte";
import type { Entry } from "./api";

function hit(path: string): Entry {
  const name = path.slice(path.lastIndexOf("/") + 1);
  return {
    name,
    path,
    kind: "file",
    symlink: false,
    size: 1,
    modifiedMs: 1,
    createdMs: 1,
    hidden: false,
    readonly: false,
    mode: null,
  };
}

/** A backend whose find_files waits until the test calls `finish`. */
function backend() {
  const calls: { cmd: string; args: any }[] = [];
  let finish: ((v: unknown) => void) | null = null;
  let fail: ((e: unknown) => void) | null = null;
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === "find_files") {
        return new Promise((res, rej) => {
          finish = res;
          fail = rej;
        });
      }
      return null;
    },
    { shouldMockEvents: true },
  );
  return {
    calls: (c: string) => calls.filter((x) => x.cmd === c).map((x) => x.args),
    finish: (v: unknown) => finish?.(v),
    fail: (e: unknown) => fail?.(e),
  };
}

const setup = () => {
  const props = { location: { fs: "local", path: "/home/u" }, onopen: vi.fn(), onclose: vi.fn() };
  render(FindDialog, { props });
  return props;
};

describe("FindDialog", () => {
  it("shows matches while the search is still running, then the final count", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    expect(screen.getByRole("textbox", { name: "Find files named" })).toHaveFocus();
    await user.type(screen.getByRole("textbox"), "agent");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    const { request } = be.calls("find_files")[0];
    expect(be.calls("find_files")[0]).toEqual({ location: { fs: "local", path: "/home/u" }, query: "agent", request });

    await emit("find-files-progress", { request, hits: [hit("/home/u/agent.md")], folders: 12 });
    await waitFor(() => expect(screen.getByText("agent.md")).toBeInTheDocument());
    expect(screen.getByRole("status")).toHaveTextContent("Searching… 1 match · 12 folders");
    // The folder is shown relative to the folder searched.
    expect(screen.getByText("(this folder)")).toBeInTheDocument();

    await emit("find-files-progress", { request, hits: [hit("/home/u/docs/Agent notes.txt")], folders: 40 });
    await waitFor(() => expect(screen.getByText("Agent notes.txt")).toBeInTheDocument());
    expect(screen.getByText("docs")).toBeInTheDocument();

    be.finish({ hits: [hit("/home/u/agent.md"), hit("/home/u/docs/Agent notes.txt")], truncated: false, skipped: 1 });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("2 matches in 40 folders"));
    expect(screen.getByRole("status")).toHaveTextContent("1 folder could not be read");
    expect(screen.getAllByRole("option")).toHaveLength(2);
  });

  it("ignores progress from an older search", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await user.type(screen.getByRole("textbox"), "x");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    const old = be.calls("find_files")[0].request;
    await user.type(screen.getByRole("textbox"), "y");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(2));
    expect(be.calls("cancel_find_files")).toContainEqual({ request: old });
    await emit("find-files-progress", { request: old, hits: [hit("/home/u/old-x")], folders: 3 });
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.queryByText("old-x")).toBeNull();
  });

  it("Escape stops a running search first, then closes", async () => {
    const user = userEvent.setup();
    const be = backend();
    const p = setup();
    await user.type(screen.getByRole("textbox"), "big");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    const { request } = be.calls("find_files")[0];
    await user.keyboard("{Escape}");
    expect(be.calls("cancel_find_files")).toContainEqual({ request });
    expect(p.onclose).not.toHaveBeenCalled();
    be.fail({ kind: "cancelled", message: "operation cancelled" });
    await waitFor(() => expect(screen.getByRole("status")).not.toHaveTextContent("Searching"));
    await user.keyboard("{Escape}");
    expect(p.onclose).toHaveBeenCalled();
  });

  it("arrows move through matches and Enter opens the selected one", async () => {
    const user = userEvent.setup();
    const be = backend();
    const p = setup();
    await user.type(screen.getByRole("textbox"), "a");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    be.finish({ hits: [hit("/home/u/a1"), hit("/home/u/a2"), hit("/home/u/a3")], truncated: true, skipped: 0 });
    await waitFor(() => expect(screen.getAllByRole("option")).toHaveLength(3));
    expect(screen.getByRole("status")).toHaveTextContent("stopped early, more may match");
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowUp}");
    expect(screen.getAllByRole("option")[1]).toHaveAttribute("aria-selected", "true");
    await user.keyboard("{Enter}");
    expect(p.onopen).toHaveBeenCalledWith(expect.objectContaining({ path: "/home/u/a2" }));
  });

  it("says so when nothing matches", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await user.type(screen.getByRole("textbox"), "zzz");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    be.finish({ hits: [], truncated: false, skipped: 0 });
    expect(await screen.findByText(/No file or folder names contain “zzz”/)).toBeInTheDocument();
  });
});

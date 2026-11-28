import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import FindDialog from "./FindDialog.svelte";
import { FindSession } from "./find-session.svelte";
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
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "agent");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    const { request } = be.calls("find_files")[0];
    expect(be.calls("find_files")[0]).toEqual({
      location: { fs: "local", path: "/home/u" },
      query: "agent",
      request,
      criteria: expect.objectContaining({ content: "", ignoreCase: true, includeHidden: false, kind: "any", minSize: null }),
    });

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
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "x");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    const old = be.calls("find_files")[0].request;
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "y");
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
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "big");
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
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "a");
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
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "zzz");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    be.finish({ hits: [], truncated: false, skipped: 0 });
    expect(await screen.findByText(/No file or folder names contain “zzz”/)).toBeInTheDocument();
  });

  it("UI-UNIT-053 searches text with options and shows the matching line", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "*.rs;*.toml");
    await user.type(screen.getByRole("textbox", { name: "Containing text" }), "fn main");
    await user.click(screen.getByRole("button", { name: /Options/ }));
    await user.click(screen.getByRole("checkbox", { name: "Ignore case" }));
    await user.click(screen.getByRole("checkbox", { name: "Regex in text" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Kind" }), "files");
    await user.type(screen.getByRole("textbox", { name: "At least" }), "1 KB");
    await user.selectOptions(screen.getByRole("combobox", { name: "Modified" }), "7d");
    expect(screen.getByRole("button", { name: /Options/ })).toHaveTextContent("Options (5)");
    await waitFor(() =>
      expect(be.calls("find_files").at(-1)).toEqual(
        expect.objectContaining({
          query: "*.rs;*.toml",
          criteria: expect.objectContaining({
            content: "fn main",
            ignoreCase: false,
            contentRegex: true,
            kind: "files",
            minSize: 1000,
            newerThanMs: expect.any(Number),
          }),
        }),
      ),
    );
    const { request } = be.calls("find_files").at(-1);
    await emit("find-files-progress", {
      request,
      hits: [{ ...hit("/home/u/src/main.rs"), snippet: { line: 3, text: "fn main() {" } }],
      folders: 4,
      files: 9,
    });
    await waitFor(() => expect(screen.getByText("main.rs")).toBeInTheDocument());
    expect(screen.getByText("fn main() {")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Searching… 1 match · 4 folders · 9 files read");
    be.finish({ hits: [{ ...hit("/home/u/src/main.rs"), snippet: { line: 3, text: "fn main() {" } }], truncated: false, skipped: 0, filesRead: 12, binary: 2, filesSkipped: 0, tooLarge: 0 });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("1 match in 4 folders · 12 files read · 2 binary files skipped"));
  });

  it("UI-UNIT-054 a bad size or regex is shown instead of searching", async () => {
    const user = userEvent.setup();
    const be = backend();
    setup();
    await user.click(screen.getByRole("button", { name: /Options/ }));
    await user.type(screen.getByRole("textbox", { name: "At most" }), "lots");
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Sizes look like 500 KB or 2 MB."));
    expect(be.calls("find_files")).toHaveLength(0);
    await user.clear(screen.getByRole("textbox", { name: "At most" }));
    await user.type(screen.getByRole("textbox", { name: "Find files named" }), "(");
    await user.click(screen.getByRole("checkbox", { name: "Regex in name" }));
    await waitFor(() => expect(be.calls("find_files").length).toBeGreaterThan(0));
    be.fail({ kind: "invalid", message: "invalid request: name pattern: unclosed group" });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("unclosed group"));
  });

  it("UI-UNIT-055 Show in pane hands over a search that keeps running after the dialog closes", async () => {
    const user = userEvent.setup();
    const be = backend();
    const session = new FindSession();
    const onfeed = vi.fn();
    const props = { location: { fs: "sftp:1", path: "/srv" }, onopen: vi.fn(), onclose: vi.fn(), onfeed, session };
    const { unmount } = render(FindDialog, { props });
    await user.click(screen.getByRole("button", { name: /Options/ }));
    expect(screen.getByText(/On servers, files over 10 MB are not searched for text/)).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Containing text" }), "todo");
    await waitFor(() => expect(be.calls("find_files")).toHaveLength(1));
    const { request } = be.calls("find_files")[0];
    await emit("find-files-progress", { request, hits: [hit("/srv/a.txt")], folders: 1, files: 1 });
    await waitFor(() => expect(screen.getByRole("button", { name: "Show in pane" })).toBeInTheDocument());
    await user.keyboard("{Control>}{Enter}{/Control}");
    expect(onfeed).toHaveBeenCalledWith(session);
    await user.click(screen.getByRole("button", { name: "Show in pane" }));
    expect(onfeed).toHaveBeenCalledTimes(2);

    // Closing the dialog leaves the search running; new matches still arrive.
    const seen: string[] = [];
    session.onhits = (_id, hits) => seen.push(...hits.map((h) => h.path));
    unmount();
    expect(be.calls("cancel_find_files")).toHaveLength(0);
    await emit("find-files-progress", { request, hits: [hit("/srv/deep/b.txt")], folders: 3, files: 5 });
    await waitFor(() => expect(seen).toEqual(["/srv/deep/b.txt"]));
    expect(session.searching).toBe(true);
    session.stop();
    expect(be.calls("cancel_find_files")).toContainEqual({ request });
  });
});

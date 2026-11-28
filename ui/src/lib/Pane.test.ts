import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import Pane from "./Pane.svelte";
import { PaneGroup, PaneState } from "./pane-state.svelte";
import { FindSession } from "./find-session.svelte";
import { installMockBackend, tree } from "../../tests/mock-backend";
import type { Entry } from "./api";

const found = (path: string, kind: "file" | "dir" = "file"): Entry => ({
  name: path.slice(path.lastIndexOf("/") + 1),
  path,
  kind,
  symlink: false,
  size: 3,
  modifiedMs: 1,
  createdMs: 1,
  hidden: false,
  readonly: false,
  mode: null,
});

// jsdom has no layout; the pane measures its list width with bind:clientWidth.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

describe("UI-UNIT-056 Pane showing search results", () => {
  it("lists matches by relative path with a results bar that stops the search and goes back", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/w/": "", "/w/a/": "", "/w/a/x.txt": "abc", "/w/b.txt": "abc" }));
    const state = new PaneState();
    await state.open({ fs: "local", path: "/w" });
    const session = new FindSession();
    session.id = 42;
    session.searching = true;
    session.folders = 7;
    session.files = 3;
    state.showResults({ fs: "local", path: "/w" }, "x", [found("/w/a/x.txt"), found("/w/b.txt")], 42);
    const group = new PaneGroup(state);
    render(Pane, { props: { group, index: 0, active: true, onfocus: vi.fn(), oncontext: vi.fn(), ondrop: vi.fn(), search: session } });

    const bar = screen.getByTestId("pane-left-results");
    expect(bar).toHaveTextContent("Search results for “x” in /w");
    expect(bar).toHaveTextContent("7 folders, 3 files");
    const rows = screen.getAllByTestId("pane-left-row");
    expect(rows.map((r) => r.querySelector(".label")?.textContent)).toEqual(["a/x.txt", "b.txt"]);
    // Rows keep their real names for drag and drop and tests; the pane is no drop target itself.
    expect(rows[0].dataset.name).toBe("x.txt");
    expect(screen.getByTestId("pane-left").dataset.path).toBeUndefined();

    const user = userEvent.setup();
    await user.click(within(bar).getByRole("button", { name: "Stop" }));
    // Stop goes to the session (it has no request running here, so nothing is sent).
    session.searching = false;
    await waitFor(() => expect(within(bar).queryByRole("button", { name: "Stop" })).toBeNull());

    await user.click(within(bar).getByRole("button", { name: "Close search results" }));
    await waitFor(() => expect(screen.queryByTestId("pane-left-results")).toBeNull());
    expect(state.location).toEqual({ fs: "local", path: "/w" });
    expect(screen.getAllByTestId("pane-left-row").map((r) => r.dataset.name)).toEqual(["a", "b.txt"]);
  });
});

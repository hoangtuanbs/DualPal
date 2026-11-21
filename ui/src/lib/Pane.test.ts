import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import Pane from "./Pane.svelte";
import { PaneGroup, PaneState } from "./pane-state.svelte";
import { installMockBackend, tree } from "../../tests/mock-backend";
import { api, type Location } from "./api";

const at = (path: string): Location => ({ fs: "local", path });

/** A pane that has been to /a, then /b: it is showing /b, with /a behind it. */
async function paneAtB() {
  const mock = installMockBackend();
  mock.setTree("local", tree({ "/a/": "", "/b/": "" }));
  const p = new PaneState();
  await p.open(at("/a"));
  await p.open(at("/b"));
  return new PaneGroup(p);
}

/** Dispatch a mouse event with this button; true when the page's default was not prevented. */
function mouse(el: HTMLElement, type: "mouseup" | "mousedown" | "auxclick", button: number): boolean {
  const ev = new MouseEvent(type, { button, bubbles: true, cancelable: true });
  el.dispatchEvent(ev);
  return !ev.defaultPrevented;
}

function renderPane(group: PaneGroup) {
  render(Pane, {
    props: { group, index: 0, active: true, onfocus: vi.fn(), oncontext: vi.fn(), ondrop: vi.fn() },
  });
  return screen.getByTestId("pane-left");
}

beforeEach(() => {
  installMockBackend();
  // jsdom has no layout; the pane watches its width.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});

afterEach(() => vi.unstubAllGlobals());

describe("UI-MOUSE-013 side buttons go back and forward in the pane", () => {
  it("button 4 (back) and button 5 (forward) on mouse up move this pane", async () => {
    const group = await paneAtB();
    const el = renderPane(group);

    expect(mouse(el, "mouseup", 3)).toBe(false); // webview navigation is prevented
    await waitFor(() => expect(group.current.location).toEqual(at("/a")));

    mouse(el, "mouseup", 4);
    await waitFor(() => expect(group.current.location).toEqual(at("/b")));
  });

  it("the auxiliary click is swallowed, but does not move anything", async () => {
    const group = await paneAtB();
    const el = renderPane(group);

    expect(mouse(el, "auxclick", 3)).toBe(false);
    expect(mouse(el, "mousedown", 4)).toBe(false);
    await new Promise((r) => setTimeout(r, 0));
    expect(group.current.location).toEqual(at("/b"));
  });

  it("other buttons are left to the pane", async () => {
    const group = await paneAtB();
    const el = renderPane(group);

    expect(mouse(el, "mouseup", 0)).toBe(true);
    expect(mouse(el, "mouseup", 1)).toBe(true);
    await new Promise((r) => setTimeout(r, 0));
    expect(group.current.location).toEqual(at("/b"));
  });
});

describe("UI-UNIT-017 history notices in the pane", () => {
  it("shows which folders were skipped even when the next one fails", async () => {
    const mock = installMockBackend();
    mock.setTree("local", tree({ "/a/": "", "/b/": "", "/c/": "" }));
    const p = new PaneState();
    await p.open(at("/a"));
    await p.open(at("/b"));
    await p.open(at("/c"));
    mock.setTree("local", tree({ "/c/": "" })); // /b is gone
    const real = api.listDir;
    vi.spyOn(api, "listDir").mockImplementation((loc: Location) =>
      loc.path === "/a" ? Promise.reject({ kind: "permissionDenied", message: "permission denied: /a" }) : real(loc),
    );
    try {
      renderPane(new PaneGroup(p));
      await p.back();
      expect(await screen.findByTestId("pane-left-error")).toHaveTextContent("permission denied: /a");
      expect(screen.getByTestId("pane-left-notice")).toHaveTextContent("“b”");
    } finally {
      vi.restoreAllMocks();
    }
  });
});

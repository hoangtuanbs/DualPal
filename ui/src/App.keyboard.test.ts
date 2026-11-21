import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import App from "./App.svelte";
import { installMockBackend } from "../tests/mock-backend";

beforeEach(() => {
  installMockBackend();
  // jsdom has no layout; the panes watch their width.
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

const key = (k: string, init: KeyboardEventInit) =>
  window.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

// The app listens for backend events, which the mock backend does not send.
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {}, emit: async () => {}, once: async () => () => {} }));

describe("UI-KEY-036 Alt+Left and Alt+Right go back and forward in the active pane", () => {
  it("walks the left pane's history with the keys", async () => {
    render(App);
    const left = await waitFor(() => screen.getByTestId("pane-left"));
    await waitFor(() => expect(left.getAttribute("data-path")).toBeTruthy());
    const start = left.getAttribute("data-path");

    // Breadcrumb to the root: a new step in the history.
    const root = screen.getByTestId("pane-left-crumbs").querySelector("button") as HTMLButtonElement;
    root.click();
    await waitFor(() => expect(left.getAttribute("data-path")).toBe("/"));

    const ev = new KeyboardEvent("keydown", { key: "ArrowLeft", altKey: true, bubbles: true, cancelable: true });
    window.dispatchEvent(ev);
    expect(ev.defaultPrevented).toBe(true);
    await waitFor(() => expect(left.getAttribute("data-path")).toBe(start));

    key("ArrowRight", { altKey: true });
    await waitFor(() => expect(left.getAttribute("data-path")).toBe("/"));

    // A plain arrow is not a history step.
    key("ArrowLeft", {});
    await new Promise((r) => setTimeout(r, 20));
    expect(left.getAttribute("data-path")).toBe("/");
  });
});

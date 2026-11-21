import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import DiffView from "./DiffView.svelte";
import { diffApi } from "./diff-api";
import { defaultScenario, file, installMockBackend } from "../../../tests/mock-backend";

const L = { fs: "local", path: "/home/user/left.txt" };
const R = { fs: "local", path: "/home/user/right.txt" };

function setup(left: string | Uint8Array, right: string | Uint8Array) {
  try {
    localStorage.clear();
  } catch {
    // no storage
  }
  const scenario = defaultScenario();
  for (const [loc, c] of [
    [L, left],
    [R, right],
  ] as const) {
    const name = loc.path.split("/").at(-1)!;
    scenario.trees.local.set(loc.path, typeof c === "string" ? { ...file(name, c), path: loc.path } : { ...file(name, "", { size: c.length }), path: loc.path, bytes: c });
  }
  const mock = installMockBackend(scenario);
  const onclose = vi.fn();
  const onexternal = vi.fn(async () => {});
  render(DiffView, { left: L, right: R, onclose, onexternal });
  return { mock, onclose, onexternal };
}

const texts = (side: string) => [...document.querySelectorAll(`[data-testid=diff-${side}] .row`)].map((r) => r.querySelector(".tx")!.textContent);
const kinds = (side: string) => [...document.querySelectorAll(`[data-testid=diff-${side}] .row`)].map((r) => r.className.replace(/\s*svelte-\S+/g, "").replace("row ", "").trim());
const status = () => screen.getByTestId("diff-status").textContent;

const LEFT = "same\nold word\nkeep\nend\n";
const RIGHT = "same\nnew word\nkeep\nadded\nend\n";

describe("UI-UNIT-041 DiffView", () => {
  it("opens both files with diff_open and shows them side by side with tints and changed words", async () => {
    const { mock } = setup(LEFT, RIGHT);
    await waitFor(() => expect(texts("right")).toEqual(["same", "new word", "keep", "added", "end"]));
    expect(mock.lastCall("diff_open")).toEqual({
      request: expect.any(Number),
      left: L,
      right: R,
      options: { ignoreWhitespace: false, ignoreCase: false, ignoreLineEndings: false },
      encodings: null,
    });
    expect(texts("left")).toEqual(["same", "old word", "keep", "", "end"]);
    expect(kinds("left")).toEqual(["same", "removed", "same", "filler", "same"]);
    expect(kinds("right")).toEqual(["same", "added", "same", "added", "same"]);
    const marks = [...document.querySelectorAll("[data-testid=diff-right] mark")].map((m) => m.textContent);
    expect(marks).toEqual(["new"]);
    expect(status()).toBe("2 changes");
    expect(screen.getByTestId("diff-overview").querySelectorAll(".marker")).toHaveLength(2);
    expect(screen.getByTestId("diff-left-head").textContent).toContain("left.txt");
  });

  it("switches to inline: removed lines, then added ones", async () => {
    setup(LEFT, RIGHT);
    await waitFor(() => expect(status()).toBe("2 changes"));
    await userEvent.click(screen.getByRole("button", { name: "Inline" }));
    await waitFor(() => expect(texts("inline")).toEqual(["same", "old word", "new word", "keep", "added", "end"]));
    expect(kinds("inline")).toEqual(["same", "removed", "added", "same", "added", "same"]);
    expect(screen.getByRole("button", { name: "Inline" })).toHaveAttribute("aria-pressed", "true");
  });

  it("goes to the next and previous change with Alt ↓ / Alt ↑ and F7 / Shift F7", async () => {
    setup(LEFT, RIGHT);
    await waitFor(() => expect(status()).toBe("2 changes"));
    await fireEvent.keyDown(window, { key: "ArrowDown", altKey: true });
    expect(status()).toBe("Change 1 of 2");
    expect(document.querySelectorAll("[data-testid=diff-left] .row.current")).toHaveLength(1);
    await fireEvent.keyDown(window, { key: "F7" });
    expect(status()).toBe("Change 2 of 2");
    await fireEvent.keyDown(window, { key: "F7" }); // stays on the last
    expect(status()).toBe("Change 2 of 2");
    await fireEvent.keyDown(window, { key: "F7", shiftKey: true });
    expect(status()).toBe("Change 1 of 2");
    await fireEvent.keyDown(window, { key: "ArrowUp", altKey: true });
    expect(status()).toBe("Change 1 of 2");
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(status()).toBe("Change 2 of 2");
  });

  it("copies a change to either side and saves it", async () => {
    const { mock } = setup(LEFT, RIGHT);
    await waitFor(() => expect(status()).toBe("2 changes"));
    await userEvent.click(screen.getByRole("button", { name: "Copy change 1 to the right" }));
    await waitFor(() => expect(status()).toBe("Change 1 of 1"));
    expect(mock.lastCall("diff_apply")).toEqual({ session: 1, revision: 1, hunk: 0, to: "right" });
    expect(texts("right")).toEqual(["same", "old word", "keep", "added", "end"]);
    expect(screen.getByRole("button", { name: "Save right" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save left" })).toBeDisabled();
    // Alt ← copies the current change ("added") to the left.
    await fireEvent.keyDown(window, { key: "ArrowLeft", altKey: true });
    await waitFor(() => expect(status()).toBe("The files are identical"));
    expect(mock.lastCall("diff_apply")).toEqual({ session: 1, revision: 2, hunk: 0, to: "left" });
    await userEvent.click(screen.getByRole("button", { name: "Save right" }));
    await waitFor(() => expect(screen.getByTestId("diff-message").textContent).toBe("Saved right.txt."));
    expect(mock.lastCall("diff_save")).toEqual({ session: 1, side: "right" });
    expect(screen.getByRole("button", { name: "Save right" })).toBeDisabled();
  });

  it("asks before closing with unsaved changes", async () => {
    const { mock, onclose } = setup(LEFT, RIGHT);
    await waitFor(() => expect(status()).toBe("2 changes"));
    // Nothing changed: Esc closes at once.
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(onclose).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Copy change 2 to the left" }));
    await waitFor(() => expect(status()).toBe("Change 1 of 1"));
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByTestId("diff-unsaved")).toHaveTextContent("“left.txt” changed.");
    expect(onclose).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByTestId("diff-unsaved")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await userEvent.click(within("diff-unsaved", "Save"));
    await waitFor(() => expect(onclose).toHaveBeenCalledTimes(2));
    expect(mock.lastCall("diff_save")).toEqual({ session: 1, side: "left" });
  });

  it("closes without saving on Don’t Save", async () => {
    const { mock, onclose } = setup(LEFT, RIGHT);
    await waitFor(() => expect(status()).toBe("2 changes"));
    await userEvent.click(screen.getByRole("button", { name: "Copy change 1 to the left" }));
    await waitFor(() => expect(status()).toBe("Change 1 of 1"));
    await userEvent.click(screen.getByRole("button", { name: "Close" }));
    await userEvent.click(within("diff-unsaved", "Don’t Save"));
    expect(onclose).toHaveBeenCalledTimes(1);
    expect(mock.calls("diff_save")).toEqual([]);
  });

  it("compares again with options and tells identical from ignored differences", async () => {
    const { mock } = setup("Hello\n", "hello\n");
    await waitFor(() => expect(status()).toBe("1 change"));
    await userEvent.click(screen.getByRole("checkbox", { name: "Ignore case" }));
    await waitFor(() => expect(status()).toBe("No differences with these options"));
    expect(mock.lastCall("diff_set_options")).toEqual({ session: 1, options: { ignoreWhitespace: false, ignoreCase: true, ignoreLineEndings: false } });
  });

  it("says when binary files differ, and opens the external app", async () => {
    const { onexternal } = setup(new Uint8Array([0, 1, 2]), new Uint8Array([0, 1, 3]));
    expect(await screen.findByTestId("diff-binary")).toHaveTextContent("Binary files differ.");
    await userEvent.click(screen.getByRole("button", { name: "Open in external app…" }));
    expect(onexternal).toHaveBeenCalledWith(L, R);
  });

  it("shows an open error", async () => {
    installMockBackend(defaultScenario());
    render(DiffView, { left: L, right: R, onclose: () => {} });
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not compare the files: not found: /home/user/left.txt");
  });
});

function within(testId: string, name: string): HTMLElement {
  return [...screen.getByTestId(testId).querySelectorAll("button")].find((b) => b.textContent === name)!;
}

describe("UI-UNIT-042 compare view api argument names", () => {
  it("sends each diff_* command's exact arguments", async () => {
    const mock = installMockBackend(defaultScenario());
    const call = (p: Promise<unknown>) => p.catch(() => {});
    const opts = { ignoreWhitespace: true, ignoreCase: false, ignoreLineEndings: true };
    await call(diffApi.open(3, L, R, opts, ["utf-8", null]));
    await call(diffApi.lines(1, "left", 10, 20));
    await call(diffApi.setOptions(1, opts));
    await call(diffApi.setEncoding(1, "right", "windows-1252"));
    await call(diffApi.apply(1, 4, 2, "left"));
    await call(diffApi.save(1, "right"));
    await call(diffApi.cancel(3));
    await call(diffApi.close(1));
    expect(mock.lastCall("diff_open")).toEqual({ request: 3, left: L, right: R, options: opts, encodings: ["utf-8", null] });
    expect(mock.lastCall("diff_lines")).toEqual({ session: 1, side: "left", from: 10, count: 20 });
    expect(mock.lastCall("diff_set_options")).toEqual({ session: 1, options: opts });
    expect(mock.lastCall("diff_set_encoding")).toEqual({ session: 1, side: "right", encoding: "windows-1252" });
    expect(mock.lastCall("diff_apply")).toEqual({ session: 1, revision: 4, hunk: 2, to: "left" });
    expect(mock.lastCall("diff_save")).toEqual({ session: 1, side: "right" });
    expect(mock.lastCall("diff_cancel")).toEqual({ request: 3 });
    expect(mock.lastCall("diff_close")).toEqual({ session: 1 });
  });
});

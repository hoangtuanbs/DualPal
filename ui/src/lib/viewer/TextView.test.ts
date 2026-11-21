import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import TextView from "./TextView.svelte";
import HexView from "./HexView.svelte";
import { memorySource } from "./source";
import { bigSource, LINE } from "../../../tests/viewer/fixtures";

const utf8 = (s: string) => new TextEncoder().encode(s);
const rowsText = () => [...document.querySelectorAll("[data-testid=text-view] .row .tx")].map((e) => e.textContent);
const lineNos = () => [...document.querySelectorAll("[data-testid=text-view] .row .ln")].map((e) => e.textContent);
const label = (n: number) => `line ${String(n).padStart(9, "0")}`;

describe("UI-UNIT-034 TextView pages through big files", () => {
  it("shows the first screen with line numbers", async () => {
    render(TextView, { source: memorySource(utf8("alpha\nbeta\ngamma\n")), encoding: "utf-8", rows: 10 });
    await waitFor(() => expect(rowsText()).toEqual(["alpha", "beta", "gamma"]));
    expect(lineNos()).toEqual(["1", "2", "3"]);
  });

  it("reads only around the screen of a 1 GB file while paging, jumping and going to the end", async () => {
    const src = bigSource(70_000_000);
    render(TextView, { source: src, encoding: "utf-8", rows: 20 });
    const view = screen.getByTestId("text-view");
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    await fireEvent.keyDown(view, { key: "PageDown" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(20)));
    expect(lineNos()[0]).toBe("20");
    await fireEvent.keyDown(view, { key: "ArrowDown" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(21)));
    await fireEvent.keyDown(view, { key: "ArrowUp" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(20)));
    await fireEvent.wheel(view, { deltaY: 18 * 5 });
    await waitFor(() => expect(rowsText()[0]).toBe(label(25)));

    src.reads.length = 0;
    await fireEvent.keyDown(view, { key: "End" });
    await waitFor(() => expect(rowsText().at(-1)).toBe(label(70_000_000)));
    expect(rowsText().length).toBe(20);
    // Line numbers come from counting, after the jump.
    await waitFor(() => expect(lineNos().at(-1)).toBe("70000000"));
    for (const [o, n] of src.reads) {
      expect(o).toBeGreaterThan(src.size - 200_000);
      expect(n).toBeLessThanOrEqual(65_536);
    }
    // Going further down stays at the end.
    await fireEvent.keyDown(view, { key: "PageDown" });
    await new Promise((r) => setTimeout(r, 20));
    expect(rowsText().at(-1)).toBe(label(70_000_000));
    await fireEvent.keyDown(view, { key: "Home" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
  });

  it("scrolls to an offset and marks the highlight", async () => {
    const src = bigSource(1000);
    const { component, rerender } = render(TextView, { source: src, encoding: "utf-8", rows: 10 });
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    const at = 500 * LINE + 5; // the number of line 501
    await component.scrollToOffset(at, 3);
    await waitFor(() => expect(rowsText()[0]).toBe(label(498)));
    await rerender({ highlight: { offset: at, length: 9 } });
    await waitFor(() => expect(screen.getByTestId("text-match").textContent).toBe("000000501"));
    expect(component.topOffset()).toBe(497 * LINE);
  });

  it("wraps long lines and keeps the place when wrapping changes", async () => {
    const { rerender } = render(TextView, {
      source: memorySource(utf8("abcdefghij\nxy\n")),
      encoding: "utf-8",
      rows: 10,
      cols: 4,
      wrap: false,
    });
    await waitFor(() => expect(rowsText()).toEqual(["abcdefghij", "xy"]));
    await rerender({ wrap: true });
    await waitFor(() => expect(rowsText()).toEqual(["abcd", "efgh", "ij", "xy"]));
    expect(lineNos()).toEqual(["1", "", "", "2"]);
    await rerender({ lineNumbers: false });
    await waitFor(() => expect(lineNos()).toEqual([]));
  });

  it("decodes with the chosen encoding", async () => {
    const { rerender } = render(TextView, { source: memorySource(new Uint8Array([0x63, 0x61, 0x66, 0xe9])), encoding: "utf-8", rows: 5 });
    await waitFor(() => expect(rowsText()).toEqual(["caf�"]));
    await rerender({ encoding: "windows-1252" });
    await waitFor(() => expect(rowsText()).toEqual(["café"]));
  });
});

describe("UI-UNIT-035 HexView", () => {
  const hexRows = () => [...document.querySelectorAll("[data-testid=hex-view] .row")].map((r) => r.textContent?.replace(/\s+/g, " ").trim());

  it("shows offset, 16 bytes and ASCII per row, and pages", async () => {
    const bytes = new Uint8Array(100).map((_, i) => i + 0x30);
    render(HexView, { source: memorySource(bytes), rows: 2 });
    await waitFor(() => expect(hexRows().length).toBe(2));
    expect(hexRows()[0]).toBe("00000000 30 31 32 33 34 35 36 37 38 39 3A 3B 3C 3D 3E 3F 0123456789:;<=>?");
    const view = screen.getByTestId("hex-view");
    await fireEvent.keyDown(view, { key: "End" });
    await waitFor(() => expect(hexRows()[1]).toMatch(/^00000060 90 91 92 93 \.\.\.\.$/));
    await fireEvent.keyDown(view, { key: "Home" });
    await waitFor(() => expect(hexRows()[0]).toMatch(/^00000000/));
  });

  it("marks the highlighted bytes and scrolls to an offset", async () => {
    const src = bigSource(10_000);
    const { component, rerender } = render(HexView, { source: src, rows: 4 });
    component.scrollToOffset(16 * 100, 1);
    await rerender({ highlight: { offset: 16 * 100 + 2, length: 3 } });
    await waitFor(() => expect(document.querySelectorAll("[data-testid=hex-view] .m").length).toBe(6));
    expect(component.topOffset()).toBe(16 * 99);
  });
});

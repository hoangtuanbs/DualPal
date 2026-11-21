import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import TextView from "./TextView.svelte";
import HexView from "./HexView.svelte";
import { memorySource, type ByteSource } from "./source";
import { bigSource, LINE } from "../../../tests/viewer/fixtures";

/** A source whose reads can be held back (a slow server) or made to fail (a dropped connection). */
function controlled(src: ByteSource) {
  let gate: Promise<void> | null = null;
  let open = () => {};
  let failing = false;
  const source: ByteSource = {
    size: src.size,
    read: async (offset, length) => {
      if (gate) await gate;
      if (failing) throw { kind: "connection", message: "connection dropped" };
      return src.read(offset, length);
    },
    countLines: src.countLines,
  };
  return {
    source,
    hold: () => (gate = new Promise((r) => (open = r))),
    release: () => {
      gate = null;
      open();
    },
    fail: (yes: boolean) => (failing = yes),
  };
}

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

  it("keeps the end at the bottom when the screen grows after End", async () => {
    const src = bigSource(1000);
    const { rerender } = render(TextView, { source: src, encoding: "utf-8", rows: 5 });
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    await fireEvent.keyDown(screen.getByTestId("text-view"), { key: "End" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(996)));
    await rerender({ rows: 9 });
    await waitFor(() => expect(rowsText()).toHaveLength(9));
    expect(rowsText()[0]).toBe(label(992));
    expect(rowsText().at(-1)).toBe(label(1000));
    // A file shorter than the screen still starts at the top.
    await rerender({ source: bigSource(3) });
    await waitFor(() => expect(rowsText()).toEqual([label(1), label(2), label(3)]));
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

  it("adds up moves made while a read is on its way", async () => {
    const c = controlled(bigSource(1000));
    const { component } = render(TextView, { source: c.source, encoding: "utf-8", rows: 5 });
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    c.hold();
    const moves = [component.scrollBy(1), component.scrollBy(1), component.scrollBy(3)];
    c.release();
    await Promise.all(moves);
    await waitFor(() => expect(rowsText()[0]).toBe(label(6)));
  });

  it("drops a slow jump that a later move replaced", async () => {
    const c = controlled(bigSource(100_000));
    const { component } = render(TextView, { source: c.source, encoding: "utf-8", rows: 5 });
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    await fireEvent.keyDown(screen.getByTestId("text-view"), { key: "PageDown" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(5)));
    c.hold();
    const far = component.scrollToOffset(50_000 * LINE, 0);
    component.scrollToStart();
    c.release();
    await far;
    await new Promise((r) => setTimeout(r, 20));
    expect(rowsText()[0]).toBe(label(1));
    expect(screen.getByTestId("text-view").dataset.top).toBe("0");
  });

  it("gives the byte of the top row inside a wrapped line", async () => {
    const { component } = render(TextView, { source: memorySource(utf8("abcdefghij\nxy\n")), encoding: "utf-8", rows: 2, cols: 4, wrap: true });
    await waitFor(() => expect(rowsText()).toEqual(["abcd", "efgh"]));
    await component.scrollBy(2);
    await waitFor(() => expect(rowsText()).toEqual(["ij", "xy"]));
    expect(component.topOffset()).toBe(8);
  });

  it("puts the scroll bar at the bottom at the end, and jumps to whole bytes", async () => {
    const { component } = render(TextView, { source: bigSource(200), encoding: "utf-8", rows: 100 });
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    const bar = screen.getByRole("scrollbar");
    expect(bar).toHaveAttribute("aria-valuenow", "0");
    await fireEvent.keyDown(screen.getByTestId("text-view"), { key: "End" });
    await waitFor(() => expect(rowsText()[0]).toBe(label(101)));
    expect(bar).toHaveAttribute("aria-valuenow", "100");

    const big = render(TextView, { source: bigSource(100_000), encoding: "utf-8", rows: 10, label: "Big" });
    const view = screen.getAllByTestId("text-view")[1];
    await waitFor(() => expect(view.querySelector(".tx")?.textContent).toBe(label(1)));
    await big.component.scrollToFraction(0.123456789);
    await waitFor(() => expect(Number(view.dataset.top)).toBeGreaterThan(100_000));
    expect(Number.isInteger(Number(view.dataset.top))).toBe(true);
    expect(Number.isInteger(big.component.topOffset())).toBe(true);
  });

  it("shows a failed read instead of rows of another place, and tries again", async () => {
    const c = controlled(bigSource(1000));
    render(TextView, { source: c.source, encoding: "utf-8", rows: 5 });
    const view = screen.getByTestId("text-view");
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    await fireEvent.keyDown(view, { key: "End" });
    await waitFor(() => expect(rowsText().at(-1)).toBe(label(1000)));
    c.fail(true);
    await fireEvent.keyDown(view, { key: "Home" });
    expect(await screen.findByRole("alert")).toHaveTextContent("connection dropped");
    expect(rowsText()).toEqual([]);
    c.fail(false);
    await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(rowsText()[0]).toBe(label(1)));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
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

  it("keeps the last row at the bottom when the screen grows after End", async () => {
    const { rerender } = render(HexView, { source: memorySource(new Uint8Array(160)), rows: 3 });
    await fireEvent.keyDown(screen.getByTestId("hex-view"), { key: "End" });
    await waitFor(() => expect(hexRows()[0]).toMatch(/^00000070/));
    await rerender({ rows: 6 });
    await waitFor(() => expect(hexRows()).toHaveLength(6));
    expect(hexRows()[0]).toMatch(/^00000040/);
    expect(hexRows().at(-1)).toMatch(/^00000090/);
  });

  it("shows a failed read instead of rows of another place, and tries again", async () => {
    const c = controlled(memorySource(new Uint8Array(160)));
    render(HexView, { source: c.source, rows: 2 });
    await waitFor(() => expect(hexRows()).toHaveLength(2));
    c.fail(true);
    await fireEvent.keyDown(screen.getByTestId("hex-view"), { key: "End" });
    expect(await screen.findByRole("alert")).toHaveTextContent("connection dropped");
    expect(hexRows()).toEqual([]);
    c.fail(false);
    await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(hexRows()[0]).toMatch(/^00000080/));
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

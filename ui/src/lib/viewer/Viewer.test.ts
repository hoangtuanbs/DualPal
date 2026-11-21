import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockConvertFileSrc } from "@tauri-apps/api/mocks";
import Viewer from "./Viewer.svelte";
import { viewerApi } from "./viewer-api";
import { defaultScenario, file, installMockBackend, type MemEntry } from "../../../tests/mock-backend";

/** A text file of `n` numbered rows, "row 000001\n" (11 bytes each). */
const rows = (n: number) => Array.from({ length: n }, (_, i) => `row ${String(i + 1).padStart(6, "0")}\n`).join("");

function setup(name: string, content: string | Uint8Array) {
  const scenario = defaultScenario();
  const path = `/home/user/${name}`;
  const e: MemEntry =
    typeof content === "string"
      ? { ...file(name, content), path }
      : { ...file(name, "", { size: content.length }), path, bytes: content };
  scenario.trees.local.set(path, e);
  const mock = installMockBackend(scenario);
  const onclose = vi.fn();
  const { content: _c, bytes: _b, ...entry } = e;
  render(Viewer, { location: { fs: "local", path }, entry, onclose });
  return { mock, onclose, path };
}

const shownText = () => [...document.querySelectorAll("[data-testid=text-view] .row .tx")].map((e) => e.textContent);

describe("UI-UNIT-036 Viewer", () => {
  it("opens a text file through the backend and switches modes with keys and buttons", async () => {
    const { mock, path } = setup("notes.txt", "alpha\nbeta\n");
    await waitFor(() => expect(shownText()).toEqual(["alpha", "beta"]));
    expect(mock.lastCall("viewer_open")).toEqual({ location: { fs: "local", path } });
    await fireEvent.keyDown(window, { key: "2" });
    expect(await screen.findByTestId("hex-view")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Hex" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: "Image" })).not.toBeInTheDocument();
    await fireEvent.keyDown(window, { key: "3" }); // no media for text: stays
    expect(screen.getByTestId("hex-view")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Text" }));
    await waitFor(() => expect(shownText()).toEqual(["alpha", "beta"]));
  });

  it("shows images as the image, and text or hex on 1 / 2", async () => {
    mockConvertFileSrc("linux");
    setup("photo.png", "\u0089PNG");
    expect(await screen.findByRole("img", { name: "photo.png" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Image" })).toHaveAttribute("aria-pressed", "true");
    await fireEvent.keyDown(window, { key: "2" });
    expect(await screen.findByTestId("hex-view")).toBeInTheDocument();
    await fireEvent.keyDown(window, { key: "3" });
    expect(await screen.findByRole("img", { name: "photo.png" })).toBeInTheDocument();
  });

  it("opens binary files in hex", async () => {
    setup("data.bin", new Uint8Array([0x7f, 0x45, 0x4c, 0x46, 0, 1, 2]));
    expect(await screen.findByTestId("hex-view")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId("hex-view").textContent).toContain("7F 45 4C 46 00 01 02"));
  });

  it("reads only the pages on screen of a long file", async () => {
    const { mock } = setup("long.log", rows(60_000)); // 660 KB
    await waitFor(() => expect(shownText()[0]).toBe("row 000001"));
    const reads = mock.calls("viewer_read");
    expect(reads.length).toBeGreaterThan(0);
    for (const r of reads) expect(r.offset as number).toBeLessThan(200_000);
    await fireEvent.keyDown(screen.getByTestId("text-view"), { key: "End" });
    await waitFor(() => expect(shownText().at(-1)).toBe("row 060000"));
    const tail = mock.calls("viewer_read").slice(reads.length);
    for (const r of tail) expect(r.offset as number).toBeGreaterThan(400_000);
  });

  it("finds text beyond the loaded pages, next and previous, and Esc closes find then the viewer", async () => {
    const text = rows(30_000).split("\n");
    text[20_000] = "the Needle here";
    text[25_000] = "another needle";
    const { mock, onclose } = setup("big.txt", text.join("\n"));
    await waitFor(() => expect(shownText()[0]).toBe("row 000001"));
    await fireEvent.keyDown(window, { key: "f", ctrlKey: true });
    const field = await screen.findByTestId("viewer-search");
    expect(field).toHaveFocus();
    await userEvent.type(field, "needle{Enter}");
    await waitFor(() => expect(screen.getByTestId("text-match").textContent).toBe("Needle"));
    const first = mock.lastCall("viewer_search")!;
    expect(first).toMatchObject({ from: 0, backward: false, ignoreCase: true, needle: [...new TextEncoder().encode("needle")] });
    const at = 20_000 * 11 + 4; // "Needle" on line 20 001
    expect(shownText()).toContain("the Needle here");

    await fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect(screen.getByTestId("text-match").textContent).toBe("needle"));
    expect(mock.lastCall("viewer_search")).toMatchObject({ from: at + 1 });
    await fireEvent.keyDown(window, { key: "F3" });
    await waitFor(() => expect(screen.getByTestId("viewer-search-status").textContent).toContain("No more matches below"));
    await fireEvent.keyDown(window, { key: "F3", shiftKey: true });
    await waitFor(() => expect(screen.getByTestId("text-match").textContent).toBe("Needle"));
    expect(mock.lastCall("viewer_search")).toMatchObject({ backward: true });

    await fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("viewer-search")).not.toBeInTheDocument();
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.keyDown(window, { key: "Escape" });
    expect(onclose).toHaveBeenCalled();
  });

  it("finds hex bytes in hex mode", async () => {
    const { mock } = setup("data.txt", "zzzzOKzzzz");
    await waitFor(() => expect(shownText()).toEqual(["zzzzOKzzzz"]));
    await fireEvent.keyDown(window, { key: "2" });
    await fireEvent.keyDown(window, { key: "f", metaKey: true });
    const field = await screen.findByTestId("viewer-search");
    await userEvent.type(field, "4F 4B{Enter}");
    await waitFor(() => expect(document.querySelectorAll("[data-testid=hex-view] .m").length).toBe(4));
    expect(mock.lastCall("viewer_search")).toMatchObject({ needle: [0x4f, 0x4b], ignoreCase: false });
  });

  it("switches the encoding", async () => {
    setup("latin.txt", new Uint8Array([0x63, 0x61, 0x66, 0xe9]));
    await waitFor(() => expect(shownText()).toEqual(["caf�"]));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Encoding" }), "windows-1252");
    await waitFor(() => expect(shownText()).toEqual(["café"]));
  });

  it("F3 closes it when nothing is being searched, and the file is closed in the backend", async () => {
    const { onclose } = setup("notes.txt", "x\n");
    await waitFor(() => expect(shownText()).toEqual(["x"]));
    await fireEvent.keyDown(window, { key: "F3" });
    expect(onclose).toHaveBeenCalled();
  });

  it("shows an error when the file cannot be opened", async () => {
    const mock = installMockBackend();
    mock.failNext("viewer_open", { kind: "notFound", message: "not found: gone.txt" });
    render(Viewer, { location: { fs: "local", path: "/x" }, entry: { ...file("gone.txt", "x"), path: "/x" }, onclose: () => {} });
    expect(await screen.findByRole("alert")).toHaveTextContent("not found: gone.txt");
  });
});

describe("UI-UNIT-037 viewer api passes exact argument names", () => {
  it("records the arguments the Rust side expects", async () => {
    const mock = installMockBackend();
    const loc = { fs: "local", path: "/home/user/readme.md" };
    const f = await viewerApi.open(loc);
    expect(mock.lastCall("viewer_open")).toEqual({ location: loc });
    expect([...(await viewerApi.read(f.session, 2, 4))]).toEqual([...new TextEncoder().encode("Read")]);
    expect(mock.lastCall("viewer_read")).toEqual({ session: f.session, offset: 2, length: 4 });
    await viewerApi.search(f.session, 7, new Uint8Array([1, 2]), 5, true, false);
    expect(mock.lastCall("viewer_search")).toEqual({ session: f.session, request: 7, needle: [1, 2], from: 5, backward: true, ignoreCase: false });
    await viewerApi.countLines(f.session, 8, 3, "utf-16le");
    expect(mock.lastCall("viewer_count_lines")).toEqual({ session: f.session, request: 8, upto: 3, encoding: "utf-16le" });
    await viewerApi.cancel(8);
    expect(mock.lastCall("viewer_cancel")).toEqual({ request: 8 });
    await viewerApi.close(f.session);
    expect(mock.lastCall("viewer_close")).toEqual({ session: f.session });
  });
});

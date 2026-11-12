import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockConvertFileSrc } from "@tauri-apps/api/mocks";
import Inspector from "./Inspector.svelte";
import type { Entry } from "./api";
import { file, installMockBackend } from "../../tests/mock-backend";

function entry(name: string, size = 10, extra: Partial<Entry> = {}): Entry {
  const { content: _c, ...e } = file(name, "x".repeat(size), extra);
  return { ...e, path: `/home/user/${name}` };
}

function setup(e: Entry | null, texts: Record<string, { text: string; truncated?: boolean; binary?: boolean }> = {}) {
  const mock = installMockBackend();
  // Answer read_text from `texts` instead of the default tree.
  const reads = vi.fn((path: string) => {
    const t = texts[path];
    if (!t) throw { kind: "notFound", message: path };
    return { text: t.text, truncated: !!t.truncated, binary: !!t.binary };
  });
  (globalThis as any).__TAURI_INTERNALS__.invoke = ((orig) => (cmd: string, args: any, opts: any) =>
    cmd === "read_text" ? Promise.resolve().then(() => reads(args.location.path)) : orig(cmd, args, opts))(
    (globalThis as any).__TAURI_INTERNALS__.invoke,
  );
  const props = { entry: e, fs: "local", onsetmode: vi.fn(), onsaved: vi.fn() };
  const r = render(Inspector, { props });
  return { mock, props, ...r };
}

describe("UI-CMP-009 Inspector", () => {
  it("shows a hint with nothing selected", () => {
    setup(null);
    expect(screen.getByText(/Select an item/)).toBeInTheDocument();
  });

  it("previews by type: images as pictures, text in the quick editor", async () => {
    mockConvertFileSrc("linux");
    const img = setup(entry("photo.jpg", 2000));
    expect(await screen.findByRole("img", { name: "photo.jpg" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /^Edit / })).toBeNull();
    img.unmount();

    setup(entry("notes.txt"), { "/home/user/notes.txt": { text: "hello" } });
    const editor = await screen.findByRole("textbox", { name: "Edit notes.txt" });
    await waitFor(() => expect(editor).toHaveValue("hello"));
    expect(screen.getByText("Quick edit")).toBeInTheDocument();
  });

  it("marks edits, saves with Ctrl/Cmd S and reports Saved", async () => {
    const user = userEvent.setup();
    const { mock, props } = setup(entry("notes.txt"), { "/home/user/notes.txt": { text: "hello" } });
    const editor = await screen.findByRole("textbox", { name: "Edit notes.txt" });
    await waitFor(() => expect(editor).toBeEnabled());
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.type(editor, " world");
    expect(screen.getByText("Edited")).toBeInTheDocument();
    await user.keyboard("{Control>}s{/Control}");
    await waitFor(() => expect(screen.getByText("Saved")).toBeInTheDocument());
    expect(mock.lastCall("write_text")).toEqual({
      location: { fs: "local", path: "/home/user/notes.txt" },
      text: "hello world",
    });
    expect(props.onsaved).toHaveBeenCalled();
  });

  it("Revert restores the loaded text without saving", async () => {
    const user = userEvent.setup();
    const { mock } = setup(entry("notes.txt"), { "/home/user/notes.txt": { text: "hello" } });
    const editor = await screen.findByRole("textbox", { name: "Edit notes.txt" });
    await waitFor(() => expect(editor).toBeEnabled());
    await user.type(editor, "!!");
    await user.click(screen.getByRole("button", { name: "Revert" }));
    expect(editor).toHaveValue("hello");
    expect(mock.calls("write_text")).toEqual([]);
  });

  it("warns that a truncated file is too large and does not offer to edit binary files", async () => {
    const big = setup(entry("big.log"), { "/home/user/big.log": { text: "partial", truncated: true } });
    expect(await screen.findByText(/Too large to edit here/)).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /^Edit / })).toBeNull();
    big.unmount();

    setup(entry("data.txt"), { "/home/user/data.txt": { text: "", binary: true } });
    await waitFor(() => expect(screen.queryByRole("textbox", { name: "Edit data.txt" })).toBeNull());
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("does not even read a file over the edit limit", async () => {
    const { mock } = setup(entry("huge.txt", 0, { size: 5_000_000 }));
    expect(await screen.findByText(/Too large to edit here/)).toBeInTheDocument();
    expect(mock.calls("read_text")).toEqual([]);
  });
});

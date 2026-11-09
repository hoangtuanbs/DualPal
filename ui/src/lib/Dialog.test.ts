import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import Dialog from "./Dialog.svelte";

describe("UI-CMP-001 Dialog", () => {
  it("confirms on Enter with the trimmed input", async () => {
    const user = userEvent.setup();
    const onconfirm = vi.fn();
    render(Dialog, { props: { title: "New Folder", input: "untitled", onconfirm, oncancel: () => {} } });

    const input = screen.getByRole("textbox");
    await user.clear(input);
    await user.type(input, "  My Folder  ");
    await user.keyboard("{Enter}");

    expect(onconfirm).toHaveBeenCalledWith("My Folder", true);
  });

  it("cancels on Escape", async () => {
    const user = userEvent.setup();
    const oncancel = vi.fn();
    render(Dialog, { props: { title: "Delete?", message: "Sure?", danger: true, onconfirm: () => {}, oncancel } });

    await user.keyboard("{Escape}");
    expect(oncancel).toHaveBeenCalled();
  });

  it("refuses an empty required input", async () => {
    const user = userEvent.setup();
    const onconfirm = vi.fn();
    render(Dialog, { props: { title: "Rename", input: "name", onconfirm, oncancel: () => {} } });

    const input = screen.getByRole("textbox");
    await user.clear(input);
    await user.keyboard("{Enter}");
    expect(onconfirm).not.toHaveBeenCalled();
  });
});

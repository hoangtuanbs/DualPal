import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { mockIPC } from "@tauri-apps/api/mocks";
import MultiRename from "./MultiRename.svelte";
import type { Entry, RenamePreset } from "./api";
import { file } from "../../tests/mock-backend";

/** A backend whose preview applies plain Replace rules and flags clashes,
 *  and which keeps presets. */
function backend(presets: RenamePreset[] = []) {
  const calls: { cmd: string; args: any }[] = [];
  mockIPC((cmd, raw) => {
    const args = raw as any;
    calls.push({ cmd, args });
    switch (cmd) {
      case "rename_presets":
        return presets;
      case "save_rename_preset":
        presets = [...presets.filter((p) => p.name !== args.preset.name), args.preset];
        return presets;
      case "remove_rename_preset":
        presets = presets.filter((p) => p.name !== args.name);
        return presets;
      case "rename_preview": {
        const out = args.inputs.map((i: { name: string }) => {
          let n = i.name;
          for (const r of args.rules) if (r.type === "replace" && r.find) n = n.split(r.find).join(r.replace);
          return { name: i.name, newName: n, issue: null as string | null };
        });
        for (const o of out) {
          const twins = out.filter((x: { newName: string }) => x.newName === o.newName).length;
          if (twins > 1 || args.others.includes(o.newName)) o.issue = "name already used";
          if (o.newName.includes("(")) throw { kind: "invalid", message: "invalid pattern" };
        }
        return out;
      }
    }
    return null;
  });
  return { calls: (c: string) => calls.filter((x) => x.cmd === c).map((x) => x.args) };
}

function entry(name: string): Entry {
  const { content: _c, ...e } = file(name);
  return { ...e, path: `/pics/${name}` };
}

function setup() {
  const props = {
    items: [entry("IMG_1.jpg"), entry("IMG_2.jpg")],
    folder: { fs: "local", path: "/pics" },
    others: ["holiday-3.jpg"],
    onclose: vi.fn(),
    onrename: vi.fn(),
  };
  render(MultiRename, { props });
  return props;
}

const renameButton = () => screen.getByRole("button", { name: /^Rename$/ });

describe("UI-CMP-006 MultiRename", () => {
  it("previews live and renames the changed names", async () => {
    const user = userEvent.setup();
    backend();
    const p = setup();
    await waitFor(() => expect(screen.getByText("0 of 2 names change")).toBeInTheDocument());
    expect(renameButton()).toBeDisabled();

    await user.type(screen.getByLabelText("Find"), "IMG_");
    await user.type(screen.getByLabelText("Replace with"), "holiday-");
    await waitFor(() => expect(screen.getByText("2 of 2 names change")).toBeInTheDocument());
    const preview = within(screen.getByLabelText("Preview"));
    expect(preview.getByText("holiday-1.jpg")).toBeInTheDocument();

    await user.click(renameButton());
    expect(p.onrename).toHaveBeenCalledWith([
      { location: { fs: "local", path: "/pics/IMG_1.jpg" }, newName: "holiday-1.jpg" },
      { location: { fs: "local", path: "/pics/IMG_2.jpg" }, newName: "holiday-2.jpg" },
    ]);
  });

  it("highlights issues and disables Rename while any name has one", async () => {
    const user = userEvent.setup();
    backend();
    const p = setup();
    // IMG_2 → holiday-3.jpg clashes with a name already in the folder.
    await user.type(screen.getByLabelText("Find"), "IMG_2");
    await user.type(screen.getByLabelText("Replace with"), "holiday-3");
    await waitFor(() => expect(screen.getByText("1 name needs fixing")).toBeInTheDocument());
    expect(screen.getByText("name already used")).toBeInTheDocument();
    expect(document.querySelector(".prow.bad")).not.toBeNull();
    expect(renameButton()).toBeDisabled();
    await user.click(renameButton());
    expect(p.onrename).not.toHaveBeenCalled();
  });

  it("shows an error from the preview instead of crashing", async () => {
    const user = userEvent.setup();
    backend();
    setup();
    await user.type(screen.getByLabelText("Replace with"), "(");
    await user.type(screen.getByLabelText("Find"), "IMG");
    await waitFor(() => expect(screen.getByText("invalid pattern")).toBeInTheDocument());
    expect(renameButton()).toBeDisabled();
  });

  it("saves a preset and loads it back", async () => {
    const user = userEvent.setup();
    const be = backend([{ name: "Lower", rules: [{ type: "case", case: "lower", scope: "whole" }], builtin: true }]);
    setup();
    await user.type(screen.getByLabelText("Find"), "IMG_");
    await user.click(screen.getByRole("button", { name: "Save Preset…" }));
    await user.type(screen.getByPlaceholderText("Preset name"), "Strip IMG");
    await user.click(screen.getByRole("button", { name: /^Save$/ }));
    await waitFor(() => expect(be.calls("save_rename_preset")).toHaveLength(1));
    expect(be.calls("save_rename_preset")[0].preset).toEqual({
      name: "Strip IMG",
      rules: [{ type: "replace", find: "IMG_", replace: "", regex: false, caseSensitive: false, scope: "name" }],
      builtin: false,
    });
    // A user preset can be deleted; loading the built-in one replaces the rules.
    expect(screen.getByRole("button", { name: "Delete preset" })).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("Preset"), "Lower");
    expect(screen.queryByLabelText("Find")).toBeNull();
    expect(screen.getByText("1. Change case")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete preset" })).toBeNull();
    await user.selectOptions(screen.getByLabelText("Preset"), "Strip IMG");
    expect(screen.getByLabelText("Find")).toHaveValue("IMG_");
  });

  it("Escape and Cancel close without renaming", async () => {
    const user = userEvent.setup();
    backend();
    const p = setup();
    await user.keyboard("{Escape}");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(p.onclose).toHaveBeenCalledTimes(2);
    expect(p.onrename).not.toHaveBeenCalled();
  });
});

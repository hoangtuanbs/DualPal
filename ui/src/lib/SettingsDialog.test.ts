import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { installMockBackend, type MockHandle } from "../../tests/mock-backend";
import SettingsDialog, { type KeyCommand } from "./SettingsDialog.svelte";
import { applyAppearance, prefs } from "./prefs.svelte";
import { api, type Settings } from "./api";

const commands: KeyCommand[] = [
  { id: "copy", title: "Copy to Other Pane", group: "File" },
  { id: "refresh", title: "Refresh", group: "View" },
  { id: "swap", title: "Swap Panes", group: "View" },
];

let mock: MockHandle;

async function open(section: "appearance" | "panes" | "apps" | "keyboard" = "appearance") {
  await prefs.load();
  const onclose = vi.fn();
  render(SettingsDialog, { commands, mac: false, start: section, onclose });
  return { onclose, user: userEvent.setup() };
}

const lastPatch = () => mock.lastCall("update_settings")?.patch as Record<string, unknown> | undefined;
const keyRow = (id: string) => screen.getAllByTestId("key-row").find((r) => r.dataset.command === id)!;

beforeEach(() => {
  mock = installMockBackend();
  mock.setSettings({ editor: null, diffTool: null });
  applyAppearance({ editor: null, diffTool: null });
});

describe("SettingsDialog", () => {
  it("UI-UNIT-025 appearance choices apply at once and are saved", async () => {
    const { user } = await open();
    await user.click(screen.getByLabelText("Dark"));
    expect(document.documentElement.dataset.theme).toBe("dark");
    await waitFor(() => expect(lastPatch()).toEqual({ theme: "dark" }));
    await user.click(screen.getByLabelText("Large"));
    expect(document.documentElement.dataset.fontSize).toBe("large");
    await user.click(screen.getByLabelText("Compact"));
    expect(document.documentElement.dataset.density).toBe("compact");
    await waitFor(() => expect(lastPatch()).toEqual({ density: "compact" }));
    // System leaves the theme to the system again.
    await user.click(screen.getByLabelText("System"));
    expect(document.documentElement.dataset.theme).toBeUndefined();
    await waitFor(() => expect(prefs.settings).toMatchObject({ theme: "system", fontSize: "large", density: "compact" }));
  });

  it("UI-UNIT-026 panes and apps sections save their fields; Esc closes", async () => {
    const { user, onclose } = await open("panes");
    await user.click(screen.getByTestId("settings-show-hidden"));
    await waitFor(() => expect(lastPatch()).toEqual({ showHidden: true }));
    await user.click(screen.getByLabelText("Size"));
    await waitFor(() => expect(lastPatch()).toEqual({ hiddenColumns: ["size"] }));
    // Right pane: a chosen folder.
    const right = screen.getByRole("group", { name: "Right pane opens at start" });
    await user.click(within(right).getByLabelText("This folder:"));
    await user.type(screen.getByLabelText("Right pane folder"), "/srv/data{Enter}");
    await user.tab();
    await waitFor(() =>
      expect(lastPatch()).toEqual({
        paneStart: [
          { mode: "home", folder: null },
          { mode: "folder", folder: "/srv/data" },
        ],
      }),
    );

    await user.click(screen.getByTestId("settings-tab-apps"));
    await user.type(screen.getByTestId("settings-editor"), "/usr/bin/code");
    await user.tab();
    await waitFor(() => expect(lastPatch()).toEqual({ editor: "/usr/bin/code" }));
    await user.click(screen.getByTestId("settings-auto-update"));
    await waitFor(() => expect(lastPatch()).toEqual({ updateChecks: false }));

    await user.keyboard("{Escape}");
    expect(onclose).toHaveBeenCalled();
  });

  it("UI-UNIT-027 records a shortcut, offers to replace a conflict, and resets", async () => {
    const { user, onclose } = await open("keyboard");
    expect(within(keyRow("refresh")).getByText("Ctrl+R")).toBeInTheDocument();

    // A new key for a command without one.
    await user.click(screen.getByRole("button", { name: "Add a shortcut for Swap Panes" }));
    expect(within(keyRow("swap")).getByText("Press keys…")).toBeInTheDocument();
    await user.keyboard("{F9}");
    await waitFor(() => expect(lastPatch()).toEqual({ keyBindings: { swap: ["F9"] } }));
    expect(within(keyRow("swap")).getByText("F9")).toBeInTheDocument();

    // Esc while recording stops recording and does not close the window.
    await user.click(screen.getByRole("button", { name: "Add a shortcut for Copy to Other Pane" }));
    await user.keyboard("{Escape}");
    expect(onclose).not.toHaveBeenCalled();
    expect(within(keyRow("copy")).queryByText("Press keys…")).toBeNull();

    // Copy's F5 replaced by Ctrl+R, which Refresh has: asks first.
    await user.click(within(keyRow("copy")).getByText("F5"));
    await user.keyboard("{Control>}r{/Control}");
    const conflict = screen.getByTestId("key-conflict");
    expect(conflict).toHaveTextContent("Ctrl+R is used by “Refresh”");
    await user.click(within(conflict).getByRole("button", { name: "Replace" }));
    await waitFor(() => expect(lastPatch()).toEqual({ keyBindings: { swap: ["F9"], copy: ["Mod+R"], refresh: [] } }));
    expect(within(keyRow("copy")).getByText("Ctrl+R")).toBeInTheDocument();
    expect(within(keyRow("refresh")).queryByText("Ctrl+R")).toBeNull();

    // A key that moves the cursor cannot be bound.
    await user.click(screen.getByRole("button", { name: "Add a shortcut for Refresh" }));
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("status")).toHaveTextContent("cannot be changed");

    // Reset Refresh: Copy has its Ctrl+R now, so it asks first; Cancel keeps it.
    await user.click(within(keyRow("refresh")).getByRole("button", { name: "Reset" }));
    expect(screen.getByTestId("key-conflict")).toHaveTextContent("Ctrl+R is used by “Copy to Other Pane”");
    await user.click(within(screen.getByTestId("key-conflict")).getByRole("button", { name: "Cancel" }));
    expect(within(keyRow("refresh")).queryByText("Ctrl+R")).toBeNull();
    await user.click(within(keyRow("refresh")).getByRole("button", { name: "Reset" }));
    await user.click(within(screen.getByTestId("key-conflict")).getByRole("button", { name: "Replace" }));
    await waitFor(() => expect(lastPatch()).toEqual({ keyBindings: { swap: ["F9"], copy: [] } }));
    expect(within(keyRow("refresh")).getByText("Ctrl+R")).toBeInTheDocument();
    expect(within(keyRow("copy")).queryByText("Ctrl+R")).toBeNull();
    await user.click(screen.getByTestId("keys-reset-all"));
    await waitFor(() => expect(lastPatch()).toEqual({ keyBindings: {} }));
    expect(within(keyRow("copy")).getByText("F5")).toBeInTheDocument();
    expect(within(keyRow("refresh")).getByText("Ctrl+R")).toBeInTheDocument();
  });

  it("UI-UNIT-029 Tab stays inside Settings, and the focus goes back when it closes", async () => {
    const outside = document.createElement("button");
    outside.textContent = "Behind";
    document.body.append(outside);
    outside.focus();
    await prefs.load();
    const { unmount } = render(SettingsDialog, { commands, mac: false, start: "appearance", onclose: vi.fn() });
    const user = userEvent.setup();
    const dialog = screen.getByTestId("settings-dialog");
    // Shift Tab from the first control goes to the last; Tab from the last to the first.
    screen.getByTestId("settings-tab-appearance").focus();
    await user.tab({ shift: true });
    expect(document.activeElement).toBe(screen.getByLabelText("Normal", { selector: "[name=density]" }));
    await user.tab();
    expect(document.activeElement).toBe(screen.getByTestId("settings-tab-appearance"));
    for (let i = 0; i < 12; i++) {
      await user.tab();
      expect(dialog.contains(document.activeElement)).toBe(true);
    }
    unmount();
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });

  it("UI-UNIT-027 switching section stops recording, and the dialog follows the section asked for", async () => {
    await prefs.load();
    const onclose = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(SettingsDialog, { commands, mac: false, start: "keyboard", onclose });
    await user.click(screen.getByRole("button", { name: "Add a shortcut for Swap Panes" }));
    expect(within(keyRow("swap")).getByText("Press keys…")).toBeInTheDocument();
    // Typing in Apps goes to the field, not to the shortcut.
    await user.click(screen.getByRole("tab", { name: "Apps" }));
    const before = mock.calls("update_settings").length;
    await user.keyboard("{F9}");
    expect(mock.calls("update_settings").length).toBe(before);
    await user.click(screen.getByRole("tab", { name: "Keyboard" }));
    expect(within(keyRow("swap")).queryByText("Press keys…")).toBeNull();
    expect(within(keyRow("swap")).queryByText("F9")).toBeNull();

    // The app asks for Appearance while the dialog is open.
    await rerender({ commands, mac: false, start: "appearance", onclose });
    await waitFor(() => expect(screen.getByRole("heading", { name: "Appearance" })).toBeInTheDocument());
  });

  it("UI-UNIT-025 a failed save does not read back over a later change", async () => {
    await prefs.load();
    let fail!: (e: unknown) => void;
    const spy = vi.spyOn(api, "updateSettings");
    spy.mockImplementationOnce(() => new Promise((_, reject) => (fail = reject)));
    spy.mockImplementationOnce(async (patch) => ({ ...prefs.settings, ...patch }) as Settings);
    const read = vi.spyOn(api, "settings");
    try {
      const first = prefs.update({ theme: "dark" });
      await prefs.update({ density: "compact" });
      fail({ kind: "io", message: "disk full" });
      await expect(first).rejects.toBeTruthy();
      expect(read).not.toHaveBeenCalled();
      expect(prefs.settings.density).toBe("compact");
    } finally {
      vi.restoreAllMocks();
    }
  });
});

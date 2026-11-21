import { describe, expect, it } from "vitest";
import {
  DEFAULT_KEYS,
  assignCombo,
  comboFromEvent,
  conflicts,
  effectiveBindings,
  formatCombo,
  formatKeys,
  lookup,
  normalizeCombo,
  resetBinding,
  unbindable,
  withBinding,
  worksInFields,
  type KeyEventLike,
} from "./keymap";

/** A key press as the browser reports it. `code` defaults from the key. */
function press(key: string, mods: Partial<Pick<KeyEventLike, "ctrlKey" | "metaKey" | "altKey" | "shiftKey">> = {}, code?: string): KeyEventLike {
  const guess = /^[a-z]$/i.test(key) ? `Key${key.toUpperCase()}` : /^[0-9]$/.test(key) ? `Digit${key}` : key === "." ? "Period" : key === "," ? "Comma" : key;
  return { key, code: code ?? guess, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods };
}

const defaults = effectiveBindings({});

describe("keymap", () => {
  // UI-UNIT-021 — the default bindings run the same commands for the same
  // keys as the README keyboard table, on Windows/Linux and on a Mac.
  it("UI-UNIT-021 default bindings match today's shortcuts", () => {
    const ctrl = { ctrlKey: true };
    const table: [KeyEventLike, string][] = [
      [press("Tab"), "switch"],
      [press("Tab", { shiftKey: true }), "switch"],
      [press("Backspace"), "up"],
      [press("ArrowUp", ctrl), "up"],
      [press("Enter", { altKey: true, shiftKey: true }), "folderSizes"],
      [press("a", ctrl), "selectAll"],
      [press("F2"), "rename"],
      [press("F2", { shiftKey: true }), "multiRename"],
      [press("F4"), "edit"],
      [press("F5"), "copy"],
      [press("F6"), "move"],
      [press("F7"), "newFolder"],
      [press("F8"), "trash"],
      [press("Delete"), "trash"],
      [press("Backspace", ctrl), "trash"],
      [press("F8", { shiftKey: true }), "delete"],
      [press("Delete", { shiftKey: true }), "delete"],
      [press("Backspace", { ctrlKey: true, altKey: true }), "delete"],
      [press("+"), "selectPattern"],
      [press("+", { shiftKey: true }, "Equal"), "selectPattern"],
      [press("-"), "deselectPattern"],
      [press("f", ctrl), "filter"],
      [press("F", { ctrlKey: true, shiftKey: true }), "find"],
      [press("1", ctrl), "list"],
      [press("2", ctrl), "icons"],
      [press("t", ctrl), "newTab"],
      [press("w", ctrl), "closeTab"],
      [press("Tab", ctrl), "nextTab"],
      [press("Tab", { ctrlKey: true, shiftKey: true }), "prevTab"],
      [press("i", ctrl), "info"],
      [press("r", ctrl), "refresh"],
      [press(">", { ctrlKey: true, shiftKey: true }, "Period"), "hidden"],
      [press(".", { ctrlKey: true, shiftKey: true }), "hidden"],
      [press("P", { ctrlKey: true, shiftKey: true }), "palette"],
      [press("T", { ctrlKey: true, shiftKey: true }), "terminal"],
      [press("Y", { ctrlKey: true, shiftKey: true }), "compareFolders"],
      [press("D", { ctrlKey: true, shiftKey: true }), "compareFiles"],
      [press("L", { ctrlKey: true, shiftKey: true }), "linked"],
      [press(",", ctrl), "settings"],
    ];
    for (const [e, id] of table) {
      expect(lookup(defaults, e, false)?.id, `${JSON.stringify(e)} (Windows/Linux)`).toBe(id);
      // On a Mac ⌘ is the modifier; Ctrl still works for it, and Ctrl Tab is Ctrl.
      const mac = e.ctrlKey && e.key !== "Tab" ? { ...e, ctrlKey: false, metaKey: true } : e;
      expect(lookup(defaults, mac, true)?.id, `${JSON.stringify(mac)} (Mac)`).toBe(id);
      expect(lookup(defaults, e, true)?.id, `${JSON.stringify(e)} (Mac, Ctrl)`).toBe(id);
    }
    // Keys that move the cursor are not bindings, and plain letters do nothing.
    for (const e of [press("ArrowDown"), press("Enter"), press(" "), press("Home"), press("a"), press("F10", { shiftKey: true })]) {
      expect(lookup(defaults, e, false)).toBeNull();
    }
    // The palette shows the keys as it did before key bindings existed.
    expect(formatCombo("Mod+Shift+T", false)).toBe("Ctrl+Shift+T");
    expect(formatCombo("Mod+Shift+.", false)).toBe("Ctrl+Shift+.");
    expect(formatCombo("Mod+ArrowUp", false)).toBe("Ctrl+↑");
    expect(formatCombo("-", false)).toBe("−");
    expect(formatCombo("Alt+Shift+Enter", false)).toBe("Alt+Shift+Enter");
    expect(formatCombo("Mod+Shift+P", true)).toBe("⇧⌘P");
    expect(formatCombo("Ctrl+Tab", true)).toBe("⌃Tab");
    expect(formatKeys(DEFAULT_KEYS.trash, false)).toBe("F8, Delete, Ctrl+Backspace");
  });

  it("UI-UNIT-022 records a key press as a combination", () => {
    expect(comboFromEvent(press("k", { ctrlKey: true, shiftKey: true }), false)).toBe("Mod+Shift+K");
    expect(comboFromEvent(press("k", { metaKey: true }), true)).toBe("Mod+K");
    expect(comboFromEvent(press("k", { ctrlKey: true }), true)).toBe("Ctrl+K");
    // A Mac's Alt turns letters into symbols: the key position wins.
    expect(comboFromEvent(press("®", { altKey: true }, "KeyR"), true)).toBe("Alt+R");
    expect(comboFromEvent(press("F9"), false)).toBe("F9");
    expect(comboFromEvent(press(" "), false)).toBe("Space");
    // A modifier on its own is not a shortcut yet.
    expect(comboFromEvent(press("Shift", { shiftKey: true }, "ShiftLeft"), false)).toBeNull();
    expect(comboFromEvent(press("Control", { ctrlKey: true }, "ControlLeft"), false)).toBeNull();
    // Recorded and default spellings agree; Ctrl and ⌘ are one key off the Mac.
    expect(normalizeCombo("Ctrl+Tab", false)).toBe("Mod+Tab");
    expect(normalizeCombo("Ctrl+Tab", true)).toBe("Ctrl+Tab");
    expect(normalizeCombo("shift+mod+p", false)).toBe("Mod+Shift+P");
    // A recorded binding is found again.
    const mine = effectiveBindings({ refresh: ["F9"] });
    expect(lookup(mine, press("F9"), false)?.id).toBe("refresh");
    expect(lookup(mine, press("r", { ctrlKey: true }), false)).toBeNull();
    // Keys that move the cursor, and Shift F10, cannot be bound.
    expect(unbindable("ArrowDown")).toMatch(/cannot be changed/);
    expect(unbindable("Shift+ArrowDown")).toMatch(/cannot be changed/);
    expect(unbindable("Shift+F10")).toMatch(/menu/);
    expect(unbindable("Mod+ArrowDown")).toBeNull();
    expect(unbindable("F9")).toBeNull();
    // Only Ctrl/⌘ and function keys work while typing in a field.
    expect(worksInFields("Mod+,")).toBe(true);
    expect(worksInFields("F9")).toBe(true);
    expect(worksInFields("P")).toBe(false);
  });

  it("UI-UNIT-023 finds conflicts and moves a key that is replaced", () => {
    expect(conflicts(defaults, "Mod+R", null, false)).toEqual(["refresh"]);
    expect(conflicts(defaults, "Ctrl+R", null, false)).toEqual(["refresh"]);
    expect(conflicts(defaults, "Ctrl+R", null, true)).toEqual([]);
    expect(conflicts(defaults, "Mod+R", "refresh", false)).toEqual([]);
    expect(conflicts(defaults, "F9", null, false)).toEqual([]);

    // Give Ctrl R to Copy: Refresh loses it, Copy keeps F5 too.
    const next = assignCombo({}, "copy", "Mod+R", null, false);
    expect(next).toEqual({ copy: ["F5", "Mod+R"], refresh: [] });
    const b = effectiveBindings(next);
    expect(lookup(b, press("r", { ctrlKey: true }), false)?.id).toBe("copy");
    expect(lookup(b, press("F5"), false)?.id).toBe("copy");

    // Replacing one key of a command keeps its others.
    expect(assignCombo({}, "trash", "F9", "Delete", false)).toEqual({ trash: ["F8", "F9", "Mod+Backspace"] });
    // Back to the defaults: nothing is stored.
    expect(assignCombo({ copy: ["F9"] }, "copy", "F5", "F9", false)).toEqual({});
  });

  it("UI-UNIT-024 resets one command or all", () => {
    let o = withBinding({}, "refresh", ["F9"], false);
    o = withBinding(o, "copy", [], false);
    expect(effectiveBindings(o).refresh).toEqual(["F9"]);
    expect(effectiveBindings(o).copy).toEqual([]);
    expect(lookup(effectiveBindings(o), press("F5"), false)).toBeNull();
    o = resetBinding(o, "refresh");
    expect(o).toEqual({ copy: [] });
    expect(effectiveBindings(o).refresh).toEqual(["Mod+R"]);
    expect(effectiveBindings({})).toEqual(Object.fromEntries(Object.entries(DEFAULT_KEYS).map(([k, v]) => [k, [...v]])));
    // The same keys as the defaults, in another spelling, store nothing.
    expect(withBinding({}, "nextTab", ["Mod+Tab"], false)).toEqual({});
    expect(withBinding({}, "refresh", ["Mod+R", "mod+r"], false)).toEqual({});
  });
});

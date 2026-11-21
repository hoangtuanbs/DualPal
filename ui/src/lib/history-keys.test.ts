import { describe, expect, it } from "vitest";
import { historyKey } from "./history-keys";

const press = (key: string, mods: Partial<Record<"altKey" | "ctrlKey" | "metaKey" | "shiftKey", boolean>> = {}) => ({
  key,
  altKey: false,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  ...mods,
});

describe("UI-KEY-036 back and forward keys", () => {
  it("Alt+Left and Alt+Right go back and forward on every platform", () => {
    for (const mac of [false, true]) {
      expect(historyKey(press("ArrowLeft", { altKey: true }), mac)).toBe("back");
      expect(historyKey(press("ArrowRight", { altKey: true }), mac)).toBe("forward");
    }
  });

  it("⌘[ and ⌘] go back and forward on macOS only", () => {
    expect(historyKey(press("[", { metaKey: true }), true)).toBe("back");
    expect(historyKey(press("]", { metaKey: true }), true)).toBe("forward");
    expect(historyKey(press("[", { metaKey: true }), false)).toBeNull();
  });

  it("leaves plain arrows and other combinations alone", () => {
    expect(historyKey(press("ArrowLeft"), false)).toBeNull();
    expect(historyKey(press("ArrowLeft", { altKey: true, shiftKey: true }), false)).toBeNull();
    expect(historyKey(press("ArrowLeft", { altKey: true, ctrlKey: true }), false)).toBeNull();
    expect(historyKey(press("[", { ctrlKey: true }), true)).toBeNull();
    expect(historyKey(press("a", { altKey: true }), false)).toBeNull();
  });
});

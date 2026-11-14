import { afterEach, describe, expect, it } from "vitest";
import { SPLIT_DEFAULT, SPLIT_MIN_PX, clampSplit, loadSplit, saveSplit, splitAt, splitForKey } from "./split";

describe("pane split", () => {
  afterEach(() => localStorage.clear());

  it("keeps both panes at least the minimum width", () => {
    expect(clampSplit(0.01, 1000)).toBe(SPLIT_MIN_PX / 1000);
    expect(clampSplit(0.99, 1000)).toBe(1 - SPLIT_MIN_PX / 1000);
    expect(clampSplit(0.3, 1000)).toBe(0.3);
    // Too narrow for two minimum panes: stay centred.
    expect(clampSplit(0.2, 200)).toBe(0.5);
    expect(clampSplit(Number.NaN, 1000)).toBe(SPLIT_DEFAULT);
  });

  it("follows the pointer across the panes", () => {
    expect(splitAt(400, 100, 1000)).toBe(0.3);
    expect(splitAt(0, 100, 1000)).toBe(SPLIT_MIN_PX / 1000);
    expect(splitAt(50, 0, 0)).toBe(SPLIT_DEFAULT);
  });

  it("moves with arrow keys, Home, End and Enter", () => {
    expect(splitForKey(0.5, "ArrowLeft", false, 1000)).toBeCloseTo(0.48);
    expect(splitForKey(0.5, "ArrowRight", true, 1000)).toBeCloseTo(0.6);
    expect(splitForKey(0.5, "Home", false, 1000)).toBe(SPLIT_MIN_PX / 1000);
    expect(splitForKey(0.5, "End", false, 1000)).toBe(1 - SPLIT_MIN_PX / 1000);
    expect(splitForKey(0.3, "Enter", false, 1000)).toBe(SPLIT_DEFAULT);
    expect(splitForKey(0.5, "a", false, 1000)).toBeNull();
  });

  it("remembers the split, and starts centred without one", () => {
    expect(loadSplit()).toBe(SPLIT_DEFAULT);
    saveSplit(0.3456);
    expect(loadSplit()).toBe(0.346);
    localStorage.setItem("dualpal.split", "junk");
    expect(loadSplit()).toBe(SPLIT_DEFAULT);
  });
});

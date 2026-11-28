import { describe, expect, it } from "vitest";
import { dateRange, defaultForm, hasCriteria, parseSize, toCriteria } from "./find-session.svelte";

describe("UI-UNIT-050 Find form: sizes, dates and criteria", () => {
  it("parses sizes in the size column's units", () => {
    expect(parseSize("")).toBeNull();
    expect(parseSize("  ")).toBeNull();
    expect(parseSize("500")).toBe(500);
    expect(parseSize("10 MB")).toBe(10_000_000);
    expect(parseSize("1.5k")).toBe(1500);
    expect(parseSize("2,5 GB")).toBe(2_500_000_000);
    expect(parseSize("ten")).toBeUndefined();
    expect(parseSize("5 parsecs")).toBeUndefined();
  });

  it("turns presets into modified-date limits from local midnight", () => {
    const now = new Date(2026, 10, 28, 15, 30);
    const midnight = new Date(2026, 10, 28).getTime();
    expect(dateRange("any", "", now)).toEqual({});
    expect(dateRange("today", "", now)).toEqual({ newerThanMs: midnight });
    expect(dateRange("7d", "", now)).toEqual({ newerThanMs: midnight - 6 * 86_400_000 });
    expect(dateRange("30d", "", now)).toEqual({ newerThanMs: midnight - 29 * 86_400_000 });
    expect(dateRange("newer", "2026-01-02", now)).toEqual({ newerThanMs: new Date(2026, 0, 2).getTime() });
    expect(dateRange("older", "2026-01-02", now)).toEqual({ olderThanMs: new Date(2026, 0, 2).getTime() });
    expect(dateRange("older", "", now)).toBeUndefined();
  });

  it("builds find_files criteria and reports bad input", () => {
    const form = { ...defaultForm(), name: " *.rs;*.toml ", content: "fn main", minSize: "1 KB", date: "today" as const };
    const now = new Date(2026, 10, 28, 9);
    expect(toCriteria(form, now)).toEqual({
      query: "*.rs;*.toml",
      criteria: {
        nameRegex: false,
        content: "fn main",
        contentRegex: false,
        ignoreCase: true,
        includeBinary: false,
        includeHidden: false,
        kind: "any",
        minSize: 1000,
        maxSize: null,
        newerThanMs: new Date(2026, 10, 28).getTime(),
        olderThanMs: null,
      },
    });
    expect(toCriteria({ ...form, maxSize: "big" })).toEqual({ problem: expect.stringContaining("Sizes") });
    expect(toCriteria({ ...form, date: "newer" })).toEqual({ problem: "Choose a day." });
  });

  it("needs a name, text, size or date to search", () => {
    expect(hasCriteria(defaultForm())).toBe(false);
    expect(hasCriteria({ ...defaultForm(), kind: "files", includeHidden: true })).toBe(false);
    expect(hasCriteria({ ...defaultForm(), content: "x" })).toBe(true);
    expect(hasCriteria({ ...defaultForm(), maxSize: "1" })).toBe(true);
    expect(hasCriteria({ ...defaultForm(), date: "7d" })).toBe(true);
  });
});

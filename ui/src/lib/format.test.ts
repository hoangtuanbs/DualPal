import { describe, expect, it } from "vitest";
import { formatBytes, formatDate, formatShortDate, formatTime } from "./format";

describe("UI-UNIT-001 formatBytes boundaries", () => {
  it("formats bytes, kilobytes and terabytes with decimal units", () => {
    expect(formatBytes(0)).toBe("0 bytes");
    expect(formatBytes(1)).toBe("1 byte");
    expect(formatBytes(999)).toBe("999 bytes");
    expect(formatBytes(1000)).toBe("1.0 KB");
    expect(formatBytes(10_000)).toBe("10 KB");
    expect(formatBytes(1_000_000_000_000)).toBe("1.0 TB");
    expect(formatBytes(1_000_000_000_000_000)).toBe("1000 TB");
  });
});

describe("UI-UNIT-002 formatDate / formatShortDate null", () => {
  it("returns an empty string for null", () => {
    expect(formatDate(null)).toBe("");
    expect(formatShortDate(null)).toBe("");
  });

  it("returns a non-empty string for a timestamp", () => {
    expect(formatDate(1_700_000_000_000)).not.toBe("");
    expect(formatShortDate(1_700_000_000_000)).not.toBe("");
  });

  it("formatTime returns a time string", () => {
    expect(formatTime(1_700_000_000_000)).not.toBe("");
  });
});

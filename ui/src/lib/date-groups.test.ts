import { describe, expect, it } from "vitest";
import { dateBoundaries, dateGroup, firstDayOfWeek } from "./date-groups";

// Friday 27 November 2026, 15:39 local time, as in Explorer's example.
const NOW = new Date(2026, 10, 27, 15, 39);
const at = (m: number, d: number, h = 12) => new Date(2026, m, d, h).getTime();

describe("dateGroup", () => {
  const monday = dateBoundaries(NOW, 1);

  it("puts times in Explorer's groups", () => {
    expect(dateGroup(at(10, 27, 0), monday)).toBe("today");
    expect(dateGroup(at(10, 26, 23), monday)).toBe("yesterday");
    expect(dateGroup(at(10, 24), monday)).toBe("thisWeek");
    expect(dateGroup(at(10, 23, 0), monday)).toBe("thisWeek");
    expect(dateGroup(at(10, 22), monday)).toBe("lastWeek");
    expect(dateGroup(at(10, 16, 9), monday)).toBe("lastWeek");
    expect(dateGroup(at(10, 13), monday)).toBe("thisMonth");
    expect(dateGroup(at(10, 1), monday)).toBe("thisMonth");
    expect(dateGroup(at(9, 31), monday)).toBe("lastMonth");
    expect(dateGroup(at(9, 1, 0), monday)).toBe("lastMonth");
    expect(dateGroup(at(8, 30), monday)).toBe("thisYear");
    expect(dateGroup(at(0, 1, 0), monday)).toBe("thisYear");
    expect(dateGroup(new Date(2025, 11, 31).getTime(), monday)).toBe("older");
    expect(dateGroup(at(10, 28, 0), monday)).toBe("future");
    expect(dateGroup(null, monday)).toBe("unknown");
  });

  it("starts the week on the locale's first day", () => {
    const sunday = dateBoundaries(NOW, 7);
    // Sunday the 22nd belongs to this week when weeks start on Sunday.
    expect(dateGroup(at(10, 22), sunday)).toBe("thisWeek");
    expect(dateGroup(at(10, 21), sunday)).toBe("lastWeek");
    expect(dateGroup(at(10, 15), sunday)).toBe("lastWeek");
    expect(dateGroup(at(10, 14), sunday)).toBe("thisMonth");
  });

  it("lets last week reach into last month", () => {
    // Tuesday 3 March 2026: last week began Monday 23 February.
    const b = dateBoundaries(new Date(2026, 2, 3, 10), 1);
    expect(dateGroup(new Date(2026, 1, 24).getTime(), b)).toBe("lastWeek");
    expect(dateGroup(new Date(2026, 1, 20).getTime(), b)).toBe("lastMonth");
    // Sunday 1 March is in last week, not "earlier this month".
    expect(dateGroup(new Date(2026, 2, 1).getTime(), b)).toBe("lastWeek");
    expect(dateGroup(new Date(2026, 2, 2).getTime(), b)).toBe("yesterday");
  });

  it("knows the first day of the week by locale", () => {
    expect([1, 7]).toContain(firstDayOfWeek("en-US"));
    expect(firstDayOfWeek("not a locale!")).toBe(1);
  });
});

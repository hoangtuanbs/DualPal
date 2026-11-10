import { describe, expect, it } from "vitest";
import { nameFilter } from "./quick-filter";

describe("UI-UNIT-017 quick filter name matching", () => {
  it("matches nothing special when the text is empty", () => {
    expect(nameFilter("")).toBeNull();
  });

  it("matches names that contain the text, ignoring case", () => {
    const keep = nameFilter("REPORT")!;
    expect(keep("Q3-report.pdf")).toBe(true);
    expect(keep("reports")).toBe(true);
    expect(keep("summary.pdf")).toBe(false);
  });

  it("treats * and ? as a wildcard pattern over the whole name", () => {
    const star = nameFilter("*.jpg")!;
    expect(star("photo.JPG")).toBe(true);
    expect(star("photo.jpg.bak")).toBe(false);

    const question = nameFilter("a?c")!;
    expect(question("abc")).toBe(true);
    expect(question("abbc")).toBe(false);

    // Anchored: "rep*" does not match a name that only contains "rep".
    expect(nameFilter("rep*")!("my-rep")).toBe(false);
  });

  it("treats regex characters in the text literally", () => {
    const keep = nameFilter("a.b")!;
    expect(keep("a.b")).toBe(true);
    expect(keep("axb")).toBe(false);
    expect(nameFilter("*(1)*")!("file (1).txt")).toBe(true);
  });
});

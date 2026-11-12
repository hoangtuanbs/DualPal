import { expect, test } from "@playwright/test";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

// UI-KEY-000 — every row of the README keyboard table has a UI-KEY test.
// A shortcut added to the README without a test, or a test that disappears,
// fails here.

/** README "Key" column → the test that covers it. */
const COVERED_BY: Record<string, string> = {
  "Tab": "UI-KEY-001",
  "↑ ↓ / Page Up / Page Down / Home / End": "UI-KEY-002",
  "Enter": "UI-KEY-003",
  "Backspace, Ctrl/⌘ ↑": "UI-KEY-004",
  "Space / Insert": "UI-KEY-005",
  "Shift ↑ / Shift ↓": "UI-KEY-006",
  "Ctrl/⌘ A": "UI-KEY-007",
  "Menu key / Shift F10": "UI-KEY-008",
  "F2": "UI-KEY-009",
  "Shift F2": "UI-KEY-010",
  "F4": "UI-KEY-011",
  "F5 / F6": "UI-KEY-012", // F6: UI-KEY-013
  "F7": "UI-KEY-014",
  "F8 / Delete": "UI-KEY-015",
  "Shift F8 / Shift Delete": "UI-KEY-016",
  "+ / −": "UI-KEY-017",
  "Ctrl/⌘ 1 / Ctrl/⌘ 2": "UI-KEY-018",
  "Ctrl/⌘ T, Ctrl/⌘ W": "UI-KEY-019",
  "Ctrl Tab, Ctrl Shift Tab": "UI-KEY-020",
  "Ctrl/⌘ I": "UI-KEY-021",
  "Ctrl/⌘ S (in the editor)": "UI-KEY-022",
  "Ctrl/⌘ R": "UI-KEY-023",
  "Ctrl/⌘ Shift .": "UI-KEY-024",
  "Ctrl/⌘ Shift P": "UI-KEY-025",
  "Ctrl/⌘ Shift Y": "UI-KEY-026",
  "Ctrl/⌘ Shift D": "UI-KEY-027",
  "Ctrl/⌘ Shift L": "UI-KEY-028",
  "Ctrl/⌘ Shift F": "UI-KEY-031",
  "Space on a folder": "UI-KEY-032",
  "Alt Shift Enter": "UI-KEY-033",
  "Ctrl/⌘ F": "UI-KEY-034",
};

const root = join(here, "..", "..", "..");

function readmeKeys(): string[] {
  const readme = readFileSync(join(root, "README.md"), "utf8");
  // From the heading to the next one.
  const section = readme.split(/^## Keyboard\s*$/m)[1]?.split(/^## /m)[0];
  expect(section, "README has a Keyboard section").toBeTruthy();
  return section
    .split(/\r?\n/)
    .filter((l) => l.startsWith("|") && !/^\|\s*(Key|-+)\s*\|/.test(l))
    .map((l) => l.split("|")[1].trim())
    .filter(Boolean);
}

test("UI-KEY-000 every README keyboard row has a UI-KEY test", () => {
  const keys = readmeKeys();
  expect(keys.length).toBeGreaterThan(20);
  const unmapped = keys.filter((k) => !(k in COVERED_BY));
  expect(unmapped, "README shortcuts without a UI-KEY test").toEqual([]);

  const specs = readdirSync(here)
    .filter((f) => f.endsWith(".spec.ts") && f !== "keyboard-meta.spec.ts")
    .map((f) => readFileSync(join(here, f), "utf8"))
    .join("\n");
  const missing = [...new Set([...Object.values(COVERED_BY), "UI-KEY-013"])].filter(
    (id) => !specs.includes(`"${id} `),
  );
  expect(missing, "UI-KEY ids with no test titled after them").toEqual([]);
});

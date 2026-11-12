// REL-001: list the tests that are flaky in a nightly run.
//   - Rust: nextest's JUnit report (profile `nightly`, retries = 2) marks a
//     test that failed and then passed on a retry with <flakyFailure>.
//   - UI: Playwright's JSON report with --repeat-each 3: a test whose repeats
//     both passed and failed is flaky.
// Prints a Markdown list (empty output = nothing flaky).
//
// Usage: node scripts/ci/flaky-report.mjs [--junit <file>] [--playwright <file>]
import { existsSync, readFileSync } from "node:fs";

const args = process.argv.slice(2);
const arg = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};

const flaky = [];

const junit = arg("--junit");
if (junit && existsSync(junit)) {
  const xml = readFileSync(junit, "utf8");
  // <testcase name="…" classname="…"> … </testcase> (or self-closing)
  for (const m of xml.matchAll(/<testcase\b([^>]*?)(\/>|>([\s\S]*?)<\/testcase>)/g)) {
    const attrs = m[1];
    const body = m[3] ?? "";
    if (!/<flakyFailure\b/.test(body)) continue;
    const name = attrs.match(/\bname="([^"]*)"/)?.[1] ?? "?";
    const cls = attrs.match(/\bclassname="([^"]*)"/)?.[1] ?? "";
    const tries = (body.match(/<flakyFailure\b/g) ?? []).length + 1;
    flaky.push(`Rust \`${cls ? cls + "::" : ""}${name}\` passed on try ${tries}`);
  }
}

const pw = arg("--playwright");
if (pw && existsSync(pw)) {
  const report = JSON.parse(readFileSync(pw, "utf8"));
  /** title path → outcomes of every repeat and retry */
  const outcomes = new Map();
  const walk = (suite, path) => {
    for (const s of suite.suites ?? []) walk(s, [...path, s.title]);
    for (const spec of suite.specs ?? []) {
      for (const t of spec.tests ?? []) {
        const key = [...path, spec.title, t.projectName].filter(Boolean).join(" › ");
        const list = outcomes.get(key) ?? [];
        for (const r of t.results ?? []) list.push(r.status);
        outcomes.set(key, list);
      }
    }
  };
  for (const s of report.suites ?? []) walk(s, [s.title]);
  for (const [key, list] of outcomes) {
    const passed = list.filter((s) => s === "passed").length;
    const failed = list.filter((s) => s === "failed" || s === "timedOut").length;
    if (passed && failed) flaky.push(`UI \`${key}\` passed ${passed} of ${passed + failed} runs`);
  }
}

if (flaky.length) console.log(flaky.map((f) => `- ${f}`).join("\n"));

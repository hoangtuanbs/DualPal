import { expect, test } from "@playwright/test";
import { cursorIndex, openApp, point, row } from "./pom/app";

// PERF-006: a folder of 5 000 items: from the ready signal, End brings the
// last row into view in under a second.

test("PERF-006 5 000 rows: End shows the last row within 1 s", async ({ page }) => {
  const files: Record<string, string> = { "/home/user/": "" };
  for (let i = 0; i < 5000; i++) files[`/home/user/item-${String(i).padStart(4, "0")}.txt`] = "x";
  await openApp(page, { home: "/home/user", files });
  await expect(page.getByTestId("pane-left-status")).toContainText("5000 items");
  await point(page, "left", "item-0000.txt");

  // Timed in the page, frame by frame, so Playwright's polling interval
  // does not add to the figure.
  const ms = await page.evaluate(
    () =>
      new Promise<number>((resolve, reject) => {
        const t0 = performance.now();
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true }));
        const check = () => {
          const el = document.querySelector('[data-testid="pane-left"] [data-name="item-4999.txt"]');
          const r = el?.getBoundingClientRect();
          if (r && r.height > 0 && r.top >= 0 && r.bottom <= innerHeight) return resolve(performance.now() - t0);
          if (performance.now() - t0 > 5000) return reject(new Error("last row never came into view"));
          requestAnimationFrame(check);
        };
        requestAnimationFrame(check);
      }),
  );
  await expect(row(page, "left", "item-4999.txt")).toBeInViewport();
  expect(await cursorIndex(page, "left")).toBe(4999);
  console.log(`PERF-006: End to last row visible in ${ms} ms`);
  expect(ms, "End to last row visible").toBeLessThan(1000);
});

import { expect, test } from "@playwright/test";
import { defaultScenario, openApp, point, row } from "./pom/app";

// SEC-006: the asset protocol may serve any file (scope `**`, a documented
// risk), so previewing a file must never run script from it. An SVG is shown
// with <img>, which runs no script; HTML is shown as text.

const evilSvg =
  '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" onload="top.__pwned = \'svg-onload\'">' +
  "<script>top.__pwned = 'svg-script'</script><rect width=\"10\" height=\"10\" fill=\"red\"/></svg>";
const evilHtml = "<html><body><script>top.__pwned = 'html'</script><img src=x onerror=\"top.__pwned='html-img'\"></body></html>";

test("SEC-006 previewing SVG or HTML with script in it runs no script", async ({ page }) => {
  await openApp(page, {
    ...defaultScenario,
    files: { ...defaultScenario.files, "/home/user/evil.svg": evilSvg, "/home/user/evil.html": evilHtml },
  });
  const pwned = () => page.evaluate(() => (window as any).__pwned ?? null);
  const inspector = page.getByTestId("inspector");

  await point(page, "left", "evil.svg");
  if (!(await inspector.isVisible())) await page.keyboard.press("ControlOrMeta+i");
  const img = inspector.getByRole("img", { name: "evil.svg" });
  await expect(img).toBeVisible();
  // The picture really loaded (so its script had the chance to run).
  await expect.poll(() => img.evaluate((el) => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);

  await point(page, "left", "evil.html");
  // HTML is text to edit, not a page to render.
  await expect(inspector.getByRole("textbox", { name: "Edit evil.html" })).toHaveValue(evilHtml);
  await expect(inspector.locator("iframe")).toHaveCount(0);

  // Icon view shows the SVG as a thumbnail: no script there either.
  await page.keyboard.press("ControlOrMeta+2");
  await expect(row(page, "left", "evil.svg")).toBeVisible();

  await page.waitForTimeout(300); // a gap for any onload/onerror to fire
  expect(await pwned()).toBeNull();
});

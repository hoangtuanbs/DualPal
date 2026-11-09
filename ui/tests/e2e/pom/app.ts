import type { Page } from "@playwright/test";
import { installBrowserMock, type BrowserScenario } from "../browser-mock";

export const defaultScenario: BrowserScenario = {
  home: "/home/user",
  files: {
    "/home/user/": "",
    "/home/user/readme.md": "# Readme\n",
    "/home/user/docs/": "",
    "/home/user/docs/notes.txt": "alpha\nbeta\n",
    "/home/user/docs/report.pdf": "pdf-bytes",
    "/home/user/a.txt": "aaa",
    "/home/user/b.txt": "bbb",
    "/home/user/empty.txt": "",
    "/home/user/.hidden": "h",
  },
};

/** Load the app with the mock backend and wait for the PREP-06 ready signal. */
export async function openApp(page: Page, scenario: BrowserScenario = defaultScenario) {
  await page.addInitScript(installBrowserMock as never, scenario);
  // A second switch so a production build can be tested too.
  await page.addInitScript(() => localStorage.setItem("dualpalTest", "1"));
  await page.goto("/");
  await page.evaluate(() => (window as any).__DUALPAL_TEST__.ready);
}

/** A file pane (0 = left, 1 = right). */
export function pane(page: Page, side: "left" | "right") {
  return page.getByTestId(`pane-${side}`);
}

/** A row in a pane by its data-name. */
export function row(page: Page, side: "left" | "right", name: string) {
  return pane(page, side).locator(`[data-name="${name}"]`);
}

export function toolbar(page: Page) {
  return page.getByTestId("toolbar");
}

export function dialog(page: Page) {
  return page.getByTestId("dialog");
}

export function ops(page: Page) {
  return page.getByTestId("ops-panel");
}

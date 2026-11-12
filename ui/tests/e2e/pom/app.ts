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

/** Arguments of every recorded call to `cmd`, oldest first. */
export async function calls(page: Page, cmd: string): Promise<any[]> {
  return page.evaluate(
    (c) => (window as any).__mock.calls.filter((x: { cmd: string }) => x.cmd === c).map((x: { args: unknown }) => x.args),
    cmd,
  );
}

/** Forget the calls recorded so far (e.g. the startup listing). */
export async function clearCalls(page: Page) {
  await page.evaluate(() => ((window as any).__mock.calls.length = 0));
}

/** The `operation` argument of every `enqueue_operation` call. */
export async function enqueued(page: Page): Promise<any[]> {
  return (await calls(page, "enqueue_operation")).map((a) => a.operation);
}

/** Send a backend event to the app, as Tauri would. */
export async function emit(page: Page, event: string, payload: unknown) {
  await page.evaluate(([e, p]) => (window as any).__mock.emit(e, p), [event, payload] as const);
}

/** Steer the mock: e.g. `{ nextOpError: {...} }` or `{ holdOps: true }`. */
export async function control(page: Page, values: Record<string, unknown>) {
  await page.evaluate((v) => Object.assign((window as any).__mock.control, v), values);
}

/** Index of the row under the cursor in a pane, or -1. */
export async function cursorIndex(page: Page, side: "left" | "right") {
  const el = pane(page, side).locator(".cursor").first();
  if ((await el.count()) === 0) return -1;
  return Number(await el.getAttribute("data-index"));
}

/** Names of the marked rows in a pane. */
export async function marked(page: Page, side: "left" | "right") {
  return pane(page, side).locator('[aria-selected="true"][data-name]').evaluateAll((els) =>
    els.map((e) => (e as HTMLElement).dataset.name!),
  );
}

/** Put the cursor on a row by clicking it, leaving the focus on the pane. */
export async function point(page: Page, side: "left" | "right", name: string) {
  await row(page, side, name).click();
}

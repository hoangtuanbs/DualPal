import { expect, test } from "@playwright/test";
import { calls, defaultScenario, openApp } from "./pom/app";

const RELEASE = {
  version: "0.3.0",
  tag: "v0.3.0",
  name: "Dual Pal 0.3.0",
  url: "https://github.com/hoangtuanbs/DualPal/releases/tag/v0.3.0",
  publishedAt: "2026-11-13T10:00:00Z",
};

test.describe("UI-E2E release notice", () => {
  test("UI-E2E-UPD-001 a newer GitHub release without updater files links to its download page", async ({ page }) => {
    await openApp(page, { ...defaultScenario, results: { latest_release: RELEASE } });

    await page.getByRole("button", { name: "About", exact: true }).click();
    const about = page.getByTestId("about-dialog");
    await about.getByRole("button", { name: /Check for Updates/ }).click();
    await expect(about).toContainText("Dual Pal 0.3.0 is available. Download it");

    const bar = page.getByTestId("update-bar");
    await expect(bar).toContainText("Dual Pal 0.3.0 is available. You have 0.1.0.");
    await bar.getByRole("button", { name: "Download…" }).click();
    expect(await calls(page, "open_url")).toContainEqual({ url: RELEASE.url });

    await bar.getByRole("button", { name: "Skip this version" }).click();
    await expect(bar).toBeHidden();
    const saved = await calls(page, "save_settings");
    expect(saved.at(-1).settings.skippedVersion).toBe("0.3.0");
  });

  test("UI-E2E-UPD-002 the About window turns the automatic check off", async ({ page }) => {
    await openApp(page);
    await page.getByRole("button", { name: "About", exact: true }).click();
    const box = page.getByTestId("auto-update-check");
    await expect(box).toBeChecked();
    await box.uncheck();
    await expect(box).not.toBeChecked();
    const saved = await calls(page, "save_settings");
    expect(saved.at(-1).settings.updateChecks).toBe(false);
  });
});

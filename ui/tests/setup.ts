// Vitest setup: jest-dom matchers, a fake Tauri window, and clean mocks
// between tests.
import "@testing-library/jest-dom/vitest";
import { mockWindows, clearMocks } from "@tauri-apps/api/mocks";
import { cleanup } from "@testing-library/svelte";
import { afterEach } from "vitest";

mockWindows("main");

afterEach(() => {
  cleanup();
  clearMocks();
});

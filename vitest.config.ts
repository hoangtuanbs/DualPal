import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Reuses the Svelte 5 setup from vite.config.ts. `root` is `ui`, so all
// relative paths below are under `ui/`.
export default defineConfig({
  root: "ui",
  plugins: [svelte()],
  resolve: {
    // jsdom tests need the browser (client) build of Svelte, not the SSR one.
    conditions: ["browser"],
  },
  test: {
    environment: "jsdom",
    setupFiles: ["tests/setup.ts"],
    include: ["src/**/*.test.ts", "tests/**/*.test.ts"],
    coverage: {
      provider: "v8",
      include: ["src/lib/**/*.ts"],
      reporter: ["text", "json-summary", "html"],
    },
  },
});

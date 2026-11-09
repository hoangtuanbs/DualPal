import { describe, expect, it } from "vitest";
import { drag, dragging } from "./drag.svelte";

describe("UI-UNIT-015 drag state transitions", () => {
  it("is not dragging until items are set, and Shift toggles move", () => {
    expect(dragging()).toBe(false);
    drag.items = [{ name: "a.txt", path: "/a.txt", kind: "file", symlink: false, size: 0, modifiedMs: null, createdMs: null, hidden: false, readonly: false, mode: null }];
    drag.from = 0;
    expect(dragging()).toBe(true);

    expect(drag.move).toBe(false);
    drag.move = true; // Shift held on drop
    expect(drag.move).toBe(true);
  });
});

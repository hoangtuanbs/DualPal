import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import ContextMenu, { type MenuItem } from "./ContextMenu.svelte";

describe("UI-CMP-002 ContextMenu", () => {
  it("runs an item on Enter after arrow navigation, skipping disabled and separators", async () => {
    const user = userEvent.setup();
    const runOpen = vi.fn();
    const runDelete = vi.fn();
    const items: MenuItem[] = [
      { label: "Open", run: runOpen },
      { label: "Rename", disabled: true, run: vi.fn() },
      "separator",
      { label: "Delete", danger: true, run: runDelete },
    ];
    render(ContextMenu, { props: { x: 0, y: 0, items, onclose: () => {} } });

    const menuitems = screen.getAllByRole("menuitem");
    expect(menuitems).toHaveLength(3);

    await user.keyboard("{ArrowDown}"); // focus "Open"
    await user.keyboard("{Enter}");
    expect(runOpen).toHaveBeenCalled();

    await user.keyboard("{ArrowDown}"); // skip "Rename" (disabled) → "Delete"
    await user.keyboard("{ArrowDown}"); // wrap to "Open"
    await user.keyboard("{End}"); // focus "Delete"
    await user.keyboard("{Enter}");
    expect(runDelete).toHaveBeenCalled();
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    const onclose = vi.fn();
    render(ContextMenu, { props: { x: 0, y: 0, items: [{ label: "Open", run: () => {} }], onclose } });
    await user.keyboard("{Escape}");
    expect(onclose).toHaveBeenCalled();
  });
});

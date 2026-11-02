import type { Entry, Location } from "./api";

/** Where a drag would drop: a folder in one of the panes. */
export type DropTarget = { pane: number; folder: Location; name: string; row: string | null };

/** Files being dragged from one pane to the other, or onto a folder. */
export const drag = $state<{
  items: Entry[];
  from: number;
  source: Location | null;
  x: number;
  y: number;
  target: DropTarget | null;
  move: boolean;
}>({ items: [], from: -1, source: null, x: 0, y: 0, target: null, move: false });

export const dragging = () => drag.items.length > 0;

// ---- the keyboard's Menu key --------------------------------------------------
// WebView2 (Windows) sends the Menu key only as a `contextmenu` event, not as
// a key press. A right-click is always preceded by a right-button press, so a
// `contextmenu` without one came from the keyboard.

let rightPressAt = 0;
if (typeof window !== "undefined") {
  window.addEventListener(
    "pointerdown",
    (e) => {
      if (e.button === 2) rightPressAt = performance.now();
    },
    true,
  );
}

/** True if this `contextmenu` event came from the keyboard. */
export function fromKeyboard(_e: MouseEvent) {
  return performance.now() - rightPressAt > 1000;
}

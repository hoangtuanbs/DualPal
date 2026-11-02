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

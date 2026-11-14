/** Where the divider between the two panes sits: the left pane's share of the width. */

export const SPLIT_DEFAULT = 0.5;
/** Neither pane gets narrower than this many pixels while room allows. */
export const SPLIT_MIN_PX = 160;
/** Arrow keys move the divider by this share; Shift moves it further. */
export const SPLIT_STEP = 0.02;
export const SPLIT_BIG_STEP = 0.1;
const STORAGE_KEY = "dualpal.split";

/** Keep both panes at least `SPLIT_MIN_PX` wide in a `width`-pixel area. */
export function clampSplit(ratio: number, width = 0): number {
  if (!Number.isFinite(ratio)) return SPLIT_DEFAULT;
  const min = width > 0 ? Math.min(0.5, SPLIT_MIN_PX / width) : 0.1;
  return Math.min(1 - min, Math.max(min, ratio));
}

/** The split for a pointer at `x` over an area starting at `left`, `width` wide. */
export function splitAt(x: number, left: number, width: number): number {
  if (width <= 0) return SPLIT_DEFAULT;
  return clampSplit((x - left) / width, width);
}

/** The split after a key press on the divider, or null when the key is not one it uses. */
export function splitForKey(ratio: number, key: string, shift: boolean, width = 0): number | null {
  const step = shift ? SPLIT_BIG_STEP : SPLIT_STEP;
  switch (key) {
    case "ArrowLeft":
      return clampSplit(ratio - step, width);
    case "ArrowRight":
      return clampSplit(ratio + step, width);
    case "Home":
      return clampSplit(0, width);
    case "End":
      return clampSplit(1, width);
    case "Enter":
      return SPLIT_DEFAULT;
    default:
      return null;
  }
}

/** The split saved last time, or the default. Storage may be blocked. */
export function loadSplit(): number {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved !== null) return clampSplit(Number(saved));
  } catch {
    // Storage unavailable: start centred.
  }
  return SPLIT_DEFAULT;
}

export function saveSplit(ratio: number): void {
  try {
    localStorage.setItem(STORAGE_KEY, String(Math.round(ratio * 1000) / 1000));
  } catch {
    // Storage unavailable: the split lasts until the window closes.
  }
}

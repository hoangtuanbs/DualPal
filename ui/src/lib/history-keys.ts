/** The history step a key press asks for, or null. Alt+← and Alt+→ work on
 *  every platform; ⌘[ and ⌘] are also taken on macOS. */
export function historyKey(
  e: Pick<KeyboardEvent, "key" | "altKey" | "ctrlKey" | "metaKey" | "shiftKey">,
  mac: boolean,
): "back" | "forward" | null {
  if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
    if (e.key === "ArrowLeft") return "back";
    if (e.key === "ArrowRight") return "forward";
  }
  if (mac && e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
    if (e.key === "[") return "back";
    if (e.key === "]") return "forward";
  }
  return null;
}

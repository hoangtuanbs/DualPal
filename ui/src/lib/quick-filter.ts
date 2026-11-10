import { patternMatcher } from "./fileTypes";

/**
 * Name test for the quick filter text, or null when the filter is empty.
 * Text with * or ? is a wildcard pattern matched against the whole name;
 * other text matches names that contain it. Both ignore case.
 */
export function nameFilter(text: string): ((name: string) => boolean) | null {
  if (text === "") return null;
  if (/[*?]/.test(text)) return patternMatcher(text);
  const needle = text.toLowerCase();
  return (name) => name.toLowerCase().includes(needle);
}

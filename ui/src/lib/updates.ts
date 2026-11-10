// Small rules for the in-app updater: when to look for a release, where its
// notes are and how to word progress. The plugin calls live in
// updates.svelte.ts.
import { ABOUT } from "./about";

/** Look for a new release at most once a day. */
export const UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

/** Wait this long after start-up before checking, so the window settles first. */
export const UPDATE_CHECK_DELAY_MS = 10_000;

/** True when no check was recorded, the record is in the future, or a day has passed. */
export function isCheckDue(lastCheckMs: number | null | undefined, now: number): boolean {
  if (lastCheckMs == null || !Number.isFinite(lastCheckMs) || lastCheckMs > now) return true;
  return now - lastCheckMs >= UPDATE_CHECK_INTERVAL_MS;
}

/** The GitHub release page for `version`, e.g. 0.2.0 → …/releases/tag/v0.2.0. */
export function releaseNotesUrl(version: string): string {
  return `${ABOUT.projectUrl}/releases/tag/v${version}`;
}

/** Download progress in whole percent, or null while the total size is unknown. */
export function progressPercent(downloaded: number, total: number | null | undefined): number | null {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.round((downloaded / total) * 100));
}

/** Operations still queued or running. An update waits for them. */
export function activeOperationCount(operations: { status: string }[]): number {
  return operations.filter((o) => o.status === "queued" || o.status === "running").length;
}

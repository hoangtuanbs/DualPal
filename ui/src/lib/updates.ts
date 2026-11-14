// Small rules for the in-app updater: when to look for a release, how to
// compare versions, where its notes are and how to word progress. The plugin
// and GitHub calls live in updates.svelte.ts.
import { ABOUT } from "./about";

/** Look for a new release at most once a day. */
export const UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;

/** Wait this long after start-up before checking, so the window settles first. */
export const UPDATE_CHECK_DELAY_MS = 10_000;

/**
 * While the app stays open, look this often whether a daily check is due,
 * so a window left open for days still hears about a release.
 */
export const UPDATE_RECHECK_POLL_MS = 60 * 60 * 1000;

type ParsedVersion = { core: number[]; pre: string[] };

/** "v1.2.3-beta.1+build" → core [1, 2, 3], pre ["beta", "1"]. Null when it is not a version. */
function parseVersion(text: string): ParsedVersion | null {
  const m = /^\s*v?(\d+(?:\.\d+)*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?\s*$/i.exec(text);
  if (!m) return null;
  return { core: m[1].split(".").map(Number), pre: m[2] ? m[2].split(".") : [] };
}

/**
 * Compare two versions the semver way: negative when `a` is older, positive
 * when newer, 0 when equal. A pre-release (1.0.0-beta) is older than its
 * release (1.0.0). Text that is not a version compares as equal, so it never
 * triggers a notice.
 */
export function compareVersions(a: string, b: string): number {
  const x = parseVersion(a);
  const y = parseVersion(b);
  if (!x || !y) return 0;
  for (let i = 0; i < Math.max(x.core.length, y.core.length, 3); i++) {
    const d = (x.core[i] ?? 0) - (y.core[i] ?? 0);
    if (d !== 0) return Math.sign(d);
  }
  // A release is newer than any of its pre-releases.
  if (!x.pre.length || !y.pre.length) return Math.sign(y.pre.length - x.pre.length);
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    const pn = /^\d+$/.test(p);
    const qn = /^\d+$/.test(q);
    if (pn && qn) {
      const d = Number(p) - Number(q);
      if (d !== 0) return Math.sign(d);
    } else if (pn !== qn) {
      return pn ? -1 : 1;
    } else if (p !== q) {
      return p < q ? -1 : 1;
    }
  }
  return 0;
}

/** True when `latest` is a newer version than `current`. */
export function isNewerVersion(latest: string, current: string): boolean {
  return compareVersions(latest, current) > 0;
}

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

/** Date groups like Windows Explorer's "Group by Date modified": Today,
 *  Yesterday, Earlier this week, Last week, and so on. */

export type DateGroupId =
  | "future"
  | "today"
  | "yesterday"
  | "thisWeek"
  | "lastWeek"
  | "thisMonth"
  | "lastMonth"
  | "thisYear"
  | "older"
  | "unknown";

/** Newest first. Each group holds the times at or after its start and
 *  before the start of the group above it. */
export const DATE_GROUPS: { id: DateGroupId; label: string }[] = [
  { id: "future", label: "In the future" },
  { id: "today", label: "Today" },
  { id: "yesterday", label: "Yesterday" },
  { id: "thisWeek", label: "Earlier this week" },
  { id: "lastWeek", label: "Last week" },
  { id: "thisMonth", label: "Earlier this month" },
  { id: "lastMonth", label: "Last month" },
  { id: "thisYear", label: "Earlier this year" },
  { id: "older", label: "A long time ago" },
  { id: "unknown", label: "Date unknown" },
];

/** The first day of the week for the user's locale: 1 = Monday … 7 = Sunday,
 *  as in Intl.Locale's week info. Monday where the browser cannot tell. */
export function firstDayOfWeek(locale?: string): number {
  try {
    const loc = new Intl.Locale(locale ?? navigator.language) as Intl.Locale & {
      weekInfo?: { firstDay: number };
      getWeekInfo?: () => { firstDay: number };
    };
    const info = loc.getWeekInfo?.() ?? loc.weekInfo;
    if (info && info.firstDay >= 1 && info.firstDay <= 7) return info.firstDay;
  } catch {
    // Unknown locale: use the default below.
  }
  return 1;
}

/** Start times (local midnight) that separate the groups, for `now`. */
export function dateBoundaries(now: Date, weekStart = 1) {
  const day = (y: number, m: number, d: number) => new Date(y, m, d).getTime();
  const [y, m, d] = [now.getFullYear(), now.getMonth(), now.getDate()];
  // getDay: 0 = Sunday; weekStart: 7 = Sunday.
  const sinceWeekStart = (now.getDay() - (weekStart % 7) + 7) % 7;
  return {
    tomorrow: day(y, m, d + 1),
    today: day(y, m, d),
    yesterday: day(y, m, d - 1),
    thisWeek: day(y, m, d - sinceWeekStart),
    lastWeek: day(y, m, d - sinceWeekStart - 7),
    thisMonth: day(y, m, 1),
    lastMonth: day(y, m - 1, 1),
    thisYear: day(y, 0, 1),
  };
}

/** The group a modification time falls in. */
export function dateGroup(ms: number | null | undefined, b: ReturnType<typeof dateBoundaries>): DateGroupId {
  if (ms == null || !Number.isFinite(ms)) return "unknown";
  if (ms >= b.tomorrow) return "future";
  if (ms >= b.today) return "today";
  if (ms >= b.yesterday) return "yesterday";
  if (ms >= b.thisWeek) return "thisWeek";
  if (ms >= b.lastWeek) return "lastWeek";
  if (ms >= b.thisMonth) return "thisMonth";
  if (ms >= b.lastMonth) return "lastMonth";
  if (ms >= b.thisYear) return "thisYear";
  return "older";
}

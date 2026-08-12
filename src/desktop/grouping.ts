/**
 * Grouping and filtering for the dashboard (`specs/desktop-ui.md`).
 *
 * Pure functions over the item list, kept out of the components so the
 * behavior that decides what a user sees can be tested without rendering
 * anything.
 */

import {
  KIND_LABELS,
  SEVERITY_ORDER,
  SEVERITY_LABELS,
  type GroupBy,
  type ItemKind,
  type ScoredItem,
  type Severity,
} from "../types";

/** A titled run of items, rendered as one section of the dashboard. */
export interface Group {
  key: string;
  label: string;
  items: ScoredItem[];
}

/** Everything the user can narrow the list by. */
export interface Filters {
  /** Matched case-insensitively against title and repository. */
  search: string;
  /** Empty means "any account". */
  accountId: string;
  /** Empty means "any kind". */
  kind: ItemKind | "";
  /** Empty means "any severity". */
  severity: Severity | "";
}

/** Filters with nothing selected — the dashboard's initial state. */
export const NO_FILTERS: Filters = {
  search: "",
  accountId: "",
  kind: "",
  severity: "",
};

/** Applies `filters` to `items`, preserving the daemon's ordering. */
export function applyFilters(items: ScoredItem[], filters: Filters): ScoredItem[] {
  const needle = filters.search.trim().toLowerCase();
  return items.filter((item) => {
    if (filters.accountId && item.account_id !== filters.accountId) return false;
    if (filters.kind && item.kind !== filters.kind) return false;
    if (filters.severity && item.severity !== filters.severity) return false;
    if (needle) {
      const haystack = `${item.title} ${item.repo}`.toLowerCase();
      if (!haystack.includes(needle)) return false;
    }
    return true;
  });
}

/**
 * Splits `items` into display groups.
 *
 * Empty groups are dropped rather than rendered as empty headers — a
 * dashboard listing "Critical (0)" every day trains you to ignore the word.
 * Within each group the daemon's ordering is preserved, so grouping never
 * reorders items relative to each other.
 */
export function groupItems(items: ScoredItem[], groupBy: GroupBy): Group[] {
  if (groupBy === "priority") {
    return SEVERITY_ORDER.map((severity) => ({
      key: severity,
      label: SEVERITY_LABELS[severity],
      items: items.filter((item) => item.severity === severity),
    })).filter((group) => group.items.length > 0);
  }

  // By type: order the kind sections by the highest-priority item each holds,
  // so the most urgent category is still the one you read first.
  const kinds = Array.from(new Set(items.map((item) => item.kind)));
  return kinds
    .map((kind) => ({
      key: kind,
      label: KIND_LABELS[kind],
      items: items.filter((item) => item.kind === kind),
    }))
    .filter((group) => group.items.length > 0)
    .sort((a, b) => (b.items[0]?.score ?? 0) - (a.items[0]?.score ?? 0));
}

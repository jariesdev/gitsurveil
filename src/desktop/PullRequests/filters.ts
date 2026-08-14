/**
 * Client-side filtering for the Pull Requests view (`specs/desktop-ui.md`).
 *
 * Pure functions over the summary list, kept out of the component so the
 * behavior that decides what a user sees can be tested without rendering
 * anything — mirroring `src/desktop/grouping.ts`.
 *
 * Only the Status filter is daemon-side (it changes the GraphQL search
 * qualifier and re-queries `prs.list`). Everything here runs in the webview
 * over an already-fetched list.
 */

import type { PrRole, PullRequestSummary } from "../../types";

/** The attention dimensions a row can be filtered by. */
export type Attention = "draft" | "conflicted" | "ci_failing" | "approved";

/** Everything the user can narrow the list by, short of status. */
export interface PullRequestFilters {
  /** Matched case-insensitively against title and repository. */
  search: string;
  /** Empty means "any account". */
  accountId: string;
  /** Empty means "any repository". */
  repo: string;
  /** Empty means "any role". */
  role: PrRole | "";
  /** Empty means "any attention state". */
  attention: Attention | "";
}

/** Filters with nothing selected. */
export const NO_PR_FILTERS: PullRequestFilters = {
  search: "",
  accountId: "",
  repo: "",
  role: "",
  attention: "",
};

/**
 * Applies `filters` to `summaries`, preserving the input order. All
 * dimensions combine as AND; an empty value means "no constraint".
 */
export function applyPrFilters(
  summaries: PullRequestSummary[],
  filters: PullRequestFilters,
): PullRequestSummary[] {
  const needle = filters.search.trim().toLowerCase();
  return summaries.filter((pr) => {
    if (filters.accountId && pr.account_id !== filters.accountId) return false;
    if (filters.repo && pr.repo !== filters.repo) return false;
    if (filters.role && !pr.roles.includes(filters.role)) return false;
    if (filters.attention && !matchesAttention(pr, filters.attention)) return false;
    if (needle) {
      const haystack = `${pr.title} ${pr.repo}`.toLowerCase();
      if (!haystack.includes(needle)) return false;
    }
    return true;
  });
}

/** Whether `pr` satisfies one attention dimension. */
export function matchesAttention(
  pr: PullRequestSummary,
  attention: Attention,
): boolean {
  switch (attention) {
    case "draft":
      return pr.draft;
    case "conflicted":
      // Only an explicit conflict is a conflict. `unknown` means GitHub is
      // still computing mergeability and must never flag a fresh PR.
      return pr.mergeability === "conflicted";
    case "ci_failing":
      return pr.ci_status === "failing";
    case "approved":
      return pr.review_decision === "approved";
  }
}

/**
 * Sorts most-recently-updated first, the view's default order. The daemon
 * already returns this order; sorting again here makes it independent of
 * whatever order a client happens to pass in.
 */
export function sortByRecent(
  summaries: PullRequestSummary[],
): PullRequestSummary[] {
  return [...summaries].sort((a, b) => b.updated_at.localeCompare(a.updated_at));
}

/**
 * Rebuilds a stored filter set into a usable one.
 *
 * Restoring blindly is a trap: a filter naming an account you have since
 * removed would hide every row with nothing on screen explaining why. Unknown
 * fields are dropped, and an `accountId` that no longer exists is cleared —
 * the remaining dimensions stay visible in their dropdowns, so a filter that
 * happens to match nothing is still self-evident.
 */
export function revivePrFilters(
  stored: unknown,
  knownAccountIds: string[],
): PullRequestFilters {
  if (typeof stored !== "object" || stored === null) return NO_PR_FILTERS;
  const raw = stored as Record<string, unknown>;
  const str = (v: unknown) => (typeof v === "string" ? v : "");

  const accountId = str(raw.accountId);
  return {
    search: str(raw.search),
    accountId: knownAccountIds.includes(accountId) ? accountId : "",
    repo: str(raw.repo),
    role: (["authored", "review_requested", "assigned"].includes(str(raw.role))
      ? raw.role
      : "") as PullRequestFilters["role"],
    attention: (["draft", "conflicted", "ci_failing", "approved"].includes(
      str(raw.attention),
    )
      ? raw.attention
      : "") as PullRequestFilters["attention"],
  };
}

/** Whether any dimension is constraining the list. */
export function hasActivePrFilters(filters: PullRequestFilters): boolean {
  return (
    filters.search !== "" ||
    filters.accountId !== "" ||
    filters.repo !== "" ||
    filters.role !== "" ||
    filters.attention !== ""
  );
}

/**
 * Client-side filtering for the Repositories pane (`specs/desktop-ui.md`).
 *
 * Pure functions over the catalog, kept out of the component so the behavior
 * that decides what a user sees can be tested without rendering anything —
 * mirroring `src/desktop/PullRequests/filters.ts`. The daemon owns the
 * catalog; only the *view* is filtered here.
 */

import type { OrgRef, Repository } from "../types";

/** Everything the Repositories pane can be narrowed by. */
export interface RepoFilters {
  /** Empty means "any account". */
  accountId: string;
  /** The owning organization/login, scoped to `accountId`. Empty means "any". */
  org: string;
}

/** Filters with nothing selected. */
export const NO_REPO_FILTERS: RepoFilters = { accountId: "", org: "" };

/**
 * Applies `filters` to the catalog's repos and sorts the result by
 * `owner/name`. All dimensions combine as AND; an empty value means "no
 * constraint". Legacy rows with no account are unreachable by the account
 * filter, which is correct — they can't be attributed to anyone.
 */
export function applyRepoFilters(
  repos: Repository[],
  filters: RepoFilters,
): Repository[] {
  return repos
    .filter((repo) => {
      if (filters.accountId && repo.account_id !== filters.accountId) return false;
      if (filters.org && repo.owner !== filters.org) return false;
      return true;
    })
    .sort((a, b) => a.full_name.localeCompare(b.full_name));
}

/**
 * The organizations available in the Organization dropdown for one account:
 * distinct `OrgRef` names, each with a count of how many repos sit under it.
 * Rebuilding from `orgs` (rather than from the repos themselves) keeps the
 * list stable even when every repo of an org is filtered out or none exist.
 */
export function orgOptions(
  orgs: OrgRef[],
  repos: Repository[],
  accountId: string,
): { name: string; count: number }[] {
  return orgs
    .filter((org) => org.account_id === accountId)
    .map((org) => ({
      name: org.name,
      count: repos.filter(
        (repo) => repo.account_id === accountId && repo.owner === org.name,
      ).length,
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * Rebuilds a stored filter set into a usable one.
 *
 * Restoring blindly is a trap: an `accountId` naming an account you have since
 * removed, or an `org` that doesn't exist under the account, would hide every
 * row with nothing on screen explaining why. Unknown ids are dropped so the
 * pane shows everything rather than an empty list.
 */
export function reviveRepoFilters(
  stored: unknown,
  knownAccountIds: string[],
  orgNamesFor: (accountId: string) => string[],
): RepoFilters {
  if (typeof stored !== "object" || stored === null) return NO_REPO_FILTERS;
  const raw = stored as Record<string, unknown>;
  const accountId =
    typeof raw.accountId === "string" &&
    knownAccountIds.includes(raw.accountId)
      ? raw.accountId
      : "";
  const org =
    typeof raw.org === "string" &&
    accountId !== "" &&
    orgNamesFor(accountId).includes(raw.org)
      ? raw.org
      : "";
  return { accountId, org };
}

/** Whether any dimension is constraining the list. */
export function hasActiveRepoFilters(filters: RepoFilters): boolean {
  return filters.accountId !== "" || filters.org !== "";
}

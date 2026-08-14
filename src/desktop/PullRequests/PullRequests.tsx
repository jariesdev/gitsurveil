/**
 * The Pull Requests view (`specs/desktop-ui.md`, Part 1).
 *
 * A live, filterable projection of GitHub's open/closed/merged pull requests
 * across all configured accounts. Unlike the dashboard it is standing state,
 * not an event inbox: rows show draft, review decision, CI, and mergeability
 * as they are right now.
 *
 * Data comes from `prs.list`, fetched on mount and whenever the Status filter
 * changes (status is daemon-side — it changes the GraphQL search qualifier).
 * Everything else filters in-memory via `filters.ts`, so changing account,
 * repository, role, attention, or the search box never touches the daemon.
 * There is no polling while the view is open.
 *
 * Clicking a row opens the same `PrDetail` pane the dashboard uses; conflicted
 * rows get an inline "Resolve conflicts" button that opens the same
 * `ConflictResolver`. Both reuse the existing components unchanged.
 */

import { useCallback, useEffect, useMemo, useState } from "react";
import { listPullRequests, openUrl, reposList } from "../../ipc";
import { ContextMenu } from "../ContextMenu";
import type {
  AccountRef,
  PrRole,
  PrState,
  PullRequestSummary,
} from "../../types";
import { ConflictResolver } from "../ConflictResolver";
import { age } from "../ItemRow";
import { PrDetail } from "../PrDetail";
import {
  applyPrFilters,
  hasActivePrFilters,
  NO_PR_FILTERS,
  revivePrFilters,
  sortByRecent,
  type Attention,
  type PullRequestFilters,
} from "./filters";
import { usePersistentState } from "../usePersistentState";

const ROLE_LABELS: Record<PrRole, string> = {
  authored: "Authored",
  review_requested: "Review requested",
  assigned: "Assigned",
};

const ATTENTION_LABELS: Record<Attention, string> = {
  draft: "Draft",
  conflicted: "Conflicted",
  ci_failing: "CI failing",
  approved: "Approved",
};

/** Status choices. "All" (empty) fetches open, closed, and merged rows. */
const STATUS_CHOICES: { value: PrState | ""; label: string }[] = [
  { value: "open", label: "Open" },
  { value: "closed", label: "Closed" },
  { value: "merged", label: "Merged" },
  { value: "", label: "All" },
];

export function PullRequests({
  accounts,
  onOpenRepos,
}: {
  accounts: AccountRef[];
  /** Jumps to the Repositories view, reached when resolving needs a clone. */
  onOpenRepos: () => void;
}) {
  /** Fetched from the daemon; `null` until the first query lands. */
  const [rows, setRows] = useState<PullRequestSummary[] | null>(null);
  /** Repos with a registered local clone (`repos.list`), used to gate
   * conflict resolution. */
  const [clonedRepos, setClonedRepos] = useState<string[]>([]);
  // Persisted, not plain state: switching sidebar views unmounts this
  // component and closing the window drops the webview, either of which would
  // otherwise reset the filters the user just set.
  const [status, setStatus] = usePersistentState<PrState | "">(
    "pr.status",
    "open",
    (stored, fallback) =>
      ["open", "closed", "merged", ""].includes(stored as string)
        ? (stored as PrState | "")
        : fallback,
  );
  const [filters, setFilters] = usePersistentState<PullRequestFilters>(
    "pr.filters",
    NO_PR_FILTERS,
    (stored) => revivePrFilters(stored, accounts.map((a) => a.id)),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The PR whose detail pane is open, if any. */
  const [selected, setSelected] = useState<{ repo: string; number: number } | null>(
    null,
  );
  /** The PR being resolved in the three-pane editor, if any. */
  const [resolving, setResolving] = useState<{ repo: string; number: number } | null>(
    null,
  );
  /** A conflicted PR we couldn't open a resolver for — no local clone. */
  const [noClone, setNoClone] = useState<{ repo: string; number: number } | null>(
    null,
  );
  /** The row's right-click menu, anchored to where the cursor was. */
  const [menu, setMenu] = useState<{
    pr: PullRequestSummary;
    x: number;
    y: number;
  } | null>(null);

  const load = useCallback(async (state: PrState | "") => {
    setBusy(true);
    try {
      // Repos ride along so "Resolve conflicts" can tell, before opening the
      // resolver, whether a local clone exists for that repository.
      const [fetched, catalog] = await Promise.all([
        listPullRequests({
          state: state === "" ? undefined : state,
        }),
        reposList(),
      ]);
      setRows(fetched);
      setClonedRepos(catalog.repos.filter((r) => r.tracked).map((r) => r.full_name));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  // Re-query when status changes — it alters the GraphQL qualifier. Account,
  // repository, role, attention, and search never reach the daemon.
  useEffect(() => {
    void load(status);
  }, [status, load]);

  /** Repositories present in the current results, for the filter dropdown. */
  const repoChoices = useMemo(
    () => Array.from(new Set((rows ?? []).map((pr) => pr.repo))).sort(),
    [rows],
  );

  const visible = useMemo(
    () => (rows ? sortByRecent(applyPrFilters(rows, filters)) : []),
    [rows, filters],
  );

  const hiddenCount = (rows?.length ?? 0) - visible.length;

  /** One handler for both resolve routes (this row and `PrDetail`'s button).
   * The resolver needs a configured local clone; without one, explain why
   * instead of failing obscurely, with a path to the Repositories tab. */
  function handleResolve(repo: string, number: number) {
    if (clonedRepos.includes(repo)) {
      setResolving({ repo, number });
    } else {
      setNoClone({ repo, number });
    }
  }

  return (
    <div className="relative flex h-full">
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex flex-wrap items-center gap-2 border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
          <input
            type="search"
            value={filters.search}
            onChange={(e) => setFilters({ ...filters, search: e.target.value })}
            placeholder="Search title or repository"
            aria-label="Search"
            className="min-w-48 flex-1 rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />

          <Select
            label="Status"
            value={status}
            onChange={(s) => setStatus(s as PrState | "")}
            options={STATUS_CHOICES}
          />
          <Select
            label="Account"
            value={filters.accountId}
            onChange={(accountId) => setFilters({ ...filters, accountId })}
            options={[
              { value: "", label: "All accounts" },
              ...accounts.map((a) => ({ value: a.id, label: a.login })),
            ]}
          />
          <Select
            label="Repository"
            value={filters.repo}
            onChange={(repo) => setFilters({ ...filters, repo })}
            options={[
              { value: "", label: "All repositories" },
              ...repoChoices.map((repo) => ({ value: repo, label: repo })),
            ]}
          />
          <Select
            label="Role"
            value={filters.role}
            onChange={(role) => setFilters({ ...filters, role: role as PrRole | "" })}
            options={[
              { value: "", label: "All roles" },
              ...(Object.keys(ROLE_LABELS) as PrRole[]).map((role) => ({
                value: role,
                label: ROLE_LABELS[role],
              })),
            ]}
          />
          <Select
            label="Attention"
            value={filters.attention}
            onChange={(attention) =>
              setFilters({ ...filters, attention: attention as Attention | "" })
            }
            options={[
              { value: "", label: "All attention" },
              ...(Object.keys(ATTENTION_LABELS) as Attention[]).map((attention) => ({
                value: attention,
                label: ATTENTION_LABELS[attention],
              })),
            ]}
          />
        </header>

        <div className="flex-1 overflow-y-auto">
          {error ? (
            <p className="p-10 text-center text-sm text-neutral-500">{error}</p>
          ) : busy && !rows ? (
            <p className="p-10 text-center text-sm text-neutral-500">Loading…</p>
          ) : accounts.length === 0 ? (
            <p className="p-10 text-center text-sm text-neutral-500">
              Add a GitHub account first — pull requests are listed per account.
            </p>
          ) : visible.length === 0 ? (
            <p className="p-10 text-center text-sm text-neutral-500">
              {(rows?.length ?? 0) === 0
                ? "No pull requests found for this status."
                : "No pull requests match these filters."}
            </p>
          ) : (
            <ul>
              {visible.map((pr) => (
                <li key={`${pr.repo}#${pr.number}`}>
                  <div
                    className="flex items-center gap-3 border-b border-neutral-200 px-4 py-2 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-800/50"
                    onContextMenu={(event) => {
                      event.preventDefault();
                      setMenu({ pr, x: event.clientX, y: event.clientY });
                    }}
                  >
                    <button
                      type="button"
                      onClick={() => setSelected({ repo: pr.repo, number: pr.number })}
                      className="min-w-0 flex-1 text-left"
                      title={pr.title}
                    >
                      <div className="truncate text-sm text-neutral-900 dark:text-neutral-100">
                        {pr.title}
                      </div>
                      <div className="flex flex-wrap items-center gap-1.5 text-[11px] text-neutral-500">
                        <span className="truncate">
                          {pr.repo}#{pr.number}
                        </span>
                        <span aria-hidden="true">·</span>
                        <span className="truncate">{pr.author}</span>
                        {accounts.length > 1 && (
                          <>
                            <span aria-hidden="true">·</span>
                            <span className="truncate">
                              {accountLogin(accounts, pr.account_id)}
                            </span>
                          </>
                        )}
                        {pr.draft && <Badge className="bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">Draft</Badge>}
                        {pr.ci_status === "failing" && (
                          <Badge className="bg-red-100 text-red-700 dark:bg-red-950 dark:text-red-300">
                            CI failing
                          </Badge>
                        )}
                        {pr.review_decision !== "none" && (
                          <Badge
                            className={
                              pr.review_decision === "approved"
                                ? "bg-green-100 text-green-700 dark:bg-green-950 dark:text-green-300"
                                : pr.review_decision === "changes_requested"
                                  ? "bg-red-100 text-red-700 dark:bg-red-950 dark:text-red-300"
                                  : "bg-amber-100 text-amber-700 dark:bg-amber-950 dark:text-amber-300"
                            }
                          >
                            {reviewLabel(pr.review_decision)}
                          </Badge>
                        )}
                        {pr.mergeability === "blocked" && (
                          <Badge className="bg-amber-100 text-amber-700 dark:bg-amber-950 dark:text-amber-300">
                            Blocked
                          </Badge>
                        )}
                        {pr.state !== "open" && (
                          <Badge className="bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">
                            {pr.state === "merged" ? "Merged" : "Closed"}
                          </Badge>
                        )}
                      </div>
                    </button>

                    {pr.mergeability === "conflicted" && (
                      <button
                        type="button"
                        onClick={() => handleResolve(pr.repo, pr.number)}
                        className="shrink-0 rounded border border-red-300 px-2 py-0.5 text-[11px] text-red-700 hover:bg-red-50 dark:border-red-900 dark:text-red-300 dark:hover:bg-red-950"
                      >
                        Resolve conflicts
                      </button>
                    )}

                    {pr.unresolved_threads > 0 && (
                      <span
                        className="relative shrink-0"
                        title={`${pr.unresolved_threads} unresolved review thread${pr.unresolved_threads === 1 ? "" : "s"}`}
                      >
                        <ChatBubbleIcon className="h-4 w-4 text-neutral-500 dark:text-neutral-400" />
                        <span className="absolute -right-1.5 -top-1.5 min-w-4 rounded-full bg-amber-400 px-0.5 text-center text-[10px] font-semibold leading-4 tabular-nums text-amber-950">
                          {pr.unresolved_threads}
                        </span>
                      </span>
                    )}

                    <span
                      className="shrink-0 tabular-nums text-[11px] text-neutral-500 dark:text-neutral-400"
                      title={`Updated ${pr.updated_at}`}
                    >
                      {age(pr.updated_at)}
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* Shown whenever anything is constraining the list, not only when
            rows were hidden: with an empty result and active filters, nothing
            is "hidden" yet the filters are still the reason the pane is
            empty. */}
        {(hiddenCount > 0 || hasActivePrFilters(filters)) && (
          <footer className="flex items-center gap-2 border-t border-neutral-200 px-4 py-1.5 text-[11px] text-neutral-500 dark:border-neutral-800">
            <span>
              {hiddenCount > 0
                ? `${hiddenCount} pull request${hiddenCount === 1 ? "" : "s"} hidden by filters`
                : "Filters are active"}
            </span>
            {/* Filters now survive closing the window, so a user can return to
                a short list days later having forgotten why. This is the way
                back. */}
            <button
              type="button"
              onClick={() => setFilters(NO_PR_FILTERS)}
              className="underline underline-offset-2"
            >
              Clear filters
            </button>
          </footer>
        )}

        {noClone && (
          <div className="flex items-center gap-3 border-t border-amber-200 bg-amber-50 px-4 py-2 text-xs dark:border-amber-900 dark:bg-amber-950">
            <span className="min-w-0 flex-1">
              Can’t open the conflict resolver for {noClone.repo}#{noClone.number} —
              no local clone is configured for this repository.
            </span>
            <button
              type="button"
              onClick={() => {
                setNoClone(null);
                onOpenRepos();
              }}
              className="shrink-0 rounded border border-neutral-300 px-2 py-0.5 dark:border-neutral-700"
            >
              Open Repositories
            </button>
            <button
              type="button"
              aria-label="Dismiss"
              onClick={() => setNoClone(null)}
              className="shrink-0 rounded px-1.5 text-neutral-500 hover:bg-neutral-200 dark:hover:bg-neutral-700"
            >
              Dismiss
            </button>
          </div>
        )}
      </div>

      {selected && !resolving && (
        <PrDetail
          key={`${selected.repo}#${selected.number}`}
          repo={selected.repo}
          number={selected.number}
          onClose={() => setSelected(null)}
          onChanged={() => void load(status)}
          onResolve={() => handleResolve(selected.repo, selected.number)}
        />
      )}

      {resolving && (
        <div className="absolute inset-0 z-20">
          <ConflictResolver
            repo={resolving.repo}
            number={resolving.number}
            onClose={() => setResolving(null)}
            onResolved={() => void load(status)}
          />
        </div>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            {
              label: `Open in ${accountProvider(accounts, menu.pr.account_id)}`,
              onSelect: () => {
                void openUrl(menu.pr.url);
                setMenu(null);
              },
            },
          ]}
        />
      )}
    </div>
  );
}

/** The login for an account id, falling back to a truncated id. */
function accountLogin(accounts: AccountRef[], id: string): string {
  return accounts.find((a) => a.id === id)?.login ?? id;
}

/** The provider display name for the account a PR belongs to: GitHub and
 * GitLab by well-known host, otherwise the host itself (e.g. a GitHub
 * Enterprise hostname). Falls back to "browser" when the account is unknown. */
function accountProvider(accounts: AccountRef[], id: string): string {
  const host = accounts.find((a) => a.id === id)?.host;
  if (host === "github.com") return "GitHub";
  if (host === "gitlab.com") return "GitLab";
  return host ?? "browser";
}

/** A chat bubble, marking how many unresolved review threads a PR has. */
function ChatBubbleIcon({ className }: { className: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z" />
    </svg>
  );
}

/** Human label for a non-trivial review decision. */
function reviewLabel(decision: PullRequestSummary["review_decision"]): string {
  switch (decision) {
    case "approved":
      return "Approved";
    case "changes_requested":
      return "Changes requested";
    case "review_required":
      return "Review required";
    case "none":
      return "None";
  }
}

/** Small pill for a row's state markers. */
function Badge({ children, className }: { children: React.ReactNode; className: string }) {
  return (
    <span className={`rounded px-1 ${className}`}>{children}</span>
  );
}

/** Labeled `<select>`; the label is visually hidden but read by screen readers. */
function Select({
  label,
  value,
  onChange,
  options,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  options: { value: string; label: string }[];
}) {
  return (
    <select
      aria-label={label}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
    >
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  );
}

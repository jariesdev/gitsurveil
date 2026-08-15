/**
 * The dashboard: everything currently waiting on you, grouped by priority
 * (default) or by type, with filters and search (`specs/desktop-ui.md`).
 */

import { useMemo, useState } from "react";
import { dismissItem, openUrl, pollNow } from "../ipc";
import {
  KIND_LABELS,
  SEVERITY_LABELS,
  SEVERITY_ORDER,
  type AccountRef,
  type GroupBy,
  type ItemKind,
  type ScoredItem,
  type Severity,
} from "../types";
import { applyFilters, groupItems, NO_FILTERS, type Filters } from "./grouping";
import { ConflictResolver } from "./ConflictResolver";
import { ItemRow } from "./ItemRow";
import { PrDetail } from "./PrDetail";

/** Every item kind, in the order the filter dropdown lists them. */
const ALL_KINDS: ItemKind[] = [
  "ci_failed",
  "review_requested",
  "review_state_changed",
  "mentioned",
  "reviewed_by_me",
  "assigned",
  "authored",
  "participating",
  "ready_to_merge",
];

export function Dashboard({
  items,
  accounts,
  onRefresh,
  onOpenAccounts,
}: {
  items: ScoredItem[];
  accounts: AccountRef[];
  onRefresh: () => void;
  /** Jumps to the Accounts view for the "add your first account" empty state. */
  onOpenAccounts: () => void;
}) {
  const [groupBy, setGroupBy] = useState<GroupBy>("priority");
  const [filters, setFilters] = useState<Filters>(NO_FILTERS);
  const [busy, setBusy] = useState(false);
  /** The PR whose detail pane is open, if any. */
  const [selected, setSelected] = useState<{ repo: string; number: number } | null>(
    null,
  );
  /** The PR being resolved in the three-pane editor, if any. */
  const [resolving, setResolving] = useState<{ repo: string; number: number } | null>(
    null,
  );

  const visible = useMemo(() => applyFilters(items, filters), [items, filters]);
  const groups = useMemo(() => groupItems(visible, groupBy), [visible, groupBy]);

  const hiddenCount = items.length - visible.length;

  async function handleDismiss(id: string) {
    // No local refresh needed: the dismiss command makes the Rust shell emit
    // `items-changed`, and the app-level listener refetches once.
    await dismissItem(id);
  }

  async function handlePoll() {
    setBusy(true);
    try {
      await pollNow();
      onRefresh();
    } finally {
      setBusy(false);
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
          label="Account"
          value={filters.accountId}
          onChange={(accountId) => setFilters({ ...filters, accountId })}
          options={[
            { value: "", label: "All accounts" },
            ...accounts.map((a) => ({ value: a.id, label: a.login })),
          ]}
        />
        <Select
          label="Type"
          value={filters.kind}
          onChange={(kind) => setFilters({ ...filters, kind: kind as ItemKind | "" })}
          options={[
            { value: "", label: "All types" },
            ...ALL_KINDS.map((k) => ({ value: k, label: KIND_LABELS[k] })),
          ]}
        />
        <Select
          label="Severity"
          value={filters.severity}
          onChange={(severity) =>
            setFilters({ ...filters, severity: severity as Severity | "" })
          }
          options={[
            { value: "", label: "All severities" },
            ...SEVERITY_ORDER.filter((s) => s !== "idle").map((s) => ({
              value: s,
              label: SEVERITY_LABELS[s],
            })),
          ]}
        />

        <div
          role="group"
          aria-label="Group by"
          className="flex overflow-hidden rounded border border-neutral-300 dark:border-neutral-700"
        >
          {(["priority", "type"] as GroupBy[]).map((mode) => (
            <button
              key={mode}
              type="button"
              aria-pressed={groupBy === mode}
              onClick={() => setGroupBy(mode)}
              className={`px-2 py-1 text-xs capitalize ${
                groupBy === mode
                  ? "bg-neutral-900 text-white dark:bg-neutral-100 dark:text-neutral-900"
                  : "bg-transparent"
              }`}
            >
              {mode}
            </button>
          ))}
        </div>

        <button
          type="button"
          onClick={() => void handlePoll()}
          disabled={busy}
          className="rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
        >
          {busy ? "Checking…" : "Check now"}
        </button>
      </header>

      <div className="flex-1 overflow-y-auto">
        {groups.length === 0 ? (
          items.length === 0 && accounts.length === 0 ? (
            <div className="p-10 text-center">
              <p className="text-sm text-neutral-500">
                No account yet — add one to start monitoring.
              </p>
              <button
                type="button"
                onClick={onOpenAccounts}
                className="mt-3 rounded bg-neutral-900 px-3 py-1.5 text-sm text-white dark:bg-neutral-100 dark:text-neutral-900"
              >
                Add account
              </button>
            </div>
          ) : items.length === 0 ? (
            <p className="p-10 text-center text-sm text-neutral-500">
              Nothing needs your attention.
            </p>
          ) : (
            <p className="p-10 text-center text-sm text-neutral-500">
              No items match these filters.
            </p>
          )
        ) : (
          groups.map((group) => (
            <section key={group.key}>
              <h2 className="sticky top-0 bg-neutral-100 px-4 py-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-600 dark:bg-neutral-800 dark:text-neutral-400">
                {group.label}
                <span className="ml-1 font-normal">({group.items.length})</span>
              </h2>
              <ul>
                {group.items.map((item) => (
                  <li key={item.id}>
                    <ItemRow
                      item={item}
                      onOpen={() => {
                        // Items without a number aren't pull requests (some
                        // notification threads carry none), so there's nothing
                        // for the detail pane to load — go to GitHub instead.
                        if (item.number !== null) {
                          setSelected({ repo: item.repo, number: item.number });
                        } else {
                          void openUrl(item.url);
                        }
                      }}
                      onDismiss={() => void handleDismiss(item.id)}
                    />
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}
      </div>

      {hiddenCount > 0 && (
        <footer className="border-t border-neutral-200 px-4 py-1.5 text-[11px] text-neutral-500 dark:border-neutral-800">
          {hiddenCount} item{hiddenCount === 1 ? "" : "s"} hidden by filters
        </footer>
      )}
      </div>

      {selected && !resolving && (
        <PrDetail
          key={`${selected.repo}#${selected.number}`}
          repo={selected.repo}
          number={selected.number}
          onClose={() => setSelected(null)}
          onChanged={onRefresh}
          onResolve={() => setResolving(selected)}
        />
      )}

      {resolving && (
        <div className="absolute inset-0 z-20">
          <ConflictResolver
            repo={resolving.repo}
            number={resolving.number}
            onClose={() => setResolving(null)}
            onResolved={onRefresh}
          />
        </div>
      )}
    </div>
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

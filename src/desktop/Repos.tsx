/**
 * The Repositories pane (`specs/desktop-ui.md`).
 *
 * Renders the daemon's repository catalog: every discovered repo across every
 * account, filterable by account and organization. Per-repo actions live in a
 * right-click menu — open in browser, clone (background job with a progress
 * bar), pick an existing local clone to map, or remove the path. Clones are
 * HTTPS-only background jobs (`repos.clone`/`repos.clone_status`); the daemon
 * owns the transfer, this pane just shows it happening.
 *
 * "Pick existing clone…" never writes or deletes anything in the chosen
 * folder: `repos.set` only validates it (a git repo whose `origin` is this
 * repo) and records the mapping in the daemon's catalog.
 */

import { useCallback, useEffect, useState } from "react";
import { open as pickDirectory } from "@tauri-apps/plugin-dialog";
import {
  openUrl,
  reposClone,
  reposCloneStatus,
  reposRefresh,
  reposRemove,
  reposSet,
} from "../ipc";
import type { AccountRef, CloneStatus, RepoCatalog, Repository } from "../types";
import { ContextMenu } from "./ContextMenu";
import {
  applyRepoFilters,
  hasActiveRepoFilters,
  NO_REPO_FILTERS,
  orgOptions,
  reviveRepoFilters,
  type RepoFilters,
} from "./repoFilters";
import { usePersistentState } from "./usePersistentState";

/** One tracked background clone, keyed by job id. */
interface ActiveJob {
  /** The repo being cloned (`full_name`). */
  repo: string;
  /** `null` until the first status poll lands. */
  status: CloneStatus | null;
}

export function Repos({
  catalog,
  accounts,
  onChange,
}: {
  catalog: RepoCatalog;
  accounts: AccountRef[];
  /** Reloads the catalog after a mutating action. */
  onChange: () => void;
}) {
  const [filters, setFilters] = usePersistentState<RepoFilters>(
    "repos.filters",
    NO_REPO_FILTERS,
    (stored) =>
      reviveRepoFilters(stored, accounts.map((a) => a.id), (accountId) =>
        catalog.orgs
          .filter((org) => org.account_id === accountId)
          .map((org) => org.name),
      ),
  );
  const [jobs, setJobs] = useState<Record<string, ActiveJob>>({});
  const [menu, setMenu] = useState<{ repo: Repository; x: number; y: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      await reposRefresh();
      onChange();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [onChange]);

  /** Picks a destination folder, or `null` when the user cancels. */
  const pickTarget = useCallback(async () => {
    const picked = await pickDirectory({
      directory: true,
      multiple: false,
      title: "Choose a folder",
    });
    return typeof picked === "string" ? picked : null;
  }, []);

  /** Starts a background clone and tracks its job for the progress bar. */
  const handleClone = useCallback(
    async (repo: Repository) => {
      const target = await pickTarget();
      if (target === null) return;
      setError(null);
      try {
        const jobId = await reposClone(repo.full_name, target);
        setJobs((prev) => {
          const next = { ...prev };
          for (const [id, job] of Object.entries(next)) {
            if (job.repo === repo.full_name) delete next[id];
          }
          next[jobId] = { repo: repo.full_name, status: null };
          return next;
        });
      } catch (e) {
        setError(String(e));
      }
    },
    [pickTarget],
  );

  /** Registers an existing local clone as the repo's path. */
  const handleChangePath = useCallback(
    async (repo: Repository) => {
      const path = await pickTarget();
      if (path === null) return;
      setError(null);
      try {
        await reposSet(repo.full_name, path);
        onChange();
      } catch (e) {
        // The daemon validates the path is a git repo whose origin is this
        // repo; that message is what the user needs to fix.
        setError(String(e));
      }
    },
    [pickTarget, onChange],
  );

  const handleRemove = useCallback(
    async (repo: Repository) => {
      setError(null);
      try {
        await reposRemove(repo.full_name);
        onChange();
      } catch (e) {
        setError(String(e));
      }
    },
    [onChange],
  );

  // Polls every running clone once a second and repaints the progress bars.
  // When a job finishes the catalog is reloaded so the row flips to tracked;
  // a failed job stays visible with its error until the user acts.
  //
  // A fresh job has `status === null` (nothing polled yet) — it counts as
  // running, or the interval below would never start and the job would sit in
  // a stuck indeterminate bar until a manual refresh.
  useEffect(() => {
    const running = Object.entries(jobs).filter(
      ([, job]) => job.status === null || job.status.status === "running",
    );
    if (running.length === 0) return;
    const timer = window.setInterval(() => {
      for (const [jobId, job] of running) {
        void reposCloneStatus(jobId).then((status) => {
          if (status === null) {
            // Unknown job: the daemon restarted and cleaned it up. Drop it.
            setJobs((prev) => {
              const next = { ...prev };
              delete next[jobId];
              return next;
            });
            return;
          }
          const wasRunning = job.status === null || job.status.status === "running";
          setJobs((prev) => ({ ...prev, [jobId]: { repo: job.repo, status } }));
          if (wasRunning && status.status === "done") onChange();
        });
      }
    }, 1000);
    return () => window.clearInterval(timer);
  }, [jobs, onChange]);

  const visible = applyRepoFilters(catalog.repos, filters);
  const orgs = orgOptions(catalog.orgs, catalog.repos, filters.accountId);
  const hiddenCount = catalog.repos.length - visible.length;

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-wrap items-center gap-2 border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
        <div className="min-w-0 flex-1">
          <h2 className="text-base font-semibold">Repositories</h2>
          <p className="truncate text-[11px] text-neutral-500">
            Repositories across your accounts. Clones and conflict resolution
            run on local copies; nothing is pushed without an explicit action.
          </p>
        </div>

        <button
          type="button"
          onClick={() => void refresh()}
          disabled={busy}
          className="rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
        >
          {busy ? "Refreshing…" : "Refresh"}
        </button>
      </header>

      <div className="flex items-center gap-2 border-b border-neutral-200 px-4 py-2 dark:border-neutral-800">
        <Select
          label="Account"
          value={filters.accountId}
          onChange={(accountId) => setFilters({ ...filters, accountId, org: "" })}
          options={[
            { value: "", label: "All accounts" },
            ...accounts.map((a) => ({ value: a.id, label: a.login })),
          ]}
        />
        <Select
          label="Organization"
          value={filters.org}
          onChange={(org) => setFilters({ ...filters, org })}
          options={[
            { value: "", label: "All organizations" },
            ...orgs.map((org) => ({ value: org.name, label: `${org.name} (${org.count})` })),
          ]}
        />
      </div>

      <div className="flex-1 overflow-y-auto">
        {error && (
          <p role="alert" className="border-b border-red-200 bg-red-50 px-4 py-2 text-xs text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300">
            {error}
          </p>
        )}

        {accounts.length === 0 ? (
          <p className="p-10 text-center text-sm text-neutral-500">
            Add a GitHub account first — repositories are discovered per account.
          </p>
        ) : catalog.repos.length === 0 ? (
          <div className="p-10 text-center text-sm text-neutral-500">
            <p>No repositories discovered yet.</p>
            <button
              type="button"
              onClick={() => void refresh()}
              className="mt-3 rounded border border-neutral-300 px-3 py-1.5 text-sm dark:border-neutral-700"
            >
              Scan now
            </button>
          </div>
        ) : visible.length === 0 ? (
          <p className="p-10 text-center text-sm text-neutral-500">
            No repositories match these filters.
          </p>
        ) : (
          <ul>
            {visible.map((repo) => (
              <RepoRow
                key={repo.full_name}
                repo={repo}
                accounts={accounts}
                job={Object.values(jobs).find((j) => j.repo === repo.full_name) ?? null}
                onMenu={(event) => setMenu({ repo, x: event.clientX, y: event.clientY })}
                onClone={() => void handleClone(repo)}
                onChangePath={() => void handleChangePath(repo)}
              />
            ))}
          </ul>
        )}
      </div>

      {(hiddenCount > 0 || hasActiveRepoFilters(filters)) && (
        <footer className="flex items-center gap-2 border-t border-neutral-200 px-4 py-1.5 text-[11px] text-neutral-500 dark:border-neutral-800">
          <span>
            {hiddenCount > 0
              ? `${hiddenCount} repository${hiddenCount === 1 ? "" : "s"} hidden by filters`
              : "Filters are active"}
          </span>
          <button
            type="button"
            onClick={() => setFilters(NO_REPO_FILTERS)}
            className="underline underline-offset-2"
          >
            Clear filters
          </button>
        </footer>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            {
              label: "Open in browser",
              onSelect: () => {
                void openUrl(menu.repo.url);
                setMenu(null);
              },
            },
            ...(menu.repo.tracked
              ? [
                  {
                    label: "Change clone path…",
                    onSelect: () => {
                      setMenu(null);
                      void handleChangePath(menu.repo);
                    },
                  },
                  {
                    label: "Remove clone path",
                    onSelect: () => {
                      setMenu(null);
                      void handleRemove(menu.repo);
                    },
                  },
                ]
              : [
                  {
                    label: "Clone to…",
                    onSelect: () => {
                      setMenu(null);
                      void handleClone(menu.repo);
                    },
                  },
                  {
                    label: "Pick existing clone…",
                    onSelect: () => {
                      setMenu(null);
                      void handleChangePath(menu.repo);
                    },
                  },
                ]),
          ]}
        />
      )}
    </div>
  );
}

/** One catalog row: identity, tracked state, and its live clone job. */
function RepoRow({
  repo,
  accounts,
  job,
  onMenu,
  onClone,
  onChangePath,
}: {
  repo: Repository;
  accounts: AccountRef[];
  job: ActiveJob | null;
  onMenu: (event: React.MouseEvent) => void;
  onClone: () => void;
  onChangePath: () => void;
}) {
  // A job with no status yet was just started — treat it as running so the
  // indeterminate bar appears immediately rather than a beat later.
  const running = job !== null && (job.status === null || job.status.status === "running");
  const failed = job?.status?.status === "failed";
  const account = accounts.find((a) => a.id === repo.account_id);

  return (
    <li className="border-b border-neutral-200 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-800/50">
      <div className="flex items-center gap-3 px-4 py-2">
        <button
          type="button"
          onClick={() => void openUrl(repo.url)}
          className="min-w-0 flex-1 text-left"
          title={repo.description ?? repo.full_name}
        >
          <div className="flex flex-wrap items-center gap-1.5 text-sm">
            <span className="truncate">{repo.full_name}</span>
            {repo.private && (
              <Badge className="bg-amber-100 text-amber-700 dark:bg-amber-950 dark:text-amber-300">
                Private
              </Badge>
            )}
            {account && accounts.length > 1 && (
              <Badge className="bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-200">
                {account.login}
              </Badge>
            )}
          </div>
          <div className="truncate text-[11px] text-neutral-500">
            {repo.tracked && repo.clone_path
              ? repo.clone_path
              : failed
                ? `Clone failed: ${job?.status?.error ?? "unknown error"}`
                : "No local clone"}
          </div>
        </button>

        {running && <CloneProgress received={job?.status?.received ?? 0} />}

        <button
          type="button"
          aria-label={`Actions for ${repo.full_name}`}
          onClick={onMenu}
          className="shrink-0 rounded border border-neutral-300 px-2 py-0.5 text-xs dark:border-neutral-700"
        >
          ⋯
        </button>
      </div>

      {failed && (
        <div className="flex items-center gap-2 px-4 pb-2">
          <button
            type="button"
            onClick={onClone}
            className="rounded border border-neutral-300 px-2 py-0.5 text-[11px] dark:border-neutral-700"
          >
            Retry clone
          </button>
          <button
            type="button"
            onClick={onChangePath}
            className="rounded border border-neutral-300 px-2 py-0.5 text-[11px] dark:border-neutral-700"
          >
            Pick existing clone…
          </button>
        </div>
      )}
    </li>
  );
}

/** Indeterminate progress: git2 can't predict the pack size, so the bar never
 * shows a fraction — just a moving block and the running byte count. */
function CloneProgress({ received }: { received: number }) {
  return (
    <div className="flex w-40 shrink-0 flex-col gap-0.5" title={`${formatBytes(received)} downloaded`}>
      <div className="h-1 w-full overflow-hidden rounded bg-neutral-200 dark:bg-neutral-700">
        <div className="h-full w-1/2 animate-pulse rounded bg-neutral-500" />
      </div>
      <span className="text-right text-[10px] tabular-nums text-neutral-400">
        {formatBytes(received)}
      </span>
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${bytes} B`;
}

/** Small pill for a row's state markers. */
function Badge({ children, className }: { children: React.ReactNode; className: string }) {
  return <span className={`rounded px-1 ${className}`}>{children}</span>;
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

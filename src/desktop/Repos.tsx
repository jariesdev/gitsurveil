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

import { useCallback, useEffect, useRef, useState } from "react";
import { open as pickDirectory } from "@tauri-apps/plugin-dialog";
import {
  appsList,
  appsOpen,
  openUrl,
  revealInFileManager,
  reposClone,
  reposCloneStatus,
  reposRefresh,
  reposRemove,
  reposSet,
  reposWorktreeAdd,
  reposWorktreeRemove,
  reposWorktrees,
} from "../ipc";
import type {
  AccountRef,
  CloneStatus,
  RegisteredApp,
  RepoCatalog,
  Repository,
  WorktreeInfo,
  WorktreesResult,
} from "../types";
import { ContextMenu } from "./ContextMenu";
import { copyText } from "./clipboard";
import { ConfirmDialog } from "./ConfirmDialog";
import {
  applyRepoFilters,
  hasActiveRepoFilters,
  NO_REPO_FILTERS,
  orgOptions,
  reviveRepoFilters,
  type RepoFilters,
} from "./repoFilters";
import { usePersistentState } from "./usePersistentState";

/** The native file-manager label and action for the current platform. */
const fileManagerAction = (() => {
  const ua = navigator.userAgent;
  if (ua.includes("Macintosh") || ua.includes("Mac OS"))
    return { label: "Open in Finder", supported: true as const };
  if (ua.includes("Windows"))
    return { label: "Open in Explorer", supported: true as const };
  return { label: "", supported: false as const };
})();

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
  const [wtMenu, setWtMenu] = useState<{
    repo: Repository;
    worktree: WorktreeInfo;
    x: number;
    y: number;
  } | null>(null);
  // Repos whose worktree section is open. Only tracked, cloned repos can
  // expand. Kept as a Set so the same repo toggles cleanly across reloads.
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The worktree whose dirty error is showing, so the force-delete button
   *  knows what to target. Cleared when the error is dismissed. */
  const [dirtyWorktree, setDirtyWorktree] = useState<{
    repo: Repository;
    worktree: WorktreeInfo;
  } | null>(null);
  /** When non-null, the force-delete confirm dialog is open. */
  const [confirmTarget, setConfirmTarget] = useState<{
    repo: Repository;
    worktree: WorktreeInfo;
  } | null>(null);
  // The registered "Open with…" apps. Fetched on mount: this pane remounts on
  // every navigation (`ViewErrorBoundary` is keyed by view), so it stays fresh
  // without burdening the window-wide load with rows only this pane uses.
  const [apps, setApps] = useState<RegisteredApp[]>([]);

  useEffect(() => {
    appsList()
      .then(setApps)
      .catch(() => setApps([]));
  }, []);

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

  /** Removes one worktree of a repo; the branch itself survives. */
  const handleWorktreeDelete = useCallback(
    async (repo: Repository, worktree: WorktreeInfo, force = false) => {
      setError(null);
      setDirtyWorktree(null);
      try {
        await reposWorktreeRemove(repo.full_name, worktree.name, force);
        // The worktree list is owned by the expanded `WorktreesSection`, so
        // refresh it in place — otherwise the deleted row lingers until the
        // section is collapsed and re-expanded.
        worktreeReloads.current.get(repo.full_name)?.();
        onChange();
      } catch (e) {
        const msg = String(e);
        setError(msg);
        if (!force && msg.includes("uncommitted changes or untracked files")) {
          setDirtyWorktree({ repo, worktree });
        }
      }
    },
    [onChange],
  );

  // Each expanded `WorktreesSection` registers its reload here so the parent
  // can refresh the list after a delete it triggered via the context menu.
  const worktreeReloads = useRef(new Map<string, () => void>());
  const registerWorktreeReload = useCallback((fullName: string, reload: () => void) => {
    worktreeReloads.current.set(fullName, reload);
    return () => {
      worktreeReloads.current.delete(fullName);
    };
  }, []);

  const toggleExpanded = useCallback((fullName: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(fullName)) {
        next.delete(fullName);
      } else {
        next.add(fullName);
      }
      return next;
    });
  }, []);

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
          <h2 className="text-base font-semibold">Repository and Worktrees</h2>
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
          <div
            role="alert"
            className="flex items-center gap-2 border-b border-red-200 bg-red-50 px-4 py-2 text-xs text-red-700 dark:border-red-900 dark:bg-red-950 dark:text-red-300"
          >
            <span className="min-w-0 flex-1">{error}</span>
            {dirtyWorktree && (
              <button
                type="button"
                onClick={() => setConfirmTarget(dirtyWorktree)}
                className="shrink-0 rounded bg-red-600 px-2 py-0.5 text-[11px] text-white hover:bg-red-700 dark:bg-red-700 dark:hover:bg-red-800"
              >
                Force delete
              </button>
            )}
          </div>
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
                expanded={expanded.has(repo.full_name)}
                onToggle={() => toggleExpanded(repo.full_name)}
                onMenu={(event) => setMenu({ repo, x: event.clientX, y: event.clientY })}
                onClone={() => void handleClone(repo)}
                onChangePath={() => void handleChangePath(repo)}
                onCatalogChange={onChange}
                onWorktreeMenu={(event, worktree) =>
                  setWtMenu({ repo, worktree, x: event.clientX, y: event.clientY })
                }
                registerWorktreeReload={registerWorktreeReload}
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
            ...(menu.repo.clone_path
              ? [
                  {
                    label: "Copy path",
                    onSelect: () => {
                      setMenu(null);
                      void copyText(menu.repo.clone_path!);
                    },
                  },
                  ...(fileManagerAction.supported
                    ? [
                        {
                          label: fileManagerAction.label,
                          onSelect: () => {
                            setMenu(null);
                            void revealInFileManager(menu.repo.clone_path!);
                          },
                        },
                      ]
                    : []),
                ]
              : []),
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

      {wtMenu && (
        <ContextMenu
          x={wtMenu.x}
          y={wtMenu.y}
          onClose={() => setWtMenu(null)}
          items={[
            // The daemon spawns `command <path>`; the worktree path is passed
            // through as-is. Only shown when at least one app is registered.
            ...(apps.length > 0
              ? [
                  {
                    label: "Open with",
                    children: apps.map((app) => ({
                      label: app.name,
                      onSelect: () => {
                        setWtMenu(null);
                        void appsOpen(app.command, wtMenu.worktree.path);
                      },
                    })),
                  },
                ]
              : []),
            {
              label: "Copy path",
              onSelect: () => {
                setWtMenu(null);
                void copyText(wtMenu.worktree.path);
              },
            },
            ...(fileManagerAction.supported
              ? [
                  {
                    label: fileManagerAction.label,
                    onSelect: () => {
                      setWtMenu(null);
                      void revealInFileManager(wtMenu.worktree.path);
                    },
                  },
                ]
              : []),
            {
              label: "Delete worktree",
              onSelect: () => {
                setWtMenu(null);
                void handleWorktreeDelete(wtMenu.repo, wtMenu.worktree);
              },
            },
          ]}
        />
      )}

      {confirmTarget && (
        <ConfirmDialog
          title="Force delete worktree?"
          message="This worktree has uncommitted changes and/or untracked files. They will be permanently lost."
          confirmLabel="Force delete"
          onConfirm={() => {
            const { repo, worktree } = confirmTarget;
            setConfirmTarget(null);
            void handleWorktreeDelete(repo, worktree, true);
          }}
          onClose={() => setConfirmTarget(null)}
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
  expanded,
  onToggle,
  onMenu,
  onClone,
  onChangePath,
  onCatalogChange,
  onWorktreeMenu,
  registerWorktreeReload,
}: {
  repo: Repository;
  accounts: AccountRef[];
  job: ActiveJob | null;
  expanded: boolean;
  onToggle: () => void;
  onMenu: (event: React.MouseEvent) => void;
  onClone: () => void;
  onChangePath: () => void;
  onCatalogChange: () => void;
  onWorktreeMenu: (event: React.MouseEvent, worktree: WorktreeInfo) => void;
  registerWorktreeReload: (fullName: string, reload: () => void) => () => void;
}) {
  // A job with no status yet was just started — treat it as running so the
  // indeterminate bar appears immediately rather than a beat later.
  const running = job !== null && (job.status === null || job.status.status === "running");
  const failed = job?.status?.status === "failed";
  const account = accounts.find((a) => a.id === repo.account_id);
  // Worktrees live in the clone, so only a tracked repo with a registered
  // path can expand.
  const expandable = repo.tracked && repo.clone_path !== null;

  return (
    <li className="border-b border-neutral-200 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-800/50">
      <div
        className="flex cursor-pointer items-center gap-3 px-4 py-2"
        // Single click toggles the worktree panel; double click opens the repo
        // in the browser. (A double click also fires two single clicks, which
        // toggle twice and land back where they started — harmless.)
        onClick={onToggle}
        onDoubleClick={() => void openUrl(repo.url)}
      >
        {expandable && (
          <button
            type="button"
            aria-expanded={expanded}
            aria-label={`Worktrees for ${repo.full_name}`}
            onClick={(event) => {
              event.stopPropagation();
              onToggle();
            }}
            className={`shrink-0 text-xs text-neutral-500 transition-transform ${
              expanded ? "rotate-90" : ""
            }`}
          >
            ▶
          </button>
        )}

        <div
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
        </div>

        {running && <CloneProgress received={job?.status?.received ?? 0} />}

        <button
          type="button"
          aria-label={`Actions for ${repo.full_name}`}
          onClick={(event) => {
            event.stopPropagation();
            onMenu(event);
          }}
          className="shrink-0 rounded border border-neutral-300 px-2 py-0.5 text-xs dark:border-neutral-700"
        >
          ⋯
        </button>
      </div>

      {expanded && expandable && (
        <WorktreesSection
          repo={repo}
          onChange={onCatalogChange}
          onWorktreeMenu={onWorktreeMenu}
          registerWorktreeReload={registerWorktreeReload}
        />
      )}

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

/** The expanded worktree panel of a tracked, cloned repo: its user-created
 * worktrees plus an inline "add" form. Data comes from `repos.worktrees` on
 * every expand, so worktrees created or removed outside gitsurveil show up
 * too. Deleting happens via the row's context menu; adding is this form. */
function WorktreesSection({
  repo,
  onChange,
  onWorktreeMenu,
  registerWorktreeReload,
}: {
  repo: Repository;
  /** Reloads the catalog after a successful add. */
  onChange: () => void;
  onWorktreeMenu: (event: React.MouseEvent, worktree: WorktreeInfo) => void;
  /** Lets the parent refresh this list after a context-menu delete. */
  registerWorktreeReload: (fullName: string, reload: () => void) => () => void;
}) {
  const [data, setData] = useState<WorktreesResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [branch, setBranch] = useState("");
  const [path, setPath] = useState("");
  // The path is prefilled from the branch and stays in sync until the user
  // edits it by hand — after that their text wins.
  const [pathTouched, setPathTouched] = useState(false);
  const [adding, setAdding] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await reposWorktrees(repo.full_name));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [repo.full_name]);

  useEffect(() => {
    void load();
    return registerWorktreeReload(repo.full_name, () => void load());
  }, [load, registerWorktreeReload, repo.full_name]);

  /** `wt-{owner}-{name}-{branch}` as a sibling of the clone. */
  const defaultPath = useCallback(
    (forBranch: string) => {
      const safe = forBranch.trim().replace(/[/\\]/g, "-") || "work";
      const base = repo.clone_path ? parentDir(repo.clone_path) : "";
      return `${base ? `${base}/` : ""}wt-${repo.owner}-${repo.name}-${safe}`;
    },
    [repo.clone_path, repo.owner, repo.name],
  );

  const handleBranchChange = (value: string) => {
    setBranch(value);
    if (!pathTouched) setPath(defaultPath(value));
  };

  const handleAdd = async () => {
    const trimmedBranch = branch.trim();
    const trimmedPath = path.trim();
    if (!trimmedBranch || !trimmedPath || adding) return;
    setAdding(true);
    setError(null);
    try {
      await reposWorktreeAdd(repo.full_name, trimmedBranch, trimmedPath);
      setBranch("");
      setPathTouched(false);
      setPath(defaultPath(""));
      onChange();
      void load();
    } catch (e) {
      setError(String(e));
    } finally {
      setAdding(false);
    }
  };

  return (
    <div className="border-t border-neutral-200 bg-neutral-50 px-6 py-2 dark:border-neutral-800 dark:bg-neutral-900/50">
      {loading && <p className="py-1 text-xs text-neutral-500">Loading worktrees…</p>}
      {error && (
        <p role="alert" className="py-1 text-xs text-red-600 dark:text-red-400">
          {error}
        </p>
      )}

      {(data?.worktrees ?? []).map((worktree) => (
        <div
          key={worktree.name}
          onContextMenu={(event) => {
            event.preventDefault();
            onWorktreeMenu(event, worktree);
          }}
          className="flex cursor-default items-center gap-2 rounded px-2 py-1 hover:bg-neutral-100 dark:hover:bg-neutral-800"
        >
          <span className="shrink-0 text-xs font-medium">{worktree.branch}</span>
          {/* The branch's work has already landed, so this worktree is
              probably disposable — informational only, the user decides.
              Right-click → "Delete worktree" is still the only way to act. */}
          {worktree.merged_pr && (
            <button
              type="button"
              onClick={(event) => {
                // The row is not itself clickable, but the panel around it
                // is; opening the PR must not toggle anything.
                event.stopPropagation();
                void openUrl(worktree.merged_pr!.url);
              }}
              title={`Merged in #${worktree.merged_pr.number}: ${worktree.merged_pr.title}`}
              className="shrink-0 cursor-pointer"
            >
              <Badge className="bg-violet-100 text-[10px] text-violet-700 dark:bg-violet-950 dark:text-violet-300">
                Merged #{worktree.merged_pr.number}
              </Badge>
            </button>
          )}
          <span className="min-w-0 flex-1 truncate text-[11px] text-neutral-500">{worktree.path}</span>
          {worktree.head && (
            <span className="shrink-0 font-mono text-[11px] text-neutral-400">{worktree.head}</span>
          )}
        </div>
      ))}
      {data && data.worktrees.length === 0 && !loading && (
        <p className="py-1 text-xs text-neutral-500">No worktrees yet.</p>
      )}

      <form
        className="mt-2 flex items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void handleAdd();
        }}
      >
        <input
          list={`wt-branches-${repo.full_name}`}
          value={branch}
          onChange={(event) => handleBranchChange(event.target.value)}
          placeholder="Branch — pick or type a new one"
          aria-label={`Branch for new ${repo.full_name} worktree`}
          className="w-44 rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
        />
        <datalist id={`wt-branches-${repo.full_name}`}>
          {(data?.branches ?? []).map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
        <input
          value={path}
          onChange={(event) => {
            setPath(event.target.value);
            setPathTouched(true);
          }}
          placeholder="Target path"
          aria-label={`Path for new ${repo.full_name} worktree`}
          className="min-w-0 flex-1 rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
        />
        <button
          type="submit"
          disabled={adding || !branch.trim() || !path.trim()}
          className="shrink-0 rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
        >
          Add
        </button>
      </form>
      <p className="mt-1 text-[11px] text-neutral-400">
        Relative paths resolve next to the clone ({repo.clone_path ? parentDir(repo.clone_path) : "…"}).
        Deleting a worktree keeps its branch.
      </p>
    </div>
  );
}

/** The parent directory of an absolute path (used for the default worktree
 * location). Falls back to the input itself when there's no separator. */
function parentDir(path: string): string {
  const index = path.lastIndexOf("/");
  return index > 0 ? path.slice(0, index) : path;
}

/** Indeterminate progress: git2 can't predict the pack size, so the bar never
 * shows a fraction — just a moving block and the running byte count. */
function CloneProgress({ received }: { received: number }) {  return (
    <div className="flex w-40 shrink-0 flex-col gap-0.5" title={`${formatBytes(received)} downloaded`}>
      <div className="h-1 w-full overflow-hidden rounded bg-neutral-200 dark:bg-neutral-700">
        <div className="h-full w-1/2 animate-pulse rounded bg-neutral-500" />
      </div>
      <span className="text-right text-[11px] tabular-nums text-neutral-400">
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

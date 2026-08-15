/**
 * The full desktop window (`specs/desktop-ui.md`).
 *
 * Sidebar navigation over a handful of views, all fed from the daemon. Like
 * the popover it holds no state the daemon doesn't already own, so closing
 * this window (and dropping its webview) loses nothing.
 */

import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  clearHistory,
  daemonStatus,
  listAccounts,
  listHistory,
  listItems,
  listRules,
  openUrl,
  reposAckNew,
  reposList,
  reposNew,
  undismissItem,
} from "../ipc";
import type {
  AccountRef,
  RepoCatalog,
  Repository,
  Rule,
  ScoredItem,
  StatusResult,
} from "../types";
import { Accounts } from "./Accounts";
import { Dashboard } from "./Dashboard";
import { ItemRow } from "./ItemRow";
import { NewReposModal } from "./NewReposModal";
import { PullRequests } from "./PullRequests/PullRequests";
import { Repos } from "./Repos";
import { Rules } from "./Rules";
import { Settings } from "./Settings";
import { ViewErrorBoundary } from "./ErrorBoundary";

type View = "dashboard" | "pull-requests" | "history" | "rules" | "repos" | "accounts" | "settings";

const NAV: { id: View; label: string }[] = [
  { id: "dashboard", label: "Dashboard" },
  { id: "pull-requests", label: "Pull Requests" },
  { id: "history", label: "History" },
  { id: "rules", label: "Rules" },
  { id: "repos", label: "Repositories" },
  { id: "accounts", label: "Accounts" },
  { id: "settings", label: "Settings" },
];

interface Data {
  items: ScoredItem[];
  history: ScoredItem[];
  accounts: AccountRef[];
  rules: Rule[];
  repos: RepoCatalog;
  status: StatusResult;
}

export function App() {
  const [view, setView] = useState<View>("dashboard");
  const [data, setData] = useState<Data | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Repos discovered but not yet acknowledged; non-empty opens the modal. */
  const [newRepos, setNewRepos] = useState<Repository[]>([]);

  const load = useCallback(async () => {
    try {
      // One round trip per view's data. They're all cheap local socket calls,
      // so fetching together keeps the window internally consistent rather
      // than having tabs disagree about what's open.
      const [items, history, accounts, rules, repos, status] = await Promise.all([
        listItems(),
        listHistory(200),
        listAccounts(),
        listRules(),
        reposList(),
        daemonStatus(),
      ]);
      setData({ items, history, accounts, rules, repos, status });
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // An item's state can change outside this window — dismissing it in the
  // popover must remove it from an open Dashboard, restoring it in History
  // must bring it back. The Rust shell emits `items-changed` after the
  // daemon call succeeds, so we refetch once here instead of every window
  // managing its own refresh.
  useEffect(() => {
    const unlisten = listen("items-changed", () => {
      void load();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [load]);

  // The new-repositories modal is a main-window-open event, not a per-refresh
  // one: fetched once on mount so the modal doesn't keep popping back up over
  // a session the user has chosen to dismiss.
  useEffect(() => {
    reposNew()
      .then(setNewRepos)
      .catch(() => setNewRepos([]));
  }, []);

  /** The modal's only exit: dismiss the whole batch and reload. */
  const handleAckNew = useCallback(async () => {
    try {
      await reposAckNew(new Date().toISOString());
    } catch {
      // A failure just closes the modal; the repos stay "new" and the modal
      // returns next time the window opens.
    }
    setNewRepos([]);
    void load();
  }, [load]);

  if (error) {
    return (
      <Shell>
        <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
          <p className="text-sm font-medium">
            The GitSurveil service isn’t running
          </p>
          <p className="max-w-md text-xs text-neutral-500">
            Start it with{" "}
            <code className="rounded bg-neutral-200 px-1 py-0.5 dark:bg-neutral-800">
              gitsurveild --foreground
            </code>
            . Monitoring and notifications continue without this window open.
          </p>
          <button
            type="button"
            onClick={() => void load()}
            className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white dark:bg-neutral-100 dark:text-neutral-900"
          >
            Retry
          </button>
        </div>
      </Shell>
    );
  }

  if (!data) {
    return (
      <Shell>
        <p className="p-10 text-center text-sm text-neutral-500">Loading…</p>
      </Shell>
    );
  }

  return (
    <Shell>
      {newRepos.length > 0 && (
        <NewReposModal repos={newRepos} onClose={() => void handleAckNew()} />
      )}
      <div className="flex h-full">
        <nav
          aria-label="Sections"
          className="flex w-44 shrink-0 flex-col border-r border-neutral-200 bg-neutral-50 p-2 dark:border-neutral-800 dark:bg-neutral-950"
        >
          {NAV.map((entry) => (
            <button
              key={entry.id}
              type="button"
              aria-current={view === entry.id ? "page" : undefined}
              onClick={() => setView(entry.id)}
              className={`rounded px-2 py-1.5 text-left text-sm ${
                view === entry.id
                  ? "bg-neutral-200 font-medium dark:bg-neutral-800"
                  : "hover:bg-neutral-100 dark:hover:bg-neutral-900"
              }`}
            >
              {entry.label}
              {entry.id === "dashboard" && data.items.length > 0 && (
                <span className="ml-1 text-[11px] text-neutral-500">
                  {data.items.length}
                </span>
              )}
            </button>
          ))}

          <div className="mt-auto px-2 py-1 text-[11px] text-neutral-500">
            {data.accounts.length === 0
              ? "No account configured"
              : `${data.accounts.length} account${data.accounts.length === 1 ? "" : "s"}`}
            <br />
            service v{data.status.version}
          </div>
        </nav>

        <main className="min-w-0 flex-1 overflow-hidden">
          {/* Keyed by view so switching away from a crashed pane resets it. */}
          <ViewErrorBoundary key={view} onReset={() => setView("dashboard")}>
            {view === "dashboard" && (
            <Dashboard
              items={data.items}
              accounts={data.accounts}
              onRefresh={() => void load()}
            />
          )}
          {view === "history" && (
            <History items={data.history} onRefresh={() => void load()} />
          )}
          {view === "pull-requests" && (
            <PullRequests
              accounts={data.accounts}
              onOpenRepos={() => setView("repos")}
            />
          )}
          {view === "rules" && <Rules rules={data.rules} />}
          {view === "repos" && (
            <Repos
              catalog={data.repos}
              accounts={data.accounts}
              onChange={() => void load()}
            />
          )}
          {view === "accounts" && (
            <Accounts
              accounts={data.accounts}
              catalog={data.repos}
              onChange={() => void load()}
            />
          )}
          {view === "settings" && <Settings />}
          </ViewErrorBoundary>
        </main>
      </div>
    </Shell>
  );
}

/** Resolved and dismissed items. Dismissed ones can be restored from here. */
function History({
  items,
  onRefresh,
}: {
  items: ScoredItem[];
  onRefresh: () => void;
}) {
  if (items.length === 0) {
    return (
      <p className="p-10 text-center text-sm text-neutral-500">
        Nothing here yet. Items appear once they’re resolved or dismissed.
      </p>
    );
  }
  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-between border-b border-neutral-200 px-6 py-3 dark:border-neutral-800">
        <p className="text-xs text-neutral-500">
          {items.length} resolved or dismissed
        </p>
        <button
          type="button"
          onClick={async () => {
            if (
              !confirm(
                `Clear ${items.length} items from history? This can’t be undone.`,
              )
            ) {
              return;
            }
            await clearHistory();
            onRefresh();
          }}
          className="rounded border border-neutral-300 px-2.5 py-1 text-[11px] text-neutral-600 hover:bg-neutral-100 dark:border-neutral-700 dark:text-neutral-300 dark:hover:bg-neutral-800"
        >
          Clear all history
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <ul>
          {items.map((item) => (
            <li key={item.id} className="flex items-center">
              <div className="min-w-0 flex-1">
                <ItemRow item={item} onOpen={() => void openUrl(item.url)} />
              </div>
              {item.state === "dismissed" && (
                <button
                  type="button"
                  onClick={async () => {
                    // The undismiss command emits `items-changed`, which
                    // refreshes both History and the Dashboard via the
                    // app-level listener — no local refresh needed.
                    await undismissItem(item.id);
                  }}
                  className="mr-4 shrink-0 rounded border border-neutral-300 px-2 py-0.5 text-[11px] dark:border-neutral-700"
                >
                  Restore
                </button>
              )}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="h-screen overflow-hidden bg-white text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      {children}
    </div>
  );
}

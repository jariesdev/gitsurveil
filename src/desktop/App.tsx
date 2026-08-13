/**
 * The full desktop window (`specs/desktop-ui.md`).
 *
 * Sidebar navigation over a handful of views, all fed from the daemon. Like
 * the popover it holds no state the daemon doesn't already own, so closing
 * this window (and dropping its webview) loses nothing.
 */

import { useCallback, useEffect, useState } from "react";
import {
  daemonStatus,
  listAccounts,
  listHistory,
  listItems,
  listRepos,
  listRules,
  openUrl,
  undismissItem,
} from "../ipc";
import type {
  AccountRef,
  RepoConfig,
  Rule,
  ScoredItem,
  StatusResult,
} from "../types";
import { Accounts } from "./Accounts";
import { Dashboard } from "./Dashboard";
import { ItemRow } from "./ItemRow";
import { PullRequests } from "./PullRequests/PullRequests";
import { Repos } from "./Repos";
import { Rules } from "./Rules";

type View = "dashboard" | "pull-requests" | "history" | "rules" | "repos" | "accounts";

const NAV: { id: View; label: string }[] = [
  { id: "dashboard", label: "Dashboard" },
  { id: "pull-requests", label: "Pull Requests" },
  { id: "history", label: "History" },
  { id: "rules", label: "Rules" },
  { id: "repos", label: "Repositories" },
  { id: "accounts", label: "Accounts" },
];

interface Data {
  items: ScoredItem[];
  history: ScoredItem[];
  accounts: AccountRef[];
  rules: Rule[];
  repos: RepoConfig[];
  status: StatusResult;
}

export function App() {
  const [view, setView] = useState<View>("dashboard");
  const [data, setData] = useState<Data | null>(null);
  const [error, setError] = useState<string | null>(null);

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
        listRepos(),
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

  if (error) {
    return (
      <Shell>
        <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
          <p className="text-sm font-medium">
            The gitsurveil service isn’t running
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
          {view === "pull-requests" && <PullRequests accounts={data.accounts} />}
          {view === "rules" && <Rules rules={data.rules} />}
          {view === "repos" && (
            <Repos repos={data.repos} onChange={() => void load()} />
          )}
          {view === "accounts" && (
            <Accounts accounts={data.accounts} onChange={() => void load()} />
          )}
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
    <div className="h-full overflow-y-auto">
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
                  await undismissItem(item.id);
                  onRefresh();
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
  );
}

function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="h-screen overflow-hidden bg-white text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      {children}
    </div>
  );
}

/**
 * Account management (`specs/desktop-ui.md`).
 *
 * PAT entry only for now — OAuth device flow is a later addition. The token
 * goes straight to the Rust side, which validates it against GitHub before
 * storing it in the OS keychain; it is never held in component state longer
 * than the submit, and never sent anywhere else.
 */

import { useState } from "react";
import { addAccount, removeAccount, reposSetNotify } from "../ipc";
import type { AccountRef, RepoCatalog } from "../types";

export function Accounts({
  accounts,
  catalog,
  onChange,
}: {
  accounts: AccountRef[];
  catalog: RepoCatalog;
  onChange: () => void;
}) {
  const [host, setHost] = useState("github.com");
  const [token, setToken] = useState("");
  const [apiBase, setApiBase] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isEnterprise = host.trim() !== "" && host.trim() !== "github.com";

  async function handleAdd(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await addAccount(host.trim(), token.trim(), apiBase.trim() || undefined);
      setToken("");
      onChange();
    } catch (e) {
      // The daemon's validation error is the useful message here ("bad
      // credentials", "missing scope"), so surface it rather than a generic one.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove(account: AccountRef) {
    await removeAccount(account.id);
    onChange();
  }

  return (
    <div className="mx-auto max-w-2xl p-6">
      <h2 className="text-base font-semibold">Accounts</h2>

      {accounts.length === 0 ? (
        <p className="mt-2 text-sm text-neutral-500">
          No accounts yet. Add a personal access token to start monitoring.
        </p>
      ) : (
        <ul className="mt-4 divide-y divide-neutral-200 dark:divide-neutral-800">
          {accounts.map((account) => (
            <li key={account.id} className="py-2">
              <div className="flex items-center justify-between">
                <div>
                  <div className="text-sm">{account.login}</div>
                  <div className="text-[11px] text-neutral-500">
                    {account.host}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => void handleRemove(account)}
                  className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
                >
                  Remove
                </button>
              </div>
              <NotifyChecklist
                account={account}
                catalog={catalog}
                onChange={onChange}
              />
            </li>
          ))}
        </ul>
      )}

      <form onSubmit={handleAdd} className="mt-6 space-y-3">
        <h3 className="text-sm font-medium">Add an account</h3>

        <Field label="Host">
          <input
            value={host}
            onChange={(e) => setHost(e.target.value)}
            placeholder="github.com"
            className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </Field>

        {isEnterprise && (
          <Field label="API base URL">
            <input
              value={apiBase}
              onChange={(e) => setApiBase(e.target.value)}
              placeholder={`https://${host.trim()}/api/v3`}
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          </Field>
        )}

        <Field label="Personal access token">
          <input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder="ghp_…"
            autoComplete="off"
            className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
          <p className="mt-1 text-[11px] text-neutral-500">
            Needs the <code>notifications</code> and <code>repo</code> scopes.
            Stored in your OS keychain, never on disk.
          </p>
        </Field>

        {error && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {error}
          </p>
        )}

        <button
          type="submit"
          disabled={busy || !token.trim() || !host.trim()}
          className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
        >
          {busy ? "Validating…" : "Add account"}
        </button>
      </form>
    </div>
  );
}

/**
 * Per-account repo checklist: which repos feed notifications and the Pull
 * Requests view (`notify_enabled`). Collapsed by default — `<details>` needs
 * no state of its own, and an account can have far more repos than fit
 * comfortably inline.
 */
function NotifyChecklist({
  account,
  catalog,
  onChange,
}: {
  account: AccountRef;
  catalog: RepoCatalog;
  onChange: () => void;
}) {
  const repos = catalog.repos.filter((r) => r.account_id === account.id);
  if (repos.length === 0) return null;

  async function toggle(repo: string, enabled: boolean) {
    await reposSetNotify(account.id, repo, enabled);
    onChange();
  }

  return (
    <details className="mt-2">
      <summary className="cursor-pointer text-xs text-neutral-500">
        Notify me about {repos.filter((r) => r.notify_enabled).length} of{" "}
        {repos.length} repositories
      </summary>
      <ul className="mt-2 space-y-1.5 pl-1">
        {repos.map((repo) => (
          <li key={repo.full_name} className="flex items-center gap-2">
            <input
              id={`notify-${account.id}-${repo.full_name}`}
              type="checkbox"
              checked={repo.notify_enabled}
              onChange={(e) => void toggle(repo.full_name, e.target.checked)}
              className="h-4 w-4"
            />
            <label
              htmlFor={`notify-${account.id}-${repo.full_name}`}
              className="text-xs"
            >
              {repo.full_name}
            </label>
          </li>
        ))}
      </ul>
    </details>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs text-neutral-600 dark:text-neutral-400">
        {label}
      </span>
      {children}
    </label>
  );
}

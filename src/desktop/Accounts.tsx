/**
 * Account management (`specs/desktop-ui.md`).
 *
 * The add form lives in `AccountForm`, shared with the first-run onboarding
 * screen so both always offer the exact same fields, provider choices, and
 * token guidance. This view wraps it with the account list, per-account
 * remove, and the per-repo notification checklist.
 */

import { useState } from "react";
import { removeAccount, reposSetNotify, updateAccountToken } from "../ipc";
import type { AccountRef, RepoCatalog } from "../types";
import { AccountForm } from "./AccountForm";

export function Accounts({
  accounts,
  catalog,
  onChange,
}: {
  accounts: AccountRef[];
  catalog: RepoCatalog;
  onChange: () => void;
}) {
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
                <div className="flex gap-2">
                  <UpdateTokenButton account={account} onChange={onChange} />
                  <button
                    type="button"
                    onClick={() => void handleRemove(account)}
                    className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
                  >
                    Remove
                  </button>
                </div>
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

      <div className="mt-6">
        <h3 className="text-sm font-medium">Add an account</h3>
        <div className="mt-3">
          <AccountForm onAdded={onChange} />
        </div>
      </div>
    </div>
  );
}

/**
 * Inline token-update button per account. Expands a minimal form with just
 * the new PAT field; the host and API base are unchanged so only the token
 * needs to be re-entered. The new token is validated against GitHub before
 * it replaces the old one in the OS keychain.
 */
function UpdateTokenButton({
  account,
  onChange,
}: {
  account: AccountRef;
  onChange: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!open) {
    return (
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
      >
        Update token
      </button>
    );
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await updateAccountToken(account.id, token.trim());
      setToken("");
      setOpen(false);
      onChange();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      onSubmit={(e) => void handleSubmit(e)}
      className="flex items-center gap-2"
    >
      <input
        type="password"
        value={token}
        onChange={(e) => setToken(e.target.value)}
        placeholder="New PAT"
        autoComplete="off"
        className="w-48 rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
      />
      <button
        type="submit"
        disabled={busy || !token.trim()}
        className="rounded bg-neutral-900 px-2 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
      >
        {busy ? "Validating…" : "Save"}
      </button>
      <button
        type="button"
        onClick={() => {
          setOpen(false);
          setToken("");
          setError(null);
        }}
        className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
      >
        Cancel
      </button>
      {error && (
        <span role="alert" className="text-xs text-red-600 dark:text-red-400">
          {error}
        </span>
      )}
    </form>
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

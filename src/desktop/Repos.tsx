/**
 * Local clone paths (`specs/conflict-resolver.md`).
 *
 * Conflict resolution needs a local clone to work on, and it never touches the
 * user's checkout — it creates temporary worktrees inside the repo. The daemon
 * validates each path on save (is a git repo, `origin` points at the GitHub
 * repo), so a misconfigured path fails here rather than halfway through a
 * resolution.
 */

import { useState } from "react";
import { removeRepo, setRepo } from "../ipc";
import type { RepoConfig } from "../types";

export function Repos({
  repos,
  onChange,
}: {
  repos: RepoConfig[];
  onChange: () => void;
}) {
  const [repo, setRepoSlug] = useState("");
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleAdd(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await setRepo(repo.trim(), path.trim());
      setRepoSlug("");
      setPath("");
      onChange();
    } catch (e) {
      // The daemon's validation message ("not a git repository",
      // "origin does not point at `acme/api`") is what the user needs to fix.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove(entry: RepoConfig) {
    await removeRepo(entry.repo);
    onChange();
  }

  return (
    <div className="mx-auto max-w-2xl p-6">
      <h2 className="text-base font-semibold">Repositories</h2>
      <p className="mt-1 text-xs text-neutral-500">
        Local clone paths used for resolving pull-request conflicts. Resolution
        happens in temporary worktrees, never in your checkout.
      </p>

      {repos.length === 0 ? (
        <p className="mt-4 text-sm text-neutral-500">
          No clone paths configured. Add one to resolve conflicts in-app.
        </p>
      ) : (
        <ul className="mt-4 divide-y divide-neutral-200 dark:divide-neutral-800">
          {repos.map((entry) => (
            <li
              key={entry.repo}
              className="flex items-center justify-between py-2"
            >
              <div className="min-w-0">
                <div className="text-sm">{entry.repo}</div>
                <div className="truncate text-[11px] text-neutral-500">
                  {entry.path}
                </div>
              </div>
              <button
                type="button"
                onClick={() => void handleRemove(entry)}
                className="ml-3 shrink-0 rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}

      <form onSubmit={handleAdd} className="mt-6 space-y-3">
        <h3 className="text-sm font-medium">Add a clone path</h3>

        <Field label="Repository">
          <input
            value={repo}
            onChange={(e) => setRepoSlug(e.target.value)}
            placeholder="owner/name"
            className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </Field>

        <Field label="Local clone path">
          <input
            value={path}
            onChange={(e) => setPath(e.target.value)}
            placeholder="/path/to/clone"
            className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
          <p className="mt-1 text-[11px] text-neutral-500">
            Must be a git repository whose <code>origin</code> remote is this
            GitHub repo.
          </p>
        </Field>

        {error && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {error}
          </p>
        )}

        <button
          type="submit"
          disabled={busy || !repo.trim() || !path.trim()}
          className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
        >
          {busy ? "Validating…" : "Add clone path"}
        </button>
      </form>
    </div>
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

/**
 * Application settings (`specs/desktop-ui.md`).
 *
 * Currently one section: the "Open with…" applications offered in the
 * Repositories pane's worktree context menus. The daemon owns the registry
 * (`apps.add` / `apps.remove`) and spawns `command <path>` when one is chosen;
 * this pane just lists and edits it. Apps are loaded on mount rather than
 * through the window-wide load, so the shared `App` state doesn't carry rows
 * it never renders.
 */

import { useCallback, useEffect, useState } from "react";
import { appsAdd, appsList, appsRemove } from "../ipc";
import type { RegisteredApp } from "../types";

export function Settings() {
  const [apps, setApps] = useState<RegisteredApp[]>([]);
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setApps(await appsList());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  async function handleAdd(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await appsAdd(name.trim(), command.trim());
      setName("");
      setCommand("");
      await reload();
    } catch (e) {
      // The daemon's validation error is the useful message ("already
      // registered", "not a single executable name"), so surface it.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleRemove(app: RegisteredApp) {
    setError(null);
    try {
      await appsRemove(app.command);
      await reload();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <div className="mx-auto max-w-2xl p-6">
      <h2 className="text-base font-semibold">Settings</h2>

      <h3 className="mt-6 text-sm font-medium">Open with… applications</h3>
      <p className="mt-1 text-xs text-neutral-500">
        Apps offered in the "Open with…" submenu of worktree context menus.
        Each is an executable on your PATH or an absolute path to one; choosing
        one opens the worktree with{" "}
        <code className="rounded bg-neutral-200 px-1 py-0.5 dark:bg-neutral-800">
          command &lt;path&gt;
        </code>
        .
      </p>

      {apps.length === 0 ? (
        <p className="mt-4 text-sm text-neutral-500">
          No applications yet. Add one to enable "Open with…" on worktrees.
        </p>
      ) : (
        <ul className="mt-4 divide-y divide-neutral-200 dark:divide-neutral-800">
          {apps.map((app) => (
            <li
              key={app.command}
              className="flex items-center justify-between py-2"
            >
              <div>
                <div className="text-sm">{app.name}</div>
                <div className="font-mono text-[11px] text-neutral-500">
                  {app.command}
                </div>
              </div>
              <button
                type="button"
                onClick={() => void handleRemove(app)}
                className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}

      <form onSubmit={handleAdd} className="mt-6 space-y-3">
        <h4 className="text-xs font-medium text-neutral-600 dark:text-neutral-400">
          Add an application
        </h4>

        <Field label="Name">
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. VS Code"
            className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </Field>

        <Field label="Command">
          <input
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder="e.g. code"
            className="w-full rounded border border-neutral-200 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
          />
        </Field>
        <p className="mt-1 text-[11px] text-neutral-500">
          An executable name on your PATH (e.g.{" "}
          <code className="rounded bg-neutral-200 px-1 py-0.5 dark:bg-neutral-800">
            code
          </code>
          ) or an absolute path to one (e.g.{" "}
          <code className="rounded bg-neutral-200 px-1 py-0.5 dark:bg-neutral-800">
            /usr/local/bin/code
          </code>
          ) — no arguments or spaces.
        </p>

        {error && (
          <p role="alert" className="text-xs text-red-600 dark:text-red-400">
            {error}
          </p>
        )}

        <button
          type="submit"
          disabled={busy || !name.trim() || !command.trim()}
          className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
        >
          {busy ? "Adding…" : "Add application"}
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

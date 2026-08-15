/**
 * Application settings (`specs/desktop-ui.md`).
 *
 * Two sections: which notification kinds may interrupt you
 * (`notifications.prefs` / `notifications.set_pref`), and the "Open with…"
 * applications offered in the Repositories pane's worktree context menus. The
 * daemon owns both registries; this pane just lists and edits them. Data is
 * loaded on mount rather than through the window-wide load, so the shared
 * `App` state doesn't carry rows it never renders. Adding an application
 * happens in a modal (`AddAppModal.tsx`), keeping this pane a short list.
 */

import { useCallback, useEffect, useState } from "react";
import {
  appsAdd,
  appsList,
  appsRemove,
  notificationsPrefs,
  notificationsSetPref,
} from "../ipc";
import { KIND_LABELS, type KindPref, type RegisteredApp } from "../types";
import { AddAppModal } from "./AddAppModal";

export function Settings() {
  const [apps, setApps] = useState<RegisteredApp[]>([]);
  const [prefs, setPrefs] = useState<KindPref[]>([]);
  const [prefsError, setPrefsError] = useState<string | null>(null);
  const [appsError, setAppsError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    notificationsPrefs()
      .then(setPrefs)
      .catch((e) => setPrefsError(String(e)));
  }, []);

  async function togglePref(kind: KindPref["kind"], enabled: boolean) {
    setPrefsError(null);
    setPrefs((prev) => prev.map((p) => (p.kind === kind ? { ...p, enabled } : p)));
    try {
      await notificationsSetPref(kind, enabled);
    } catch (e) {
      // Roll back on failure — the daemon didn't persist it, so the
      // checkbox must not claim otherwise.
      setPrefs((prev) => prev.map((p) => (p.kind === kind ? { ...p, enabled: !enabled } : p)));
      setPrefsError(String(e));
    }
  }

  const reload = useCallback(async () => {
    try {
      setApps(await appsList());
    } catch (e) {
      setAppsError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** Registers an app; the modal closes itself on success. */
  async function addApp(name: string, command: string) {
    await appsAdd(name, command);
    await reload();
  }

  async function handleRemove(app: RegisteredApp) {
    setAppsError(null);
    try {
      await appsRemove(app.command);
      await reload();
    } catch (e) {
      setAppsError(String(e));
    }
  }

  return (
    /* The scroll container spans the whole pane so the scrollbar sits at the
       viewport's right edge; the content stays centered and width-capped. */
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-2xl p-6">
        <h2 className="text-base font-semibold">Settings</h2>

        <h3 className="mt-6 text-sm font-medium">Notifications</h3>
        <p className="mt-1 text-xs text-neutral-500">
          Which kinds of activity may interrupt you with a desktop notification.
          Unchecking a kind never hides it from the Dashboard or history — it
          only stops the interruption.
        </p>

        {prefsError && (
          <p role="alert" className="mt-2 text-xs text-red-600 dark:text-red-400">
            {prefsError}
          </p>
        )}

        <ul className="mt-3 space-y-1.5">
          {prefs.map((pref) => (
            <li key={pref.kind} className="flex items-center gap-2">
              <input
                id={`notify-kind-${pref.kind}`}
                type="checkbox"
                checked={pref.enabled}
                onChange={(e) => void togglePref(pref.kind, e.target.checked)}
                className="h-4 w-4"
              />
              <label htmlFor={`notify-kind-${pref.kind}`} className="text-sm">
                {KIND_LABELS[pref.kind]}
              </label>
            </li>
          ))}
        </ul>

        <h3 className="mt-8 text-sm font-medium">Open with… applications</h3>
        <p className="mt-1 text-xs text-neutral-500">
          Apps offered in the "Open with…" submenu of worktree context menus.
          Each is an executable on your PATH or an absolute path to one; choosing
          one opens the worktree with{" "}
          <code className="rounded bg-neutral-200 px-1 py-0.5 dark:bg-neutral-800">
            command &lt;path&gt;
          </code>
          .
        </p>

        {appsError && (
          <p role="alert" className="mt-2 text-xs text-red-600 dark:text-red-400">
            {appsError}
          </p>
        )}

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

        <button
          type="button"
          onClick={() => setAdding(true)}
          className="mt-4 rounded border border-neutral-300 px-2.5 py-1 text-xs dark:border-neutral-700"
        >
          Add application
        </button>

        {adding && (
          <AddAppModal onAdd={addApp} onClose={() => setAdding(false)} />
        )}
      </div>
    </div>
  );
}

/**
 * The "Add an application" modal (`specs/desktop-ui.md`).
 *
 * The Settings pane's application form lives in a modal so the pane stays a
 * short list; clicking **Add application** opens this. Owns the form state;
 * `onAdd` performs the daemon call and the parent reloads on success. Errors
 * (e.g. "already registered") are shown inline and keep the modal open.
 */

import { useCallback, useEffect, useState } from "react";
import { open as pickFile } from "@tauri-apps/plugin-dialog";

export function AddAppModal({
  onAdd,
  onClose,
}: {
  /** Registers the app with the daemon; throws with the daemon's message. */
  onAdd: (name: string, command: string) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Escape closes, same as the backdrop and the Cancel button — no reason to
  // force the user through the form once they've changed their mind.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  /** Browses for an executable and fills the command field with its path. */
  const pickExecutable = useCallback(async () => {
    const picked = await pickFile({
      directory: false,
      multiple: false,
      title: "Choose an executable",
    });
    if (typeof picked === "string") setCommand(picked);
  }, []);

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await onAdd(name.trim(), command.trim());
      onClose();
    } catch (e) {
      // The daemon's validation error is the useful message ("already
      // registered", "not a single executable name"), so surface it.
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Add an application"
      className="fixed inset-0 z-50 flex items-center justify-center bg-neutral-950/40 p-6"
      onPointerDown={(event) => {
        // Clicking the backdrop dismisses, same as the button.
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="max-h-full w-full max-w-md overflow-y-auto rounded-lg border border-neutral-200 bg-white shadow-xl dark:border-neutral-700 dark:bg-neutral-900">
        <header className="border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
          <h2 className="text-sm font-semibold">Add an application</h2>
          <p className="mt-0.5 text-[11px] text-neutral-500">
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
        </header>

        <form onSubmit={handleSubmit} className="space-y-3 px-4 py-3">
          <Field label="Name">
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. VS Code"
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          </Field>

          <div>
            <span className="mb-1 block text-xs text-neutral-600 dark:text-neutral-400">
              Application or Command
            </span>
            <div className="flex gap-2">
              <input
                aria-label="Application or Command"
                value={command}
                onChange={(e) => setCommand(e.target.value)}
                placeholder="e.g. code"
                className="min-w-0 flex-1 rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
              />
              <button
                type="button"
                onClick={() => void pickExecutable()}
                className="shrink-0 rounded border border-neutral-300 px-2 py-1 text-sm dark:border-neutral-700"
              >
                Browse…
              </button>
            </div>
          </div>

          {error && (
            <p role="alert" className="text-xs text-red-600 dark:text-red-400">
              {error}
            </p>
          )}

          <footer className="flex justify-end gap-2 border-t border-neutral-100 pt-3 dark:border-neutral-800">
            <button
              type="button"
              onClick={onClose}
              disabled={busy}
              className="rounded border border-neutral-300 px-3 py-1.5 text-sm disabled:opacity-50 dark:border-neutral-700"
            >
              Cancel
            </button>
            <button
              type="submit"
              disabled={busy || !name.trim() || !command.trim()}
              className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
            >
              {busy ? "Adding…" : "Add"}
            </button>
          </footer>
        </form>
      </div>
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

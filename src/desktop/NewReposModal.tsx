/**
 * The new-repositories modal (`specs/desktop-ui.md`).
 *
 * Shown when the main window opens and discovery found repos the user hasn't
 * acknowledged yet. It is dismiss-all only: the single action acks the whole
 * batch (`repos.ack_new`). Per-repo decisions live in the Repositories pane,
 * so this modal deliberately has no per-row controls — one glance, one click.
 */

import { useEffect } from "react";
import type { Repository } from "../types";

export function NewReposModal({
  repos,
  onClose,
}: {
  /** The unacked repos, newest-first. */
  repos: Repository[];
  /** Dismisses the whole batch (the modal's only way out). */
  onClose: () => void;
}) {
  // Escape is a dismissal too — the modal's contract is "seen or dismissed",
  // so there is no path that keeps the modal on screen once the user acts.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="New repositories"
      className="fixed inset-0 z-50 flex items-center justify-center bg-neutral-950/40 p-6"
      onPointerDown={(event) => {
        // Clicking the backdrop dismisses, same as the button.
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="max-h-full w-full max-w-md overflow-y-auto rounded-lg border border-neutral-200 bg-white shadow-xl dark:border-neutral-700 dark:bg-neutral-900">
        <header className="border-b border-neutral-200 px-4 py-3 dark:border-neutral-800">
          <h2 className="text-sm font-semibold">New repositories</h2>
          <p className="mt-0.5 text-[11px] text-neutral-500">
            {repos.length === 1
              ? "1 repository was discovered you haven’t seen yet."
              : `${repos.length} repositories were discovered you haven’t seen yet.`}
          </p>
        </header>

        <ul className="divide-y divide-neutral-100 dark:divide-neutral-800">
          {repos.map((repo) => (
            <li key={repo.full_name} className="px-4 py-2">
              <div className="text-sm">{repo.full_name}</div>
              {repo.description && (
                <div className="truncate text-[11px] text-neutral-500">
                  {repo.description}
                </div>
              )}
            </li>
          ))}
        </ul>

        <footer className="flex justify-end border-t border-neutral-200 px-4 py-3 dark:border-neutral-800">
          <button
            type="button"
            autoFocus
            onClick={onClose}
            className="rounded bg-neutral-900 px-3 py-1.5 text-sm text-white dark:bg-neutral-100 dark:text-neutral-900"
          >
            Not now
          </button>
        </footer>
      </div>
    </div>
  );
}

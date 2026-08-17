/**
 * Reusable confirmation dialog.
 *
 * A lightweight modal that asks the user to confirm or cancel a potentially
 * destructive action. Follows the same backdrop / Escape / focus-trap pattern
 * as `AddAppModal` and `NewReposModal`.
 */

import { useEffect } from "react";

export function ConfirmDialog({
  title,
  message,
  confirmLabel,
  confirmClass,
  busy,
  onConfirm,
  onClose,
}: {
  title: string;
  message: string;
  confirmLabel: string;
  /** Extra Tailwind classes for the confirm button (e.g. danger colour). */
  confirmClass?: string;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
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
      aria-label={title}
      className="fixed inset-0 z-50 flex items-center justify-center bg-neutral-950/40 p-6"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="max-h-full w-full max-w-sm overflow-y-auto rounded-lg border border-neutral-200 bg-white shadow-xl dark:border-neutral-700 dark:bg-neutral-900">
        <header className="px-4 py-3">
          <h2 className="text-sm font-semibold">{title}</h2>
          <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-400">
            {message}
          </p>
        </header>

        <footer className="flex justify-end gap-2 border-t border-neutral-100 px-4 py-3 dark:border-neutral-800">
          <button
            type="button"
            onClick={onClose}
            disabled={busy}
            className="rounded border border-neutral-300 px-3 py-1.5 text-sm disabled:opacity-50 dark:border-neutral-700"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={onConfirm}
            disabled={busy}
            className={`rounded px-3 py-1.5 text-sm text-white disabled:opacity-50 ${
              confirmClass ?? "bg-red-600 hover:bg-red-700 dark:bg-red-700 dark:hover:bg-red-800"
            }`}
          >
            {busy ? "Working…" : confirmLabel}
          </button>
        </footer>
      </div>
    </div>
  );
}

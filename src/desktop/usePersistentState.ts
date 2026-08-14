/**
 * State that outlives the component and the webview.
 *
 * Two things destroy ordinary `useState` here. Switching sidebar views
 * unmounts the view being left, and closing a window drops its webview
 * entirely — the latter deliberately, since an idle app holding a live webview
 * is what the memory budget exists to prevent (`specs/architecture.md`).
 *
 * So anything the user expects to still be there when they come back has to
 * live outside React. `localStorage` is the right home for *presentation*
 * state — which filter is selected, which tab is open. Domain state stays in
 * the daemon, as `CLAUDE.md` requires; a dropdown selection is not domain
 * state.
 */

import { useCallback, useEffect, useState } from "react";

/** Namespaced so a key can never collide with anything else on this origin. */
const PREFIX = "gitsurveil.";

/**
 * Like `useState`, but seeded from and written through to `localStorage`.
 *
 * `revive` gets the parsed value and returns the state to use. It exists so
 * callers can reject values that no longer make sense — a filter naming an
 * account that has since been removed would otherwise restore an empty list
 * with no visible cause. Returning the fallback discards the stored value.
 */
export function usePersistentState<T>(
  key: string,
  fallback: T,
  revive?: (stored: unknown, fallback: T) => T,
): [T, (value: T) => void] {
  const storageKey = PREFIX + key;

  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw === null) return fallback;
      const parsed: unknown = JSON.parse(raw);
      return revive ? revive(parsed, fallback) : (parsed as T);
    } catch {
      // Corrupt or unreadable storage must never take the view down with it;
      // a lost filter selection is a far smaller problem than a blank pane.
      return fallback;
    }
  });

  // Written in an effect rather than inside the setter so the stored value
  // still tracks state that changed for any other reason.
  useEffect(() => {
    try {
      localStorage.setItem(storageKey, JSON.stringify(value));
    } catch {
      // Storage can be full or disabled. Persistence is a convenience; losing
      // it is not worth surfacing an error over.
    }
  }, [storageKey, value]);

  const set = useCallback((next: T) => setValue(next), []);
  return [value, set];
}

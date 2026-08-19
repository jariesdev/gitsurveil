import { useCallback, useEffect, useRef } from "react";

/**
 * Returns a **stable** (identity-preserving) debounced version of
 * `callback`. When the returned function is called, any pending timer is
 * cleared and `callback` is scheduled to run after `delayMs` milliseconds.
 * If called again before the timer fires the previous timer is cancelled
 * and a new one starts — so `callback` only executes once the caller has
 * stopped invoking it for `delayMs`.
 *
 * The pending timer is cleaned up on unmount. Both `callback` and
 * `delayMs` are read through refs, so the returned function always uses
 * the latest values without changing identity.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function useDebouncedCallback<T extends (...args: any[]) => void>(
  callback: T,
  delayMs: number,
): T {
  const callbackRef = useRef(callback);
  const delayRef = useRef(delayMs);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Keep refs current on every render so the memoized function below
  // always reads the latest values.
  callbackRef.current = callback;
  delayRef.current = delayMs;

  // Clean up any pending timer on unmount.
  useEffect(() => {
    return () => {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
      }
    };
  }, []);

  // eslint-disable-next-line react-hooks/exhaustive-deps
  const debounced = useCallback(
    ((...args: unknown[]) => {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
      }
      timerRef.current = setTimeout(() => {
        timerRef.current = null;
        callbackRef.current(...args);
      }, delayRef.current);
    }) as T,
    [],
  );

  return debounced;
}

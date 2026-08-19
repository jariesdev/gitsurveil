import { useEffect, useRef } from "react";

/**
 * Sets up a repeating interval that calls `callback` every `delayMs`
 * milliseconds. The callback is kept up to date via a ref, so it always
 * sees the latest closure variables without needing to recreate the
 * interval.
 *
 * The interval is cleared automatically when the component unmounts or
 * when `delayMs` changes.
 */
export function useInterval(callback: () => void, delayMs: number): void {
  const callbackRef = useRef(callback);

  // Always point at the latest callback so the interval never calls a
  // stale closure.
  useEffect(() => {
    callbackRef.current = callback;
  }, [callback]);

  useEffect(() => {
    const id = window.setInterval(() => callbackRef.current(), delayMs);
    return () => window.clearInterval(id);
  }, [delayMs]);
}

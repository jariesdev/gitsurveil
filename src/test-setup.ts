/** Vitest setup: adds jest-dom matchers (`toBeInTheDocument`, etc.). */
import "@testing-library/jest-dom/vitest";

// jsdom in this configuration ships no Web Storage implementation, so tests
// covering persistence have nothing to persist into. A real webview always
// provides one; this is a stand-in for the test environment only.
//
// `usePersistentState` tolerates storage being absent entirely — that path is
// covered by its own tests — but verifying that a value actually survives an
// unmount needs somewhere for it to survive.
if (typeof globalThis.localStorage === "undefined") {
  const store = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, String(v)),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
      key: (i: number) => [...store.keys()][i] ?? null,
      get length() {
        return store.size;
      },
    },
  });
}

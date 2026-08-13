/**
 * A small context menu shown at the cursor, used for right-click actions on
 * PR list rows.
 *
 * The webview's native context menu is suppressed in production
 * (`src/main.tsx`), so any right-click affordance has to be built here. It is
 * deliberately minimal — a stack of labelled buttons — because that is all the
 * current callers need; add items, not knobs, as needs grow.
 */

import { useLayoutEffect, useRef, useState } from "react";

/** One entry in the menu. */
export interface ContextMenuItem {
  /** The visible label, e.g. "Open in browser". */
  label: string;
  /** Runs when the item is chosen. The parent is responsible for closing. */
  onSelect: () => void;
}

/** Room left between the menu and the viewport edge when clamping. */
const EDGE_MARGIN = 8;

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  /** Cursor position in viewport coordinates (the `clientX/Y` of the right-click). */
  x: number;
  y: number;
  items: ContextMenuItem[];
  /** Closes the menu. Called on outside click, Escape, scroll, or resize. */
  onClose: () => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  // Initial render sits at the cursor; measured once the menu is in the DOM so
  // it is clamped to the viewport instead of overflowing off an edge.
  const [position, setPosition] = useState({ x, y });

  useLayoutEffect(() => {
    const el = menuRef.current;
    if (!el) return;
    // jsdom reports no layout (`offsetWidth` is undefined/0); guard so a
    // missing measurement clamps to the cursor position instead of `NaN`.
    const width = el.offsetWidth || 0;
    const height = el.offsetHeight || 0;
    const clamped = {
      x: Math.min(x, Math.max(0, window.innerWidth - width - EDGE_MARGIN)),
      y: Math.min(y, Math.max(0, window.innerHeight - height - EDGE_MARGIN)),
    };
    setPosition((prev) => (prev.x === clamped.x && prev.y === clamped.y ? prev : clamped));
  }, [x, y]);

  useLayoutEffect(() => {
    // `scroll` needs capture: the PR list scrolls in a nested container, and
    // its scroll events never reach `window` on the bubble phase.
    const dismiss = () => onClose();
    const onPointerDown = (event: PointerEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        onClose();
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
    };
  }, [onClose]);

  return (
    <div
      ref={menuRef}
      role="menu"
      className="fixed z-50 min-w-40 rounded-md border border-neutral-200 bg-white py-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900"
      style={{ left: position.x, top: position.y }}
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          autoFocus={items[0] === item}
          onClick={item.onSelect}
          className="block w-full px-3 py-1.5 text-left text-xs text-neutral-700 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-800"
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}

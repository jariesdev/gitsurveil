/**
 * A small context menu shown at the cursor, used for right-click actions on
 * PR list rows and worktree rows.
 *
 * The webview's native context menu is suppressed unconditionally
 * (`src/main.tsx`), so any right-click affordance has to be built here. It is
 * deliberately minimal — a stack of labelled buttons, with hover submenus for
 * parent items that have children — because that is all the current callers
 * need; add items, not knobs, as needs grow.
 */

import { useLayoutEffect, useRef, useState } from "react";

/** One entry in the menu. */
export interface ContextMenuItem {
  /** The visible label, e.g. "Open in browser". */
  label: string;
  /** Runs when the item is chosen. The parent is responsible for closing. A
   * parent item with `children` has no `onSelect`. */
  onSelect?: () => void;
  /** Child entries, shown in a submenu on hover to the right of the item. */
  children?: ContextMenuItem[];
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
      <MenuItems items={items} onClose={onClose} depth={0} />
    </div>
  );
}

/** Renders a flat stack of entries; parent items render hover submenus. */
function MenuItems({
  items,
  onClose,
  depth,
}: {
  items: ContextMenuItem[];
  onClose: () => void;
  /** 0 for the root menu; only the root's first item autofocuses. */
  depth: number;
}) {
  return (
    <>
      {items.map((item) =>
        item.children && item.children.length > 0 ? (
          <SubMenuItem key={item.label} item={item} onClose={onClose} />
        ) : (
          <button
            key={item.label}
            type="button"
            role="menuitem"
            autoFocus={depth === 0 && items[0] === item}
            onClick={item.onSelect}
            className="block w-full px-3 py-1.5 text-left text-xs text-neutral-700 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-800"
          >
            {item.label}
          </button>
        ),
      )}
    </>
  );
}

/**
 * A menu entry that opens a submenu on hover instead of selecting.
 *
 * The submenu is a `fixed` element at viewport coordinates, still rendered as
 * a DOM descendant of the anchor's wrapper — so hovering the submenu keeps the
 * wrapper hovered and the submenu open, and outside-click detection (which
 * walks `menuRef.contains`) sees it as inside the menu.
 */
function SubMenuItem({
  item,
  onClose,
}: {
  item: ContextMenuItem;
  onClose: () => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const subRef = useRef<HTMLDivElement>(null);
  // Overrides for the `left-full top-0` defaults (open right, top-aligned).
  // Only a left-flip and/or the bottom clamp are ever set, so the common case
  // needs no measurement and the submenu never waits on one to appear.
  const [override, setOverride] = useState<{
    left?: number | "auto";
    right?: number;
    top?: number;
  }>({});

  useLayoutEffect(() => {
    if (!open) return;
    const wrapper = wrapperRef.current;
    const el = subRef.current;
    if (!wrapper || !el) return;
    // jsdom reports no layout; guard like the root menu does.
    const width = el.offsetWidth || 0;
    const height = el.offsetHeight || 0;
    const rect = wrapper.getBoundingClientRect();
    const next: { left?: number | "auto"; right?: number; top?: number } = {};
    // Flip to the left only when the right edge won't fit and the left side
    // can; otherwise keep the default (right) side and let it clamp.
    const fitsRight = rect.right + width <= window.innerWidth - EDGE_MARGIN;
    const fitsLeft = rect.left - width >= EDGE_MARGIN;
    if (!fitsRight && fitsLeft) {
      next.left = "auto"; // drop the class's `left-full`
      next.right = 100; // right edge flush with the wrapper's left edge
    }
    // Default top aligns with the wrapper; nudge down to keep the bottom in
    // view, and never let it climb above the top margin.
    let top = 0;
    const overflowBottom = rect.top + height - (window.innerHeight - EDGE_MARGIN);
    if (overflowBottom > 0) top -= overflowBottom;
    if (rect.top < EDGE_MARGIN) top = EDGE_MARGIN - rect.top;
    next.top = top;
    setOverride((prev) =>
      prev.left === next.left && prev.right === next.right && prev.top === next.top
        ? prev
        : next,
    );
  }, [open]);

  return (
    <div
      ref={wrapperRef}
      className="relative"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button
        type="button"
        role="menuitem"
        className="flex w-full items-center justify-between gap-3 px-3 py-1.5 text-left text-xs text-neutral-700 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-800"
      >
        {item.label}
        <span aria-hidden className="text-neutral-400 dark:text-neutral-500">
          ›
        </span>
      </button>
      {open && (
        <div
          ref={subRef}
          role="menu"
          className="absolute left-full top-0 z-50 min-w-40 rounded-md border border-neutral-200 bg-white py-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900"
          style={{
            left: override.left,
            right: override.right,
            top: override.top,
          }}
        >
          <MenuItems items={item.children ?? []} onClose={onClose} depth={1} />
        </div>
      )}
    </div>
  );
}

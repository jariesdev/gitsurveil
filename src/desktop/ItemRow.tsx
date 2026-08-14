/** One row of the dashboard or history list. */

import { useState } from "react";
import { KIND_LABELS, type ScoredItem, type Severity } from "../types";
import { copyText } from "./clipboard";
import { ContextMenu } from "./ContextMenu";

/** Dot color per severity band, matching the tray icon palette. */
const SEVERITY_DOT: Record<Severity, string> = {
  critical: "bg-red-500",
  high: "bg-orange-500",
  normal: "bg-blue-500",
  info: "bg-neutral-400",
  idle: "bg-neutral-300",
};

/** Relative age like "3h" or "2d". */
export function age(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  if (!Number.isFinite(ms) || ms < 0) return "";
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

export function ItemRow({
  item,
  onOpen,
  onDismiss,
}: {
  item: ScoredItem;
  onOpen: () => void;
  /** Omitted in history, where there is nothing left to dismiss. */
  onDismiss?: () => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  return (
    <div
      className="group flex items-center gap-3 border-b border-neutral-200 px-4 py-2 hover:bg-neutral-50 dark:border-neutral-800 dark:hover:bg-neutral-800/50"
      onContextMenu={(event) => {
        event.preventDefault();
        setMenu({ x: event.clientX, y: event.clientY });
      }}
    >
      <span
        className={`h-2 w-2 shrink-0 rounded-full ${SEVERITY_DOT[item.severity]} ${
          item.muted ? "opacity-40" : ""
        }`}
        role="img"
        aria-label={
          item.muted
            ? `${item.severity} priority, muted`
            : `${item.severity} priority`
        }
      />

      <button
        type="button"
        onClick={onOpen}
        className="min-w-0 flex-1 text-left"
        title={item.title}
      >
        <div className="truncate text-sm text-neutral-900 dark:text-neutral-100">
          {item.title}
        </div>
        <div className="flex items-center gap-1.5 text-[11px] text-neutral-500">
          <span>{KIND_LABELS[item.kind]}</span>
          <span aria-hidden="true">·</span>
          <span className="truncate">
            {item.repo}
            {item.number !== null ? `#${item.number}` : ""}
          </span>
          {item.author && (
            <>
              <span aria-hidden="true">·</span>
              <span className="truncate">{item.author}</span>
            </>
          )}
          {item.ci_status === "failing" && (
            <span className="rounded bg-red-100 px-1 text-red-700 dark:bg-red-950 dark:text-red-300">
              CI failing
            </span>
          )}
        </div>
      </button>

      <span
        className="shrink-0 tabular-nums text-[11px] text-neutral-500 dark:text-neutral-400"
        title={`Priority score ${item.score}`}
      >
        {age(item.updated_at)}
      </span>

      {onDismiss && (
        // Revealed on hover to keep the row quiet, but always reachable by
        // keyboard — focus-within makes it visible when tabbed to.
        <button
          type="button"
          onClick={onDismiss}
          aria-label={`Dismiss ${item.title}`}
          className="shrink-0 rounded px-1.5 py-0.5 text-[11px] text-neutral-500 opacity-0 hover:bg-neutral-200 focus:opacity-100 group-hover:opacity-100 dark:hover:bg-neutral-700"
        >
          Dismiss
        </button>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            {
              label: "Copy URL",
              onSelect: () => {
                void copyText(item.url);
                setMenu(null);
              },
            },
          ]}
        />
      )}
    </div>
  );
}

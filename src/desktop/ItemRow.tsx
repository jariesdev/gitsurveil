/** One row of the dashboard or history list. */

import { useRef, useState } from "react";
import { KIND_LABELS, type ScoredItem, type Severity } from "../types";
import { browsersList, openUrl, openUrlWithBrowser } from "../ipc";
import { copyText } from "./clipboard";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";

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
  active = false,
  onOpen,
  onDismiss,
}: {
  item: ScoredItem;
  /** Whether this row's detail pane is the one currently open. */
  active?: boolean;
  onOpen: () => void;
  /** Omitted in history, where there is nothing left to dismiss. */
  onDismiss?: () => void;
}) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const browsersRef = useRef<string[] | null>(null);
  const [browsersLoaded, setBrowsersLoaded] = useState(false);

  const handleContextMenu = (event: React.MouseEvent) => {
    event.preventDefault();
    if (!browsersLoaded) {
      void browsersList()
        .then((list) => {
          browsersRef.current = list;
          setBrowsersLoaded(true);
        })
        .catch(() => {
          browsersRef.current = [];
          setBrowsersLoaded(true);
        });
    }
    setMenu({ x: event.clientX, y: event.clientY });
  };

  const contextItems: ContextMenuItem[] = [
    {
      label: "Open in Browser",
      children: [
        {
          label: "Default Browser",
          onSelect: () => {
            void openUrl(item.url);
            setMenu(null);
          },
        },
        ...(browsersLoaded && browsersRef.current && browsersRef.current.length > 0
          ? browsersRef.current.map((name) => ({
              label: name,
              onSelect: () => {
                void openUrlWithBrowser(item.url, name);
                setMenu(null);
              },
            }))
          : []),
      ],
    },
    {
      label: "Copy URL",
      onSelect: () => {
        void copyText(item.url);
        setMenu(null);
      },
    },
  ];

  return (
    <div
      aria-current={active || undefined}
      className={`group flex items-center gap-3 border-b border-l-2 border-b-neutral-200 px-4 py-2 dark:border-b-neutral-800 ${
        active
          ? "border-l-blue-500 bg-blue-50 dark:bg-blue-950/30"
          : "border-l-transparent hover:bg-neutral-50 dark:hover:bg-neutral-800/50"
      }`}
      onContextMenu={handleContextMenu}
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
          items={contextItems}
        />
      )}
    </div>
  );
}

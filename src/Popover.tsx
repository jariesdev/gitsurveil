/**
 * The notifications-only popover (`specs/menubar-ui.md`).
 *
 * Deliberately read-only: it lists what needs attention and opens items on
 * GitHub. Management features (filters, rules, PR actions) belong to the full
 * desktop UI, so this stays small enough to mount instantly every time the
 * webview is rebuilt after a tray click.
 */

import { useCallback, useEffect, useState } from "react";
import { copyText } from "./desktop/clipboard";
import { ContextMenu } from "./desktop/ContextMenu";
import { daemonStatus, listItems, openMainWindow, openUrl } from "./ipc";
import { KIND_LABELS, type ScoredItem, type Severity, type StatusResult } from "./types";

/** What the popover is currently showing. */
type LoadState =
  | { phase: "loading" }
  | { phase: "ready"; items: ScoredItem[]; status: StatusResult }
  /** Almost always "the daemon isn't running", which gets its own UI. */
  | { phase: "unreachable"; message: string };

/** Relative age like "3h" or "2d", for a compact list row. */
function age(iso: string): string {
  const ms = Date.now() - new Date(iso).getTime();
  if (!Number.isFinite(ms) || ms < 0) return "";
  const mins = Math.floor(ms / 60000);
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/** Tailwind class for each severity band, matching the tray icon colors. */
const SEVERITY_DOT: Record<Severity, string> = {
  critical: "bg-red-500",
  high: "bg-orange-500",
  normal: "bg-blue-500",
  info: "bg-neutral-400",
  idle: "bg-neutral-300",
};

/**
 * Leading dot showing the item's priority band. Muted items render hollow:
 * still ranked and still visible, but explicitly not going to interrupt.
 */
function SeverityDot({
  severity,
  muted,
}: {
  severity: Severity;
  muted: boolean;
}) {
  return (
    <span
      className={`inline-block h-2 w-2 shrink-0 rounded-full ${SEVERITY_DOT[severity]} ${
        muted ? "opacity-40" : ""
      }`}
      aria-label={muted ? `${severity} priority, muted` : `${severity} priority`}
      role="img"
    />
  );
}

/** Colored dot conveying CI state at a glance. */
function CiDot({ status }: { status: ScoredItem["ci_status"] }) {
  if (status === "none") return null;
  const color =
    status === "failing"
      ? "bg-red-500"
      : status === "passing"
        ? "bg-green-500"
        : "bg-amber-500";
  return (
    <span
      className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${color}`}
      aria-label={`CI ${status}`}
      role="img"
    />
  );
}

/** One row in the list. */
function ItemRow({ item }: { item: ScoredItem }) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  return (
    <>
      <button
        type="button"
        onClick={() => void openUrl(item.url)}
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu({ x: event.clientX, y: event.clientY });
        }}
        className="flex w-full flex-col gap-0.5 border-b border-neutral-200 px-3 py-2 text-left hover:bg-neutral-100 dark:border-neutral-800 dark:hover:bg-neutral-800"
      >
        <div className="flex items-center gap-1.5 text-[11px] text-neutral-500 dark:text-neutral-400">
          <SeverityDot severity={item.severity} muted={item.muted} />
          <CiDot status={item.ci_status} />
          <span className="font-medium">{KIND_LABELS[item.kind]}</span>
          <span aria-hidden="true">·</span>
          <span className="truncate">
            {item.repo}
            {item.number !== null ? `#${item.number}` : ""}
          </span>
          <span className="ml-auto shrink-0 tabular-nums">{age(item.updated_at)}</span>
        </div>
        <div className="truncate text-[13px] text-neutral-900 dark:text-neutral-100">
          {item.title}
        </div>
      </button>

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
    </>
  );
}

/** Root component of the popover window. */
export function Popover() {
  const [state, setState] = useState<LoadState>({ phase: "loading" });

  const load = useCallback(async () => {
    try {
      // One round trip each; the daemon serves both from memory/SQLite, so
      // this stays fast enough to run on every popover open rather than
      // caching anything across the webview's (very short) lifetime.
      const [items, status] = await Promise.all([listItems(), daemonStatus()]);
      setState({ phase: "ready", items, status });
    } catch (error) {
      setState({ phase: "unreachable", message: String(error) });
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (state.phase === "loading") {
    return (
      <Shell>
        <p className="p-4 text-center text-[13px] text-neutral-500">Loading…</p>
      </Shell>
    );
  }

  if (state.phase === "unreachable") {
    return (
      <Shell>
        <div className="p-4 text-center">
          <p className="text-[13px] font-medium text-neutral-900 dark:text-neutral-100">
            The gitsurveil service isn’t running
          </p>
          <p className="mt-1 text-[11px] text-neutral-500">
            Start it with{" "}
            <code className="rounded bg-neutral-200 px-1 py-0.5 dark:bg-neutral-800">
              cargo run -p gitsurveild -- --foreground
            </code>
          </p>
          <button
            type="button"
            onClick={() => void load()}
            className="mt-3 rounded bg-neutral-900 px-3 py-1 text-[12px] text-white dark:bg-neutral-100 dark:text-neutral-900"
          >
            Retry
          </button>
        </div>
      </Shell>
    );
  }

  const { items, status } = state;

  return (
    <Shell>
      <header className="flex items-center justify-between border-b border-neutral-200 px-3 py-2 dark:border-neutral-800">
        <span className="text-[13px] font-semibold text-neutral-900 dark:text-neutral-100">
          {items.length === 0
            ? "All clear"
            : `${items.length} item${items.length === 1 ? "" : "s"}`}
        </span>
        <button
          type="button"
          onClick={() => void openMainWindow()}
          className="text-[11px] text-neutral-500 underline-offset-2 hover:underline"
        >
          {status.account_count === 0
            ? "Set up an account"
            : "Open gitsurveil"}
        </button>
      </header>

      {items.length === 0 ? (
        <p className="p-6 text-center text-[13px] text-neutral-500">
          Nothing needs your attention.
        </p>
      ) : (
        <ul className="flex-1 overflow-y-auto">
          {items.map((item) => (
            <li key={item.id}>
              <ItemRow item={item} />
            </li>
          ))}
        </ul>
      )}
    </Shell>
  );
}

/** Window chrome shared by every popover state. */
function Shell({ children }: { children: React.ReactNode }) {
  return (
    <div className="flex h-screen flex-col overflow-hidden bg-white text-neutral-900 dark:bg-neutral-900 dark:text-neutral-100">
      {children}
    </div>
  );
}

/**
 * The three-pane conflict resolver (`specs/conflict-resolver.md`).
 *
 * Left pane: the PR branch's side of the selected conflict. Right pane: the
 * base branch's side. Center: the editable merged file. Every hunk starts as
 * its raw marker block in the center; "use ours/theirs/both" replaces it, and
 * anything can be hand-edited. A file counts as resolved once its saved text
 * holds no markers; committing is only possible when every file is resolved.
 *
 * Everything acts on the daemon's temp worktree — the user's local clone and
 * the remote are untouched until "Push & finish" is clicked.
 */

import { useEffect, useRef, useState } from "react";
import {
  conflictAbort,
  conflictCommit,
  conflictFile,
  conflictPrepare,
  conflictPush,
  conflictSave,
} from "../ipc";
import type { ConflictFile, ConflictHunk, ConflictSession } from "../types";

export function ConflictResolver({
  repo,
  number,
  onClose,
  onResolved,
}: {
  repo: string;
  number: number;
  onClose: () => void;
  /** Called when a resolution is pushed to GitHub, so the dashboard refreshes. */
  onResolved: () => void;
}) {
  const [session, setSession] = useState<ConflictSession | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  /** Live conflict data for the active file. */
  const [file, setFile] = useState<ConflictFile | null>(null);
  const [activePath, setActivePath] = useState<string | null>(null);
  const [selectedHunk, setSelectedHunk] = useState<number | null>(null);
  /** Hunks already resolved via a pane button (their raw block is gone). */
  const [applied, setApplied] = useState<ReadonlySet<number>>(new Set());
  /** Per-path resolution bookkeeping for the progress footer. */
  const [status, setStatus] = useState<Record<string, { saved: boolean; resolved: boolean; dirty: boolean }>>({});
  /** "editing" until a commit lands; then the only path forward is the push. */
  const [phase, setPhase] = useState<"editing" | "committed">("editing");

  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    let cancelled = false;
    conflictPrepare(repo, number)
      .then((s) => {
        if (cancelled) return;
        setSession(s);
        const first = s.files[0];
        if (first) {
          setActivePath(first.path);
          void loadFile(s.session_id, first.path);
        }
        setError(null);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repo, number]);

  async function loadFile(sessionId: string, path: string) {
    setActivePath(path);
    setSelectedHunk(null);
    setApplied(new Set());
    try {
      const f = await conflictFile(sessionId, path);
      setFile(f);
      const conflictIndexes = conflictIndexesOf(f);
      if (conflictIndexes.length > 0) setSelectedHunk(conflictIndexes[0]);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }

  function updateStatus(path: string, patch: Partial<{ saved: boolean; resolved: boolean; dirty: boolean }>) {
    setStatus((prev) => {
      const base = prev[path] ?? { saved: false, resolved: false, dirty: false };
      return { ...prev, [path]: { ...base, ...patch } };
    });
  }

  function hasMarkers(text: string): boolean {
    return /^<<<<<<<|^=======$|^>>>>>>>/m.test(text);
  }

  /** Replaces the selected hunk's raw marker block with one side's content. */
  function applySide(side: "ours" | "theirs" | "both") {
    if (file === null || selectedHunk === null) return;
    const hunk = hunkAt(file, selectedHunk);
    if (!hunk) return;
    const raw = hunk.raw.join("");
    const replacement =
      side === "both"
        ? hunk.ours.join("") + hunk.theirs.join("")
        : (side === "ours" ? hunk.ours : hunk.theirs).join("");
    const el = textareaRef.current;
    if (!el) return;
    const index = el.value.indexOf(raw);
    if (index === -1) return; // already edited by hand; nothing to replace
    const next = el.value.slice(0, index) + replacement + el.value.slice(index + raw.length);
    el.value = next;
    updateStatus(file.path, { dirty: true });
    setApplied((prev) => new Set(prev).add(selectedHunk));
    setSelectedHunk(null);
  }

  async function saveActiveFile(pick?: "ours" | "theirs") {
    if (!session || !file) return;
    setBusy(true);
    setError(null);
    try {
      if (pick) {
        await conflictSave(session.session_id, file.path, undefined, pick);
        updateStatus(file.path, { saved: true, resolved: true, dirty: false });
      } else {
        const text = textareaRef.current?.value ?? "";
        await conflictSave(session.session_id, file.path, text);
        updateStatus(file.path, { saved: true, resolved: !hasMarkers(text), dirty: false });
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function commit() {
    if (!session) return;
    setBusy(true);
    setError(null);
    try {
      await conflictCommit(session.session_id, `Merge ${session.head} into ${session.base}`);
      setPhase("committed");
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function pushAndFinish() {
    if (!session) return;
    setBusy(true);
    setError(null);
    try {
      await conflictPush(session.session_id);
      onResolved();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function abort() {
    if (!session) return;
    if (!confirm("Abandon this resolution? Unsaved edits are lost.")) return;
    setBusy(true);
    setError(null);
    try {
      await conflictAbort(session.session_id);
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (error && !session) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
        <p className="text-sm font-medium">Couldn’t start conflict resolution</p>
        <p role="alert" className="max-w-md text-xs text-neutral-500">{error}</p>
        <p className="max-w-md text-xs text-neutral-500">
          A local clone path for <code>{repo}</code> must be configured in
          Repositories first; the resolution worktree is created from it.
        </p>
        <button
          type="button"
          onClick={onClose}
          className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
        >
          Back
        </button>
      </div>
    );
  }

  if (!session) {
    return (
      <div className="flex h-full items-center justify-center">
        <p className="text-sm text-neutral-500">Preparing resolution worktree…</p>
      </div>
    );
  }

  const files = session.files;
  const resolvedCount = files.filter((f) => status[f.path]?.resolved).length;
  const allResolved = resolvedCount === files.length && files.length > 0;
  const active = file ?? null;

  return (
    <div className="flex h-full flex-col bg-white dark:bg-neutral-900">
      <header className="flex items-center gap-2 border-b border-neutral-200 px-4 py-2 text-sm dark:border-neutral-800">
        <span className="font-semibold">
          Resolve conflicts — {repo}#{number}
        </span>
        <span className="text-xs text-neutral-500">
          {session.head} → {session.base}
        </span>
        <span className="ml-auto rounded bg-neutral-100 px-2 py-0.5 text-[11px] dark:bg-neutral-800">
          {resolvedCount} of {files.length} files resolved
        </span>
      </header>

      {error && (
        <p role="alert" className="border-b border-neutral-200 px-4 py-1 text-xs text-red-600 dark:border-neutral-800 dark:text-red-400">
          {error}
        </p>
      )}

      <div className="flex min-h-0 flex-1">
        <FileList
          files={files}
          status={status}
          activePath={activePath}
          onSelect={(path) => {
            if (activePath !== path && session) void loadFile(session.session_id, path);
          }}
        />

        {active ? (
          <div className="flex min-w-0 flex-1 flex-col">
            <HunkBar
              hunks={conflictIndexesOf(active)}
              file={active}
              selected={selectedHunk}
              applied={applied}
              onSelect={setSelectedHunk}
            />

            {active.binary || active.large ? (
              <WholeFilePick
                large={active.large}
                busy={busy}
                onPick={async (pick) => {
                  await saveActiveFile(pick);
                }}
              />
            ) : (
              <ThreePane
                key={active.path}
                file={active}
                selectedHunk={selectedHunk}
                textareaRef={textareaRef}
                onApply={applySide}
                onEdit={() => {
                  if (activePath) updateStatus(activePath, { dirty: true });
                }}
                applied={applied}
              />
            )}

            <footer className="flex items-center gap-2 border-t border-neutral-200 px-3 py-2 dark:border-neutral-800">
              <button
                type="button"
                disabled={busy || phase === "committed"}
                onClick={() => void saveActiveFile()}
                className="rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
              >
                {active.binary || active.large ? "Save" : "Save file"}
              </button>
              {phase === "committed" ? (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void pushAndFinish()}
                  className="ml-auto rounded bg-green-700 px-2 py-1 text-xs text-white disabled:opacity-50"
                >
                  Push &amp; finish
                </button>
              ) : (
                <button
                  type="button"
                  disabled={busy || !allResolved}
                  onClick={() => void commit()}
                  className="ml-auto rounded bg-neutral-900 px-2 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
                >
                  Commit resolution
                </button>
              )}
              <button
                type="button"
                disabled={busy}
                onClick={() => void abort()}
                className="rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
              >
                Abort
              </button>
            </footer>
          </div>
        ) : (
          <div className="flex flex-1 items-center justify-center">
            <p className="text-sm text-neutral-500">No conflicted files.</p>
          </div>
        )}
      </div>
    </div>
  );
}

/** The indexes (into `segments`) of every conflict hunk in a file. */
function conflictIndexesOf(file: ConflictFile): number[] {
  return file.segments
    .map((s, i) => (s.kind === "conflict" ? i : -1))
    .filter((i) => i >= 0);
}

/** The conflict hunk at segment index `i`, if it is one. */
function hunkAt(file: ConflictFile, i: number): ConflictHunk | null {
  const s = file.segments[i];
  return s && s.kind === "conflict" ? s.hunk : null;
}

function FileList({
  files,
  status,
  activePath,
  onSelect,
}: {
  files: ConflictSession["files"];
  status: Record<string, { saved: boolean; resolved: boolean; dirty: boolean }>;
  activePath: string | null;
  onSelect: (path: string) => void;
}) {
  return (
    <aside
      aria-label="Conflicted files"
      className="w-60 shrink-0 border-r border-neutral-200 overflow-y-auto dark:border-neutral-800"
    >
      <ul>
        {files.map((f) => {
          const st = status[f.path];
          const resolved = st?.resolved && !st.dirty;
          return (
            <li key={f.path}>
              <button
                type="button"
                onClick={() => onSelect(f.path)}
                aria-current={activePath === f.path ? "true" : undefined}
                className={`block w-full px-3 py-2 text-left text-xs hover:bg-neutral-100 dark:hover:bg-neutral-800 ${
                  activePath === f.path ? "bg-neutral-100 dark:bg-neutral-800" : ""
                }`}
              >
                <span className="block truncate font-medium">{f.path}</span>
                <span className="mt-0.5 flex items-center gap-1.5 text-[11px] text-neutral-500">
                  <span className="rounded bg-neutral-200 px-1 dark:bg-neutral-700">
                    {f.conflicts} conflict{f.conflicts === 1 ? "" : "s"}
                  </span>
                  {resolved ? (
                    <span className="text-green-700 dark:text-green-400">resolved</span>
                  ) : (
                    <span className="text-amber-700 dark:text-amber-400">unresolved</span>
                  )}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </aside>
  );
}

function HunkBar({
  hunks,
  file,
  selected,
  applied,
  onSelect,
}: {
  hunks: number[];
  file: ConflictFile;
  selected: number | null;
  applied: ReadonlySet<number>;
  onSelect: (index: number) => void;
}) {
  if (hunks.length === 0) {
    return (
      <div className="border-b border-neutral-200 px-3 py-1.5 text-[11px] text-green-700 dark:border-neutral-800 dark:text-green-400">
        No conflict markers left in this file — save it to mark it resolved.
      </div>
    );
  }
  return (
    <div
      role="group"
      aria-label="Conflicts in this file"
      className="flex flex-wrap items-center gap-1.5 border-b border-neutral-200 px-3 py-1.5 dark:border-neutral-800"
    >
      <span className="text-[11px] text-neutral-500">
        {hunks.length} conflict{hunks.length === 1 ? "" : "s"}
      </span>
      {hunks.map((i) => {
        const hunk = hunkAt(file, i);
        if (!hunk) return null;
        const isSelected = selected === i;
        const isApplied = applied.has(i);
        return (
          <button
            key={i}
            type="button"
            aria-pressed={isSelected}
            onClick={() => onSelect(i)}
            className={`rounded px-1.5 py-0.5 text-[11px] ${
              isApplied
                ? "bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-400"
                : isSelected
                  ? "bg-neutral-900 text-white dark:bg-neutral-100 dark:text-neutral-900"
                  : "border border-neutral-300 dark:border-neutral-700"
            }`}
          >
            line {hunk.start_line}
            {isApplied ? " ✓" : ""}
          </button>
        );
      })}
    </div>
  );
}

function ThreePane({
  file,
  selectedHunk,
  textareaRef,
  applied,
  onApply,
  onEdit,
}: {
  file: ConflictFile;
  selectedHunk: number | null;
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  applied: ReadonlySet<number>;
  onApply: (side: "ours" | "theirs" | "both") => void;
  /** Called on every keystroke in the resolution pane, to track unsaved edits. */
  onEdit: () => void;
}) {
  const hunk = selectedHunk !== null ? hunkAt(file, selectedHunk) : null;
  const isApplied = selectedHunk !== null && applied.has(selectedHunk);

  return (
    <>
      <div className="flex min-h-0 flex-1 flex-col">
        {hunk && (
          <div className="flex shrink-0 gap-1.5 border-b border-neutral-200 px-3 py-1.5 dark:border-neutral-800">
            <button
              type="button"
              disabled={isApplied}
              onClick={() => onApply("ours")}
              className="rounded bg-neutral-900 px-2 py-0.5 text-[11px] text-white disabled:opacity-40 dark:bg-neutral-100 dark:text-neutral-900"
            >
              Use ours
            </button>
            <button
              type="button"
              disabled={isApplied}
              onClick={() => onApply("theirs")}
              className="rounded bg-neutral-900 px-2 py-0.5 text-[11px] text-white disabled:opacity-40 dark:bg-neutral-100 dark:text-neutral-900"
            >
              Use theirs
            </button>
            <button
              type="button"
              disabled={isApplied}
              onClick={() => onApply("both")}
              className="rounded border border-neutral-300 px-2 py-0.5 text-[11px] disabled:opacity-40 dark:border-neutral-700"
            >
              Use both
            </button>
            {isApplied && (
              <span className="self-center text-[11px] text-green-700 dark:text-green-400">
                applied — hand-edit the center to adjust
              </span>
            )}
          </div>
        )}

        <div className="grid min-h-0 flex-1 grid-cols-3">
          <SidePane label={hunk?.ours_label ?? "ours (PR branch)"} lines={hunk?.ours ?? []} />
          <SidePane label={hunk?.theirs_label ?? "theirs (base)"} lines={hunk?.theirs ?? []} />
          <section className="flex min-h-0 flex-col border-l border-neutral-200 dark:border-neutral-800">
            <header className="border-b border-neutral-200 px-3 py-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-500 dark:border-neutral-800">
              Resolution
            </header>
            <textarea
              ref={textareaRef}
              defaultValue={serialized(file)}
              onInput={onEdit}
              spellCheck={false}
              aria-label="Resolution"
              className="min-h-0 flex-1 resize-none overflow-auto bg-transparent p-3 font-mono text-xs leading-relaxed outline-none"
            />
          </section>
        </div>
      </div>
    </>
  );
}

function SidePane({ label, lines }: { label: string; lines: string[] }) {
  return (
    <section className="flex min-h-0 flex-col">
      <header className="border-b border-neutral-200 px-3 py-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-500 dark:border-neutral-800">
        {label}
      </header>
      <pre className="min-h-0 flex-1 overflow-auto whitespace-pre-wrap p-3 font-mono text-xs leading-relaxed text-neutral-700 dark:text-neutral-300">
        {lines.join("") || <span className="text-neutral-500">(empty)</span>}
      </pre>
    </section>
  );
}

function WholeFilePick({
  large,
  busy,
  onPick,
}: {
  large: boolean;
  busy: boolean;
  onPick: (side: "ours" | "theirs") => void;
}) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
      <p className="text-sm font-medium">
        {large ? "This file is too large to edit" : "This is a binary file"}
      </p>
      <p className="max-w-sm text-xs text-neutral-500">
        Pick which side should be kept in full. Nothing is written until you
        choose and save.
      </p>
      <div className="flex gap-2">
        <button
          type="button"
          disabled={busy}
          onClick={() => void onPick("ours")}
          className="rounded bg-neutral-900 px-3 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
        >
          Keep ours
        </button>
        <button
          type="button"
          disabled={busy}
          onClick={() => void onPick("theirs")}
          className="rounded border border-neutral-300 px-3 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
        >
          Keep theirs
        </button>
      </div>
    </div>
  );
}

/** The file as one string: context verbatim, conflicts as their raw blocks. */
function serialized(f: ConflictFile): string {
  return f.segments
    .map((s) => (s.kind === "context" ? s.lines.join("") : s.hunk.raw.join("")))
    .join("");
}

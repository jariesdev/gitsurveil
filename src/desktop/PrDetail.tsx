/**
 * The pull-request detail pane (`specs/pr-management.md`).
 *
 * Opens from a dashboard row. Every mutation here is an explicit click, and
 * the destructive ones (close, merge) confirm first — this is the only part
 * of the app that writes to GitHub.
 */

import { useCallback, useEffect, useState } from "react";
import {
  openUrl,
  prClose,
  prComment,
  prComments,
  prDetail,
  prMerge,
  prUpdate,
} from "../ipc";
import type {
  Comment,
  MergeMethod,
  Mergeability,
  PullRequestDetail,
} from "../types";

/** How each mergeability state is described and styled. */
const MERGEABILITY: Record<Mergeability, { label: string; className: string }> = {
  clean: {
    label: "Ready to merge",
    className: "text-green-700 dark:text-green-400",
  },
  conflicted: {
    label: "Conflicts with base branch",
    className: "text-red-700 dark:text-red-400",
  },
  blocked: {
    label: "Blocked by reviews or checks",
    className: "text-amber-700 dark:text-amber-400",
  },
  unknown: {
    label: "Checking mergeability…",
    className: "text-neutral-500",
  },
};

export function PrDetail({
  repo,
  number,
  onClose,
  onChanged,
}: {
  repo: string;
  number: number;
  onClose: () => void;
  /** Called after a mutation, so the dashboard can refresh behind the pane. */
  onChanged: () => void;
}) {
  const [pr, setPr] = useState<PullRequestDetail | null>(null);
  const [comments, setComments] = useState<Comment[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [newComment, setNewComment] = useState("");
  const [mergeMethod, setMergeMethod] = useState<MergeMethod>("merge");

  const load = useCallback(async () => {
    try {
      const [detail, thread] = await Promise.all([
        prDetail(repo, number),
        prComments(repo, number),
      ]);
      setPr(detail);
      setComments(thread);
      setDraftTitle(detail.title);
      setDraftBody(detail.body);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [repo, number]);

  useEffect(() => {
    void load();
  }, [load]);

  /** Runs a mutation, then reloads so the pane can't show stale state. */
  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await load();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  if (error && !pr) {
    return (
      <Panel onClose={onClose}>
        <p role="alert" className="p-6 text-sm text-red-600 dark:text-red-400">
          {error}
        </p>
      </Panel>
    );
  }

  if (!pr) {
    return (
      <Panel onClose={onClose}>
        <p className="p-6 text-sm text-neutral-500">Loading…</p>
      </Panel>
    );
  }

  const merge = MERGEABILITY[pr.mergeability];
  const isOpen = pr.state === "open";

  return (
    <Panel onClose={onClose}>
      <div className="overflow-y-auto">
        <div className="border-b border-neutral-200 p-4 dark:border-neutral-800">
          {editing ? (
            <input
              value={draftTitle}
              onChange={(e) => setDraftTitle(e.target.value)}
              aria-label="Title"
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
            />
          ) : (
            <h2 className="text-sm font-semibold">{pr.title}</h2>
          )}

          <div className="mt-1 flex flex-wrap items-center gap-2 text-[11px] text-neutral-500">
            <span>
              {pr.repo}#{pr.number}
            </span>
            <span aria-hidden="true">·</span>
            <span className="capitalize">{pr.state}</span>
            {pr.draft && <Badge>draft</Badge>}
            <span aria-hidden="true">·</span>
            <span>
              {pr.head} → {pr.base}
            </span>
            <button
              type="button"
              onClick={() => void openUrl(pr.url)}
              className="ml-auto underline-offset-2 hover:underline"
            >
              Open on GitHub
            </button>
          </div>

          {pr.labels.length > 0 && (
            <div className="mt-2 flex flex-wrap gap-1">
              {pr.labels.map((label) => (
                <Badge key={label}>{label}</Badge>
              ))}
            </div>
          )}
        </div>

        <Section title="Description">
          {editing ? (
            <textarea
              value={draftBody}
              onChange={(e) => setDraftBody(e.target.value)}
              rows={8}
              aria-label="Description"
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 font-mono text-xs dark:border-neutral-700 dark:bg-neutral-900"
            />
          ) : (
            <p className="whitespace-pre-wrap text-xs text-neutral-700 dark:text-neutral-300">
              {pr.body || <span className="text-neutral-500">No description.</span>}
            </p>
          )}
        </Section>

        {pr.reviewers.length > 0 && (
          <Section title="Reviewers">
            <ul className="space-y-1 text-xs">
              {pr.reviewers.map((reviewer) => (
                <li key={reviewer.login} className="flex justify-between">
                  <span>{reviewer.login}</span>
                  <span className="text-neutral-500">
                    {reviewer.state.replace(/_/g, " ")}
                  </span>
                </li>
              ))}
            </ul>
          </Section>
        )}

        {pr.checks.length > 0 && (
          <Section title="Checks">
            <ul className="space-y-1 text-xs">
              {pr.checks.map((check) => (
                <li key={check.name} className="flex justify-between gap-2">
                  <button
                    type="button"
                    disabled={!check.url}
                    onClick={() => check.url && void openUrl(check.url)}
                    className="truncate text-left underline-offset-2 enabled:hover:underline"
                  >
                    {check.name}
                  </button>
                  <span
                    className={
                      check.conclusion === "success"
                        ? "text-green-700 dark:text-green-400"
                        : check.conclusion === "failure"
                          ? "text-red-700 dark:text-red-400"
                          : "text-neutral-500"
                    }
                  >
                    {check.conclusion}
                  </span>
                </li>
              ))}
            </ul>
          </Section>
        )}

        <Section title={`Conversation (${comments.length})`}>
          {comments.length === 0 ? (
            <p className="text-xs text-neutral-500">No comments yet.</p>
          ) : (
            <ul className="space-y-3">
              {comments.map((comment) => (
                <li key={comment.id} className="text-xs">
                  <div className="text-neutral-500">
                    <span className="font-medium text-neutral-700 dark:text-neutral-300">
                      {comment.author}
                    </span>
                    {comment.path && <span> on {comment.path}</span>}
                  </div>
                  <p className="mt-0.5 whitespace-pre-wrap text-neutral-700 dark:text-neutral-300">
                    {comment.body}
                  </p>
                </li>
              ))}
            </ul>
          )}

          <div className="mt-3">
            <textarea
              value={newComment}
              onChange={(e) => setNewComment(e.target.value)}
              rows={3}
              placeholder="Leave a comment"
              aria-label="New comment"
              className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
            />
            <button
              type="button"
              disabled={busy || !newComment.trim()}
              onClick={() =>
                void run(async () => {
                  await prComment(repo, number, newComment.trim());
                  setNewComment("");
                })
              }
              className="mt-1 rounded bg-neutral-900 px-2 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
            >
              Comment
            </button>
          </div>
        </Section>
      </div>

      <footer className="border-t border-neutral-200 p-3 dark:border-neutral-800">
        {error && (
          <p role="alert" className="mb-2 text-xs text-red-600 dark:text-red-400">
            {error}
          </p>
        )}

        <p className={`mb-2 text-xs ${merge.className}`}>{merge.label}</p>

        <div className="flex flex-wrap items-center gap-2">
          {editing ? (
            <>
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await prUpdate(repo, number, {
                      title: draftTitle,
                      body: draftBody,
                    });
                    setEditing(false);
                  })
                }
                className="rounded bg-neutral-900 px-2 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
              >
                Save
              </button>
              <button
                type="button"
                onClick={() => {
                  setDraftTitle(pr.title);
                  setDraftBody(pr.body);
                  setEditing(false);
                }}
                className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
              >
                Cancel
              </button>
            </>
          ) : (
            isOpen && (
              <button
                type="button"
                onClick={() => setEditing(true)}
                className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
              >
                Edit
              </button>
            )
          )}

          {isOpen && !editing && (
            <>
              <select
                aria-label="Merge method"
                value={mergeMethod}
                onChange={(e) => setMergeMethod(e.target.value as MergeMethod)}
                className="rounded border border-neutral-300 bg-white px-1 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
              >
                <option value="merge">Merge commit</option>
                <option value="squash">Squash</option>
                <option value="rebase">Rebase</option>
              </select>
              <button
                type="button"
                disabled={busy || pr.mergeability === "conflicted"}
                onClick={() => {
                  // Merging is irreversible from here; make the user say so.
                  if (!confirm(`Merge ${pr.repo}#${pr.number}?`)) return;
                  void run(() =>
                    prMerge(repo, number, mergeMethod, pr.head_sha),
                  );
                }}
                className="rounded bg-green-700 px-2 py-1 text-xs text-white disabled:opacity-50"
              >
                Merge
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => {
                  if (!confirm(`Close ${pr.repo}#${pr.number} without merging?`))
                    return;
                  void run(() => prClose(repo, number));
                }}
                className="rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
              >
                Close
              </button>
            </>
          )}
        </div>
      </footer>
    </Panel>
  );
}

function Panel({
  children,
  onClose,
}: {
  children: React.ReactNode;
  onClose: () => void;
}) {
  return (
    <aside
      aria-label="Pull request detail"
      className="flex h-full w-[28rem] shrink-0 flex-col border-l border-neutral-200 bg-white dark:border-neutral-800 dark:bg-neutral-900"
    >
      <div className="flex justify-end border-b border-neutral-200 px-2 py-1 dark:border-neutral-800">
        <button
          type="button"
          onClick={onClose}
          aria-label="Close detail"
          className="rounded px-2 py-0.5 text-xs text-neutral-500 hover:bg-neutral-100 dark:hover:bg-neutral-800"
        >
          Close
        </button>
      </div>
      {children}
    </aside>
  );
}

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="border-b border-neutral-200 p-4 dark:border-neutral-800">
      <h3 className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-neutral-500">
        {title}
      </h3>
      {children}
    </section>
  );
}

function Badge({ children }: { children: React.ReactNode }) {
  return (
    <span className="rounded bg-neutral-200 px-1.5 py-0.5 text-[10px] dark:bg-neutral-800">
      {children}
    </span>
  );
}

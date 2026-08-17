/**
 * The pull-request detail pane (`specs/pr-management.md`).
 *
 * Opens from a dashboard row. Every mutation here is an explicit click, and
 * the destructive ones (close, merge) confirm first — this is the only part
 * of the app that writes to GitHub.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  browsersList,
  openUrl,
  openUrlWithBrowser,
  prBranches,
  prClose,
  prComment,
  prCommentReply,
  prComments,
  prDetail,
  prLabels,
  prMerge,
  prResolve,
  prUpdate,
} from "../ipc";
import { renderMarkdown } from "../markdown";
import { copyText } from "./clipboard";
import { ContextMenu, type ContextMenuItem } from "./ContextMenu";
import type {
  Conversation,
  MergeMethod,
  Mergeability,
  PullRequestDetail,
  ReviewThread,
} from "../types";

/** True when two label lists carry the same names, in any order. */
function sameLabels(a: string[], b: string[]): boolean {
  const bSet = new Set(b);
  return a.length === b.length && a.every((label) => bSet.has(label));
}

/** The chips offered in the label picker: repo labels, the PR's current
 * labels (which may not exist on the repo anymore), and anything the user
 * added as a draft. */
function labelOptions(
  draft: string[],
  current: string[],
  repo: string[],
): string[] {
  return [...new Set([...repo, ...current, ...draft])].sort();
}

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
  onResolve,
}: {
  repo: string;
  number: number;
  onClose: () => void;
  /** Called after a mutation, so the dashboard can refresh behind the pane. */
  onChanged: () => void;
  /** Opens the three-pane conflict resolver for a conflicted PR. */
  onResolve: () => void;
}) {
  const [pr, setPr] = useState<PullRequestDetail | null>(null);
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");
  const [draftBody, setDraftBody] = useState("");
  const [draftBase, setDraftBase] = useState("");
  const [draftLabels, setDraftLabels] = useState<string[]>([]);
  const [draftDraft, setDraftDraft] = useState(false);
  const [branches, setBranches] = useState<string[]>([]);
  const [repoLabels, setRepoLabels] = useState<string[]>([]);
  const [newLabel, setNewLabel] = useState("");
  const [newComment, setNewComment] = useState("");
  const [mergeMethod, setMergeMethod] = useState<MergeMethod>("merge");
  const [replyingTo, setReplyingTo] = useState<string | null>(null);
  const [replyText, setReplyText] = useState("");
  const replyRef = useRef<HTMLTextAreaElement>(null);

  const load = useCallback(async () => {
    try {
      const [detail, thread] = await Promise.all([
        prDetail(repo, number),
        prComments(repo, number),
      ]);
      setPr(detail);
      setConversation(thread);
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

  // Open a reply box ready to type: focus it the moment it appears.
  useEffect(() => {
    if (replyingTo) replyRef.current?.focus();
  }, [replyingTo]);

  /** Opens the edit form seeded with the current PR, loading branch names
   * for the target-branch suggestions and repo labels for the tag picker
   * (both best-effort; the fields still work without them). */
  function openEdit() {
    if (!pr) return;
    setDraftTitle(pr.title);
    setDraftBody(pr.body);
    setDraftBase(pr.base);
    setDraftLabels(pr.labels);
    setDraftDraft(pr.draft);
    setEditing(true);
    void prBranches(repo)
      .then(setBranches)
      .catch(() => setBranches([]));
    void prLabels(repo)
      .then(setRepoLabels)
      .catch(() => setRepoLabels([]));
  }

  /** Closes the edit form, discarding drafts. */
  function cancelEdit() {
    if (!pr) return;
    setDraftTitle(pr.title);
    setDraftBody(pr.body);
    setDraftBase(pr.base);
    setDraftLabels(pr.labels);
    setDraftDraft(pr.draft);
    setNewLabel("");
    setEditing(false);
  }

  /** Selects or deselects a label chip. */
  function toggleLabel(label: string) {
    setDraftLabels((prev) =>
      prev.includes(label) ? prev.filter((l) => l !== label) : [...prev, label],
    );
  }

  /** Adds a label by name — GitHub creates it on assignment if needed. */
  function addLabel() {
    const label = newLabel.trim();
    if (label && !draftLabels.includes(label)) {
      setDraftLabels((prev) => [...prev, label]);
    }
    setNewLabel("");
  }

  /**
   * Saves only the fields the user actually changed, then reloads. Labels
   * replace the whole set, so a reordered list is a no-op.
   */
  function saveEdit() {
    if (!pr) return;
    const patch: Partial<{
      title: string;
      body: string;
      base: string;
      draft: boolean;
      labels: string[];
    }> = {};
    if (draftTitle !== pr.title) patch.title = draftTitle;
    if (draftBody !== pr.body) patch.body = draftBody;
    if (draftBase !== pr.base) patch.base = draftBase;
    if (draftDraft !== pr.draft) patch.draft = draftDraft;
    if (!sameLabels(draftLabels, pr.labels)) patch.labels = draftLabels;
    if (Object.keys(patch).length === 0) {
      setEditing(false);
      return;
    }
    void run(async () => {
      await prUpdate(repo, number, patch);
      setEditing(false);
    });
  }

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

  /** Replies inside a thread. GitHub threads a reply by parent comment id. */
  async function submitReply(thread: ReviewThread) {
    const last = thread.comments[thread.comments.length - 1];
    const body = replyText.trim();
    if (!last || !body) return;
    await run(async () => {
      await prCommentReply(repo, number, last.id, body);
      setReplyingTo(null);
      setReplyText("");
    });
  }

  /** Closes the reply box, discarding the draft. */
  function cancelReply() {
    setReplyingTo(null);
    setReplyText("");
  }

  /**
   * Reply-box keyboard handling: Shift+Enter posts (same as the button),
   * Esc cancels, and a bare Enter just adds a new line.
   */
  function replyKeyDown(
    e: React.KeyboardEvent<HTMLTextAreaElement>,
    thread: ReviewThread,
  ) {
    if (e.key === "Escape") {
      cancelReply();
    } else if (e.key === "Enter" && e.shiftKey) {
      e.preventDefault();
      void submitReply(thread);
    }
  }

  /** Flips a thread's resolve state on GitHub. */
  async function toggleResolved(thread: ReviewThread) {
    await run(async () => {
      await prResolve(repo, thread.id, !thread.resolved);
    });
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
        <div className="flex flex-1 items-center justify-center">
          <p className="p-6 text-sm text-neutral-500">Loading…</p>
        </div>
      </Panel>
    );
  }

  const merge = MERGEABILITY[pr.mergeability];
  const isOpen = pr.state === "open";
  // `Promise.all` loads detail and conversation together, so a rendered pane
  // always has both; this fallback only satisfies the null-check.
  const convo = conversation ?? { issue_comments: [], review_threads: [] };
  const messageCount =
    convo.issue_comments.length +
    convo.review_threads.reduce((n, t) => n + t.comments.length, 0);

  return (
    <Panel onClose={onClose}>
      <div className="overflow-y-auto">
        <div className="border-b border-neutral-200 p-4 dark:border-neutral-800">
          {editing ? (
            <div className="space-y-2">
              <input
                value={draftTitle}
                onChange={(e) => setDraftTitle(e.target.value)}
                aria-label="Title"
                className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-700 dark:bg-neutral-900"
              />
              <div className="flex flex-wrap gap-2">
                <input
                  value={draftBase}
                  onChange={(e) => setDraftBase(e.target.value)}
                  list="pr-base-branches"
                  aria-label="Base branch"
                  className="w-40 rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
                />
                <datalist id="pr-base-branches">
                  {branches.map((branch) => (
                    <option key={branch} value={branch} />
                  ))}
                </datalist>
              </div>
              <div>
                <div className="mb-1 text-[11px] text-neutral-500">Labels</div>
                <div className="flex flex-wrap gap-1">
                  {labelOptions(draftLabels, pr.labels, repoLabels).map((label) => (
                    <button
                      key={label}
                      type="button"
                      aria-pressed={draftLabels.includes(label)}
                      onClick={() => toggleLabel(label)}
                      className={`rounded px-1.5 py-0.5 text-[11px] ${
                        draftLabels.includes(label)
                          ? "bg-neutral-900 text-white dark:bg-neutral-100 dark:text-neutral-900"
                          : "border border-neutral-300 text-neutral-600 dark:border-neutral-700 dark:text-neutral-300"
                      }`}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                <div className="mt-1 flex gap-1">
                  <input
                    value={newLabel}
                    onChange={(e) => setNewLabel(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        addLabel();
                      }
                    }}
                    placeholder="Add a label"
                    aria-label="New label"
                    className="min-w-0 flex-1 rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
                  />
                  <button
                    type="button"
                    disabled={!newLabel.trim()}
                    onClick={addLabel}
                    className="rounded border border-neutral-300 px-2 py-1 text-xs disabled:opacity-50 dark:border-neutral-700"
                  >
                    Add
                  </button>
                </div>
              </div>
              <label className="flex items-center gap-2 text-[11px] text-neutral-500">
                <input
                  type="checkbox"
                  checked={draftDraft}
                  onChange={(e) => setDraftDraft(e.target.checked)}
                  aria-label="Draft"
                />
                Draft (not ready for review)
              </label>
            </div>
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
          ) : pr.body ? (
            <Markdown source={pr.body} />
          ) : (
            <p className="text-xs text-neutral-500">No description.</p>
          )}
        </Section>

        {pr.reviewers.length > 0 && (
          <Section title="Reviewers">
            <ul className="space-y-1 text-xs">
              {pr.reviewers.map((reviewer) => (
                <li key={reviewer.login} className="flex justify-between">
                  <span>
                    {reviewer.login}
                    <span
                      className="ml-1 text-neutral-500"
                      aria-label={`${reviewer.rounds} review round${reviewer.rounds === 1 ? "" : "s"}`}
                    >
                      · {reviewer.rounds} {reviewer.rounds === 1 ? "round" : "rounds"}
                    </span>
                  </span>
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

        <Section
          title={`Conversation (${messageCount})`}
        >
          {messageCount === 0 ? (
            <p className="text-xs text-neutral-500">No comments yet.</p>
          ) : (
            <ul className="space-y-4">
              {convo.issue_comments.map((comment) => (
                <li key={`issue-${comment.id}`} className="text-xs">
                  <div className="text-neutral-500">
                    <span className="font-medium text-neutral-700 dark:text-neutral-300">
                      {comment.author}
                    </span>
                  </div>
                  <Markdown source={comment.body} />
                </li>
              ))}

              {convo.review_threads.map((thread) => (
                <li
                  key={thread.id}
                  className="rounded border border-neutral-200 p-2 dark:border-neutral-800"
                >
                  <div className="flex items-center gap-2 text-[11px]">
                    <span className="truncate font-medium text-neutral-700 dark:text-neutral-300">
                      {thread.path ?? "General"}
                    </span>
                    {thread.resolved && <Badge>Resolved</Badge>}
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void toggleResolved(thread)}
                      className="ml-auto shrink-0 rounded border border-neutral-300 px-1.5 py-0.5 text-[11px] disabled:opacity-50 dark:border-neutral-700"
                    >
                      {thread.resolved ? "Unresolve" : "Resolve"}
                    </button>
                  </div>

                  <ul className="mt-1 space-y-2">
                    {thread.comments.map((comment, index) => (
                      <li key={`${thread.id}-${comment.id || index}`} className="text-xs">
                        <span className="text-neutral-500">
                          <span className="font-medium text-neutral-700 dark:text-neutral-300">
                            {comment.author}
                          </span>
                        </span>
                        <Markdown source={comment.body} />
                      </li>
                    ))}
                  </ul>

                  {replyingTo === thread.id ? (
                    <div className="mt-2">
                      <textarea
                        ref={replyRef}
                        value={replyText}
                        onChange={(e) => setReplyText(e.target.value)}
                        onKeyDown={(e) => replyKeyDown(e, thread)}
                        rows={3}
                        aria-label="Reply"
                        placeholder="Reply in thread"
                        className="w-full rounded border border-neutral-300 bg-white px-2 py-1 text-xs dark:border-neutral-700 dark:bg-neutral-900"
                      />
                      <div className="mt-1 flex gap-2">
                        <button
                          type="button"
                          disabled={busy || !replyText.trim()}
                          onClick={() => void submitReply(thread)}
                          className="rounded bg-neutral-900 px-2 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
                        >
                          Post reply
                        </button>
                        <button
                          type="button"
                          onClick={cancelReply}
                          className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  ) : (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => {
                        setReplyingTo(thread.id);
                        setReplyText("");
                      }}
                      className="mt-2 text-[11px] text-neutral-500 underline-offset-2 hover:underline disabled:opacity-50"
                    >
                      Reply in thread
                    </button>
                  )}
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
                onClick={saveEdit}
                className="rounded bg-neutral-900 px-2 py-1 text-xs text-white disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-900"
              >
                Save
              </button>
              <button
                type="button"
                onClick={cancelEdit}
                className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
              >
                Cancel
              </button>
            </>
          ) : (
            isOpen && (
              <button
                type="button"
                onClick={openEdit}
                className="rounded border border-neutral-300 px-2 py-1 text-xs dark:border-neutral-700"
              >
                Edit
              </button>
            )
          )}

          {isOpen && !editing && pr.mergeability === "conflicted" && (
            <button
              type="button"
              disabled={busy}
              onClick={onResolve}
              className="rounded border border-red-300 px-2 py-1 text-xs text-red-700 disabled:opacity-50 dark:border-red-800 dark:text-red-400"
            >
              Resolve conflicts
            </button>
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
                Close PR
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
    <span className="rounded bg-neutral-200 px-1.5 py-0.5 text-[11px] dark:bg-neutral-800">
      {children}
    </span>
  );
}

/** Renders sanitized markdown (see src/markdown.ts). Clicks on links open
 * the system browser via `openUrl` instead of navigating the webview; a
 * right-click offers Copy link / Open in Browser submenu. */
function Markdown({ source }: { source: string }) {
  const [menu, setMenu] = useState<{ x: number; y: number; href: string } | null>(null);
  const browsersRef = useRef<string[] | null>(null);
  const [browsersLoaded, setBrowsersLoaded] = useState(false);

  const hrefOf = (event: { target: EventTarget | null }): string | null => {
    const anchor = (event.target as HTMLElement).closest("a");
    return anchor?.getAttribute("href") ?? null;
  };

  const handleContextMenu = (e: React.MouseEvent) => {
    const href = hrefOf(e);
    if (!href) return;
    e.preventDefault();
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
    setMenu({ x: e.clientX, y: e.clientY, href });
  };

  const isHttp = menu && /^https?:\/\//.test(menu.href);
  const contextItems: ContextMenuItem[] = [
    {
      label: "Copy link",
      onSelect: () => {
        if (!menu) return;
        void copyText(menu.href);
        setMenu(null);
      },
    },
    ...(isHttp
      ? [
          {
            label: "Open in Browser",
            children: [
              {
                label: "Default Browser",
                onSelect: () => {
                  if (!menu) return;
                  void openUrl(menu.href);
                  setMenu(null);
                },
              },
              ...(browsersLoaded && browsersRef.current && browsersRef.current.length > 0
                ? browsersRef.current.map((name) => ({
                    label: name,
                    onSelect: () => {
                      if (!menu) return;
                      void openUrlWithBrowser(menu.href, name);
                      setMenu(null);
                    },
                  }))
                : []),
            ],
          },
        ]
      : []),
  ];

  return (
    <>
      <div
        className="markdown text-neutral-700 dark:text-neutral-300"
        onClick={(e) => {
          const href = hrefOf(e);
          if (!href) return;
          e.preventDefault();
          if (/^https?:\/\//.test(href)) void openUrl(href);
        }}
        onContextMenu={handleContextMenu}
        dangerouslySetInnerHTML={{ __html: renderMarkdown(source) }}
      />
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={contextItems}
        />
      )}
    </>
  );
}

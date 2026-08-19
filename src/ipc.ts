/**
 * Typed wrappers around the Tauri commands exposed by the Rust shell
 * (`crates/gitsurveil-app/src/main.rs`).
 *
 * The webview never talks to GitHub or to the daemon socket directly — it
 * calls these, and the Rust side does the work. Keeping that boundary strict
 * is what lets the webview be destroyed at any moment without losing anything.
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  AccountRef,
  CloneStatus,
  Comment,
  ConflictFile,
  ConflictSession,
  Conversation,
  ItemKind,
  KindPref,
  MergeMethod,
  PrState,
  PullRequestDetail,
  PullRequestSummary,
  RegisteredApp,
  RepoCatalog,
  Repository,
  Rule,
  ScoredItem,
  StatusResult,
  WorktreeInfo,
  WorktreesResult,
} from "./types";

/**
 * Fetches every currently open action item, already scored and sorted
 * most-urgent-first by the daemon's priority engine.
 */
export function listItems(): Promise<ScoredItem[]> {
  return invoke<ScoredItem[]>("list_items");
}

/** Fetches resolved and dismissed items for the history view. */
export function listHistory(limit?: number): Promise<ScoredItem[]> {
  return invoke<ScoredItem[]>("list_history", { limit });
}

/**
 * Archives every resolved and dismissed item: they leave the Dashboard and
 * history permanently and never come back, even if still open on GitHub.
 * Open items are untouched and there is no undo — callers must confirm with
 * the user first.
 */
export function clearHistory(): Promise<void> {
  return invoke<void>("clear_history");
}

/** Fetches the daemon's status summary. */
export function daemonStatus(): Promise<StatusResult> {
  return invoke<StatusResult>("daemon_status");
}

/** Opens `url` in the default browser and dismisses the popover. */
export function openUrl(url: string): Promise<void> {
  return invoke<void>("open_url", { url });
}

/**
 * Lists installed system browsers by checking for known `.app` bundles in
 * standard Application directories. Returns display names like
 * `"Google Chrome"`, `"Safari"`, etc.
 */
export function browsersList(): Promise<string[]> {
  return invoke<string[]>("browsers_list");
}

/** Opens `url` in the named browser (e.g. `"Google Chrome"`). */
export function openUrlWithBrowser(url: string, browser: string): Promise<void> {
  return invoke<void>("open_url_with_browser", { url, browser });
}

/**
 * Dismisses the popover. The window is hidden (not destroyed) so the next
 * tray click reuses the warm webview; the Rust shell reclaims it after an
 * idle timeout.
 */
export function closePopover(): Promise<void> {
  return invoke<void>("close_popover");
}

/** Opens the full desktop window. */
export function openMainWindow(): Promise<void> {
  return invoke<void>("open_main_window");
}

/** Hides an item locally; GitHub activity on it brings it back. */
export function dismissItem(id: string): Promise<void> {
  return invoke<void>("dismiss_item", { id });
}

/** Restores a dismissed item. */
export function undismissItem(id: string): Promise<void> {
  return invoke<void>("undismiss_item", { id });
}

/** Lists configured accounts. Never includes tokens. */
export function listAccounts(): Promise<AccountRef[]> {
  return invoke<AccountRef[]>("list_accounts");
}

/** Validates a token against `host` and registers the account. */
export function addAccount(
  host: string,
  token: string,
  apiBase?: string,
): Promise<AccountRef> {
  return invoke<AccountRef>("add_account", { host, token, apiBase });
}

/** Removes an account, its items, and its stored token. */
export function removeAccount(id: string): Promise<void> {
  return invoke<void>("remove_account", { id });
}

/** Lists the active priority rules, so the UI can explain scores. */
export function listRules(): Promise<Rule[]> {
  return invoke<Rule[]>("list_rules");
}

/**
 * Lists the repository catalog (`specs/desktop-ui.md`): every discovered repo
 * with its tracked/clone state, plus the orgs each account can filter by.
 */
export function reposList(): Promise<RepoCatalog> {
  return invoke<RepoCatalog>("repos_list");
}

/** Registers an existing local clone path; validates and marks the repo tracked. */
export function reposSet(repo: string, path: string): Promise<Repository> {
  return invoke<Repository>("repos_set", { repo, path });
}

/**
 * Sets whether a repo's items feed notifications and the Pull Requests view,
 * independent of its clone-tracking state.
 */
export function reposSetNotify(
  accountId: string,
  repo: string,
  enabled: boolean,
): Promise<Repository> {
  return invoke<Repository>("repos_set_notify", { accountId, repo, enabled });
}

/** Removes a repo's local clone path. Idempotent; the catalog row survives. */
export function reposRemove(repo: string): Promise<void> {
  return invoke<void>("repos_remove", { repo });
}

/** Repositories discovered but never acknowledged, newest-first. */
export function reposNew(): Promise<Repository[]> {
  return invoke<Repository[]>("repos_new");
}

/** Dismisses every currently-new repository; returns how many were acked. */
export function reposAckNew(firstSeenAt: string): Promise<number> {
  return invoke<number>("repos_ack_new", { firstSeenAt });
}

/** Forces a discovery cycle for every account; returns the fresh catalog. */
export function reposRefresh(): Promise<RepoCatalog> {
  return invoke<RepoCatalog>("repos_refresh");
}

/** Starts a background clone into `target`; returns a `job_id` to poll. */
export function reposClone(repo: string, target: string): Promise<string> {
  return invoke<string>("repos_clone", { repo, target });
}

/** One clone job's current status, or `null` when the job id is unknown. */
export function reposCloneStatus(jobId: string): Promise<CloneStatus | null> {
  return invoke<CloneStatus | null>("repos_clone_status", { jobId });
}

/**
 * A repo's user-created worktrees plus the branches a new one can be created
 * from. Derived from the clone's git metadata on each call.
 */
export function reposWorktrees(repo: string): Promise<WorktreesResult> {
  return invoke<WorktreesResult>("repos_worktrees", { repo });
}

/**
 * Creates a worktree for `branch` at `path`. `branch` may be an existing
 * local/remote branch or a brand-new name; `path` may be relative to the
 * clone's parent. Errors if the target is non-empty or the branch is checked
 * out elsewhere — nothing pre-existing is ever touched.
 */
export function reposWorktreeAdd(
  repo: string,
  branch: string,
  path: string,
): Promise<WorktreeInfo> {
  return invoke<WorktreeInfo>("repos_worktree_add", { repo, branch, path });
}

/** Removes a worktree (keeping its branch); refuses dirty worktrees unless `force` is true. */
export function reposWorktreeRemove(
  repo: string,
  name: string,
  force?: boolean,
): Promise<void> {
  return invoke<void>("repos_worktree_remove", { repo, name, force: force ?? false });
}

/** Asks the daemon to poll now rather than waiting for the next cycle. */
export function pollNow(): Promise<void> {
  return invoke<void>("poll_now");
}

// ---- pull requests (`specs/pr-management.md`) ----------------------------
// Every mutating call here runs only from an explicit click.

/** Full detail for one pull request. */
export function prDetail(repo: string, number: number): Promise<PullRequestDetail> {
  return invoke<PullRequestDetail>("pr_detail", { repo, number });
}

/** Creates a pull request. */
export function prCreate(args: {
  repo: string;
  base: string;
  head: string;
  title: string;
  body: string;
  draft: boolean;
}): Promise<PullRequestDetail> {
  return invoke<PullRequestDetail>("pr_create", args);
}

/** Applies a partial update; omitted fields are left unchanged. */
export function prUpdate(
  repo: string,
  number: number,
  patch: Partial<{
    title: string;
    body: string;
    base: string;
    draft: boolean;
    labels: string[];
    reviewers: string[];
  }>,
): Promise<PullRequestDetail> {
  return invoke<PullRequestDetail>("pr_update", { repo, number, patch });
}

/** Closes a pull request without merging. */
export function prClose(
  repo: string,
  number: number,
  comment?: string,
): Promise<void> {
  return invoke<void>("pr_close", { repo, number, comment });
}

/** Merges a pull request. `headSha` guards against a PR that moved. */
export function prMerge(
  repo: string,
  number: number,
  method: MergeMethod,
  headSha: string,
  title?: string,
): Promise<void> {
  return invoke<void>("pr_merge", { repo, number, method, headSha, title });
}

/** The conversation on a pull request: issue comments plus review threads. */
export function prComments(repo: string, number: number): Promise<Conversation> {
  return invoke<Conversation>("pr_comments", { repo, number });
}

/** Posts a comment on a pull request. */
export function prComment(
  repo: string,
  number: number,
  body: string,
): Promise<Comment> {
  return invoke<Comment>("pr_comment", { repo, number, body });
}

/** Replies inside a review thread; `inReplyTo` is the last comment's id. */
export function prCommentReply(
  repo: string,
  number: number,
  inReplyTo: number,
  body: string,
): Promise<Comment> {
  return invoke<Comment>("pr_comment_reply", {
    repo,
    number,
    inReplyTo,
    body,
  });
}

/** Resolves or unresolves a review thread by its GraphQL id. */
export function prResolve(
  repo: string,
  threadId: string,
  resolved: boolean,
): Promise<{ resolved: boolean }> {
  return invoke<{ resolved: boolean }>("pr_resolve", {
    repo,
    threadId,
    resolved,
  });
}

/** Branch names in a repository, for the create-PR form. */
export function prBranches(repo: string): Promise<string[]> {
  return invoke<string[]>("pr_branches", { repo });
}

/** Label names defined on a repository, for the edit form's picker. */
export function prLabels(repo: string): Promise<string[]> {
  return invoke<string[]>("pr_labels", { repo });
}

/**
 * Rows for the Pull Requests view. `state` re-queries the daemon (it changes
 * the GraphQL search qualifier); `accountId` restricts to one account. Every
 * other filter is applied client-side in `src/desktop/PullRequests/filters.ts`.
 */
export function listPullRequests(args?: {
  accountId?: string;
  state?: PrState;
}): Promise<PullRequestSummary[]> {
  return invoke<PullRequestSummary[]>("prs_list", {
    accountId: args?.accountId,
    state: args?.state,
  });
}

// ---- notification preferences (`specs/notifications.md`) ----------------

/** Every item kind's current notification preference, enabled by default. */
export function notificationsPrefs(): Promise<KindPref[]> {
  return invoke<KindPref[]>("notifications_prefs");
}

/** Sets whether `kind` may produce a notification. */
export function notificationsSetPref(kind: ItemKind, enabled: boolean): Promise<void> {
  return invoke<void>("notifications_set_pref", { kind, enabled });
}

// ---- registered apps (`specs/desktop-ui.md`) -----------------------------
// The "Open with" apps for worktree context menus. The daemon stores the
// registry and spawns the process; these just forward.

/** Lists the registered "Open with" applications, sorted by display name. */
export function appsList(): Promise<RegisteredApp[]> {
  return invoke<RegisteredApp[]>("apps_list");
}

/** Registers an application (`name` shows in the menu, `command` is the bare executable). */
export function appsAdd(name: string, command: string): Promise<RegisteredApp> {
  return invoke<RegisteredApp>("apps_add", { name, command });
}

/** Forgets a registered application. Idempotent. */
export function appsRemove(command: string): Promise<void> {
  return invoke<void>("apps_remove", { command });
}

/** Opens `path` with a registered application (daemon spawns `command <path>`). */
export function appsOpen(command: string, path: string): Promise<void> {
  return invoke<void>("apps_open", { command, path });
}

/** Reveals `path` in the native file manager (Finder / Explorer). */
export function revealInFileManager(path: string): Promise<void> {
  return invoke<void>("reveal_in_file_manager", { path });
}

// ---- conflict resolution (`specs/conflict-resolver.md`) ------------------
// All of these act on the daemon's temp worktree — the user's local clone is
// never touched.

/** Starts a resolution session for `repo#number`. Requires a configured clone. */export function conflictPrepare(
  repo: string,
  number: number,
): Promise<ConflictSession> {
  return invoke<ConflictSession>("conflict_prepare", { repo, number });
}

/** The conflict regions of one file, read live from the session worktree. */
export function conflictFile(
  sessionId: string,
  path: string,
): Promise<ConflictFile> {
  return invoke<ConflictFile>("conflict_file", { sessionId, path });
}

/** Writes a resolution: full `content`, or a whole-file `pick` of a side. */
export function conflictSave(
  sessionId: string,
  path: string,
  content?: string,
  pick?: "ours" | "theirs",
): Promise<void> {
  return invoke<void>("conflict_save", { sessionId, path, content, pick });
}

/** Stages resolved files and creates the merge commit in the worktree. */
export function conflictCommit(
  sessionId: string,
  message: string,
): Promise<void> {
  return invoke<void>("conflict_commit", { sessionId, message });
}

/** Pushes the resolution to the PR's head and tears the session down. */
export function conflictPush(sessionId: string): Promise<void> {
  return invoke<void>("conflict_push", { sessionId });
}

/** Abandons the session; idempotent, leaves the clone and remote untouched. */
export function conflictAbort(sessionId: string): Promise<void> {
  return invoke<void>("conflict_abort", { sessionId });
}

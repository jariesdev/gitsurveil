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
  Comment,
  ConflictFile,
  ConflictSession,
  Conversation,
  MergeMethod,
  PrState,
  PullRequestDetail,
  PullRequestSummary,
  RepoConfig,
  Rule,
  ScoredItem,
  StatusResult,
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

/** Fetches the daemon's status summary. */
export function daemonStatus(): Promise<StatusResult> {
  return invoke<StatusResult>("daemon_status");
}

/** Opens `url` in the default browser and dismisses the popover. */
export function openUrl(url: string): Promise<void> {
  return invoke<void>("open_url", { url });
}

/** Closes (and destroys) the popover window. */
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

/** Lists configured local clone paths for the conflict resolver. */
export function listRepos(): Promise<RepoConfig[]> {
  return invoke<RepoConfig[]>("list_repos");
}

/** Registers a local clone path for one repo; daemon validates it. */
export function setRepo(repo: string, path: string): Promise<RepoConfig[]> {
  return invoke<RepoConfig[]>("set_repo", { repo, path });
}

/** Removes a repo's local clone path. Idempotent. */
export function removeRepo(repo: string): Promise<RepoConfig[]> {
  return invoke<RepoConfig[]>("remove_repo", { repo });
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

// ---- conflict resolution (`specs/conflict-resolver.md`) ------------------
// All of these act on the daemon's temp worktree — the user's local clone is
// never touched.

/** Starts a resolution session for `repo#number`. Requires a configured clone. */
export function conflictPrepare(
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

/**
 * TypeScript mirror of the `gitsurveil-proto` Rust crate.
 *
 * These types must stay in sync with `crates/gitsurveil-proto/src/item.rs`.
 * Per `CLAUDE.md` the long-term plan is to generate this file from the Rust
 * definitions; until that generator exists, treat the Rust crate as the source
 * of truth and update this file alongside it.
 */

/** The kind of event an {@link ActionItem} represents. */
export type ItemKind =
  | "review_requested"
  | "assigned"
  | "mentioned"
  | "participating"
  | "ci_failed"
  | "review_state_changed"
  | "ready_to_merge"
  | "authored"
  | "reviewed_by_me";

/** Local lifecycle state of an item, distinct from GitHub's own state. */
export type ItemState = "open" | "done" | "dismissed";

/** Aggregate CI/check-run status for a pull request. */
export type CiStatus = "none" | "pending" | "passing" | "failing";

/** One normalized action item served by the daemon. */
export interface ActionItem {
  id: string;
  account_id: string;
  kind: ItemKind;
  state: ItemState;
  repo: string;
  number: number | null;
  title: string;
  url: string;
  author: string;
  created_at: string;
  updated_at: string;
  first_seen_at: string;
  last_seen_at: string;
  ci_status: CiStatus;
  raw_kind: string;
  /**
   * Daemon-internal fingerprint of the activity that makes an item qualify.
   * Mirrors the Rust field for completeness, but is `#[serde(skip)]`ped — it
   * never crosses IPC, so the UI always sees it absent.
   */
  activity?: string | null;
}

/**
 * One item kind's notification preference (`notifications.prefs`). Gates
 * only the OS notification/tray interruption for that kind — items of a
 * disabled kind still appear in the Dashboard and history.
 */
export interface KindPref {
  kind: ItemKind;
  enabled: boolean;
}

/** Coarse urgency band derived from an item's score. */
export type Severity = "idle" | "info" | "normal" | "high" | "critical";

/**
 * An {@link ActionItem} plus the priority the daemon assigned it. Serialized
 * flat, so an item's own fields sit alongside `score`/`severity`/`muted`.
 */
export interface ScoredItem extends ActionItem {
  score: number;
  severity: Severity;
  /** A rule silenced notifications for this item; it still lists. */
  muted: boolean;
}

/** The daemon's health/status summary. */
export interface StatusResult {
  version: string;
  uptime_secs: number;
  account_count: number;
  open_item_count: number;
  top_severity: Severity;
}

/** Human-readable label for each item kind, used in the list UI. */
export const KIND_LABELS: Record<ItemKind, string> = {
  review_requested: "Review requested",
  assigned: "Assigned",
  mentioned: "Mentioned",
  participating: "Participating",
  ci_failed: "CI failed",
  review_state_changed: "Changes requested",
  ready_to_merge: "Ready to merge",
  authored: "Your PR",
  reviewed_by_me: "PR you reviewed",
};

/** Display order for severity groups, most urgent first. */
export const SEVERITY_ORDER: Severity[] = [
  "critical",
  "high",
  "normal",
  "info",
  "idle",
];

/** Human-readable heading for each severity group. */
export const SEVERITY_LABELS: Record<Severity, string> = {
  critical: "Critical",
  high: "High",
  normal: "Normal",
  info: "Info",
  idle: "Idle",
};

/** A configured GitHub account. Never carries a token. */
export interface AccountRef {
  id: string;
  host: string;
  api_base: string;
  login: string;
  auth_kind: "pat" | "oauth_device";
}

/** One priority rule, as stored in the daemon's config. */
export interface Rule {
  id: string;
  enabled: boolean;
  match: {
    kind?: ItemKind[];
    repo?: string;
    author?: string[];
  };
  effect: {
    add?: number;
    pin_severity?: Severity;
    mute_notifications?: boolean;
  };
}

/**
 * A repository known to the daemon's catalog (`specs/desktop-ui.md`).
 *
 * Rows are keyed by `(account_id, full_name)`; `full_name` is the
 * `"owner/name"` identifier the rest of the API already uses.
 */
export interface Repository {
  /** The account the repo was discovered under; `null` for legacy rows. */
  account_id: string | null;
  /** `github.com` or a GitHub Enterprise host. */
  host: string;
  /** The owning organization or user login. */
  owner: string;
  /** The repository name, without the owner. */
  name: string;
  /** `"owner/name"`. */
  full_name: string;
  /** Browser URL of the repository. */
  url: string;
  description: string | null;
  private: boolean;
  default_branch: string;
  /** HTTPS clone URL used by the clone engine. */
  clone_url: string;
  /** Absolute path of the registered local clone, when tracked. */
  clone_path: string | null;
  /** Whether a local clone is registered (`repos.set` or a finished clone). */
  tracked: boolean;
  /** When the daemon first saw the repo. Basis of new-repo detection. */
  first_seen_at: string;
  /** When the user acknowledged the new repo; `null` until they have. */
  notified_at: string | null;
  /** When discovery last refreshed this row. */
  last_refreshed_at: string;
  /**
   * Whether this repo's items feed notifications and the Pull Requests view.
   * Independent of `tracked` — a repo can have a local clone without being
   * watched, or be watched with no clone registered. Defaults to `true`.
   */
  notify_enabled: boolean;
}

/** One organization (or owner login) discovered for an account. */
export interface OrgRef {
  account_id: string;
  host: string;
  /** The organization or owner login. */
  name: string;
}

/** Everything the Repositories pane renders. */
export interface RepoCatalog {
  /** Distinct organizations per account, for the Organization filter. */
  orgs: OrgRef[];
  /** Every discovered repository, tracked or not. */
  repos: Repository[];
}

/** Which phase a `repos.clone` background job is in. */
export type CloneState = "running" | "done" | "failed";

/** Status of one `repos.clone` background job, polled by the UI. */
export interface CloneStatus {
  job_id: string;
  status: CloneState;
  /** Bytes received so far; meaningful only while running. */
  received: number;
  /**
   * Total bytes git expects. 0 for the whole transfer — git2 can't predict
   * the pack size, so the UI shows an indeterminate bar when total is 0.
   */
  total: number;
  /** The tracked repository, present once the clone finished. */
  repo: Repository | null;
  /** Failure detail, present when the clone failed. */
  error: string | null;
}

/**
 * One user-created worktree of a cloned repo (`specs/desktop-ui.md`). The
 * name is what `git worktree list` registers; the branch may be "(detached)".
 */
export interface WorktreeInfo {
  name: string;
  /** Absolute path to the worktree's working directory. */
  path: string;
  /** Checked-out branch short name, or "(detached)". */
  branch: string;
  /** Short (7-char) head commit id, empty when the head is unreadable. */
  head: string;
}

/**
 * `repos.worktrees`: a repo's worktrees plus every branch a new one could be
 * created from (local names, and remote names deduped to their short form).
 */
export interface WorktreesResult {
  worktrees: WorktreeInfo[];
  branches: string[];
}

/**
 * An application registered for the worktree "Open with" menu
 * (`specs/desktop-ui.md`). `command` is a bare executable name on `PATH` the
 * daemon runs as `command <path>` — never through a shell.
 */
export interface RegisteredApp {
  /** Display name shown in the "Open with" submenu. */
  name: string;
  /** Bare command-line executable on `PATH`, e.g. `code`. */
  command: string;
}

/** How the dashboard groups items. */
export type GroupBy = "priority" | "type";

/** Whether a pull request can be merged as-is. */
export type Mergeability = "clean" | "conflicted" | "blocked" | "unknown";

/** How to merge a pull request. */
export type MergeMethod = "merge" | "squash" | "rebase";

/** A reviewer and the state of their review. */
export interface Reviewer {
  login: string;
  state: string;
}

/** One CI check on the head commit. */
export interface Check {
  name: string;
  conclusion: string;
  url: string | null;
}

/** Everything the PR detail pane renders. */
export interface PullRequestDetail {
  repo: string;
  number: number;
  title: string;
  body: string;
  state: string;
  draft: boolean;
  base: string;
  head: string;
  author: string;
  labels: string[];
  reviewers: Reviewer[];
  checks: Check[];
  mergeability: Mergeability;
  url: string;
  /** Passed back on merge so a moved PR can't be merged by mistake. */
  head_sha: string;
}

/** One comment in a pull request's conversation. */
export interface Comment {
  id: number;
  author: string;
  body: string;
  created_at: string;
  path: string | null;
}

/** A review thread: a code comment plus its replies. */
export interface ReviewThread {
  /** GitHub's thread id, required by the resolve/unresolve mutation. */
  id: string;
  path: string | null;
  /** Whether the thread is resolved on GitHub. */
  resolved: boolean;
  comments: Comment[];
}

/** The conversation on a pull request. */
export interface Conversation {
  issue_comments: Comment[];
  review_threads: ReviewThread[];
}

/** Why a pull request appears in the user's list. A set, not a single value:
 * one PR can be authored *and* self-assigned, and must then be one row with
 * both badges rather than two rows. */
export type PrRole = "authored" | "review_requested" | "assigned";

/** GitHub lifecycle state of a pull request. */
export type PrState = "open" | "closed" | "merged";

/** The aggregate review decision GitHub has reached on a pull request. */
export type ReviewDecision =
  | "approved"
  | "changes_requested"
  | "review_required"
  | "none";

/** One row in the Pull Requests view. A live projection of GitHub search,
 * distinct from the event-shaped {@link ActionItem}: it carries standing state
 * (draft, review decision, mergeability) an inbox item cannot. */
export interface PullRequestSummary {
  /** The account this PR was fetched under. */
  account_id: string;
  /** `"owner/name"`. */
  repo: string;
  /** PR number. */
  number: number;
  /** Title. */
  title: string;
  /** Link to the PR on GitHub. */
  url: string;
  /** Author login. */
  author: string;
  /** Why the PR is in the list; may be several entries. */
  roles: PrRole[];
  /** GitHub lifecycle state. */
  state: PrState;
  /** Whether the PR is a draft. */
  draft: boolean;
  /** Aggregate CI status. */
  ci_status: CiStatus;
  /** The aggregate review decision. */
  review_decision: ReviewDecision;
  /** Number of unresolved review threads (comments awaiting a reply or a
   * resolve). Zero when the PR has no open threads. */
  unresolved_threads: number;
  /** Whether it can be merged as-is. `unknown` means GitHub is still
   * computing it — never treat that as conflicted. */
  mergeability: Mergeability;
  /** ISO-8601 creation time. */
  created_at: string;
  /** ISO-8601 last-update time. */
  updated_at: string;
}

/**
 * One ordered piece of a conflicted file (`specs/conflict-resolver.md`).
 * Context segments are never edited; only conflict hunks change during
 * resolution. Line content is verbatim (terminators included) so serializing
 * unmodified segments back to text is byte-exact.
 */
export type ConflictSegment =
  | { kind: "context"; lines: string[] }
  | { kind: "conflict"; hunk: ConflictHunk };

/** One `<<<<<<<` … `>>>>>>>` conflict block in a file. */
export interface ConflictHunk {
  /** 1-based line number of the `<<<<<<<` marker in the original file. */
  start_line: number;
  /** 1-based line number of the `>>>>>>>` marker (inclusive). */
  end_line: number;
  /** Verbatim marker block (terminators included); center pane's initial content. */
  raw: string[];
  /** Text after `<<<<<<<` (e.g. `HEAD`); null when git left no label. */
  ours_label: string | null;
  /** The PR branch's side of the conflict (left pane). */
  ours: string[];
  /** diff3 `|||||||` section; null for non-diff3 conflicts. */
  base: string[] | null;
  /** Text after `|||||||` (e.g. `merged common ancestor`); null when absent. */
  base_label: string | null;
  /** The base branch's side of the conflict (right pane). */
  theirs: string[];
  /** Text after `>>>>>>>` (e.g. the base branch name); null when absent. */
  theirs_label: string | null;
}

/** A file's conflicted content in ordered segments. */
export interface ConflictFile {
  /** Path relative to the repository root. */
  path: string;
  /** True when git refuses a text merge (whole-file pick only). */
  binary: boolean;
  /** True when the file exceeds the text-editing threshold (whole-file pick only). */
  large: boolean;
  segments: ConflictSegment[];
}

/** Summary entry: enough to render the file list without the file's content. */
export interface ConflictFileSummary {
  path: string;
  conflicts: number;
  binary: boolean;
  large: boolean;
}

/** A live conflict-resolution session on a temp worktree. */
export interface ConflictSession {
  session_id: string;
  repo: string;
  number: number;
  base: string;
  head: string;
  worktree_path: string;
  files: ConflictFileSummary[];
}

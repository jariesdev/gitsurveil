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
  | "ready_to_merge";

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
 * One configured local clone path (`specs/conflict-resolver.md`). The daemon
 * validates the path on `repos.set`; conflict resolution only works for repos
 * listed here.
 */
export interface RepoConfig {
  /** `"owner/name"` exactly as it appears on GitHub. */
  repo: string;
  /** Absolute path to a local clone of that repository. */
  path: string;
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

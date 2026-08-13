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
  | "review_state_changed";

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

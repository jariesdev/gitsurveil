//! The unified `ActionItem` model described in `specs/github-integration.md`.
//!
//! Every GitHub notification, review request, assignment, or CI failure is
//! normalized into one of these so the priority engine, UIs, and storage
//! layer only ever deal with one shape, regardless of which GitHub endpoint
//! it came from.

use serde::{Deserialize, Serialize};

/// The kind of event an [`ActionItem`] represents.
///
/// Ordering here is not significance order — significance is computed by the
/// priority engine (`specs/priority-engine.md`), not encoded in the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// The user was requested as a reviewer on a pull request.
    ReviewRequested,
    /// A pull request or issue is assigned to the user.
    Assigned,
    /// The user was `@mentioned` in a comment, issue, or PR body.
    Mentioned,
    /// The user is participating in a thread they are not directly named on.
    Participating,
    /// A CI check failed on a pull request authored by the user.
    CiFailed,
    /// A reviewer requested changes on the user's own pull request.
    ReviewStateChanged,
    /// The user's own pull request is approved, green, and mergeable — one
    /// click from landing. Fired only on the transition, never for a
    /// merely-open authored PR (`specs/priority-engine.md`).
    ReadyToMerge,
    /// A pull request authored by the user needs attention: a comment from
    /// someone else, an unresolved review thread, or a failing CI check. A
    /// merely-open authored PR (only commits / the user's own comments)
    /// produces no item (`specs/priority-engine.md`).
    Authored,
    /// A pull request the user reviewed has an unanswered reply in a review
    /// thread the user commented in — it clears once the user replies back.
    /// Distinct from [`Self::ReviewRequested`], which is about a review still
    /// owed.
    ReviewedByMe,
}

/// One item kind's notification preference (`notifications.prefs`,
/// `specs/notifications.md` § Preferences). Gates only the OS
/// notification/tray interruption for that kind — items of a disabled kind
/// still appear in the Dashboard and history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindPref {
    /// Which kind this preference applies to.
    pub kind: ItemKind,
    /// Whether items of this kind may produce a notification. Defaults to
    /// `true` for every kind.
    pub enabled: bool,
}

/// Local lifecycle state of an item. Distinct from GitHub's own
/// open/closed/merged state, which lives in `raw_kind`/API responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// Still relevant; shown in the active list.
    Open,
    /// Resolved upstream (closed/merged) and moved to history.
    Done,
    /// Hidden by the user locally; resurrected if GitHub activity resumes it.
    Dismissed,
}

/// How an [`AccountRef`] authenticates to its host.
///
/// See `specs/github-integration.md` — the token itself is never part of
/// this struct or any other proto type; it lives only in the OS keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    /// Personal access token, pasted by the user.
    Pat,
    /// OAuth device flow.
    OauthDevice,
}

/// A configured GitHub account (Cloud or Enterprise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRef {
    /// Stable local identifier; also the keychain key for this account's token.
    pub id: String,
    /// Host, e.g. `"github.com"` or an Enterprise hostname.
    pub host: String,
    /// API base URL, e.g. `https://api.github.com` or `https://<host>/api/v3`.
    pub api_base: String,
    /// GitHub login, resolved via `GET /user` at account setup.
    pub login: String,
    /// How this account authenticates.
    pub auth_kind: AuthKind,
}

/// One normalized action item, sourced from any of the polled GitHub
/// endpoints (`specs/github-integration.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionItem {
    /// Stable id: a hash of `account_id` + source kind + source id. Stable
    /// across polls so the diff/dedup logic can recognize a carried-over item.
    pub id: String,
    /// The account this item was fetched under.
    pub account_id: String,
    /// What kind of event this is.
    pub kind: ItemKind,
    /// Local lifecycle state.
    pub state: ItemState,
    /// `"owner/name"`.
    pub repo: String,
    /// PR/issue number, when applicable (some notification threads have none).
    pub number: Option<u64>,
    /// Item title.
    pub title: String,
    /// Link to the item on GitHub.
    pub url: String,
    /// Login of the item's author.
    pub author: String,
    /// GitHub's `created_at` for the underlying object.
    pub created_at: String,
    /// GitHub's `updated_at` for the underlying object; drives diff semantics.
    pub updated_at: String,
    /// When this daemon instance first observed the item.
    pub first_seen_at: String,
    /// When this daemon instance last observed the item in a poll.
    pub last_seen_at: String,
    /// Aggregate CI status, when known.
    pub ci_status: CiStatus,
    /// The original GitHub reason/type string, kept for debugging and to
    /// support future priority rules without a schema change.
    pub raw_kind: String,
    /// Daemon-internal fingerprint of the activity that makes this item
    /// qualify (e.g. the set of comments and unresolved threads behind an
    /// `Authored` item). Compared across polls to detect qualifying
    /// *transitions* for notifications. Never serialized over IPC.
    #[serde(skip)]
    pub activity: Option<String>,
    /// Permanently archived by "Clear all history": the item no longer shows
    /// in the Dashboard or history, and the poller never resurrects it — not
    /// even when GitHub reports new activity. Never serialized over IPC —
    /// archived items are excluded before any response leaves the daemon.
    #[serde(skip)]
    pub archived: bool,
}

/// Aggregate CI/check-run status for a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiStatus {
    /// No checks configured or reported.
    None,
    /// At least one check is still running.
    Pending,
    /// All checks passed.
    Passing,
    /// At least one required check failed.
    Failing,
}

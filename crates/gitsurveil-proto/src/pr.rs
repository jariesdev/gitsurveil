//! Pull-request types shared with the UI (`specs/pr-management.md`).
//!
//! Deliberately a projection of GitHub's models rather than a mirror: only
//! the fields the desktop UI actually renders or edits appear here, so the
//! wire format doesn't churn every time GitHub adds a field.

use serde::{Deserialize, Serialize};

use crate::CiStatus;

/// Whether a pull request can be merged as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mergeability {
    /// Ready to merge.
    Clean,
    /// Conflicts with the base branch — the conflict resolver's entry point.
    Conflicted,
    /// Mergeable, but blocked by required reviews or failing checks.
    Blocked,
    /// GitHub hasn't finished computing it yet; it computes mergeability
    /// asynchronously, so this is a normal transient state, not an error.
    Unknown,
}

/// A reviewer and the state of their review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewer {
    /// GitHub login.
    pub login: String,
    /// `approved`, `changes_requested`, `commented`, or `pending`.
    pub state: String,
    /// How many review rounds this reviewer has submitted on this PR.
    /// Zero means they've been requested but haven't reviewed yet.
    pub rounds: u32,
}

/// One CI check on the head commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// Check name as GitHub reports it.
    pub name: String,
    /// `success`, `failure`, `pending`, …
    pub conclusion: String,
    /// Link to the run, when there is one.
    pub url: Option<String>,
}

/// Everything the PR detail pane renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestDetail {
    /// `"owner/name"`.
    pub repo: String,
    /// PR number.
    pub number: u64,
    /// Title.
    pub title: String,
    /// Body, as raw markdown.
    pub body: String,
    /// `open`, `closed`, or `merged`.
    pub state: String,
    /// Whether the PR is a draft.
    pub draft: bool,
    /// Branch being merged into.
    pub base: String,
    /// Branch being merged from.
    pub head: String,
    /// Author login.
    pub author: String,
    /// Labels applied.
    pub labels: Vec<String>,
    /// Requested and completed reviews.
    pub reviewers: Vec<Reviewer>,
    /// Checks on the head commit.
    pub checks: Vec<Check>,
    /// Whether it can be merged as-is.
    pub mergeability: Mergeability,
    /// Link to the PR on GitHub.
    pub url: String,
    /// Head commit SHA. Mutations pass this back so an action can't be
    /// applied to a PR that moved underneath the user.
    pub head_sha: String,
}

/// One comment in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// GitHub's comment id.
    pub id: u64,
    /// Author login.
    pub author: String,
    /// Body, as raw markdown.
    pub body: String,
    /// ISO-8601 creation time.
    pub created_at: String,
    /// Path this comment is anchored to, for review comments.
    pub path: Option<String>,
}

/// A review thread: a code comment plus its replies. A thread resolves as a
/// whole on GitHub, which is what turns the row badge off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    /// GitHub's thread id, required by the resolve/unresolve mutation.
    pub id: String,
    /// Path the thread's comments are anchored to.
    pub path: Option<String>,
    /// Whether the thread is resolved on GitHub.
    pub resolved: bool,
    /// The thread's comments, oldest first.
    pub comments: Vec<Comment>,
}

/// The conversation on a pull request. Top-level issue comments and review
/// threads are deliberately distinct: only review comments group into threads
/// with a resolve state, while issue comments are the PR's flat timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// Top-level comments on the PR.
    pub issue_comments: Vec<Comment>,
    /// Review comment threads, each carrying its own resolve state.
    pub review_threads: Vec<ReviewThread>,
}

/// How to merge a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    /// Merge commit.
    Merge,
    /// Squash and merge.
    Squash,
    /// Rebase and merge.
    Rebase,
}

impl MergeMethod {
    /// The string GitHub's merge endpoint expects.
    pub fn as_api_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

/// Why a pull request appears in the user's list. A set rather than a single
/// value: one PR can be authored *and* self-assigned, and must then be one
/// summary row carrying both roles instead of two rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrRole {
    /// The user opened the PR.
    Authored,
    /// The user was requested as a reviewer.
    ReviewRequested,
    /// The PR is assigned to the user.
    Assigned,
}

/// GitHub lifecycle state of a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    /// Still open.
    Open,
    /// Closed without merging.
    Closed,
    /// Merged.
    Merged,
}

/// The aggregate review decision GitHub has reached on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    /// At least one approving review and none requesting changes.
    Approved,
    /// At least one review requesting changes.
    ChangesRequested,
    /// Review is required but no decision has been reached.
    ReviewRequired,
    /// No reviews and review not required.
    None,
}

/// One row in the Pull Requests view (`specs/desktop-ui.md`).
///
/// A live projection of GitHub's search results, deliberately separate from
/// the event-shaped [`ActionItem`](crate::ActionItem): it carries standing
/// state (draft, review decision, mergeability) that an inbox item model
/// cannot hold without distortion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestSummary {
    /// The account this PR was fetched under.
    pub account_id: String,
    /// `"owner/name"`.
    pub repo: String,
    /// PR number.
    pub number: u64,
    /// Title.
    pub title: String,
    /// Link to the PR on GitHub.
    pub url: String,
    /// Author login.
    pub author: String,
    /// Why the PR is in the list; may be several entries.
    pub roles: Vec<PrRole>,
    /// GitHub lifecycle state.
    pub state: PrState,
    /// Whether the PR is a draft.
    pub draft: bool,
    /// Aggregate CI status.
    pub ci_status: CiStatus,
    /// The aggregate review decision.
    pub review_decision: ReviewDecision,
    /// Number of unresolved review threads (comments awaiting a reply or a
    /// resolve). Zero when the PR has no open threads.
    pub unresolved_threads: u64,
    /// Whether it can be merged as-is. `Unknown` means GitHub is still
    /// computing it — never treat that as conflicted.
    pub mergeable: Mergeability,
    /// ISO-8601 creation time.
    pub created_at: String,
    /// ISO-8601 last-update time.
    pub updated_at: String,
    /// The PR's head branch name (e.g. `feature/x`), when GitHub reported
    /// one. This is what ties a PR to a local branch — the Repositories
    /// pane matches it against a worktree's checked-out branch to mark the
    /// worktree as merged.
    pub head_ref: Option<String>,
}

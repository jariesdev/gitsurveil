//! Pull-request types shared with the UI (`specs/pr-management.md`).
//!
//! Deliberately a projection of GitHub's models rather than a mirror: only
//! the fields the desktop UI actually renders or edits appear here, so the
//! wire format doesn't churn every time GitHub adds a field.

use serde::{Deserialize, Serialize};

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

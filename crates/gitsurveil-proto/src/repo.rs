//! Repository catalog and clone-job types (`specs/desktop-ui.md`).
//!
//! The daemon discovers the user's GitHub repositories in the background and
//! serves the catalog here. A repo becomes "tracked" once the user registers a
//! local clone path for it (`repos.set`) or asks the daemon to clone it
//! (`repos.clone`) — conflict resolution only works for tracked repos. New-repo
//! detection rides the same rows: a repo with `tracked == false` and
//! `notified_at == None` hasn't been acknowledged yet and is offered to the UI
//! when the main window opens.

use serde::{Deserialize, Serialize};

/// A repository known to the daemon's catalog.
///
/// Rows are keyed by `(account_id, full_name)`; `full_name` is the
/// `"owner/name"` identifier the rest of the API already uses. `account_id` is
/// [`None`] only for rows imported from a pre-catalog config, where no account
/// could be determined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    /// The account the repo was discovered under; [`None`] for legacy-imported
    /// rows with no determinable account.
    pub account_id: Option<String>,
    /// `github.com` or a GitHub Enterprise host, for disambiguating repos that
    /// share a `full_name` across accounts.
    pub host: String,
    /// The owning organization or user login.
    pub owner: String,
    /// The repository name, without the owner.
    pub name: String,
    /// `"owner/name"` — the identifier used by `repos.set`, `repos.clone`, and
    /// the existing item/PR APIs.
    pub full_name: String,
    /// Browser URL of the repository.
    pub url: String,
    /// The repository's description, if it has one.
    pub description: Option<String>,
    /// Whether the repository is private.
    pub private: bool,
    /// The default branch name (e.g. `main`).
    pub default_branch: String,
    /// HTTPS clone URL used by the clone engine. Derived from the REST API
    /// response, so it is correct for GitHub Enterprise too.
    pub clone_url: String,
    /// Absolute path of the registered local clone, present once the repo is
    /// tracked. The daemon never writes here; the user owns these paths.
    pub clone_path: Option<String>,
    /// Whether a local clone is registered for this repo (`repos.set` or a
    /// finished `repos.clone`). Conflict resolution requires this.
    pub tracked: bool,
    /// When the daemon first saw the repo. The basis of new-repo detection.
    pub first_seen_at: String,
    /// When the user acknowledged the new repo; [`None`] until they have.
    pub notified_at: Option<String>,
    /// When discovery last refreshed this row.
    pub last_refreshed_at: String,
}

/// One organization (or owner login) discovered for an account, used to group
/// the catalog in the Repositories pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgRef {
    /// The account the org belongs to.
    pub account_id: String,
    /// The account's host, so identical org names under different accounts
    /// (or hosts) render distinctly.
    pub host: String,
    /// The organization or owner login.
    pub name: String,
}

/// Everything the Repositories pane renders: the orgs to group by and every
/// discovered repository, tracked or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoCatalog {
    /// Distinct organizations per account, for the Organization filter.
    pub orgs: Vec<OrgRef>,
    /// Every discovered repository for every account.
    pub repos: Vec<Repository>,
}

/// Which phase a clone job is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloneState {
    /// A background task is fetching the repository.
    Running,
    /// The clone finished and the repo is now tracked.
    Done,
    /// The clone failed; [`CloneStatus::error`] holds the reason.
    Failed,
}

/// One worktree registered in a repo's git metadata (`repos.worktrees`).
///
/// Derived from the clone on every request — the daemon keeps no table for
/// these, so worktrees created or removed outside gitsurveil (git CLI, IDEs)
/// show up too. Conflict-session worktrees (named `gitsurveil-*`) are filtered
/// out before this is served: they're transient, pruned at daemon startup, and
/// the UI must never offer to delete one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// The worktree's registered name — the key `git worktree list` uses.
    pub name: String,
    /// Absolute path of the worktree's working directory.
    pub path: String,
    /// The checked-out branch shorthand (e.g. `feature/x`), or
    /// `"(detached)"` when the worktree's HEAD is detached.
    pub branch: String,
    /// Short commit id of the worktree's HEAD.
    pub head: String,
}

/// Everything the Repositories pane needs to render one repo's worktrees:
/// the worktree list itself plus the branches a new worktree can be created
/// from (`repos.worktrees`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreesResult {
    /// The repo's user-created worktrees, in git registration order.
    pub worktrees: Vec<WorktreeInfo>,
    /// Branch shortnames a new worktree can check out: every local branch,
    /// plus remote-tracking branches (`origin/x`) that don't shadow a local
    /// one. The UI offers these in the add-worktree combobox; a name typed
    /// beyond them is created fresh in the new worktree.
    pub branches: Vec<String>,
}

/// Status of one `repos.clone` background job, polled by the UI.
///
/// Progress is byte-based: git reports how many bytes of the pack have arrived.
/// git2 cannot predict the final pack size up front, so `total` stays 0 for
/// the whole transfer and the UI renders an indeterminate progress bar with
/// the running byte count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloneStatus {
    /// The job identifier, echoed from `repos.clone`.
    pub job_id: String,
    /// Which phase the job is in.
    pub status: CloneState,
    /// Bytes received so far. Meaningful only while [`CloneState::Running`];
    /// 0 once the job is done or failed.
    pub received: u64,
    /// Total bytes git expects to fetch. 0 for the whole transfer — git2
    /// doesn't know the pack size in advance, so a `total` of 0 means the UI
    /// should show an indeterminate bar.
    pub total: u64,
    /// The tracked repository, present once the clone finished. Lets the UI
    /// mark the row done without refetching the whole catalog.
    pub repo: Option<Repository>,
    /// Failure detail, present when the clone failed.
    pub error: Option<String>,
}

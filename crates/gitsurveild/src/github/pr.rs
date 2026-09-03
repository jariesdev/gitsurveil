//! Pull-request reads and mutations (`specs/pr-management.md`).
//!
//! Every mutating call here is reached only through an explicit user action in
//! the UI — nothing in the poll loop calls into this module. That's the
//! "nothing is posted to GitHub without an explicit user action" rule from
//! `CLAUDE.md`, enforced structurally rather than by convention.
//!
//! Uses the same plain `reqwest` client as the rest of the GitHub layer so
//! request headers and error handling stay uniform.

use std::collections::{HashMap, HashSet};

use gitsurveil_proto::{
    Check, CiStatus, Comment, Conversation, MergeMethod, Mergeability, PrRole, PrState,
    PullRequestDetail, PullRequestSummary, ReviewDecision, ReviewThread, Reviewer,
};
use serde::Deserialize;
use serde_json::json;

use crate::error::{DaemonError, Result};
use crate::github::GitHubClient;

/// Fields we edit on an existing PR. `None` means "leave unchanged", so the
/// UI can send a partial update without having to echo back every field it
/// didn't touch.
#[derive(Debug, Default, Clone, serde::Deserialize)]
pub struct PrPatch {
    /// New title.
    pub title: Option<String>,
    /// New body.
    pub body: Option<String>,
    /// New base branch.
    pub base: Option<String>,
    /// Draft/ready-for-review toggle.
    pub draft: Option<bool>,
    /// Replaces the label set entirely.
    pub labels: Option<Vec<String>>,
    /// Adds these reviewers; GitHub has no "replace reviewers" call.
    pub reviewers: Option<Vec<String>>,
}

impl GitHubClient {
    /// Fetches everything the PR detail pane renders.
    pub async fn pr_detail(&self, repo: &str, number: u64) -> Result<PullRequestDetail> {
        let pr: RawPr = self
            .get_json(&format!("/repos/{repo}/pulls/{number}"))
            .await?;

        // Reviews and checks are separate endpoints; fetched concurrently
        // since neither depends on the other and the detail pane needs both
        // before it can render.
        let reviews_path = format!("/repos/{repo}/pulls/{number}/reviews");
        let checks_path = format!("/repos/{repo}/commits/{}/check-runs", pr.head.sha);
        let (reviews, checks) = tokio::join!(
            self.get_json::<Vec<RawReview>>(&reviews_path),
            self.get_json::<RawCheckRuns>(&checks_path)
        );

        let reviewers = dedupe_reviewers(reviews.unwrap_or_default(), pr.requested_reviewers.as_ref());

        let checks = checks
            .map(|c| {
                c.check_runs
                    .into_iter()
                    .map(|run| Check {
                        name: run.name,
                        conclusion: run.conclusion.unwrap_or_else(|| run.status.clone()),
                        url: run.html_url,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(PullRequestDetail {
            repo: repo.to_string(),
            number,
            title: pr.title,
            body: pr.body.unwrap_or_default(),
            state: if pr.merged_at.is_some() {
                "merged".to_string()
            } else {
                pr.state
            },
            draft: pr.draft.unwrap_or(false),
            base: pr.base.ref_name,
            head: pr.head.ref_name,
            author: pr.user.map(|u| u.login).unwrap_or_default(),
            labels: pr.labels.into_iter().map(|l| l.name).collect(),
            reviewers,
            checks,
            mergeability: mergeability_of(&pr.mergeable, pr.mergeable_state.as_deref()),
            url: pr.html_url,
            head_sha: pr.head.sha,
        })
    }

    /// Creates a pull request and returns its detail.
    pub async fn pr_create(
        &self,
        repo: &str,
        base: &str,
        head: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> Result<PullRequestDetail> {
        let created: RawPr = self
            .post_json(
                &format!("/repos/{repo}/pulls"),
                json!({
                    "title": title,
                    "body": body,
                    "base": base,
                    "head": head,
                    "draft": draft,
                }),
            )
            .await?;
        self.pr_detail(repo, created.number).await
    }

    /// Applies a partial update to an existing PR.
    ///
    /// Labels and reviewers live on different endpoints from the PR itself,
    /// so a full update is up to three calls; they're issued in sequence and
    /// the first failure aborts, leaving the rest unapplied rather than
    /// guessing at a rollback.
    pub async fn pr_update(
        &self,
        repo: &str,
        number: u64,
        patch: &PrPatch,
    ) -> Result<PullRequestDetail> {
        let mut body = serde_json::Map::new();
        if let Some(title) = &patch.title {
            body.insert("title".into(), json!(title));
        }
        if let Some(text) = &patch.body {
            body.insert("body".into(), json!(text));
        }
        if let Some(base) = &patch.base {
            body.insert("base".into(), json!(base));
        }
        if let Some(draft) = patch.draft {
            body.insert("draft".into(), json!(draft));
        }
        if !body.is_empty() {
            let _: RawPr = self
                .patch_json(
                    &format!("/repos/{repo}/pulls/{number}"),
                    serde_json::Value::Object(body),
                )
                .await?;
        }

        if let Some(labels) = &patch.labels {
            let _: serde_json::Value = self
                .put_json(
                    &format!("/repos/{repo}/issues/{number}/labels"),
                    json!({ "labels": labels }),
                )
                .await?;
        }

        if let Some(reviewers) = &patch.reviewers {
            if !reviewers.is_empty() {
                let _: serde_json::Value = self
                    .post_json(
                        &format!("/repos/{repo}/pulls/{number}/requested_reviewers"),
                        json!({ "reviewers": reviewers }),
                    )
                    .await?;
            }
        }

        self.pr_detail(repo, number).await
    }

    /// Closes a PR without merging, optionally leaving a comment first so the
    /// explanation lands before the close event in the timeline.
    pub async fn pr_close(&self, repo: &str, number: u64, comment: Option<&str>) -> Result<()> {
        if let Some(text) = comment.filter(|c| !c.trim().is_empty()) {
            self.pr_comment(repo, number, text).await?;
        }
        let _: RawPr = self
            .patch_json(
                &format!("/repos/{repo}/pulls/{number}"),
                json!({ "state": "closed" }),
            )
            .await?;
        Ok(())
    }

    /// Merges a PR.
    ///
    /// `head_sha` is sent as `sha`, so GitHub rejects the merge if the branch
    /// moved since the UI loaded it — a stale merge is exactly the mistake
    /// worth making impossible rather than merely unlikely.
    pub async fn pr_merge(
        &self,
        repo: &str,
        number: u64,
        method: MergeMethod,
        head_sha: &str,
        commit_title: Option<&str>,
    ) -> Result<()> {
        let mut body = serde_json::Map::new();
        body.insert("merge_method".into(), json!(method.as_api_str()));
        body.insert("sha".into(), json!(head_sha));
        if let Some(title) = commit_title {
            body.insert("commit_title".into(), json!(title));
        }
        let _: serde_json::Value = self
            .put_json(
                &format!("/repos/{repo}/pulls/{number}/merge"),
                serde_json::Value::Object(body),
            )
            .await?;
        Ok(())
    }

    /// Fetches the conversation: issue comments via REST plus review threads
    /// via GraphQL (the only way to get each thread's id and resolve state).
    ///
    /// One request each: the issue timeline, and the PR's threads with their
    /// comments. Resolving is a mutation on a thread id, so the id must come
    /// from the same query the UI reads — it is the key both render and
    /// resolve act on.
    pub async fn pr_comments(&self, repo: &str, number: u64) -> Result<Conversation> {
        let issue_path = format!("/repos/{repo}/issues/{number}/comments");
        let issue_comments: Vec<RawComment> = self.get_json(&issue_path).await.unwrap_or_default();

        let (owner, name) = repo
            .split_once('/')
            .ok_or_else(|| DaemonError::Config(format!("invalid repo slug: {repo}")))?;
        let envelope: RawConversationEnvelope = self
            .octocrab
            .graphql(&conversation_request(owner, name, number))
            .await
            .map_err(DaemonError::GitHub)?;
        let review_threads = envelope
            .repository
            .pull_request
            .map(|p| p.review_threads.nodes)
            .unwrap_or_default();

        Ok(Conversation {
            issue_comments: issue_comments
                .into_iter()
                .map(|c| c.into_comment())
                .collect(),
            review_threads: review_threads
                .into_iter()
                .map(|t| t.into_thread())
                .collect(),
        })
    }

    /// Posts a top-level comment on a PR.
    pub async fn pr_comment(&self, repo: &str, number: u64, body: &str) -> Result<Comment> {
        let created: RawComment = self
            .post_json(
                &format!("/repos/{repo}/issues/{number}/comments"),
                json!({ "body": body }),
            )
            .await?;
        Ok(created.into_comment())
    }

    /// Replies to the last comment in a review thread. GitHub's REST API
    /// threads a reply by passing the parent comment's id as `in_reply_to`.
    pub async fn pr_comment_reply(
        &self,
        repo: &str,
        number: u64,
        in_reply_to: u64,
        body: &str,
    ) -> Result<Comment> {
        let created: RawComment = self
            .post_json(
                &format!("/repos/{repo}/pulls/{number}/comments"),
                json!({ "body": body, "in_reply_to": in_reply_to }),
            )
            .await?;
        Ok(created.into_comment())
    }

    /// Resolves (`true`) or unresolves (`false`) a review thread. Needs the
    /// thread's GraphQL id, which `pr_comments` returns.
    pub async fn resolve_thread(&self, thread_id: &str, resolved: bool) -> Result<bool> {
        let resp: RawResolveEnvelope = self
            .octocrab
            .graphql(&resolve_thread_request(thread_id, resolved))
            .await
            .map_err(DaemonError::GitHub)?;
        Ok(resp
            .resolved_thread
            .or(resp.unresolved_thread)
            .map(|t| t.thread.is_resolved)
            .unwrap_or(resolved))
    }

    /// Branches in a repository, for the create-PR form's pickers.
    pub async fn list_branches(&self, repo: &str) -> Result<Vec<String>> {
        let branches: Vec<RawBranch> = self
            .get_json(&format!("/repos/{repo}/branches?per_page=100"))
            .await?;
        Ok(branches.into_iter().map(|b| b.name).collect())
    }

    /// Labels defined on a repository, for the edit form's picker. Assigning a
    /// label that isn't in this list is still valid — GitHub creates it — so
    /// the picker also lets the user type a new name.
    pub async fn list_labels(&self, repo: &str) -> Result<Vec<String>> {
        let labels: Vec<RawLabel> = self
            .get_json(&format!("/repos/{repo}/labels?per_page=100"))
            .await?;
        Ok(labels.into_iter().map(|l| l.name).collect())
    }

    /// Lists the pull requests relevant to this account's user
    /// (`specs/desktop-ui.md`, Pull Requests view).
    ///
    /// One GraphQL request: three aliases (`authored`, `reviewRequested`,
    /// `assigned`) over a shared fragment, so a PR the user opened *and* was
    /// assigned lands once with both roles rather than twice. `state` narrows
    /// the search qualifier (`None` = all states). This is a live query — the
    /// view never caches it, and the poller never calls it.
    pub async fn list_pull_requests(
        &self,
        state: Option<PrState>,
    ) -> Result<Vec<PullRequestSummary>> {
        let qualifier = search_qualifier(state);
        // `is:pr` keeps issue nodes out of the results, so the fragment can
        // assume PullRequest (the poller's query predates this and is looser).
        let query = format!(
            r#"query {{
  authored: search(query: "{qualifier} author:@me", type: ISSUE, first: 100) {{ nodes {{ ...prFields }} }}
  reviewRequested: search(query: "{qualifier} review-requested:@me", type: ISSUE, first: 100) {{ nodes {{ ...prFields }} }}
  assigned: search(query: "{qualifier} assignee:@me", type: ISSUE, first: 100) {{ nodes {{ ...prFields }} }}
}}
fragment prFields on PullRequest {{
  __typename number title url createdAt updatedAt state isDraft reviewDecision mergeable
  headRefName
  author {{ login }} repository {{ nameWithOwner }}
  commits(last: 1) {{ nodes {{ commit {{ statusCheckRollup {{ state }} }} }} }}
  reviewThreads(first: 100) {{ nodes {{ isResolved }} }}
}}"#,
            qualifier = qualifier
        );

        let body = json!({ "query": query });
        let resp: SearchEnvelope = self
            .octocrab
            .graphql(&body)
            .await
            .map_err(DaemonError::GitHub)?;

        let sets = [
            (resp.authored.nodes, PrRole::Authored),
            (resp.review_requested.nodes, PrRole::ReviewRequested),
            (resp.assigned.nodes, PrRole::Assigned),
        ];
        let mut summaries = merged_summaries(&self.account_id, sets, state);
        // The UI sorts by recency anyway; returning them pre-sorted keeps the
        // wire order stable for clients that don't.
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }
}

/// Builds the review-thread query body.
///
/// `owner`/`name` come from a caller-supplied slug and travel as **variables**,
/// never interpolated into the query text: a `"` in either would otherwise
/// close the string literal and let arbitrary GraphQL run under the user's
/// token. Split out from the request so that guarantee is testable offline.
fn conversation_request(owner: &str, name: &str, number: u64) -> serde_json::Value {
    const QUERY: &str = r#"query($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          id isResolved path
          comments(first: 100) {
            nodes { databaseId author { login } body createdAt }
          }
        }
      }
    }
  }
}"#;
    json!({
        "query": QUERY,
        "variables": { "owner": owner, "name": name, "number": number },
    })
}

/// Builds the resolve/unresolve mutation body.
///
/// The operation is chosen between two constants, so no caller input reaches
/// the query text; `thread_id` travels as a variable for the same reason as
/// [`conversation_request`].
fn resolve_thread_request(thread_id: &str, resolved: bool) -> serde_json::Value {
    const RESOLVE: &str = r#"mutation($threadId: ID!) {
  resolveReviewThread(input: { threadId: $threadId }) { thread { isResolved } }
}"#;
    const UNRESOLVE: &str = r#"mutation($threadId: ID!) {
  unresolveReviewThread(input: { threadId: $threadId }) { thread { isResolved } }
}"#;
    json!({
        "query": if resolved { RESOLVE } else { UNRESOLVE },
        "variables": { "threadId": thread_id },
    })
}

/// The search qualifier for a requested [`PrState`]. `None` means "all
/// states", which is just every PR.
fn search_qualifier(state: Option<PrState>) -> &'static str {
    match state {
        Some(PrState::Open) => "is:open is:pr",
        // A merged PR reports `closed` in search too, so closed-without-merge
        // must be narrowed and then verified node-by-node (see `state_matches`).
        Some(PrState::Closed) => "is:closed is:pr",
        Some(PrState::Merged) => "is:merged is:pr",
        None => "is:pr",
    }
}

/// Whether a node's authoritative GraphQL state satisfies the requested
/// filter. The qualifier is a coarse narrowing (search indexes merged PRs as
/// closed); this is the exact gate.
fn state_matches(actual: &str, requested: Option<PrState>) -> bool {
    match requested {
        None => true,
        Some(PrState::Open) => actual == "OPEN",
        Some(PrState::Closed) => actual == "CLOSED",
        Some(PrState::Merged) => actual == "MERGED",
    }
}

/// Merges the three per-role result sets into one row per `(repo, number)`,
/// unioning roles so a PR in two sets is one summary with two badges.
fn merged_summaries(
    account_id: &str,
    sets: impl IntoIterator<Item = (Vec<SearchPrNode>, PrRole)>,
    state: Option<PrState>,
) -> Vec<PullRequestSummary> {
    let mut by_key: HashMap<(String, u64), PullRequestSummary> = HashMap::new();
    for (nodes, role) in sets {
        for node in nodes {
            if node.typename.as_deref() != Some("PullRequest") {
                continue;
            }
            if !state_matches(&node.state, state) {
                continue;
            }
            let key = (node.repository.name_with_owner.clone(), node.number);
            let entry = by_key
                .entry(key)
                .or_insert_with(|| node.into_summary(account_id));
            if !entry.roles.contains(&role) {
                entry.roles.push(role);
            }
        }
    }
    by_key.into_values().collect()
}

/// Maps GitHub's two mergeability signals onto one enum.
///
/// `mergeable` is null while GitHub computes it in the background, which is
/// why "unknown" is a first-class state rather than an error.
fn mergeability_of(mergeable: &Option<bool>, state: Option<&str>) -> Mergeability {
    match (mergeable, state) {
        (Some(false), _) => Mergeability::Conflicted,
        (Some(true), Some("blocked")) => Mergeability::Blocked,
        (Some(true), _) => Mergeability::Clean,
        (None, Some("dirty")) => Mergeability::Conflicted,
        (None, _) => Mergeability::Unknown,
    }
}

// ---- GitHub wire shapes -------------------------------------------------
// Only the fields used above; GitHub returns far more.

#[derive(Debug, Deserialize)]
struct RawPr {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    draft: Option<bool>,
    html_url: String,
    merged_at: Option<String>,
    mergeable: Option<bool>,
    mergeable_state: Option<String>,
    user: Option<RawUser>,
    base: RawRef,
    head: RawRef,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    requested_reviewers: Option<Vec<RawUser>>,
}

#[derive(Debug, Deserialize)]
struct RawRef {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RawReview {
    user: Option<RawUser>,
    state: String,
}

/// Collapses GitHub's per-round review rows into one entry per reviewer.
///
/// `GET /pulls/{number}/reviews` returns one row per review round in
/// chronological order, so a reviewer who submitted multiple reviews appears
/// once per round. A later review supersedes the earlier one: each login
/// appears exactly once, carrying the state of its latest round and the
/// total number of rounds submitted, in first-seen order. Rows without a
/// user are skipped (nothing to attribute them to). Reviewers who are still
/// requested but never submitted a review are appended as `pending` with
/// zero rounds.
fn dedupe_reviewers(reviews: Vec<RawReview>, requested_reviewers: Option<&Vec<RawUser>>) -> Vec<Reviewer> {
    let mut reviewers: Vec<Reviewer> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for review in reviews {
        let Some(login) = review.user.map(|u| u.login) else {
            continue;
        };
        if seen.insert(login.clone()) {
            reviewers.push(Reviewer {
                login,
                state: review.state.to_lowercase(),
                rounds: 1,
            });
        } else if let Some(existing) = reviewers.iter_mut().find(|r| r.login == login) {
            // Later round supersedes the earlier one (e.g. CHANGES_REQUESTED
            // then APPROVED) — the pane should show the current position.
            existing.state = review.state.to_lowercase();
            existing.rounds += 1;
        }
    }
    for requested in requested_reviewers.into_iter().flatten() {
        if !reviewers.iter().any(|r| r.login == requested.login) {
            reviewers.push(Reviewer {
                login: requested.login.clone(),
                state: "pending".to_string(),
                rounds: 0,
            });
        }
    }
    reviewers
}

#[derive(Debug, Deserialize)]
struct RawCheckRuns {
    check_runs: Vec<RawCheckRun>,
}

#[derive(Debug, Deserialize)]
struct RawCheckRun {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawComment {
    id: u64,
    user: Option<RawUser>,
    body: String,
    created_at: String,
    #[serde(default)]
    path: Option<String>,
}

impl RawComment {
    fn into_comment(self) -> Comment {
        Comment {
            id: self.id,
            author: self.user.map(|u| u.login).unwrap_or_default(),
            body: self.body,
            created_at: self.created_at,
            path: self.path,
        }
    }
}

// ---- GraphQL shapes for `pr_comments` and `resolve_thread` --------------
// octocrab's `graphql` strips GitHub's outer `{ "data": ... }` envelope, so
// these match the query's aliases directly, like `SearchEnvelope` below.

#[derive(Debug, Deserialize)]
struct RawConversationEnvelope {
    repository: RawRepository,
}

#[derive(Debug, Deserialize)]
struct RawRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<RawPullRequestThreads>,
}

#[derive(Debug, Deserialize)]
struct RawPullRequestThreads {
    #[serde(rename = "reviewThreads")]
    review_threads: RawReviewThreads,
}

#[derive(Debug, Deserialize)]
struct RawReviewThreads {
    nodes: Vec<RawReviewThread>,
}

#[derive(Debug, Deserialize)]
struct RawReviewThread {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    #[serde(default)]
    path: Option<String>,
    comments: RawThreadComments,
}

impl RawReviewThread {
    fn into_thread(self) -> ReviewThread {
        ReviewThread {
            id: self.id,
            path: self.path,
            resolved: self.is_resolved,
            comments: self.comments.nodes.into_iter().map(|c| c.into_comment()).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawThreadComments {
    nodes: Vec<RawThreadComment>,
}

#[derive(Debug, Deserialize)]
struct RawThreadComment {
    #[serde(rename = "databaseId")]
    #[serde(default)]
    database_id: Option<u64>,
    author: Option<RawUser>,
    body: String,
    #[serde(rename = "createdAt")]
    created_at: String,
}

impl RawThreadComment {
    fn into_comment(self) -> Comment {
        Comment {
            // GraphQL exposes no thread comment id via REST-consistent paths,
            // but `databaseId` is the REST id — the value replies must use.
            id: self.database_id.unwrap_or_default(),
            author: self.author.map(|u| u.login).unwrap_or_default(),
            body: self.body,
            created_at: self.created_at,
            path: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawResolveEnvelope {
    #[serde(rename = "resolveReviewThread")]
    resolved_thread: Option<RawResolvedThread>,
    #[serde(rename = "unresolveReviewThread")]
    unresolved_thread: Option<RawResolvedThread>,
}

#[derive(Debug, Deserialize)]
struct RawResolvedThread {
    thread: RawResolvedState,
}

#[derive(Debug, Deserialize)]
struct RawResolvedState {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
}

#[derive(Debug, Deserialize)]
struct RawBranch {
    name: String,
}

// ---- GraphQL shapes for `list_pull_requests` --------------------------
// The poller's search structs live in `client.rs` and are private there;
// these are deliberately separate — each module's wire types match only the
// fields that module reads, and the poller gets richer fields than this in
// Part 2 (`ReadyToMerge`), so sharing would couple two independent queries.

/// The payload octocrab's `graphql` returns for the PR search query: it
/// already unwraps GitHub's outer `{ "data": ... }` envelope, so this type
/// must not carry a `data` field of its own.
#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    authored: SearchResult,
    #[serde(rename = "reviewRequested")]
    review_requested: SearchResult,
    assigned: SearchResult,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    nodes: Vec<SearchPrNode>,
}

#[derive(Debug, Deserialize)]
struct SearchPrNode {
    #[serde(rename = "__typename")]
    typename: Option<String>,
    number: u64,
    title: String,
    url: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    mergeable: Option<String>,
    /// The PR's head branch. Absent only if GitHub omits it, which it does
    /// not in practice for PullRequest nodes — hence `Option`, not a panic.
    #[serde(default, rename = "headRefName")]
    head_ref: Option<String>,
    author: Option<SearchAuthor>,
    repository: SearchRepository,
    commits: SearchCommits,
    /// Review threads can be absent before any review exists, so treat the
    /// whole block as optional rather than assume GitHub always returns it.
    #[serde(default, rename = "reviewThreads")]
    review_threads: Option<SearchReviewThreads>,
}

/// A PR's review threads, used to count threads still awaiting attention.
#[derive(Debug, Deserialize)]
struct SearchReviewThreads {
    nodes: Vec<SearchReviewThreadNode>,
}

#[derive(Debug, Deserialize)]
struct SearchReviewThreadNode {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
}

#[derive(Debug, Deserialize)]
struct SearchAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct SearchRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct SearchCommits {
    nodes: Vec<SearchCommitNode>,
}

#[derive(Debug, Deserialize)]
struct SearchCommitNode {
    commit: SearchCommit,
}

#[derive(Debug, Deserialize)]
struct SearchCommit {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<SearchStatusCheckRollup>,
}

#[derive(Debug, Deserialize)]
struct SearchStatusCheckRollup {
    state: String,
}

impl SearchPrNode {
    /// Maps one GraphQL search node into a [`PullRequestSummary`] carrying a
    /// single role; the merge step unions duplicates by `(repo, number)`.
    fn into_summary(self, account_id: &str) -> PullRequestSummary {
        let ci_status = self
            .commits
            .nodes
            .first()
            .and_then(|n| n.commit.status_check_rollup.as_ref())
            .map(|r| match r.state.as_str() {
                "SUCCESS" => CiStatus::Passing,
                "FAILURE" | "ERROR" => CiStatus::Failing,
                "PENDING" | "EXPECTED" => CiStatus::Pending,
                _ => CiStatus::None,
            })
            .unwrap_or(CiStatus::None);
        // Threads without `isResolved` set to true are still open; GitHub's
        // `first: 100` cap is generous enough that a PR with more open threads
        // than that is worth flagging at the cap anyway.
        let unresolved_threads = self
            .review_threads
            .as_ref()
            .map(|t| t.nodes.iter().filter(|n| !n.is_resolved).count() as u64)
            .unwrap_or(0);
        PullRequestSummary {
            account_id: account_id.to_string(),
            repo: self.repository.name_with_owner,
            number: self.number,
            title: self.title,
            url: self.url,
            author: self.author.map(|a| a.login).unwrap_or_default(),
            roles: Vec::new(),
            state: match self.state.as_str() {
                "OPEN" => PrState::Open,
                "MERGED" => PrState::Merged,
                // CLOSED is the only remaining GraphQL PullRequestState value.
                _ => PrState::Closed,
            },
            draft: self.is_draft,
            ci_status,
            review_decision: match self.review_decision.as_deref() {
                Some("APPROVED") => ReviewDecision::Approved,
                Some("CHANGES_REQUESTED") => ReviewDecision::ChangesRequested,
                Some("REVIEW_REQUIRED") => ReviewDecision::ReviewRequired,
                // `reviewDecision` is null before any review exists; that is
                // the same "nobody has decided anything" as NONE.
                _ => ReviewDecision::None,
            },
            unresolved_threads,
            // UNKNOWN (not yet computed) is the safe default: it must never
            // be conflated with a conflict.
            mergeable: match self.mergeable.as_deref() {
                Some("CONFLICTING") => Mergeability::Conflicted,
                Some("MERGEABLE") => Mergeability::Clean,
                _ => Mergeability::Unknown,
            },
            created_at: self.created_at,
            updated_at: self.updated_at,
            head_ref: self.head_ref,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicted_wins_over_every_other_signal() {
        assert_eq!(
            mergeability_of(&Some(false), Some("blocked")),
            Mergeability::Conflicted
        );
        assert_eq!(
            mergeability_of(&None, Some("dirty")),
            Mergeability::Conflicted
        );
    }

    #[test]
    fn blocked_is_distinguished_from_clean() {
        assert_eq!(
            mergeability_of(&Some(true), Some("blocked")),
            Mergeability::Blocked
        );
        assert_eq!(
            mergeability_of(&Some(true), Some("clean")),
            Mergeability::Clean
        );
    }

    #[test]
    fn search_envelope_parses_unwrapped_data() {
        // octocrab's `graphql` strips GitHub's outer `{ "data": ... }` before
        // deserializing, so `SearchEnvelope` must parse the payload directly.
        // This embeds a slice of a real response: camelCase keys, a null
        // `reviewDecision` (no reviews yet), and a null `mergeable` (still
        // being computed) — any of which previously failed the whole query.
        let payload = serde_json::json!({
            "authored": {
                "nodes": [{
                    "__typename": "PullRequest",
                    "number": 13,
                    "title": "t",
                    "url": "u",
                    "createdAt": "2026-08-13T12:00:00Z",
                    "updatedAt": "2026-08-13T12:00:00Z",
                    "state": "OPEN",
                    "isDraft": false,
                    "reviewDecision": null,
                    "mergeable": null,
                    "author": { "login": "me" },
                    "repository": { "nameWithOwner": "acme/api" },
                    "commits": { "nodes": [{ "commit": { "statusCheckRollup": { "state": "SUCCESS" } } }] },
                    "reviewThreads": {
                        "nodes": [
                            { "isResolved": false },
                            { "isResolved": true }
                        ]
                    }
                }]
            },
            "reviewRequested": { "nodes": [] },
            "assigned": { "nodes": [] }
        });
        let envelope: SearchEnvelope =
            serde_json::from_value(payload).expect("unwrapped search payload parses");
        assert_eq!(envelope.authored.nodes[0].review_threads.as_ref().unwrap().nodes.len(), 2);
    }

    #[test]
    fn null_mergeable_is_unknown_not_an_error() {
        // GitHub computes mergeability asynchronously, so a fresh PR reports
        // null for a moment. Treating that as "cannot merge" would flash a
        // false blocker every time the pane opens.
        assert_eq!(mergeability_of(&None, None), Mergeability::Unknown);
        assert_eq!(
            mergeability_of(&None, Some("unknown")),
            Mergeability::Unknown
        );
    }

    fn search_node(repo: &str, number: u64, state: &str) -> SearchPrNode {
        SearchPrNode {
            typename: Some("PullRequest".into()),
            number,
            title: format!("PR {number}"),
            url: format!("https://github.com/{repo}/pull/{number}"),
            created_at: "2026-08-13T12:00:00Z".into(),
            updated_at: "2026-08-13T12:00:00Z".into(),
            state: state.into(),
            is_draft: false,
            review_decision: Some("APPROVED".into()),
            mergeable: Some("MERGEABLE".into()),
            head_ref: Some(format!("feature/{number}")),
            author: Some(SearchAuthor { login: "octocat".into() }),
            repository: SearchRepository {
                name_with_owner: repo.into(),
            },
            commits: SearchCommits { nodes: vec![] },
            review_threads: Some(SearchReviewThreads { nodes: vec![] }),
        }
    }

    #[test]
    fn unresolved_threads_counts_only_unresolved_threads() {
        // AC: the row badge must report open threads, not every thread.
        let mut node = search_node("acme/api", 1, "OPEN");
        node.review_threads = Some(SearchReviewThreads {
            nodes: vec![
                SearchReviewThreadNode { is_resolved: false },
                SearchReviewThreadNode { is_resolved: false },
                SearchReviewThreadNode { is_resolved: true },
            ],
        });
        let summary = node.into_summary("acc-1");
        assert_eq!(summary.unresolved_threads, 2);
    }

    #[test]
    fn no_review_threads_is_zero_unresolved() {
        let summary = search_node("acme/api", 1, "OPEN").into_summary("acc-1");
        assert_eq!(summary.unresolved_threads, 0);
    }

    #[test]
    fn a_pr_in_two_result_sets_is_one_summary_with_two_roles() {
        // AC-2.1: the whole point of `roles` being a set. A self-assigned PR
        // you authored must be one row carrying both badges, never two rows.
        let authored = vec![search_node("acme/api", 1, "OPEN")];
        let assigned = vec![search_node("acme/api", 1, "OPEN")];
        let summaries = merged_summaries(
            "acc-1",
            [
                (authored, PrRole::Authored),
                (assigned, PrRole::Assigned),
            ],
            Some(PrState::Open),
        );
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.roles.len(), 2);
        assert!(summary.roles.contains(&PrRole::Authored));
        assert!(summary.roles.contains(&PrRole::Assigned));
    }

    #[test]
    fn reviewer_appears_once_with_latest_state_and_round_count() {
        // `GET /pulls/{number}/reviews` returns one row per review round, so
        // a reviewer who reviewed twice must collapse to a single row whose
        // state is that of the latest (last) round and whose round count is 2.
        let reviews = vec![
            RawReview {
                user: Some(RawUser { login: "dave".into() }),
                state: "CHANGES_REQUESTED".into(),
            },
            RawReview {
                user: Some(RawUser { login: "erin".into() }),
                state: "COMMENTED".into(),
            },
            RawReview {
                user: Some(RawUser { login: "dave".into() }),
                state: "APPROVED".into(),
            },
        ];
        let reviewers = dedupe_reviewers(reviews, None);
        assert_eq!(reviewers.len(), 2);
        assert_eq!(reviewers[0].login, "dave");
        assert_eq!(reviewers[0].state, "approved");
        assert_eq!(reviewers[0].rounds, 2);
        assert_eq!(reviewers[1].login, "erin");
        assert_eq!(reviewers[1].state, "commented");
        assert_eq!(reviewers[1].rounds, 1);
    }

    #[test]
    fn requested_reviewers_without_a_review_are_pending() {
        let reviews = vec![RawReview {
            user: Some(RawUser { login: "dave".into() }),
            state: "APPROVED".into(),
        }];
        let requested = vec![
            RawUser { login: "dave".into() },
            RawUser { login: "erin".into() },
        ];
        let reviewers = dedupe_reviewers(reviews, Some(&requested));
        // dave already reviewed; only erin is appended, as pending.
        assert_eq!(reviewers.len(), 2);
        assert_eq!(reviewers[0].login, "dave");
        assert_eq!(reviewers[0].state, "approved");
        assert_eq!(reviewers[0].rounds, 1);
        assert_eq!(reviewers[1].login, "erin");
        assert_eq!(reviewers[1].state, "pending");
        assert_eq!(reviewers[1].rounds, 0);
    }

    #[test]
    fn review_rows_without_a_user_are_skipped() {
        let reviews = vec![
            RawReview { user: None, state: "APPROVED".into() },
            RawReview {
                user: Some(RawUser { login: "dave".into() }),
                state: "APPROVED".into(),
            },
        ];
        let reviewers = dedupe_reviewers(reviews, None);
        assert_eq!(reviewers.len(), 1);
        assert_eq!(reviewers[0].login, "dave");
        assert_eq!(reviewers[0].rounds, 1);
    }

    #[test]
    fn roles_is_never_empty() {
        // AC-2.3: every included row must explain *why* it is in the list.
        let summaries = merged_summaries(
            "acc-1",
            [(vec![search_node("acme/api", 7, "OPEN")], PrRole::Authored)],
            Some(PrState::Open),
        );
        assert!(!summaries[0].roles.is_empty());
    }

    #[test]
    fn distinct_prs_are_not_merged() {
        let summaries = merged_summaries(
            "acc-1",
            [
                (vec![search_node("acme/api", 1, "OPEN")], PrRole::Authored),
                (vec![search_node("acme/api", 2, "OPEN")], PrRole::Assigned),
            ],
            Some(PrState::Open),
        );
        assert_eq!(summaries.len(), 2);
    }

    #[test]
    fn state_filter_uses_the_nodes_authoritative_state() {
        // The qualifier alone can't separate closed-from merged (search
        // indexes merged PRs as closed); the node-level gate must.
        let summaries = merged_summaries(
            "acc-1",
            [
                (
                    vec![search_node("acme/api", 1, "OPEN"), search_node("acme/api", 2, "MERGED")],
                    PrRole::Authored,
                ),
                (
                    vec![search_node("acme/api", 3, "CLOSED")],
                    PrRole::ReviewRequested,
                ),
            ],
            Some(PrState::Closed),
        );
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].number, 3);
    }

    #[test]
    fn non_pull_request_nodes_are_skipped() {
        // `is:pr` should keep issue nodes out, but a bad response must not
        // abort the whole view.
        let mut issue = search_node("acme/api", 4, "OPEN");
        issue.typename = Some("Issue".into());
        let summaries = merged_summaries(
            "acc-1",
            [(vec![issue], PrRole::Assigned)],
            Some(PrState::Open),
        );
        assert!(summaries.is_empty());
    }

    #[test]
    fn conversation_envelope_parses_threads_with_comments() {
        // Slice of a real `reviewThreads` response: `databaseId` is the REST
        // comment id replies must use; `path` is null for general threads.
        let payload = serde_json::json!({
            "repository": {
                "pullRequest": {
                    "reviewThreads": {
                        "nodes": [{
                            "id": "PRR_kwLOAbc123",
                            "isResolved": false,
                            "path": "src/api.rs",
                            "comments": {
                                "nodes": [{
                                    "databaseId": 9001,
                                    "author": { "login": "carol" },
                                    "body": "Nits on line 5",
                                    "createdAt": "2026-08-13T12:00:00Z"
                                }]
                            }
                        }]
                    }
                }
            }
        });
        let envelope: RawConversationEnvelope =
            serde_json::from_value(payload).expect("unwrapped conversation payload parses");
        let threads = envelope
            .repository
            .pull_request
            .unwrap()
            .review_threads
            .nodes;
        assert_eq!(threads.len(), 1);
        let thread = threads.into_iter().next().unwrap().into_thread();
        assert_eq!(thread.id, "PRR_kwLOAbc123");
        assert_eq!(thread.resolved, false);
        assert_eq!(thread.path.as_deref(), Some("src/api.rs"));
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(thread.comments[0].id, 9001);
        assert_eq!(thread.comments[0].author, "carol");
        assert_eq!(thread.comments[0].body, "Nits on line 5");
    }

    #[test]
    fn conversation_envelope_handles_missing_pull_request() {
        // A bad number or a deleted PR leaves `pullRequest` null; the
        // conversation must degrade to whatever issue comments exist.
        let payload = serde_json::json!({
            "repository": { "pullRequest": null }
        });
        let envelope: RawConversationEnvelope =
            serde_json::from_value(payload).expect("null pull request parses");
        let threads = envelope.repository.pull_request;
        assert!(threads.is_none());
    }

    #[test]
    fn resolve_envelope_parses_the_present_mutation() {
        // Only one of the two aliased fields exists per request, so both must
        // be optional and the caller picks whichever is present.
        let payload = serde_json::json!({
            "resolveReviewThread": {
                "thread": { "isResolved": true }
            }
        });
        let envelope: RawResolveEnvelope =
            serde_json::from_value(payload).expect("resolve payload parses");
        let resolved = envelope
            .resolved_thread
            .or(envelope.unresolved_thread)
            .map(|t| t.thread.is_resolved)
            .unwrap_or(false);
        assert!(resolved);
    }

    /// Guards the GraphQL-injection fix. Both queries once interpolated
    /// caller-supplied values into the query text, so a `"` could close the
    /// string literal and append a second operation that ran under the user's
    /// token. The queries must be constants with values passed as variables.
    ///
    /// Asserting on the built request bodies rather than on source text: the
    /// body is what actually reaches GitHub.
    #[test]
    fn caller_supplied_values_travel_as_graphql_variables() {
        let hostile = r#"x") { evil } injected: unresolveReviewThread(input: {threadId: "y"#;

        let resolve = resolve_thread_request(hostile, true);
        let query = resolve["query"].as_str().expect("query is a string");
        assert!(
            !query.contains(hostile),
            "hostile thread id must not appear in the query text"
        );
        assert_eq!(
            resolve["variables"]["threadId"], hostile,
            "the value belongs in variables, verbatim"
        );

        let hostile_owner = r#"ow"ner"#;
        let conversation = conversation_request(hostile_owner, "name", 1);
        let query = conversation["query"].as_str().expect("query is a string");
        assert!(
            !query.contains(hostile_owner),
            "hostile owner must not appear in the query text"
        );
        assert_eq!(conversation["variables"]["owner"], hostile_owner);
    }

}

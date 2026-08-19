//! GitHub API client for one account. Two transports, chosen deliberately:
//!
//! - The `/notifications` REST endpoint is fetched with a plain [`reqwest`]
//!   client because we need raw control over the `If-None-Match` request
//!   header and the `ETag`/`X-Poll-Interval` response headers
//!   (`specs/github-integration.md`, "Rate-limit strategy") — octocrab's
//!   typed REST methods don't expose that.
//! - Review-requested/assigned items use one GraphQL query via `octocrab`,
//!   batching what would otherwise be several REST calls.

use std::collections::{BTreeSet, HashSet};

use gitsurveil_proto::{ActionItem, CiStatus, ItemKind};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::error::{DaemonError, Result};

/// Outcome of a conditional GET against `/notifications`.
pub enum NotificationsPoll {
    /// Server returned 304 — nothing changed, and the request cost no rate
    /// limit quota.
    NotModified,
    /// Fresh data, plus the `ETag` to store for next time and the
    /// `X-Poll-Interval` GitHub asked us to respect.
    Modified {
        items: Vec<ActionItem>,
        etag: Option<String>,
        poll_interval_secs: Option<u64>,
    },
}

/// A repository as GitHub's REST API reports it — the raw discovery data the
/// store flattens into `repositories` rows (`specs/desktop-ui.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRepo {
    /// The owning organization or user login.
    pub owner: String,
    /// The repository name, without the owner.
    pub name: String,
    /// Browser URL.
    pub url: String,
    /// The repository's description, if it has one.
    pub description: Option<String>,
    /// Whether the repository is private.
    pub private: bool,
    /// The default branch name.
    pub default_branch: String,
    /// HTTPS clone URL, used by the clone engine.
    pub clone_url: String,
}

impl DiscoveredRepo {
    /// `"owner/name"`, the identifier the rest of the API uses.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

/// A GitHub API client scoped to one account (one host, one token).
pub struct GitHubClient {
    /// Account id this client was built for; stamped onto every result row
    /// so multi-account lists can't be attributed to the wrong account.
    pub(crate) account_id: String,
    api_base: String,
    http: reqwest::Client,
    /// `octocrab`'s GraphQL transport, shared by the poller's search and the
    /// PR view's `list_pull_requests`.
    pub(crate) octocrab: octocrab::Octocrab,
}

impl GitHubClient {
    /// Builds a client for `account_id` against `api_base`
    /// (`https://api.github.com` for Cloud, `https://<host>/api/v3` for
    /// Enterprise) authenticating with `token`.
    pub fn new(account_id: &str, api_base: &str, token: &str) -> Result<GitHubClient> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| DaemonError::Config(e.to_string()))?,
        );
        headers.insert(USER_AGENT, HeaderValue::from_static("gitsurveil"));
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        let octocrab = octocrab::Octocrab::builder()
            .base_uri(api_base)
            .map_err(|e| DaemonError::Config(e.to_string()))?
            .personal_token(token.to_string())
            .build()
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        Ok(GitHubClient {
            account_id: account_id.to_string(),
            api_base: api_base.trim_end_matches('/').to_string(),
            http,
            octocrab,
        })
    }

    /// Sends a request to `path` (relative to the API base) and decodes the
    /// JSON response.
    ///
    /// GitHub's error bodies carry the useful part of a failure ("Validation
    /// Failed", which scope is missing), so non-2xx responses surface that
    /// message rather than a bare status code — it is what the UI shows the
    /// user when an action is rejected.
    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut req = self
            .http
            .request(method, format!("{}{}", self.api_base, path));
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        if !status.is_success() {
            let detail = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_else(|| text.clone());
            return Err(DaemonError::GitHubApi(format!("GitHub {status}: {detail}")));
        }

        // A 204 (or any empty body) is a success with nothing to decode;
        // `null` deserializes into `serde_json::Value` and `()` alike.
        let text = if text.trim().is_empty() { "null".to_string() } else { text };
        serde_json::from_str(&text).map_err(|e| DaemonError::Config(e.to_string()))
    }

    /// `GET path`, decoded as JSON.
    pub(crate) async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request_json(reqwest::Method::GET, path, None).await
    }

    /// Core requests remaining in the current rate-limit window, from
    /// `GET /rate_limit`. Discovery checks this before a full catalog pass so
    /// the background cycle never crowds out the poller's quota
    /// (`specs/github-integration.md`, "Rate-limit strategy").
    pub async fn core_remaining(&self) -> Result<u64> {
        #[derive(Deserialize)]
        struct RateLimitEnvelope {
            resources: Resources,
        }
        #[derive(Deserialize)]
        struct Resources {
            core: Core,
        }
        #[derive(Deserialize)]
        struct Core {
            remaining: u64,
        }
        let envelope: RateLimitEnvelope = self.get_json("/rate_limit").await?;
        Ok(envelope.resources.core.remaining)
    }

    /// `POST path` with a JSON body.
    pub(crate) async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        self.request_json(reqwest::Method::POST, path, Some(body))
            .await
    }

    /// `PATCH path` with a JSON body.
    pub(crate) async fn patch_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        self.request_json(reqwest::Method::PATCH, path, Some(body))
            .await
    }

    /// `PUT path` with a JSON body.
    pub(crate) async fn put_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T> {
        self.request_json(reqwest::Method::PUT, path, Some(body))
            .await
    }

    /// Validates the token and returns the authenticated login, per
    /// `specs/github-integration.md`'s account-setup validation.
    pub async fn validate(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Me {
            login: String,
        }
        let me: Me = self
            .http
            .get(format!("{}/user", self.api_base))
            .send()
            .await
            .map_err(|e| DaemonError::Config(e.to_string()))?
            .error_for_status()
            .map_err(|e| DaemonError::Config(e.to_string()))?
            .json()
            .await
            .map_err(|e| DaemonError::Config(e.to_string()))?;
        Ok(me.login)
    }

    /// Conditionally fetches `/notifications`, sending `prev_etag` as
    /// `If-None-Match` when we have one. A `304` short-circuits before any
    /// JSON parsing and is free against GitHub's rate limit.
    pub async fn poll_notifications(&self, prev_etag: Option<&str>) -> Result<NotificationsPoll> {
        let mut req = self
            .http
            .get(format!("{}/notifications", self.api_base))
            .query(&[("all", "false"), ("participating", "false")]);
        if let Some(etag) = prev_etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, etag);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        if resp.status() == StatusCode::NOT_MODIFIED {
            return Ok(NotificationsPoll::NotModified);
        }
        let resp = resp
            .error_for_status()
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let poll_interval_secs = resp
            .headers()
            .get("x-poll-interval")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        let raw: Vec<RawNotification> = resp
            .json()
            .await
            .map_err(|e| DaemonError::Config(e.to_string()))?;

        let items = raw
            .into_iter()
            .map(|n| n.into_action_item(&self.account_id))
            .collect();

        Ok(NotificationsPoll::Modified {
            items,
            etag,
            poll_interval_secs,
        })
    }

    /// Fetches every repository the account can see — those it owns and those
    /// it collaborates on — across all pages, for the Repositories pane.
    ///
    /// This is a catalog feed, not a notification poll: it is only as fresh
    /// as the last discovery pass, and it intentionally skips forks and
    /// archived repos GitHub omits from `owner,collaborator` affiliation by
    /// default.
    pub async fn list_repos(&self) -> Result<Vec<DiscoveredRepo>> {
        const PER_PAGE: usize = 100;
        const MAX_PAGES: usize = 100;
        let mut repos = Vec::new();
        for page in 1..=MAX_PAGES {
            let batch: Vec<RawRepo> = self
                .get_json(&format!(
                    "/user/repos?affiliation=owner,collaborator&per_page={PER_PAGE}&page={page}"
                ))
                .await?;
            let len = batch.len();
            repos.extend(batch.into_iter().map(Into::into));
            // A short page means the last one; GitHub never returns more than
            // `per_page` entries. The page ceiling is a defensive bound.
            if len < PER_PAGE {
                break;
            }
        }
        Ok(repos)
    }

    /// Runs the combined "review requested" + "assigned" + "ready to merge"
    /// GraphQL search (`specs/github-integration.md`) in a single request.
    ///
    /// The `authored:` alias exists for one reason: to notice when the user's
    /// own PR needs attention — a comment from someone else, an unresolved
    /// review thread, or a failed CI check. A merely-open authored PR produces
    /// no item here (`specs/priority-engine.md`); those belong in the Pull
    /// Requests view, not the inbox. The `reviewedByMe:` alias likewise only
    /// produces an item while the user has an unanswered reply in a review
    /// thread they commented in.
    ///
    /// The key sets let the poller dedupe `/notifications` feed items: a
    /// review-request notification is redundant when the PR already shows up
    /// in `review_requested_keys` (search wins) and stale when it shows up in
    /// `reviewed_by_me_keys` (the user already reviewed — GitHub drops a PR
    /// from `review-requested:@me` the moment they do).
    ///
    /// **Node budget**: GitHub caps a search-based query at 500,000 possible
    /// nodes, counted from `first`/`last` values (worst case), not actual
    /// data. Per `authored`/`reviewedByMe` PR: 1 (PR) + 100 (`comments`) +
    /// 100 threads × (1 + 20 thread comments) = 2,201. At `first: 50` that's
    /// 110,050 per alias, 220,100 across both; the two plain aliases add
    /// ~200, for ≈220,300 total. The nested `reviewThreads × comments`
    /// product dominates, so any growth there (a field on every thread, a
    /// larger `last`) must be paid for by shrinking the other sizes.
    pub async fn fetch_search_items(&self, login: &str) -> Result<SearchSnapshot> {
        const QUERY: &str = r#"
            query {
              reviewRequested: search(query: "is:open is:pr review-requested:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
              }
              assigned: search(query: "is:open is:pr assignee:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
              }
              authored: search(query: "is:open is:pr author:@me", type: ISSUE, first: 50) {
                nodes { ...prActivityFields }
              }
              reviewedByMe: search(query: "is:open is:pr reviewed-by:@me", type: ISSUE, first: 50) {
                nodes { ...prActivityFields }
              }
            }
            fragment prFields on PullRequest {
              number
              title
              url
              createdAt
              updatedAt
              isDraft
              reviewDecision
              mergeable
              author { login }
              repository { nameWithOwner }
              commits(last: 1) {
                nodes { commit { statusCheckRollup { state } } }
              }
            }
            fragment prActivityFields on PullRequest {
              ...prFields
              # `last:` (not `first:`) so newly-appended activity is always
              # visible — `first:` would hide new comments behind the window's
              # oldest entries. Sized per the node budget in
              # `fetch_search_items`: the nested reviewThreads × comments
              # product dominates the query cost, so thread comments are kept
              # small (20).
              comments(last: 100) {
                nodes { databaseId author { login } createdAt }
              }
              reviewThreads(first: 100) {
                nodes {
                  id
                  isResolved
                  comments(last: 20) { nodes { databaseId author { login } createdAt } }
                }
              }
            }
        "#;

        let body = serde_json::json!({ "query": QUERY });
        let resp: GraphQlEnvelope = self
            .octocrab
            .graphql(&body)
            .await
            .map_err(DaemonError::GitHub)?;

        let mut items = Vec::new();
        let mut review_requested_keys = HashSet::new();
        let mut reviewed_by_me_keys = HashSet::new();
        for node in resp.review_requested.nodes {
            review_requested_keys.insert((node.repository.name_with_owner.clone(), node.number));
            items.push(node.into_action_item(&self.account_id, ItemKind::ReviewRequested));
        }
        for node in resp.assigned.nodes {
            items.push(node.into_action_item(&self.account_id, ItemKind::Assigned));
        }
        for node in resp.authored.nodes {
            if let Some(item) = node.clone().into_ready_to_merge_item(&self.account_id) {
                items.push(item);
            }
            if let Some(item) = node.into_authored_item(&self.account_id, login) {
                items.push(item);
            }
        }
        for node in resp.reviewed_by_me.nodes {
            reviewed_by_me_keys.insert((node.repository.name_with_owner.clone(), node.number));
            if let Some(item) = node.into_reviewed_by_me_item(&self.account_id, login) {
                items.push(item);
            }
        }
        Ok(SearchSnapshot {
            items,
            review_requested_keys,
            reviewed_by_me_keys,
        })
    }
}

/// The poller query's outcome: the items to diff, plus the `(repo, number)`
/// key sets of the search results behind the `ReviewRequested` and
/// `ReviewedByMe` item kinds. The key sets are what the `/notifications` feed
/// is deduped against, so they must include *every* searched PR — not just the
/// ones that produced an item (a reviewed PR with no unanswered replies still
/// proves a lingering review-request notification is stale).
#[derive(Debug, Clone, Default)]
pub struct SearchSnapshot {
    /// Items to feed into the diff.
    pub items: Vec<ActionItem>,
    /// `(repo, number)` of every PR in the `review-requested:@me` search.
    pub review_requested_keys: HashSet<(String, u64)>,
    /// `(repo, number)` of every PR in the `reviewed-by:@me` search.
    pub reviewed_by_me_keys: HashSet<(String, u64)>,
}

/// Shape of one entry from `GET /user/repos`
/// (<https://docs.github.com/en/rest/repos/repos#list-repositories-for-the-authenticated-user>).
#[derive(Debug, Clone, Deserialize)]
struct RawRepo {
    #[serde(rename = "full_name")]
    full_name: String,
    name: String,
    #[serde(rename = "html_url")]
    html_url: String,
    description: Option<String>,
    private: bool,
    #[serde(rename = "default_branch")]
    default_branch: String,
    #[serde(rename = "clone_url")]
    clone_url: String,
    owner: RawOwner,
}

#[derive(Debug, Clone, Deserialize)]
struct RawOwner {
    login: String,
}

impl From<RawRepo> for DiscoveredRepo {
    fn from(raw: RawRepo) -> Self {
        let (owner, name) = raw
            .full_name
            .split_once('/')
            .map(|(o, n)| (o.to_string(), n.to_string()))
            .unwrap_or_else(|| (raw.owner.login.clone(), raw.name));
        DiscoveredRepo {
            owner,
            name,
            url: raw.html_url,
            description: raw.description,
            private: raw.private,
            default_branch: raw.default_branch,
            clone_url: raw.clone_url,
        }
    }
}

/// Shape of one entry from `GET /notifications`
/// (<https://docs.github.com/en/rest/activity/notifications>).
#[derive(Debug, Clone, Deserialize)]
struct RawNotification {
    id: String,
    reason: String,
    updated_at: String,
    subject: RawSubject,
    repository: RawRepository,
}

#[derive(Debug, Clone, Deserialize)]
struct RawSubject {
    title: String,
    url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawRepository {
    full_name: String,
    html_url: String,
}

impl RawNotification {
    fn into_action_item(self, account_id: &str) -> ActionItem {
        let kind = classify_reason(&self.reason);
        // The notifications endpoint gives an API url for `subject.url`
        // (or none for some system notifications); fall back to the repo
        // page so every item is always clickable in the UI.
        let url = self.subject.url.unwrap_or_else(|| self.repository.html_url.clone());
        let now = crate::poller::now_rfc3339();
        ActionItem {
            id: format!("{account_id}:notif:{}", self.id),
            account_id: account_id.to_string(),
            kind,
            state: gitsurveil_proto::ItemState::Open,
            repo: self.repository.full_name,
            number: None,
            title: self.subject.title,
            url,
            author: String::new(),
            created_at: self.updated_at.clone(),
            updated_at: self.updated_at,
            first_seen_at: now.clone(),
            last_seen_at: now,
            ci_status: CiStatus::None,
            raw_kind: self.reason,
            dismissed_updated_at: None,
            dismissed_at: None,
            dismissed_ci_status: None,
            activity: None,
            archived: false,
        }
    }
}

/// Maps a GitHub notification `reason` to our [`ItemKind`]
/// (<https://docs.github.com/en/rest/activity/notifications#notification-reasons>).
fn classify_reason(reason: &str) -> ItemKind {
    match reason {
        "review_requested" => ItemKind::ReviewRequested,
        "mention" | "team_mention" => ItemKind::Mentioned,
        "assign" => ItemKind::Assigned,
        "state_change" => ItemKind::ReviewStateChanged,
        // `ci_activity` fires for both pass and fail; without a conclusion
        // field on this endpoint we treat it as CiFailed (the case that
        // matters for the tray) and let the GraphQL statusCheckRollup pass
        // correct it once that lands in a future poll.
        "ci_activity" => ItemKind::CiFailed,
        _ => ItemKind::Participating,
    }
}

/// The payload octocrab's `graphql` returns for the poller query: it already
/// unwraps GitHub's outer `{ "data": ... }` envelope, so this type must not
/// carry a `data` field of its own.
#[derive(Debug, Clone, Deserialize)]
struct GraphQlEnvelope {
    #[serde(rename = "reviewRequested")]
    review_requested: SearchResult,
    assigned: SearchResult,
    authored: SearchResult,
    #[serde(rename = "reviewedByMe")]
    reviewed_by_me: SearchResult,
}

#[derive(Debug, Clone, Deserialize)]
struct SearchResult {
    nodes: Vec<PrNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct PrNode {
    number: u64,
    title: String,
    url: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    mergeable: Option<String>,
    author: Option<PrAuthor>,
    repository: PrRepository,
    commits: PrCommits,
    /// Top-level issue comments. Only requested by the `authored` alias, so
    /// absent (`None`) for every other search result.
    #[serde(default)]
    comments: Option<PrComments>,
    /// Review comment threads. Only requested by the `authored` and
    /// `reviewedByMe` aliases; absent for the rest.
    #[serde(default, rename = "reviewThreads")]
    review_threads: Option<PrReviewThreads>,
}

#[derive(Debug, Clone, Deserialize)]
struct PrComments {
    nodes: Vec<PrCommentNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct PrCommentNode {
    #[serde(rename = "databaseId")]
    database_id: Option<u64>,
    author: Option<PrAuthor>,
    #[serde(rename = "createdAt")]
    created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PrReviewThreads {
    nodes: Vec<PrReviewThreadNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct PrReviewThreadNode {
    id: String,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    comments: PrComments,
}

#[derive(Debug, Clone, Deserialize)]
struct PrAuthor {
    login: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PrRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PrCommits {
    nodes: Vec<PrCommitNode>,
}

#[derive(Debug, Clone, Deserialize)]
struct PrCommitNode {
    commit: PrCommit,
}

#[derive(Debug, Clone, Deserialize)]
struct PrCommit {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<StatusCheckRollup>,
}

#[derive(Debug, Clone, Deserialize)]
struct StatusCheckRollup {
    state: String,
}

impl PrNode {
    /// Aggregate CI status from the latest commit's check-rollup, or
    /// [`CiStatus::None`] when GitHub reports none.
    fn ci_status(&self) -> CiStatus {
        self.commits
            .nodes
            .first()
            .and_then(|n| n.commit.status_check_rollup.as_ref())
            .map(|r| match r.state.as_str() {
                "SUCCESS" => CiStatus::Passing,
                "FAILURE" | "ERROR" => CiStatus::Failing,
                "PENDING" | "EXPECTED" => CiStatus::Pending,
                _ => CiStatus::None,
            })
            .unwrap_or(CiStatus::None)
    }

    /// Whether this authored PR has crossed into `ReadyToMerge`: approved by
    /// review, mergeable, not a draft, and CI not failing. The one moment an
    /// authored PR needs its owner. Every other combination produces
    /// nothing — those PRs belong to the view, not the inbox.
    fn is_ready_to_merge(&self) -> bool {
        self.review_decision.as_deref() == Some("APPROVED")
            && self.mergeable.as_deref() == Some("MERGEABLE")
            && !self.is_draft
            && self.ci_status() != CiStatus::Failing
    }

    fn into_action_item(self, account_id: &str, kind: ItemKind) -> ActionItem {
        let ci_status = self.ci_status();
        let now = crate::poller::now_rfc3339();
        ActionItem {
            id: format!(
                "{account_id}:{kind:?}:{}#{}",
                self.repository.name_with_owner, self.number
            ),
            account_id: account_id.to_string(),
            kind,
            state: gitsurveil_proto::ItemState::Open,
            repo: self.repository.name_with_owner,
            number: Some(self.number),
            title: self.title,
            url: self.url,
            author: self.author.map(|a| a.login).unwrap_or_default(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            first_seen_at: now.clone(),
            last_seen_at: now,
            ci_status,
            raw_kind: format!("{kind:?}"),
            dismissed_updated_at: None,
            dismissed_at: None,
            dismissed_ci_status: None,
            activity: None,
            archived: false,
        }
    }

    /// Turns this authored node into a `ReadyToMerge` item, or `None` when the
    /// PR hasn't crossed the threshold. `None` is the whole point: a merely
    /// open authored PR must never notify its owner.
    fn into_ready_to_merge_item(self, account_id: &str) -> Option<ActionItem> {
        if !self.is_ready_to_merge() {
            return None;
        }
        Some(self.into_action_item(account_id, ItemKind::ReadyToMerge))
    }

    /// Whether this authored PR needs its owner: a comment from someone else,
    /// an unresolved review thread, or a failing CI check. Commit-only and
    /// own-comment activity never qualifies.
    fn needs_attention(&self, login: &str) -> bool {
        self.ci_status() == CiStatus::Failing
            || self.has_comment_from_other(login)
            || self.has_unresolved_thread()
    }

    /// True when any top-level or review-thread comment was written by someone
    /// other than the account user.
    fn has_comment_from_other(&self, login: &str) -> bool {
        self.comments
            .iter()
            .flat_map(|c| c.nodes.iter())
            .chain(
                self.review_threads
                    .iter()
                    .flat_map(|t| t.nodes.iter())
                    .flat_map(|t| t.comments.nodes.iter()),
            )
            .any(|c| c.author.as_ref().map(|a| a.login.as_str()) != Some(login))
    }

    /// True when any review thread on the PR is currently unresolved.
    fn has_unresolved_thread(&self) -> bool {
        self.review_threads
            .as_ref()
            .map(|t| t.nodes.iter().any(|n| !n.is_resolved))
            .unwrap_or(false)
    }

    /// The newest timestamp of any *unanswered* reply to the user: a comment
    /// in a thread the user commented in that is newer than the user's own
    /// latest comment in that thread. `None` when the user has no unanswered
    /// reply anywhere on the PR (including when they have replied back — that
    /// resolves the thread and drops the item, `specs/priority-engine.md`).
    fn latest_unanswered_reply(&self, login: &str) -> Option<String> {
        let mut newest: Option<String> = None;
        for thread in &self.review_threads.as_ref()?.nodes {
            let comments = &thread.comments.nodes;
            let user_latest = comments
                .iter()
                .filter(|c| c.author.as_ref().map(|a| a.login.as_str()) == Some(login))
                .map(|c| c.created_at.as_str())
                .max();
            let other_latest = comments
                .iter()
                .filter(|c| c.author.as_ref().map(|a| a.login.as_str()) != Some(login))
                .map(|c| c.created_at.as_str())
                .max();
            // RFC 3339 timestamps compare lexicographically; `.max()` picks
            // the newest. An unanswered reply exists only if someone else
            // commented after the user's own last comment in that thread.
            if let (Some(user), Some(other)) = (user_latest, other_latest) {
                if other > user {
                    newest = Some(match newest {
                        Some(n) => n.max(other.to_string()),
                        None => other.to_string(),
                    });
                }
            }
        }
        newest
    }

    /// The `ReviewedByMe` item for this PR, or `None` while the user has no
    /// unanswered reply. `updated_at` is the newest unanswered-reply timestamp
    /// so the diff treats a fresh reply as an update (and a reply by the user
    /// as the item vanishing) instead of being moved by commits.
    fn into_reviewed_by_me_item(self, account_id: &str, login: &str) -> Option<ActionItem> {
        let updated_at = self.latest_unanswered_reply(login)?;
        let mut item = self.into_action_item(account_id, ItemKind::ReviewedByMe);
        item.updated_at = updated_at;
        Some(item)
    }

    /// The `Authored` item for this PR, or `None` when none of the attention
    /// signals hold. The `activity` fingerprint pins down *which* comments and
    /// unresolved threads qualify, so the notify gate can detect transitions
    /// (new comment, thread resolved→unresolved) instead of reacting to every
    /// `updated_at` change (commits included).
    fn into_authored_item(self, account_id: &str, login: &str) -> Option<ActionItem> {
        if !self.needs_attention(login) {
            return None;
        }
        let activity = self.authored_fingerprint(login);
        let mut item = self.into_action_item(account_id, ItemKind::Authored);
        item.activity = Some(activity);
        Some(item)
    }

    /// Stable, order-independent fingerprint of the qualifying activity on an
    /// authored PR: `c:<sorted other-comment ids>;u:<sorted unresolved thread
    /// ids>`. Sorting via `BTreeSet` keeps it stable across polls regardless
    /// of response ordering.
    fn authored_fingerprint(&self, login: &str) -> String {
        let comment_ids: BTreeSet<u64> = self
            .comments
            .iter()
            .flat_map(|c| c.nodes.iter())
            .chain(
                self.review_threads
                    .iter()
                    .flat_map(|t| t.nodes.iter())
                    .flat_map(|t| t.comments.nodes.iter()),
            )
            .filter(|c| c.author.as_ref().map(|a| a.login.as_str()) != Some(login))
            .filter_map(|c| c.database_id)
            .collect();
        let unresolved: BTreeSet<&str> = self
            .review_threads
            .iter()
            .flat_map(|t| t.nodes.iter())
            .filter(|n| !n.is_resolved)
            .map(|n| n.id.as_str())
            .collect();
        format!(
            "c:{};u:{}",
            comment_ids
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
            unresolved.iter().copied().collect::<Vec<_>>().join(","),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        review_decision: &str,
        mergeable: &str,
        is_draft: bool,
        ci_state: Option<&str>,
    ) -> PrNode {
        PrNode {
            number: 1,
            title: "t".into(),
            url: "u".into(),
            created_at: "2026-08-13T12:00:00Z".into(),
            updated_at: "2026-08-13T12:00:00Z".into(),
            is_draft,
            review_decision: Some(review_decision.into()),
            mergeable: Some(mergeable.into()),
            author: Some(PrAuthor { login: "me".into() }),
            repository: PrRepository {
                name_with_owner: "acme/api".into(),
            },
            commits: PrCommits {
                nodes: vec![PrCommitNode {
                    commit: PrCommit {
                        status_check_rollup: ci_state.map(|s| StatusCheckRollup {
                            state: s.into(),
                        }),
                    },
                }],
            },
            comments: None,
            review_threads: None,
        }
    }

    fn thread_node(
        id: &str,
        is_resolved: bool,
        comments: Vec<(&str, &str, Option<u64>)>,
    ) -> PrReviewThreadNode {
        PrReviewThreadNode {
            id: id.into(),
            is_resolved,
            comments: PrComments {
                nodes: comments
                    .into_iter()
                    .map(|(login, created_at, database_id)| PrCommentNode {
                        database_id,
                        author: Some(PrAuthor { login: login.into() }),
                        created_at: created_at.into(),
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn ready_to_merge_fires_only_when_all_four_hold() {
        // The full predicate: approved + mergeable + not draft + CI not failing.
        let ready = node("APPROVED", "MERGEABLE", false, Some("SUCCESS"));
        assert!(ready.is_ready_to_merge());
        assert!(ready.clone().into_ready_to_merge_item("acc").is_some());

        // Each near-miss alone must kill the transition (AC-5.1).
        assert!(!node("CHANGES_REQUESTED", "MERGEABLE", false, Some("SUCCESS")).is_ready_to_merge());
        assert!(!node("REVIEW_REQUIRED", "MERGEABLE", false, Some("SUCCESS")).is_ready_to_merge());
        assert!(!node("APPROVED", "CONFLICTING", false, Some("SUCCESS")).is_ready_to_merge());
        assert!(!node("APPROVED", "UNKNOWN", false, Some("SUCCESS")).is_ready_to_merge());
        assert!(!node("APPROVED", "MERGEABLE", true, Some("SUCCESS")).is_ready_to_merge());
        assert!(!node("APPROVED", "MERGEABLE", false, Some("FAILURE")).is_ready_to_merge());
        assert!(!node("APPROVED", "MERGEABLE", false, Some("ERROR")).is_ready_to_merge());
    }

    #[test]
    fn pending_or_missing_ci_still_counts_as_not_failing() {
        // "CI not failing" is the gate — pending is not failing. A PR with
        // checks still running is one a reviewer already approved.
        assert!(node("APPROVED", "MERGEABLE", false, Some("PENDING")).is_ready_to_merge());
        assert!(node("APPROVED", "MERGEABLE", false, None).is_ready_to_merge());
    }

    #[test]
    fn merely_open_authored_pr_produces_no_item() {
        // AC-5.4: opening a PR must not notify you about it.
        let open = node("NONE", "UNKNOWN", false, Some("PENDING"));
        assert!(!open.is_ready_to_merge());
        assert!(open.into_ready_to_merge_item("acc").is_none());
    }

    #[test]
    fn ready_to_merge_item_carries_stable_id_and_kind() {
        let item = node("APPROVED", "MERGEABLE", false, Some("SUCCESS"))
            .into_ready_to_merge_item("acc")
            .expect("predicate holds");
        assert_eq!(item.kind, ItemKind::ReadyToMerge);
        assert_eq!(item.id, "acc:ReadyToMerge:acme/api#1");
        assert_eq!(item.ci_status, CiStatus::Passing);
        assert_eq!(item.author, "me");
    }

    #[test]
    fn reviewed_by_me_item_exists_only_while_reply_is_unanswered() {
        let with_thread = |threads: Vec<PrReviewThreadNode>| PrNode {
            review_threads: Some(PrReviewThreads { nodes: threads }),
            ..node("APPROVED", "MERGEABLE", false, Some("SUCCESS"))
        };

        // Reviewer replies after the user's comment -> open reply.
        let replied = with_thread(vec![thread_node(
            "t1",
            false,
            vec![
                ("me", "2026-08-13T10:00:00Z", Some(1)),
                ("alice", "2026-08-13T11:00:00Z", Some(2)),
            ],
        )]);
        let item = replied
            .clone()
            .into_reviewed_by_me_item("acc", "me")
            .expect("unanswered reply produces an item");
        assert_eq!(item.kind, ItemKind::ReviewedByMe);
        // updated_at is the reply time, so only replies move the item.
        assert_eq!(item.updated_at, "2026-08-13T11:00:00Z");

        // User replies back -> no longer unanswered -> no item.
        let handled = with_thread(vec![thread_node(
            "t1",
            false,
            vec![
                ("me", "2026-08-13T10:00:00Z", Some(1)),
                ("alice", "2026-08-13T11:00:00Z", Some(2)),
                ("me", "2026-08-13T12:00:00Z", Some(3)),
            ],
        )]);
        assert!(handled.into_reviewed_by_me_item("acc", "me").is_none());

        // No reply at all (only the user's own comment) -> no item.
        let silent = with_thread(vec![thread_node(
            "t1",
            false,
            vec![("me", "2026-08-13T10:00:00Z", Some(1))],
        )]);
        assert!(silent.into_reviewed_by_me_item("acc", "me").is_none());

        // No threads at all (a review without comments) -> no item.
        assert!(with_thread(vec![])
            .into_reviewed_by_me_item("acc", "me")
            .is_none());

        // Newest of several open replies wins as updated_at.
        let two = with_thread(vec![
            thread_node(
                "t1",
                false,
                vec![
                    ("me", "2026-08-13T10:00:00Z", Some(1)),
                    ("alice", "2026-08-13T11:00:00Z", Some(2)),
                ],
            ),
            thread_node(
                "t2",
                false,
                vec![
                    ("me", "2026-08-13T09:00:00Z", Some(3)),
                    ("bob", "2026-08-13T12:00:00Z", Some(4)),
                ],
            ),
        ]);
        let item = two.into_reviewed_by_me_item("acc", "me").unwrap();
        assert_eq!(item.updated_at, "2026-08-13T12:00:00Z");
    }

    #[test]
    fn authored_item_exists_only_when_attention_is_needed() {
        let with = |comments: Option<PrComments>, threads: Option<PrReviewThreads>| PrNode {
            comments,
            review_threads: threads,
            ..node("NONE", "UNKNOWN", false, Some("SUCCESS"))
        };

        // Merely open: no comments, no threads, green CI -> no item.
        assert!(with(None, None).into_authored_item("acc", "me").is_none());

        // A comment from someone else -> item.
        let comment = |login: &str, id: u64| PrCommentNode {
            database_id: Some(id),
            author: Some(PrAuthor { login: login.into() }),
            created_at: "2026-08-13T10:00:00Z".into(),
        };
        let others_comment = with(
            Some(PrComments {
                nodes: vec![comment("alice", 10)],
            }),
            None,
        );
        let item = others_comment
            .clone()
            .into_authored_item("acc", "me")
            .expect("comment from someone else qualifies");
        assert_eq!(item.kind, ItemKind::Authored);
        assert_eq!(item.activity.as_deref(), Some("c:10;u:"));

        // Only the user's own comments -> no item.
        let own_comment = with(
            Some(PrComments {
                nodes: vec![comment("me", 11)],
            }),
            None,
        );
        assert!(own_comment.into_authored_item("acc", "me").is_none());

        // An unresolved thread qualifies even without comments by others.
        let unresolved = with(
            None,
            Some(PrReviewThreads {
                nodes: vec![thread_node("t1", false, vec![("alice", "2026-08-13T09:00:00Z", None)])],
            }),
        );
        assert!(unresolved.into_authored_item("acc", "me").is_some());

        // Failing CI alone qualifies.
        let failing = with(
            None,
            None,
        );
        let failing = PrNode {
            commits: PrCommits {
                nodes: vec![PrCommitNode {
                    commit: PrCommit {
                        status_check_rollup: Some(StatusCheckRollup { state: "FAILURE".into() }),
                    },
                }],
            },
            ..failing
        };
        assert!(failing.into_authored_item("acc", "me").is_some());
    }

    #[test]
    fn authored_fingerprint_is_stable_and_orders_independently() {
        let comment = |login: &str, id: u64| PrCommentNode {
            database_id: Some(id),
            author: Some(PrAuthor { login: login.into() }),
            created_at: "2026-08-13T10:00:00Z".into(),
        };
        let base = node("NONE", "UNKNOWN", false, Some("FAILURE"));
        let a = PrNode {
            comments: Some(PrComments {
                nodes: vec![comment("alice", 2), comment("bob", 1)],
            }),
            review_threads: Some(PrReviewThreads {
                nodes: vec![
                    thread_node("t1", false, vec![("alice", "2026-08-13T09:00:00Z", None)]),
                    thread_node("t2", true, vec![("bob", "2026-08-13T09:00:00Z", None)]),
                ],
            }),
            ..base.clone()
        };
        let b = PrNode {
            comments: Some(PrComments {
                nodes: vec![comment("bob", 1), comment("alice", 2)],
            }),
            review_threads: Some(PrReviewThreads {
                nodes: vec![
                    thread_node("t2", true, vec![("bob", "2026-08-13T09:00:00Z", None)]),
                    thread_node("t1", false, vec![("alice", "2026-08-13T09:00:00Z", None)]),
                ],
            }),
            ..base
        };
        // Same content, different ordering -> identical fingerprint.
        assert_eq!(a.authored_fingerprint("me"), b.authored_fingerprint("me"));
        assert_eq!(a.authored_fingerprint("me"), "c:1,2;u:t1");
    }
}

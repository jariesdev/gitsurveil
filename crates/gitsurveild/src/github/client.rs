//! GitHub API client for one account. Two transports, chosen deliberately:
//!
//! - The `/notifications` REST endpoint is fetched with a plain [`reqwest`]
//!   client because we need raw control over the `If-None-Match` request
//!   header and the `ETag`/`X-Poll-Interval` response headers
//!   (`specs/github-integration.md`, "Rate-limit strategy") — octocrab's
//!   typed REST methods don't expose that.
//! - Review-requested/assigned items use one GraphQL query via `octocrab`,
//!   batching what would otherwise be several REST calls.

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

    /// Runs the combined "review requested" + "assigned" + "ready to merge"
    /// GraphQL search (`specs/github-integration.md`) in a single request.
    ///
    /// The `authored:` alias exists for one reason: to notice when the user's
    /// own PR becomes mergeable. It is a *transition* detector, not an
    /// "everything I authored" feed — a merely-open authored PR produces no
    /// item here (`specs/priority-engine.md`), because those belong in the
    /// Pull Requests view, not the inbox.
    pub async fn fetch_search_items(&self) -> Result<Vec<ActionItem>> {
        const QUERY: &str = r#"
            query {
              reviewRequested: search(query: "is:open is:pr review-requested:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
              }
              assigned: search(query: "is:open is:pr assignee:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
              }
              authored: search(query: "is:open is:pr author:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
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
        "#;

        let body = serde_json::json!({ "query": QUERY });
        let resp: GraphQlEnvelope = self
            .octocrab
            .graphql(&body)
            .await
            .map_err(DaemonError::GitHub)?;

        let mut items = Vec::new();
        for node in resp.review_requested.nodes {
            items.push(node.into_action_item(&self.account_id, ItemKind::ReviewRequested));
        }
        for node in resp.assigned.nodes {
            items.push(node.into_action_item(&self.account_id, ItemKind::Assigned));
        }
        for node in resp.authored.nodes {
            if let Some(item) = node.into_ready_to_merge_item(&self.account_id) {
                items.push(item);
            }
        }
        Ok(items)
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
}

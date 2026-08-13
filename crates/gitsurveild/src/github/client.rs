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
    account_id: String,
    api_base: String,
    http: reqwest::Client,
    octocrab: octocrab::Octocrab,
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
            return Err(DaemonError::Config(format!("GitHub {status}: {detail}")));
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

    /// Runs the combined "review requested" + "assigned" GraphQL search
    /// (`specs/github-integration.md`) in a single request.
    pub async fn fetch_search_items(&self) -> Result<Vec<ActionItem>> {
        const QUERY: &str = r#"
            query {
              reviewRequested: search(query: "is:open is:pr review-requested:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
              }
              assigned: search(query: "is:open assignee:@me", type: ISSUE, first: 50) {
                nodes { ...prFields }
              }
            }
            fragment prFields on PullRequest {
              number
              title
              url
              createdAt
              updatedAt
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
        for node in resp.data.review_requested.nodes {
            items.push(node.into_action_item(&self.account_id, ItemKind::ReviewRequested));
        }
        for node in resp.data.assigned.nodes {
            items.push(node.into_action_item(&self.account_id, ItemKind::Assigned));
        }
        Ok(items)
    }
}

/// Shape of one entry from `GET /notifications`
/// (<https://docs.github.com/en/rest/activity/notifications>).
#[derive(Debug, Deserialize)]
struct RawNotification {
    id: String,
    reason: String,
    updated_at: String,
    subject: RawSubject,
    repository: RawRepository,
}

#[derive(Debug, Deserialize)]
struct RawSubject {
    title: String,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
struct GraphQlEnvelope {
    data: GraphQlData,
}

#[derive(Debug, Deserialize)]
struct GraphQlData {
    #[serde(rename = "reviewRequested")]
    review_requested: SearchResult,
    assigned: SearchResult,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    nodes: Vec<PrNode>,
}

#[derive(Debug, Deserialize)]
struct PrNode {
    number: u64,
    title: String,
    url: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    author: Option<PrAuthor>,
    repository: PrRepository,
    commits: PrCommits,
}

#[derive(Debug, Deserialize)]
struct PrAuthor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct PrRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct PrCommits {
    nodes: Vec<PrCommitNode>,
}

#[derive(Debug, Deserialize)]
struct PrCommitNode {
    commit: PrCommit,
}

#[derive(Debug, Deserialize)]
struct PrCommit {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<StatusCheckRollup>,
}

#[derive(Debug, Deserialize)]
struct StatusCheckRollup {
    state: String,
}

impl PrNode {
    fn into_action_item(self, account_id: &str, kind: ItemKind) -> ActionItem {
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
}

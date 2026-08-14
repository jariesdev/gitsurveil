//! The local API server (`specs/daemon.md`): newline-delimited JSON over a
//! unix domain socket (macOS/Linux) or a named pipe (Windows).
//! Implements `status`, `items.{list,history,dismiss,undismiss}`,
//! `accounts.{add,list,remove}`, `rules.list`, `repos.{list,set,remove,new,
//! ack_new,refresh,clone,clone_status,worktrees,worktree_add,worktree_remove}`,
//! `conflicts.*`, `pr.*`, `prs.list`, `apps.{list,add,remove,open}`, and
//! `poll.now`. Later phases add more `match` arms to [`dispatch`] without
//! touching the transport code.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Utc;
use gitsurveil_proto::{
    AccountRef, AuthKind, CloneStatus, ConflictSession, PrState, PullRequestSummary, Request,
    Response, StatusResult,
};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::conflicts::session::{PrepareInputs, Session};use crate::error::{DaemonError, Result};
use crate::github::GitHubClient;
use crate::keychain;
use crate::priority::{self, Rule};
use crate::store::Store;

/// Shared state every connection's [`dispatch`] call can read.
pub struct ServerState {
    pub store: Arc<Store>,
    pub started_at: Instant,
    /// Priority rules, loaded from config at startup. Scores are recomputed on
    /// every request rather than cached, since age escalation means an item's
    /// priority drifts with the clock even when nothing about it changed.
    pub rules: Vec<Rule>,
    /// Live conflict-resolution sessions (`specs/conflict-resolver.md`),
    /// keyed by `"owner/name"` — one per repo, in memory only, torn down on
    /// daemon restart.
    pub sessions: Mutex<HashMap<String, Session>>,
    /// The daemon data dir; temp worktrees for conflict sessions are created
    /// under it, never inside the user's clone (AC-1.3).
    pub data_dir: PathBuf,
}

/// Params for `items.list`. All fields optional; an absent field means "no
/// filter on this dimension". Only account-independent listing (all open
/// items) is implemented in Phase 1 — filtering by kind/repo/severity is
/// added in `specs/desktop-ui.md`'s Phase 5 work.
#[derive(Debug, Default, Deserialize)]
struct ItemsListParams {}

/// Which pull-request operation a `pr.*` request is asking for.
#[derive(Debug, Clone, Copy)]
enum PrAction {
    Detail,
    Create,
    Update,
    Close,
    Merge,
    Comments,
    Comment,
    CommentReply,
    Resolve,
    Branches,
    Labels,
}

/// Params shared by every `pr.*` method. Unused fields stay `None` for
/// operations that don't need them, so one shape serves all eight rather than
/// eight nearly-identical structs.
#[derive(Debug, Deserialize)]
struct PrParams {
    /// Which account's credentials to act with. Defaults to the only
    /// configured account when omitted, which is the common case.
    #[serde(default)]
    account_id: Option<String>,
    /// `"owner/name"`.
    repo: String,
    /// PR number; absent for create and branch listing.
    #[serde(default)]
    number: Option<u64>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    base: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    draft: Option<bool>,
    #[serde(default)]
    patch: Option<crate::github::PrPatch>,
    #[serde(default)]
    method: Option<gitsurveil_proto::MergeMethod>,
    #[serde(default)]
    head_sha: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    /// Parent comment id for `pr.comment_reply`.
    #[serde(default)]
    in_reply_to: Option<u64>,
    /// Thread id for `pr.resolve`; `resolved` picks the mutation.
    #[serde(default)]
    thread_id: Option<String>,
    /// Desired state for `pr.resolve`.
    #[serde(default)]
    resolved: Option<bool>,
}

impl PrParams {
    fn number(&self) -> Result<u64> {
        self.number
            .ok_or_else(|| DaemonError::InvalidParams("number is required".into()))
    }

    fn require_u64(&self, value: Option<u64>, name: &str) -> Result<u64> {
        value.ok_or_else(|| DaemonError::InvalidParams(format!("{name} is required")))
    }

    fn require<'a>(&self, value: Option<&'a String>, name: &str) -> Result<&'a str> {
        value
            .map(String::as_str)
            .ok_or_else(|| DaemonError::InvalidParams(format!("{name} is required")))
    }
}

/// Params for `prs.list`. Without `account_id`, every configured account is
/// queried and the results are concatenated. `state` is `None` for "all".
#[derive(Debug, Deserialize)]
struct PrsListParams {
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    state: Option<PrState>,
}

/// Params for `items.history`.
#[derive(Debug, Deserialize)]
struct HistoryParams {
    /// Cap on rows returned; the store keeps far more than a UI should render
    /// at once.
    #[serde(default = "default_history_limit")]
    limit: usize,
}

fn default_history_limit() -> usize {
    200
}

/// Params for methods that address a single item.
#[derive(Debug, Deserialize)]
struct ItemIdParams {
    id: String,
}

/// Params for `accounts.remove`.
#[derive(Debug, Deserialize)]
struct AccountIdParams {
    id: String,
}

/// Params for `accounts.add`. `api_base` defaults to the public GitHub API;
/// set it for GitHub Enterprise (`specs/github-integration.md`).
#[derive(Debug, Deserialize)]
struct AccountsAddParams {
    host: String,
    #[serde(default)]
    api_base: Option<String>,
    token: String,
}

/// Params for `conflicts.prepare`.
#[derive(Debug, Deserialize)]
struct ConflictsPrepareParams {
    /// Which account's credentials to act with. Defaults to the only
    /// configured account when omitted.
    #[serde(default)]
    account_id: Option<String>,
    /// `"owner/name"`.
    repo: String,
    /// PR number.
    number: u64,
}

/// Params for `conflicts.file`.
#[derive(Debug, Deserialize)]
struct ConflictsFileParams {
    session_id: String,
    path: String,
}

/// Params for `conflicts.save`. Exactly one of `content` (full resolved text)
/// or `pick` (`"ours"`/`"theirs"`, whole-file copy) must be present.
#[derive(Debug, Deserialize)]
struct ConflictsSaveParams {
    session_id: String,
    path: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    pick: Option<String>,
}

/// Params for `conflicts.commit`.
#[derive(Debug, Deserialize)]
struct ConflictsCommitParams {
    session_id: String,
    message: String,
}

/// Params for `conflicts.abort` and `conflicts.push`.
#[derive(Debug, Deserialize)]
struct ConflictsSessionIdParams {
    session_id: String,
}

/// Params for `apps.add` — the display name and the bare executable on `PATH`
/// (e.g. `name: "VS Code"`, `command: "code"`).
#[derive(Debug, Deserialize)]
struct AppsAddParams {
    name: String,
    command: String,
}

/// Params for `apps.remove` — the registered command to forget.
#[derive(Debug, Deserialize)]
struct AppsRemoveParams {
    command: String,
}

/// Params for `apps.open` — which registered app opens which worktree path.
#[derive(Debug, Deserialize)]
struct AppsOpenParams {
    command: String,
    path: String,
}

/// Dispatches one decoded [`Request`] to the matching daemon capability and
/// returns its [`Response`]. Kept as a single flat `match` — one method per
/// arm, no framework — per `CLAUDE.md`'s "no frameworks in the daemon" rule.
async fn dispatch(state: &ServerState, req: Request) -> Response {
    tracing::debug!(method = %req.method, id = req.id, "api request");
    let result = match req.method.as_str() {
        "status" => handle_status(state),
        "items.list" => handle_items_list(state, req.params),
        "items.history" => handle_items_history(state, req.params),
        "items.dismiss" => handle_items_set_dismissed(state, req.params, true),
        "items.undismiss" => handle_items_set_dismissed(state, req.params, false),
        "accounts.list" => handle_accounts_list(state),
        "accounts.add" => handle_accounts_add(state, req.params).await,
        "accounts.remove" => handle_accounts_remove(state, req.params),
        "rules.list" => handle_rules_list(state),
        "repos.list" => handle_repos_list(state),
        "repos.set" => handle_repos_set(state, req.params).await,
        "repos.remove" => handle_repos_remove(state, req.params).await,
        "repos.new" => handle_repos_new(state),
        "repos.ack_new" => handle_repos_ack_new(state, req.params),
        "repos.refresh" => handle_repos_refresh(state).await,
        "repos.clone" => handle_repos_clone(state, req.params).await,
        "repos.clone_status" => handle_repos_clone_status(state, req.params),
        "repos.worktrees" => handle_repos_worktrees(state, req.params).await,
        "repos.worktree_add" => handle_repos_worktree_add(state, req.params).await,
        "repos.worktree_remove" => handle_repos_worktree_remove(state, req.params).await,
        "conflicts.prepare" => handle_conflicts_prepare(state, req.params).await,
        "conflicts.file" => handle_conflicts_file(state, req.params).await,
        "conflicts.save" => handle_conflicts_save(state, req.params).await,
        "conflicts.commit" => handle_conflicts_commit(state, req.params).await,
        "conflicts.push" => handle_conflicts_push(state, req.params).await,
        "conflicts.abort" => handle_conflicts_abort(state, req.params).await,
        "pr.detail" => handle_pr(state, req.params, PrAction::Detail).await,
        "pr.create" => handle_pr(state, req.params, PrAction::Create).await,
        "pr.update" => handle_pr(state, req.params, PrAction::Update).await,
        "pr.close" => handle_pr(state, req.params, PrAction::Close).await,
        "pr.merge" => handle_pr(state, req.params, PrAction::Merge).await,
        "pr.comments" => handle_pr(state, req.params, PrAction::Comments).await,
        "pr.comment" => handle_pr(state, req.params, PrAction::Comment).await,
        "pr.comment_reply" => handle_pr(state, req.params, PrAction::CommentReply).await,
        "pr.resolve" => handle_pr(state, req.params, PrAction::Resolve).await,
        "pr.branches" => handle_pr(state, req.params, PrAction::Branches).await,
        "pr.labels" => handle_pr(state, req.params, PrAction::Labels).await,
        "prs.list" => handle_prs_list(state, req.params).await,
        "apps.list" => handle_apps_list(state),
        "apps.add" => handle_apps_add(state, req.params),
        "apps.remove" => handle_apps_remove(state, req.params),
        "apps.open" => handle_apps_open(state, req.params),
        "poll.now" => handle_poll_now(state).await,
        other => Err(DaemonError::UnknownMethod(other.to_string())),
    };
    match result {
        Ok(value) => Response {
            id: req.id,
            result: Some(value),
            error: None,
        },
        Err(e) => Response {
            id: req.id,
            result: None,
            error: Some(e.into()),
        },
    }
}

fn handle_status(state: &ServerState) -> Result<serde_json::Value> {
    let account_count = state.store.list_accounts()?.len();
    let items = state.store.open_items()?;
    let scored = priority::score_all(&items, &state.rules, Utc::now());
    let status = StatusResult {
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        account_count,
        open_item_count: items.len(),
        top_severity: priority::top_severity(&scored),
    };
    Ok(serde_json::to_value(status).expect("StatusResult always serializes"))
}

fn handle_items_list(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let _params: ItemsListParams = if params.is_null() {
        ItemsListParams::default()
    } else {
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?
    };
    let items = state.store.open_items()?;
    // Returned already sorted most-urgent-first, so every client renders the
    // same order without reimplementing the comparison.
    let scored = priority::score_all(&items, &state.rules, Utc::now());
    Ok(serde_json::to_value(scored).expect("Vec<ScoredItem> always serializes"))
}

/// Runs one pull-request operation against the right account's client.
///
/// All eight share this entry point because they share the same preamble:
/// resolve the account, pull its token from the keychain, build a client.
async fn handle_pr(
    state: &ServerState,
    params: serde_json::Value,
    action: PrAction,
) -> Result<serde_json::Value> {
    let params: PrParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;

    let accounts = state.store.list_accounts()?;
    let account = match &params.account_id {
        Some(id) => accounts
            .iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| DaemonError::UnknownAccount(id.clone()))?,
        None => accounts
            .first()
            .ok_or_else(|| DaemonError::InvalidParams("no account configured".into()))?,
    };
    let token = keychain::get_token(&account.id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;
    let client = GitHubClient::new(&account.id, &account.api_base, &token)?;
    let repo = params.repo.as_str();

    let value = match action {
        PrAction::Detail => {
            serde_json::to_value(client.pr_detail(repo, params.number()?).await?)
        }
        PrAction::Create => serde_json::to_value(
            client
                .pr_create(
                    repo,
                    params.require(params.base.as_ref(), "base")?,
                    params.require(params.head.as_ref(), "head")?,
                    params.require(params.title.as_ref(), "title")?,
                    params.body.as_deref().unwrap_or(""),
                    params.draft.unwrap_or(false),
                )
                .await?,
        ),
        PrAction::Update => {
            let patch = params
                .patch
                .clone()
                .ok_or_else(|| DaemonError::InvalidParams("patch is required".into()))?;
            serde_json::to_value(client.pr_update(repo, params.number()?, &patch).await?)
        }
        PrAction::Close => {
            client
                .pr_close(repo, params.number()?, params.comment.as_deref())
                .await?;
            Ok(serde_json::Value::Null)
        }
        PrAction::Merge => {
            client
                .pr_merge(
                    repo,
                    params.number()?,
                    params.method.unwrap_or(gitsurveil_proto::MergeMethod::Merge),
                    params.require(params.head_sha.as_ref(), "head_sha")?,
                    params.title.as_deref(),
                )
                .await?;
            Ok(serde_json::Value::Null)
        }
        PrAction::Comments => {
            serde_json::to_value(client.pr_comments(repo, params.number()?).await?)
        }
        PrAction::Comment => serde_json::to_value(
            client
                .pr_comment(
                    repo,
                    params.number()?,
                    params.require(params.body.as_ref(), "body")?,
                )
                .await?,
        ),
        PrAction::CommentReply => serde_json::to_value(
            client
                .pr_comment_reply(
                    repo,
                    params.number()?,
                    params.require_u64(params.in_reply_to, "in_reply_to")?,
                    params.require(params.body.as_ref(), "body")?,
                )
                .await?,
        ),
        PrAction::Resolve => {
            let thread_id = params.require(params.thread_id.as_ref(), "thread_id")?;
            let resolved = params
                .resolved
                .ok_or_else(|| DaemonError::InvalidParams("resolved is required".into()))?;
            serde_json::to_value(client.resolve_thread(thread_id, resolved).await?)
        }
        PrAction::Branches => serde_json::to_value(client.list_branches(repo).await?),
        PrAction::Labels => serde_json::to_value(client.list_labels(repo).await?),
    };

    value.map_err(|e| DaemonError::Config(e.to_string()))
}

/// `prs.list` — the Pull Requests view's data source.
///
/// A live query: one GraphQL request per configured account, concatenated.
/// Never stored, never polled on a timer — the cost is bounded to a view
/// open or a status refilter. See `specs/desktop-ui.md` for the filter
/// contract (only `state` is a daemon-side qualifier; everything else is
/// client-side).
async fn handle_prs_list(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: PrsListParams = if params.is_null() {
        PrsListParams { account_id: None, state: None }
    } else {
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?
    };

    let accounts = state.store.list_accounts()?;
    let accounts: Vec<&AccountRef> = match &params.account_id {
        Some(id) => vec![accounts
            .iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| DaemonError::UnknownAccount(id.clone()))?],
        None => accounts.iter().collect(),
    };
    if accounts.is_empty() {
        return Ok(serde_json::json!([]));
    }

    let mut summaries: Vec<PullRequestSummary> = Vec::new();
    for account in accounts {
        let token = keychain::get_token(&account.id)?
            .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;
        let client = GitHubClient::new(&account.id, &account.api_base, &token)?;
        summaries.extend(client.list_pull_requests(params.state).await?);
    }
    Ok(serde_json::to_value(summaries).expect("Vec<PullRequestSummary> always serializes"))
}

fn handle_items_history(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: HistoryParams = if params.is_null() {
        HistoryParams { limit: default_history_limit() }
    } else {
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?
    };
    let items = state.store.history_items(params.limit)?;
    // Scored like anything else so history rows render with the same badges
    // as live ones, rather than needing a second, subtly different row widget.
    let scored = priority::score_all(&items, &state.rules, Utc::now());
    Ok(serde_json::to_value(scored).expect("Vec<ScoredItem> always serializes"))
}

fn handle_items_set_dismissed(
    state: &ServerState,
    params: serde_json::Value,
    dismissed: bool,
) -> Result<serde_json::Value> {
    let params: ItemIdParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    state.store.set_dismissed(&params.id, dismissed)?;
    Ok(serde_json::Value::Null)
}

/// Removes the account row *and* its keychain token. Order matters: the row
/// goes last, so a keychain failure can't leave a token behind with no
/// account referencing it.
fn handle_accounts_remove(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: AccountIdParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    keychain::delete_token(&params.id)?;
    state.store.remove_account(&params.id)?;
    Ok(serde_json::Value::Null)
}

/// Lists every registered "Open with" application, sorted by display name —
/// the data behind a worktree context menu's "Open with" submenu and the
/// Applications list on the Settings page.
fn handle_apps_list(state: &ServerState) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(state.store.list_apps()?)
        .expect("Vec<RegisteredApp> always serializes"))
}

/// Registers an application for the worktree "Open with" menu. `command` must
/// be a single whitespace-free executable name resolvable on `PATH` (e.g.
/// `code`); the daemon runs it directly as `command <path>`, never through a
/// shell, so any flag, argument, quote, or path separator is rejected here.
/// Registering the same command twice under a different name is a conflict.
fn handle_apps_add(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: AppsAddParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let name = params.name.trim();
    if name.is_empty() {
        return Err(DaemonError::InvalidParams("name is required".into()));
    }
    let command = params.command.trim();
    validate_command(command)?;
    let now = crate::poller::now_rfc3339();
    let inserted = state.store.add_app(
        &gitsurveil_proto::RegisteredApp {
            name: name.into(),
            command: command.into(),
        },
        &now,
    )?;
    if !inserted {
        return Err(DaemonError::InvalidParams(format!(
            "{command} is already registered"
        )));
    }
    Ok(serde_json::json!({ "name": name, "command": command }))
}

/// Forgets a registered application. Idempotent — removing one that isn't
/// registered succeeds, so a UI retry can't wedge.
fn handle_apps_remove(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: AppsRemoveParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    state.store.remove_app(&params.command)?;
    Ok(serde_json::Value::Null)
}

/// Launches `command <path>` — the worktree context menu's "Open with" action.
/// Only commands registered via `apps.add` are ever run (defense in depth:
/// the UI only offers registered ones, and the daemon double-checks), and the
/// path is passed as a single argument with no shell involved. Returns a
/// normalized error when the binary isn't found so the UI can say "is it
/// installed and on PATH?".
fn handle_apps_open(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: AppsOpenParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if params.path.contains('\0') {
        return Err(DaemonError::InvalidParams("path must not contain NUL".into()));
    }
    if !state.store.app_registered(&params.command)? {
        return Err(DaemonError::InvalidParams(format!(
            "{} is not registered",
            params.command
        )));
    }
    spawn_app(&params.command, &params.path)?;
    Ok(serde_json::Value::Null)
}

/// `apps.add`'s command contract: one whitespace-free token that is either an
/// executable name resolved on `PATH` or an absolute path to one. The daemon
/// runs it with `Command::new(command)` and no shell, so anything beyond a
/// bare target (flags, args, quotes, a `\0`) would either be swallowed
/// silently or panic — rejecting up front turns that into a user-facing error.
fn validate_command(command: &str) -> Result<()> {
    if command.is_empty() {
        return Err(DaemonError::InvalidParams("command is required".into()));
    }
    if command.contains('\0') || command.split_whitespace().count() > 1 {
        return Err(DaemonError::InvalidParams(
            "command must be an executable name on PATH or an absolute path (no flags, args, or spaces)"
                .into(),
        ));
    }
    Ok(())
}

/// Spawns `command <path>` as a detached child process. On Windows the
/// command goes through `cmd /C` so a `.cmd`/`.bat` shim (like the
/// npm-installed `code`) resolves the way it would in a terminal.
fn spawn_app(command: &str, path: &str) -> Result<()> {
    let map_err = |e: std::io::Error| {
        DaemonError::Config(format!(
            "could not start {command}: {e} — is it installed and on PATH?"
        ))
    };
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", command, path])
            .spawn()
            .map_err(map_err)?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new(command)
            .arg(path)
            .spawn()
            .map_err(map_err)?;
    }
    Ok(())
}

/// Forces a poll cycle now. Runs inline on the caller's connection, so the
/// response only comes back once the poll has finished and the client can
/// refresh immediately afterwards and see the result.
async fn handle_poll_now(state: &ServerState) -> Result<serde_json::Value> {
    crate::poller::poll_all_accounts(&state.store, &state.rules).await;
    Ok(serde_json::Value::Null)
}

/// Read-only for now: the graphical rule editor writes through a `rules.set`
/// method that lands with config hot-reloading. Listing them already lets the
/// UI explain *why* an item scored the way it did.
fn handle_rules_list(state: &ServerState) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(&state.rules).expect("Vec<Rule> always serializes"))
}

/// Params for `repos.set`. `path` is the local clone to associate with
/// `repo` (`"owner/name"`).
#[derive(Debug, Deserialize)]
struct RepoSetParams {
    repo: String,
    path: PathBuf,
}

/// Params for `repos.remove`.
#[derive(Debug, Deserialize)]
struct RepoRemoveParams {
    repo: String,
}

/// Params for `repos.ack_new`. The catalog row's `first_seen_at` is the ack
/// watermark: only rows seen before it are acknowledged, so a discovery that
/// lands during the call can't have its rows silently acked.
#[derive(Debug, Deserialize)]
struct RepoAckParams {
    first_seen_at: String,
}

/// Params for `repos.clone`. `target` must be an absolute path to an empty
/// or absent directory. The daemon creates the target when it is absent and,
/// on failure, removes it again so a retry starts clean — but only when it
/// created it. A pre-existing path is never deleted, no matter what.
#[derive(Debug, Deserialize)]
struct RepoCloneParams {
    repo: String,
    target: PathBuf,
}

/// Resolves the catalog row for a `repo`-only call. Repo operations don't
/// carry an `account_id`, so the account is inferred the way `pr.*` does:
/// the first account that has the repo in its catalog. Rows with no account
/// (legacy imports) can't be resolved this way and are reported unknown.
fn catalog_repo(state: &ServerState, repo: &str) -> Result<gitsurveil_proto::Repository> {
    let account_id = state
        .store
        .accounts_for_repo(repo)?
        .into_iter()
        .next()
        .ok_or_else(|| DaemonError::Config(format!("unknown repo {repo}")))?;
    state
        .store
        .find_repo(&account_id, repo)?
        .ok_or_else(|| DaemonError::Config(format!("unknown repo {repo}")))
}

/// Returns the full repository catalog: every repo the daemon knows about,
/// with its tracked/clone state, and the orgs each account can filter by
/// (`specs/desktop-ui.md`). This is the pane's single read — it replaces the
/// old `repos.list` (config block) and `list_orgs` (derived here).
fn handle_repos_list(state: &ServerState) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(state.store.list_catalog()?).expect("RepoCatalog always serializes"))
}

/// Registers a local clone path for one repo. Validates the path (is a git
/// repo, `origin` points at `owner/name`) before it's stored, so a typo'd
/// path can't silently disable conflict resolution later. Marks the repo
/// tracked and acks it as seen (a tracked repo is no longer "new").
async fn handle_repos_set(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: RepoSetParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if !is_repo_slug(&params.repo) {
        return Err(DaemonError::InvalidParams(format!(
            "repo must be \"owner/name\", got {:?}",
            params.repo
        )));
    }
    let repo = params.repo.clone();
    let path = params.path.clone();
    tokio::task::spawn_blocking(move || crate::gitops::validate_clone(&repo, &path))
        .await
        .map_err(|e| DaemonError::Io(std::io::Error::other(e)))??;

    let row = catalog_repo(state, &params.repo)?;
    let account_id = row
        .account_id
        .as_deref()
        .ok_or_else(|| DaemonError::Config(format!("cannot set a clone path for {}", params.repo)))?;
    let now = crate::poller::now_rfc3339();
    let updated =
        state
            .store
            .set_repo_path(account_id, &params.repo, &params.path.to_string_lossy(), &now)?;
    let repo_row = updated
        .ok_or_else(|| DaemonError::Config("repo vanished during repos.set".into()))?;
    Ok(serde_json::to_value(&repo_row).expect("Repository always serializes"))
}

/// Removes a repo's local clone path. Idempotent: removing a repo that isn't
/// tracked (or isn't in the catalog at all) is a no-op rather than an error,
/// so a UI retry can't wedge. The catalog row survives (untracked) —
/// discovery owns it, not the user.
async fn handle_repos_remove(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: RepoRemoveParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if let Some(account_id) = state.store.accounts_for_repo(&params.repo)?.into_iter().next() {
        state.store.untrack_repo(&account_id, &params.repo)?;
    }
    Ok(serde_json::Value::Null)
}

/// Returns repos that were discovered but never acked (`tracked = false` and
/// never notified), newest-first. `dismiss` clears the whole set via
/// `repos.ack_new`; acting on one marks it tracked (which acks it).
fn handle_repos_new(state: &ServerState) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(state.store.list_new_repos()?).expect("Vec<Repository> always serializes"))
}

/// Acknowledges every currently-new repo as seen (`specs/desktop-ui.md`):
/// the modal's "Not now" button. Returns how many rows were acked. Rows
/// tracked since they went new are skipped, so this never silently clears a
/// repo the user is mid-setup on.
fn handle_repos_ack_new(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: RepoAckParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let acked = state.store.ack_new_repos(&params.first_seen_at)?;
    Ok(serde_json::to_value(acked).expect("u64 always serializes"))
}

/// Forces a discovery cycle for every account, then returns the fresh
/// catalog. The background loop also refreshes on its own six-hour cadence;
/// this is the pane's manual "Refresh" and the modal's first-run baseline.
async fn handle_repos_refresh(state: &ServerState) -> Result<serde_json::Value> {
    crate::discovery::discover_all_accounts(&state.store).await;
    Ok(serde_json::to_value(state.store.list_catalog()?).expect("RepoCatalog always serializes"))
}

/// Starts a background clone of `repo` into `target`, returning immediately
/// with a `job_id` the UI polls via `repos.clone_status`. Clones are HTTPS
/// only (`specs/desktop-ui.md`); the account's keychain token is the
/// credential. Progress updates are byte-based; the final state also marks
/// the repo tracked, so the modal row collapses once the clone lands.
async fn handle_repos_clone(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: RepoCloneParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if !is_repo_slug(&params.repo) {
        return Err(DaemonError::InvalidParams(format!(
            "repo must be \"owner/name\", got {:?}",
            params.repo
        )));
    }
    if !params.target.is_absolute() {
        return Err(DaemonError::InvalidParams(format!(
            "target must be an absolute path, got {:?}",
            params.target
        )));
    }

    let repo_row = catalog_repo(state, &params.repo)?;
    let account_id = repo_row.account_id.as_deref().ok_or_else(|| {
        DaemonError::Config(format!("cannot clone {}: no owning account", params.repo))
    })?;
    let account = state
        .store
        .find_account(account_id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account_id.to_string()))?;
    let token = keychain::get_token(&account.id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;

    let now = crate::poller::now_rfc3339();
    // The daemon only owns the target when it is provably absent before the
    // clone starts. Any error other than NotFound (e.g. a permission failure
    // that could hide an existing target) keeps it pre-existing, so failure
    // cleanup never touches it.
    let target_owned = match std::fs::metadata(&params.target) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        _ => false,
    };
    let job_id = state.store.create_clone_job(
        account_id,
        &repo_row.full_name,
        &params.target.to_string_lossy(),
        target_owned,
        &now,
    )?;

    let store = Arc::clone(&state.store);
    let job = job_id.clone();
    let clone_url = repo_row.clone_url.clone();
    let login = account.login.clone();
    let target = params.target.clone();
    tokio::spawn(async move {
        let block_store = Arc::clone(&store);
        let block_job = job.clone();
        let block_target = target.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut last = 0u64;
            crate::gitops::clone_repo(&clone_url, &login, &token, &block_target, |received, total| {
                // git2 reports progress very frequently; write through to the
                // store only when a meaningful chunk arrived (or the first
                // report) so a big clone doesn't hammer SQLite.
                if received == 0 || received.saturating_sub(last) >= 512 * 1024 {
                    last = received;
                    let _ = block_store.update_clone_progress(&block_job, received, total);
                }
            })
        })
        .await;

        let now = crate::poller::now_rfc3339();
        match result {
            Ok(Ok(())) => {
                if let Err(e) = store.finish_clone_job(&job, &now) {
                    tracing::error!(job = %job, "could not record finished clone: {e}");
                }
            }
            Ok(Err(e)) => {
                // Remove the partial checkout so a retry into the same folder
                // starts clean — but only when the daemon created the target.
                // A pre-existing path (which `clone_repo` refuses to touch) is
                // never deleted, no matter how the clone failed.
                if target_owned {
                    match std::fs::remove_dir_all(&target) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(ioe) => {
                            tracing::warn!(job = %job, "could not remove partial clone: {ioe}")
                        }
                    }
                }
                if let Err(e) = store.fail_clone_job(&job, &e.to_string(), &now) {
                    tracing::error!(job = %job, "could not record failed clone: {e}");
                }
            }
            Err(join) => {
                if let Err(e) = store.fail_clone_job(&job, &format!("clone thread panicked: {join}"), &now) {
                    tracing::error!(job = %job, "could not record failed clone: {e}");
                }
            }
        }
    });

    Ok(serde_json::to_value(&job_id).expect("String always serializes"))
}

/// Returns one clone job's current status, or `null` when the job id is
/// unknown (e.g. the daemon restarted and cleaned it up). `None` lets the UI
/// stop polling instead of erroring.
fn handle_repos_clone_status(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    #[derive(Debug, Deserialize)]
    struct Params {
        job_id: String,
    }
    let params: Params =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let status: Option<CloneStatus> = state.store.clone_status(&params.job_id)?;
    Ok(serde_json::to_value(status).expect("Option<CloneStatus> always serializes"))
}

/// Params for `repos.worktrees`.
#[derive(Debug, Deserialize)]
struct RepoWorktreesParams {
    repo: String,
}

/// Params for `repos.worktree_add`. `branch` may be an existing local or
/// remote branch or a brand-new name (created in the new worktree); `path` is
/// the target directory, absolute or relative to the clone's parent.
#[derive(Debug, Deserialize)]
struct RepoWorktreeAddParams {
    repo: String,
    branch: String,
    path: String,
}

/// Params for `repos.worktree_remove`. `name` is the worktree's registered
/// name (`git worktree list`), not its path.
#[derive(Debug, Deserialize)]
struct RepoWorktreeRemoveParams {
    repo: String,
    name: String,
}

/// The registered clone path for `repo`, or a Config error explaining that
/// worktrees need a local clone first. Used by all three worktree handlers.
fn tracked_clone_path(state: &ServerState, repo: &str) -> Result<std::path::PathBuf> {
    catalog_repo(state, repo)?
        .clone_path
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            DaemonError::Config(format!(
                "no local clone configured for {repo} — clone it first"
            ))
        })
}

/// `repos.worktrees` — a repo's user-created worktrees plus the branches a new
/// one can be created from. Derived from the clone's git metadata on every
/// call, so worktrees made or removed outside gitsurveil show up too.
async fn handle_repos_worktrees(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: RepoWorktreesParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if !is_repo_slug(&params.repo) {
        return Err(DaemonError::InvalidParams(format!(
            "repo must be \"owner/name\", got {:?}",
            params.repo
        )));
    }
    let clone_path = tracked_clone_path(state, &params.repo)?;
    let result =
        run_git_op(move || crate::worktrees::list(&clone_path)).await?;
    Ok(serde_json::to_value(result).expect("WorktreesResult always serializes"))
}

/// `repos.worktree_add` — creates a worktree for `branch` at `path` and
/// returns its info. The target must be absent or empty; a pre-existing
/// non-empty directory is an error, never overwritten.
async fn handle_repos_worktree_add(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: RepoWorktreeAddParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if !is_repo_slug(&params.repo) {
        return Err(DaemonError::InvalidParams(format!(
            "repo must be \"owner/name\", got {:?}",
            params.repo
        )));
    }
    let clone_path = tracked_clone_path(state, &params.repo)?;
    let branch = params.branch.clone();
    let path = params.path.clone();
    let info =
        run_git_op(move || crate::worktrees::add(&clone_path, &branch, &path)).await?;
    Ok(serde_json::to_value(info).expect("WorktreeInfo always serializes"))
}

/// `repos.worktree_remove` — unregisters a worktree and removes its working
/// directory. Keeps the checked-out branch, and refuses dirty worktrees and
/// `gitsurveil-*` conflict sessions.
async fn handle_repos_worktree_remove(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: RepoWorktreeRemoveParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if !is_repo_slug(&params.repo) {
        return Err(DaemonError::InvalidParams(format!(
            "repo must be \"owner/name\", got {:?}",
            params.repo
        )));
    }
    let clone_path = tracked_clone_path(state, &params.repo)?;
    let name = params.name.clone();
    run_git_op(move || crate::worktrees::remove(&clone_path, &name)).await?;
    Ok(serde_json::Value::Null)
}

/// Runs one conflict-resolver step against a session, in a blocking thread
/// (git2 types are `!Send`, and this module never holds a `Repository` across
/// an `.await`). A panicked blocking thread surfaces as an `io_error`.
async fn run_git_op<F, T>(op: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(op)
        .await
        .map_err(|e| DaemonError::Io(std::io::Error::other(e)))?
}

/// Clones a live session out of the map by id. Missing means the session was
/// aborted or the daemon restarted — a clear error beats a silent no-op for
/// every method except `abort`, which treats absence as success.
fn session_from(sessions: &Mutex<HashMap<String, Session>>, id: &str) -> Result<Session> {
    sessions
        .lock()
        .expect("sessions mutex poisoned")
        .get(id)
        .cloned()
        .ok_or_else(|| DaemonError::InvalidParams(format!("no active conflict session for {id}")))
}

/// `conflicts.prepare` — fetch, clean-check, temp worktree, merge the base in.
/// Returns the session plus the conflicted file list (`specs/conflict-resolver.md`
/// flow steps 1–2). One session per repo: a live one rejects a second prepare
/// (AC-2.4).
async fn handle_conflicts_prepare(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: ConflictsPrepareParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    if !is_repo_slug(&params.repo) {
        return Err(DaemonError::InvalidParams(format!(
            "repo must be \"owner/name\", got {:?}",
            params.repo
        )));
    }
    let clone_path = catalog_repo(state, &params.repo)?
        .clone_path
        .map(PathBuf::from)
        .ok_or_else(|| {
            DaemonError::Config(format!(
                "no local clone configured for {} — add one in Settings",
                params.repo
            ))
        })?;

    {
        let sessions = state.sessions.lock().expect("sessions mutex poisoned");
        if sessions.contains_key(&params.repo) {
            return Err(DaemonError::Config(format!(
                "a conflict resolution for {} is already in progress — abort or push it first",
                params.repo
            )));
        }
    }

    let accounts = state.store.list_accounts()?;
    let account = match &params.account_id {
        Some(id) => accounts
            .iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| DaemonError::UnknownAccount(id.clone()))?,
        None => accounts
            .first()
            .ok_or_else(|| DaemonError::InvalidParams("no account configured".into()))?,
    };
    let token = keychain::get_token(&account.id)?
        .ok_or_else(|| DaemonError::UnknownAccount(account.id.clone()))?;
    let client = GitHubClient::new(&account.id, &account.api_base, &token)?;
    let detail = client.pr_detail(&params.repo, params.number).await?;

    let inputs = PrepareInputs {
        repo: params.repo.clone(),
        base: detail.base.clone(),
        head: detail.head.clone(),
        clone_path,
        worktree_root: state.data_dir.clone(),
        login: account.login.clone(),
        token,
    };
    let (session, files) = run_git_op(move || crate::conflicts::session::prepare(&inputs)).await?;

    state
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .insert(session.id.clone(), session.clone());
    Ok(serde_json::to_value(ConflictSession {
        session_id: session.id,
        repo: params.repo,
        number: params.number,
        base: detail.base,
        head: detail.head,
        worktree_path: session.worktree_path.to_string_lossy().into_owned(),
        files,
    })
    .expect("ConflictSession always serializes"))
}

/// `conflicts.file` — the conflict regions of one file, read from the worktree
/// so it always reflects the latest `conflicts.save` (AC-4.3).
async fn handle_conflicts_file(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: ConflictsFileParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let session = session_from(&state.sessions, &params.session_id)?;
    let path = params.path;
    let file = run_git_op(move || crate::conflicts::session::read_file(&session, &path)).await?;
    Ok(serde_json::to_value(file).expect("ConflictFile always serializes"))
}

/// `conflicts.save` — writes resolved text, or copies a whole file from one
/// side of the index (the only resolution path for binary and >5 MB files).
async fn handle_conflicts_save(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: ConflictsSaveParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let session = session_from(&state.sessions, &params.session_id)?;
    let path = params.path;
    match (params.content, params.pick.as_deref()) {
        (Some(content), None) => {
            run_git_op(move || crate::conflicts::session::save_file(&session, &path, &content))
                .await?;
        }
        (None, Some("ours")) => {
            run_git_op(move || crate::conflicts::session::pick_file(&session, &path, true)).await?;
        }
        (None, Some("theirs")) => {
            run_git_op(move || crate::conflicts::session::pick_file(&session, &path, false))
                .await?;
        }
        (None, Some(other)) => {
            return Err(DaemonError::InvalidParams(format!(
                "pick must be \"ours\" or \"theirs\", got {other:?}"
            )));
        }
        (Some(_), Some(_)) => {
            return Err(DaemonError::InvalidParams(
                "pass either content or pick, not both".into(),
            ));
        }
        (None, None) => {
            return Err(DaemonError::InvalidParams(
                "content or pick is required".into(),
            ));
        }
    }
    Ok(serde_json::Value::Null)
}

/// `conflicts.commit` — stages the resolved files and creates the merge
/// commit. Refuses while any file still contains conflict markers (AC-4.4).
async fn handle_conflicts_commit(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: ConflictsCommitParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let session = session_from(&state.sessions, &params.session_id)?;
    let message = params.message;
    run_git_op(move || crate::conflicts::session::commit_resolution(&session, &message)).await?;
    Ok(serde_json::Value::Null)
}

/// `conflicts.push` — pushes the resolution branch to the PR head and, on
/// success, tears the worktree down and drops the session.
async fn handle_conflicts_push(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: ConflictsSessionIdParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let session = session_from(&state.sessions, &params.session_id)?;
    run_git_op(move || crate::conflicts::session::push_resolution(&session)).await?;
    state
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .remove(&params.session_id);
    Ok(serde_json::Value::Null)
}

/// `conflicts.abort` — tears down the worktree and drops the session.
/// Idempotent: an already-absent session is success, not an error (AC-2.2).
async fn handle_conflicts_abort(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: ConflictsSessionIdParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let session = state
        .sessions
        .lock()
        .expect("sessions mutex poisoned")
        .remove(&params.session_id);
    if let Some(session) = session {
        run_git_op(move || crate::conflicts::session::abort(&session)).await?;
    }
    Ok(serde_json::Value::Null)
}

/// Whether `repo` looks like `"owner/name"`: exactly one `/`, neither part
/// empty. GitHub repo slugs can't contain another `/`.
fn is_repo_slug(repo: &str) -> bool {
    let mut parts = repo.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) => !owner.is_empty() && !name.is_empty(),
        _ => false,
    }
}

fn handle_accounts_list(state: &ServerState) -> Result<serde_json::Value> {
    let accounts = state.store.list_accounts()?;
    Ok(serde_json::to_value(accounts).expect("Vec<AccountRef> always serializes"))
}

/// Validates the token against GitHub (`GET /user`), then stores the token
/// in the OS keychain and the account row in SQLite — in that order, so a
/// bad token never leaves a half-configured account behind
/// (`specs/github-integration.md`, "Authentication").
async fn handle_accounts_add(
    state: &ServerState,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let params: AccountsAddParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let api_base = params
        .api_base
        .unwrap_or_else(|| "https://api.github.com".to_string());
    let id = uuid::Uuid::new_v4().to_string();

    let client = GitHubClient::new(&id, &api_base, &params.token)?;
    let login = client.validate().await?;

    keychain::set_token(&id, &params.token)?;
    let account = AccountRef {
        id,
        host: params.host,
        api_base,
        login,
        auth_kind: AuthKind::Pat,
    };
    state.store.upsert_account(&account)?;
    Ok(serde_json::to_value(account).expect("AccountRef always serializes"))
}

/// Starts the local API server. `address` is a filesystem path on unix
/// (macOS/Linux) and a pipe name (e.g. `\\.\pipe\gitsurveil`) on Windows —
/// see `main::socket_path`/`main::pipe_name` for how each is derived.
#[cfg(unix)]
pub async fn serve(state: Arc<ServerState>, address: &str) -> Result<()> {
    use tokio::net::UnixListener;

    let socket_path = Path::new(address);
    // A stale socket file from a previous crash would otherwise make bind()
    // fail with "address in use" even though nothing is listening.
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    // User-only permissions: this socket is a full-control API surface with
    // no auth of its own beyond filesystem access (`specs/architecture.md`).
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!(path = %socket_path.display(), "local API listening");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(state, stream).await {
                tracing::debug!("connection ended: {e}");
            }
        });
    }
}

#[cfg(unix)]
async fn handle_connection(
    state: Arc<ServerState>,
    stream: tokio::net::UnixStream,
) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => dispatch(&state, req).await,
            Err(e) => Response {
                id: 0,
                result: None,
                error: Some(gitsurveil_proto::ErrorPayload {
                    code: "invalid_request".into(),
                    message: e.to_string(),
                }),
            },
        };
        let mut out = serde_json::to_vec(&response).expect("Response always serializes");
        out.push(b'\n');
        write_half.write_all(&out).await?;
    }
    Ok(())
}

/// Windows named-pipe transport, mirroring the unix implementation above.
/// Written against the API documented for `tokio::net::windows::named_pipe`
/// (create a server instance, wait for a client, spawn a new instance for
/// the next client) — not yet exercised on Windows; verification happens in
/// the Phase 9 packaging pass (`specs/architecture.md`).
#[cfg(windows)]
pub async fn serve(state: Arc<ServerState>, address: &str) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    tracing::info!(pipe = %address, "local API listening");
    let mut server = ServerOptions::new().first_pipe_instance(true).create(address)?;

    loop {
        server.connect().await?;
        let connected = server;
        server = ServerOptions::new().create(address)?;

        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(state, connected).await {
                tracing::debug!("connection ended: {e}");
            }
        });
    }
}

#[cfg(windows)]
async fn handle_connection(
    state: Arc<ServerState>,
    stream: tokio::net::windows::named_pipe::NamedPipeServer,
) -> Result<()> {
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => dispatch(&state, req).await,
            Err(e) => Response {
                id: 0,
                result: None,
                error: Some(gitsurveil_proto::ErrorPayload {
                    code: "invalid_request".into(),
                    message: e.to_string(),
                }),
            },
        };
        let mut out = serde_json::to_vec(&response).expect("Response always serializes");
        out.push(b'\n');
        write_half.write_all(&out).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Repository;
    use gitsurveil_proto::{AccountRef, AuthKind};

    fn test_state() -> ServerState {
        ServerState {
            store: Arc::new(Store::open_in_memory().unwrap()),
            started_at: Instant::now(),
            rules: Vec::new(),
            sessions: Mutex::new(HashMap::new()),
            data_dir: std::env::temp_dir(),
        }
    }

    /// Seeds one account (`acc-1`) with a single discovered repo so catalog
    /// queries have a row to return. `repositories.account_id` is a FK, so
    /// the account must exist first.
    fn seed_catalog(store: &Store) {
        store
            .upsert_account(&AccountRef {
                id: "acc-1".into(),
                host: "github.com".into(),
                api_base: "https://api.github.com".into(),
                login: "octocat".into(),
                auth_kind: AuthKind::Pat,
            })
            .unwrap();
        let now = crate::poller::now_rfc3339();
        let repo = crate::github::client::DiscoveredRepo {
            owner: "acme".into(),
            name: "api".into(),
            url: "https://github.com/acme/api".into(),
            description: None,
            private: false,
            default_branch: "main".into(),
            clone_url: "https://github.com/acme/api.git".into(),
        };
        store
            .upsert_catalog("acc-1", "github.com", std::slice::from_ref(&repo), &now)
            .unwrap();
    }

    #[tokio::test]
    async fn status_reports_zero_when_empty() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 1,
                method: "status".into(),
                params: serde_json::Value::Null,
            },
        )
        .await;
        let result: StatusResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.account_count, 0);
        assert_eq!(result.open_item_count, 0);
        assert_eq!(resp.id, 1);
    }

    #[tokio::test]
    async fn status_counts_accounts_and_open_items() {
        let state = test_state();
        state
            .store
            .upsert_account(&AccountRef {
                id: "acc-1".into(),
                host: "github.com".into(),
                api_base: "https://api.github.com".into(),
                login: "octocat".into(),
                auth_kind: AuthKind::Pat,
            })
            .unwrap();
        let resp = dispatch(
            &state,
            Request {
                id: 2,
                method: "status".into(),
                params: serde_json::Value::Null,
            },
        )
        .await;
        let result: StatusResult = serde_json::from_value(resp.result.unwrap()).unwrap();
        assert_eq!(result.account_count, 1);
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 3,
                method: "does.not.exist".into(),
                params: serde_json::Value::Null,
            },
        )
        .await;
        assert!(resp.result.is_none());
        assert_eq!(resp.error.unwrap().code, "unknown_method");
    }

    #[tokio::test]
    async fn items_list_returns_empty_array_when_no_items() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 4,
                method: "items.list".into(),
                params: serde_json::Value::Null,
            },
        )
        .await;
        let items = resp.result.unwrap();
        assert_eq!(items, serde_json::json!([]));
    }

    #[tokio::test]
    async fn history_excludes_open_items() {
        let state = test_state();
        state.store.upsert_account(&AccountRef {
            id: "acc-1".into(),
            host: "github.com".into(),
            api_base: "https://api.github.com".into(),
            login: "octocat".into(),
            auth_kind: AuthKind::Pat,
        }).unwrap();
        let item = gitsurveil_proto::ActionItem {
            id: "i1".into(),
            account_id: "acc-1".into(),
            kind: gitsurveil_proto::ItemKind::Assigned,
            state: gitsurveil_proto::ItemState::Open,
            repo: "acme/api".into(),
            number: Some(1),
            title: "t".into(),
            url: "u".into(),
            author: "a".into(),
            created_at: "2026-08-13T12:00:00Z".into(),
            updated_at: "2026-08-13T12:00:00Z".into(),
            first_seen_at: "2026-08-13T12:00:00Z".into(),
            last_seen_at: "2026-08-13T12:00:00Z".into(),
            ci_status: gitsurveil_proto::CiStatus::None,
            raw_kind: "assign".into(),
        };
        state.store.upsert_item(&item).unwrap();

        let history = |state: &ServerState| {
            handle_items_history(state, serde_json::Value::Null)
                .unwrap()
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!(history(&state), 0, "an open item is not history");

        state.store.mark_item_done("i1").unwrap();
        assert_eq!(history(&state), 1, "a resolved item is");
    }

    #[tokio::test]
    async fn dismiss_and_undismiss_move_an_item_out_of_and_back_into_the_list() {
        let state = test_state();
        state.store.upsert_account(&AccountRef {
            id: "acc-1".into(),
            host: "github.com".into(),
            api_base: "https://api.github.com".into(),
            login: "octocat".into(),
            auth_kind: AuthKind::Pat,
        }).unwrap();
        state.store.upsert_item(&gitsurveil_proto::ActionItem {
            id: "i1".into(),
            account_id: "acc-1".into(),
            kind: gitsurveil_proto::ItemKind::Assigned,
            state: gitsurveil_proto::ItemState::Open,
            repo: "acme/api".into(),
            number: Some(1),
            title: "t".into(),
            url: "u".into(),
            author: "a".into(),
            created_at: "2026-08-13T12:00:00Z".into(),
            updated_at: "2026-08-13T12:00:00Z".into(),
            first_seen_at: "2026-08-13T12:00:00Z".into(),
            last_seen_at: "2026-08-13T12:00:00Z".into(),
            ci_status: gitsurveil_proto::CiStatus::None,
            raw_kind: "assign".into(),
        }).unwrap();

        let params = serde_json::json!({ "id": "i1" });
        handle_items_set_dismissed(&state, params.clone(), true).unwrap();
        assert!(state.store.open_items().unwrap().is_empty());

        handle_items_set_dismissed(&state, params, false).unwrap();
        assert_eq!(state.store.open_items().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn accounts_add_rejects_bad_params() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 5,
                method: "accounts.add".into(),
                params: serde_json::json!({ "host": "github.com" }), // missing `token`
            },
        )
        .await;
        assert_eq!(resp.error.unwrap().code, "invalid_params");
    }

    #[tokio::test]
    async fn repos_set_rejects_a_malformed_slug() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 6,
                method: "repos.set".into(),
                params: serde_json::json!({ "repo": "not-a-slug", "path": "/tmp/x" }),
            },
        )
        .await;
        assert_eq!(resp.error.unwrap().code, "invalid_params");
    }

    #[tokio::test]
    async fn repos_set_rejects_a_path_that_is_not_a_repo() {
        let state = test_state();
        let dir = std::env::temp_dir().join(format!("gs-repos-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let resp = dispatch(
            &state,
            Request {
                id: 7,
                method: "repos.set".into(),
                params: serde_json::json!({ "repo": "acme/api", "path": dir.to_string_lossy() }),
            },
        )
        .await;
        std::fs::remove_dir_all(&dir).ok();
        let err = resp.error.unwrap();
        assert_eq!(err.code, "config_error");
        assert!(err.message.contains("not a git repository"));
    }

    #[tokio::test]
    async fn repos_remove_is_idempotent() {
        let state = test_state();
        let params = serde_json::json!({ "repo": "acme/api" });
        // Unknown repos are a no-op (returns null), and a retry must not error.
        let resp = dispatch(
            &state,
            Request { id: 8, method: "repos.remove".into(), params: params.clone() },
        )
        .await;
        assert!(resp.result.is_some());
        assert_eq!(resp.result.unwrap(), serde_json::Value::Null);
        let resp = dispatch(
            &state,
            Request { id: 9, method: "repos.remove".into(), params },
        )
        .await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn repos_new_ack_new_round_trip() {
        let state = test_state();
        seed_catalog(&state.store);
        // `seed_catalog`'s first pass was a baseline (acks its rows, no flood).
        // A *later* discovery adding a brand-new repo is what makes it "new".
        let now = crate::poller::now_rfc3339();
        let newlib = crate::github::client::DiscoveredRepo {
            owner: "acme".into(),
            name: "newlib".into(),
            url: "https://github.com/acme/newlib".into(),
            description: None,
            private: false,
            default_branch: "main".into(),
            clone_url: "https://github.com/acme/newlib.git".into(),
        };
        state
            .store
            .upsert_catalog("acc-1", "github.com", std::slice::from_ref(&newlib), &now)
            .unwrap();

        let resp = dispatch(&state, Request { id: 10, method: "repos.new".into(), params: serde_json::Value::Null }).await;
        let new_repos = resp.result.unwrap();
        assert_eq!(new_repos.as_array().unwrap().len(), 1);
        assert_eq!(new_repos[0]["full_name"], "acme/newlib");

        // Dismiss-all acks the row; it leaves the new set.
        let resp = dispatch(
            &state,
            Request {
                id: 11,
                method: "repos.ack_new".into(),
                params: serde_json::json!({ "first_seen_at": now }),
            },
        )
        .await;
        assert_eq!(resp.result.unwrap(), serde_json::json!(1));
        let resp = dispatch(&state, Request { id: 12, method: "repos.new".into(), params: serde_json::Value::Null }).await;
        assert_eq!(resp.result.unwrap(), serde_json::json!([]));
    }

    #[tokio::test]
    async fn repos_list_returns_catalog_and_orgs() {
        let state = test_state();
        seed_catalog(&state.store);

        let resp = dispatch(&state, Request { id: 13, method: "repos.list".into(), params: serde_json::Value::Null }).await;
        let catalog = resp.result.unwrap();
        assert_eq!(catalog["orgs"][0]["name"], "acme");
        assert_eq!(catalog["repos"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn conflicts_prepare_rejects_a_repo_not_in_the_catalog() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 14,
                method: "conflicts.prepare".into(),
                params: serde_json::json!({ "repo": "acme/api", "number": 1 }),
            },
        )
        .await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, "config_error");
        assert!(err.message.contains("unknown repo acme/api"));
    }

    #[tokio::test]
    async fn conflicts_prepare_rejects_a_catalog_repo_without_a_clone() {
        let state = test_state();
        seed_catalog(&state.store);
        let resp = dispatch(
            &state,
            Request {
                id: 15,
                method: "conflicts.prepare".into(),
                params: serde_json::json!({ "repo": "acme/api", "number": 1 }),
            },
        )
        .await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, "config_error");
        assert!(err.message.contains("no local clone configured"));
    }

    #[tokio::test]
    async fn conflicts_abort_with_no_session_is_a_benign_no_op() {
        let state = test_state();
        // AC-2.2: aborting an unknown session must not error.
        let resp = dispatch(
            &state,
            Request {
                id: 11,
                method: "conflicts.abort".into(),
                params: serde_json::json!({ "session_id": "acme/api" }),
            },
        )
        .await;
        assert!(resp.error.is_none());
        assert!(resp.result.is_some());
    }

    #[tokio::test]
    async fn conflicts_save_requires_content_or_pick() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 12,
                method: "conflicts.save".into(),
                params: serde_json::json!({ "session_id": "acme/api", "path": "file.txt" }),
            },
        )
        .await;
        assert_eq!(resp.error.unwrap().code, "invalid_params");
    }

    /// Builds an offline fixture (bare remote + clone with divergent branches)
    /// so the conflicts handlers can run against a real session.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn conflict_fixture() -> (PathBuf, PathBuf, Session) {
        let base = std::env::temp_dir().join(format!("gs-socket-{}", uuid::Uuid::new_v4()));
        let clone = base.join("clone");
        let worktree_root = base.join("worktrees");
        std::fs::create_dir_all(&base).unwrap();
        git(&base, &["init", "--bare", "-b", "main", "remote.git"]);
        git(&base, &["clone", "remote.git", "clone"]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        git(&clone, &["config", "user.name", "Test"]);
        std::fs::write(clone.join("file.txt"), "base content\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-m", "initial"]);
        git(&clone, &["push", "-u", "origin", "main"]);
        git(&clone, &["checkout", "-b", "feature"]);
        std::fs::write(clone.join("file.txt"), "feature content\n").unwrap();
        git(&clone, &["commit", "-am", "feature change"]);
        git(&clone, &["push", "-u", "origin", "feature"]);
        git(&clone, &["checkout", "main"]);
        std::fs::write(clone.join("file.txt"), "main content\n").unwrap();
        git(&clone, &["commit", "-am", "main change"]);
        git(&clone, &["push", "-u", "origin", "main"]);

        let inputs = PrepareInputs {
            repo: "acme/api".into(),
            base: "main".into(),
            head: "feature".into(),
            clone_path: clone.clone(),
            worktree_root: worktree_root.clone(),
            login: "octocat".into(),
            token: "test-token".into(),
        };
        let (session, _) = crate::conflicts::session::prepare(&inputs).unwrap();
        (clone, base, session)
    }

    #[tokio::test]
    async fn conflicts_round_trip_file_save_commit_abort() {
        let state = test_state();
        let (clone, base, session) = conflict_fixture();
        state
            .sessions
            .lock()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        let session_id = session.id;

        // conflicts.file → segments present.
        let resp = dispatch(
            &state,
            Request {
                id: 20,
                method: "conflicts.file".into(),
                params: serde_json::json!({ "session_id": session_id, "path": "file.txt" }),
            },
        )
        .await;
        let file = resp.result.unwrap();
        assert!(
            file["segments"].as_array().unwrap().len() > 0,
            "file must report ordered segments"
        );

        // conflicts.save → a subsequent file reflects it (AC-4.3).
        let resp = dispatch(
            &state,
            Request {
                id: 21,
                method: "conflicts.save".into(),
                params: serde_json::json!({
                    "session_id": session_id,
                    "path": "file.txt",
                    "content": "resolved content\n"
                }),
            },
        )
        .await;
        assert!(resp.error.is_none());
        let resp = dispatch(
            &state,
            Request {
                id: 22,
                method: "conflicts.commit".into(),
                params: serde_json::json!({ "session_id": session_id, "message": "merge main" }),
            },
        )
        .await;
        assert!(resp.error.is_none(), "resolved commit must succeed");

        // conflicts.abort tears the session down (AC-2.1).
        let resp = dispatch(
            &state,
            Request {
                id: 23,
                method: "conflicts.abort".into(),
                params: serde_json::json!({ "session_id": session_id }),
            },
        )
        .await;
        assert!(resp.error.is_none());
        assert!(state.sessions.lock().unwrap().is_empty());

        // The clone itself was never disturbed.
        let repo = Repository::open(&clone).unwrap();
        assert_eq!(repo.head().unwrap().name().unwrap(), "refs/heads/main");
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn repos_worktrees_requires_a_tracked_clone() {
        let state = test_state();
        seed_catalog(&state.store);
        let resp = dispatch(
            &state,
            Request {
                id: 30,
                method: "repos.worktrees".into(),
                params: serde_json::json!({ "repo": "acme/api" }),
            },
        )
        .await;
        let err = resp.error.unwrap();
        assert_eq!(err.code, "config_error");
        assert!(err.message.contains("no local clone configured"));
    }

    #[tokio::test]
    async fn repos_worktree_requires_an_owner_name_slug() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 31,
                method: "repos.worktree_add".into(),
                params: serde_json::json!({
                    "repo": "not-a-slug",
                    "branch": "feature",
                    "path": "/tmp/wt"
                }),
            },
        )
        .await;
        assert_eq!(resp.error.unwrap().code, "invalid_params");
    }

    #[tokio::test]
    async fn repos_worktree_add_list_remove_round_trip() {
        let state = test_state();
        seed_catalog(&state.store);

        let base = std::env::temp_dir().join(format!("gs-wt-sock-{}", uuid::Uuid::new_v4()));
        let clone = base.join("clone");
        std::fs::create_dir_all(&base).unwrap();
        git(&base, &["init", "--bare", "-b", "main", "remote.git"]);
        git(&base, &["clone", "remote.git", "clone"]);
        git(&clone, &["config", "user.email", "test@example.com"]);
        git(&clone, &["config", "user.name", "Test"]);
        std::fs::write(clone.join("file.txt"), "base\n").unwrap();
        git(&clone, &["add", "."]);
        git(&clone, &["commit", "-m", "initial"]);
        git(&clone, &["push", "-u", "origin", "main"]);
        git(&clone, &["checkout", "-b", "feature"]);
        std::fs::write(clone.join("file.txt"), "feature\n").unwrap();
        git(&clone, &["commit", "-am", "feature"]);
        git(&clone, &["push", "-u", "origin", "feature"]);
        git(&clone, &["checkout", "main"]);
        let now = crate::poller::now_rfc3339();
        state
            .store
            .set_repo_path("acc-1", "acme/api", &clone.to_string_lossy(), &now)
            .unwrap();

        let target = base.join("wt-acme-api-feature");
        let resp = dispatch(
            &state,
            Request {
                id: 32,
                method: "repos.worktree_add".into(),
                params: serde_json::json!({
                    "repo": "acme/api",
                    "branch": "feature",
                    "path": target.to_string_lossy(),
                }),
            },
        )
        .await;
        assert!(resp.error.is_none(), "add failed: {:?}", resp.error);
        assert_eq!(resp.result.unwrap()["branch"], "feature");

        let resp = dispatch(
            &state,
            Request {
                id: 33,
                method: "repos.worktrees".into(),
                params: serde_json::json!({ "repo": "acme/api" }),
            },
        )
        .await;
        let worktrees = resp.result.unwrap()["worktrees"].as_array().unwrap().clone();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0]["name"], "wt-acme-api-feature");

        let resp = dispatch(
            &state,
            Request {
                id: 34,
                method: "repos.worktree_remove".into(),
                params: serde_json::json!({
                    "repo": "acme/api",
                    "name": "wt-acme-api-feature",
                }),
            },
        )
        .await;
        assert!(resp.error.is_none(), "remove failed: {:?}", resp.error);

        let resp = dispatch(
            &state,
            Request {
                id: 35,
                method: "repos.worktrees".into(),
                params: serde_json::json!({ "repo": "acme/api" }),
            },
        )
        .await;
        assert_eq!(resp.result.unwrap()["worktrees"].as_array().unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn apps_add_list_remove_round_trip() {
        let state = test_state();
        let resp = dispatch(
            &state,
            Request {
                id: 40,
                method: "apps.add".into(),
                params: serde_json::json!({ "name": "VS Code", "command": "code" }),
            },
        )
        .await;
        assert!(resp.error.is_none(), "add failed: {:?}", resp.error);
        assert_eq!(resp.result.unwrap(), serde_json::json!({ "name": "VS Code", "command": "code" }));

        let resp = dispatch(
            &state,
            Request {
                id: 41,
                method: "apps.add".into(),
                params: serde_json::json!({ "name": "Sublime Merge", "command": "smerge" }),
            },
        )
        .await;
        assert!(resp.error.is_none());

        let resp = dispatch(
            &state,
            Request { id: 42, method: "apps.list".into(), params: serde_json::Value::Null },
        )
        .await;
        let apps = resp.result.unwrap();
        assert_eq!(apps.as_array().unwrap().len(), 2);
        assert_eq!(apps[0]["name"], "Sublime Merge", "sorted by display name");
        assert_eq!(apps[1]["command"], "code");

        let resp = dispatch(
            &state,
            Request {
                id: 43,
                method: "apps.remove".into(),
                params: serde_json::json!({ "command": "code" }),
            },
        )
        .await;
        assert!(resp.error.is_none());
        let resp = dispatch(
            &state,
            Request {
                id: 44,
                method: "apps.remove".into(),
                params: serde_json::json!({ "command": "code" }),
            },
        )
        .await;
        assert!(resp.error.is_none(), "removing an unregistered app is a no-op");
    }

    #[tokio::test]
    async fn apps_add_rejects_duplicate_and_multiword_commands() {
        let state = test_state();
        let ok = dispatch(
            &state,
            Request {
                id: 45,
                method: "apps.add".into(),
                params: serde_json::json!({ "name": "VS Code", "command": "code" }),
            },
        )
        .await;
        assert!(ok.error.is_none());

        let dup = dispatch(
            &state,
            Request {
                id: 46,
                method: "apps.add".into(),
                params: serde_json::json!({ "name": "Code", "command": "code" }),
            },
        )
        .await;
        let err = dup.error.unwrap();
        assert_eq!(err.code, "invalid_params");
        assert!(err.message.contains("already registered"));

        let multiword = dispatch(
            &state,
            Request {
                id: 47,
                method: "apps.add".into(),
                params: serde_json::json!({ "name": "X", "command": "code --new-window" }),
            },
        )
        .await;
        assert_eq!(multiword.error.unwrap().code, "invalid_params");

        let empty = dispatch(
            &state,
            Request {
                id: 48,
                method: "apps.add".into(),
                params: serde_json::json!({ "name": "", "command": "code" }),
            },
        )
        .await;
        assert_eq!(empty.error.unwrap().code, "invalid_params");
    }

    #[tokio::test]
    async fn apps_open_rejects_unregistered_and_unknown_binary() {
        let state = test_state();
        let unregistered = dispatch(
            &state,
            Request {
                id: 49,
                method: "apps.open".into(),
                params: serde_json::json!({ "command": "code", "path": "/tmp/wt" }),
            },
        )
        .await;
        assert_eq!(unregistered.error.unwrap().code, "invalid_params");

        state.store
            .add_app(
                &gitsurveil_proto::RegisteredApp {
                    name: "Definitely Not Installed".into(),
                    command: "gitsurveil-no-such-binary-xyz".into(),
                },
                "t0",
            )
            .unwrap();
        let missing = dispatch(
            &state,
            Request {
                id: 50,
                method: "apps.open".into(),
                params: serde_json::json!({
                    "command": "gitsurveil-no-such-binary-xyz",
                    "path": "/tmp/wt",
                }),
            },
        )
        .await;
        let err = missing.error.unwrap();
        assert_eq!(err.code, "config_error");
        assert!(err.message.contains("is it installed and on PATH?"));
    }
}

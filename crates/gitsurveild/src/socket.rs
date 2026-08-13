//! The local API server (`specs/daemon.md`): newline-delimited JSON over a
//! unix domain socket (macOS/Linux) or a named pipe (Windows). Phase 1
//! Implements `status`, `items.{list,history,dismiss,undismiss}`,
//! `accounts.{add,list,remove}`, `rules.list`, and `poll.now`. Later phases
//! add more `match` arms to [`dispatch`] without touching the transport code.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Utc;
use gitsurveil_proto::{AccountRef, AuthKind, Request, Response, StatusResult};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::{Config, RepoConfig};
use crate::error::{DaemonError, Result};
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
    /// The live config. `rules`/`poll_interval_secs` are the startup snapshot
    /// (unchanged at runtime); `repos` is mutated through `repos.set` and
    /// `repos.remove`, which rewrite the file.
    pub config: Mutex<Config>,
    /// Where [`Config::save`] writes, so the API can persist its own changes.
    pub config_path: PathBuf,
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
    Branches,
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
}

impl PrParams {
    fn number(&self) -> Result<u64> {
        self.number
            .ok_or_else(|| DaemonError::InvalidParams("number is required".into()))
    }

    fn require<'a>(&self, value: Option<&'a String>, name: &str) -> Result<&'a str> {
        value
            .map(String::as_str)
            .ok_or_else(|| DaemonError::InvalidParams(format!("{name} is required")))
    }
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
        "pr.detail" => handle_pr(state, req.params, PrAction::Detail).await,
        "pr.create" => handle_pr(state, req.params, PrAction::Create).await,
        "pr.update" => handle_pr(state, req.params, PrAction::Update).await,
        "pr.close" => handle_pr(state, req.params, PrAction::Close).await,
        "pr.merge" => handle_pr(state, req.params, PrAction::Merge).await,
        "pr.comments" => handle_pr(state, req.params, PrAction::Comments).await,
        "pr.comment" => handle_pr(state, req.params, PrAction::Comment).await,
        "pr.branches" => handle_pr(state, req.params, PrAction::Branches).await,
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
        PrAction::Branches => serde_json::to_value(client.list_branches(repo).await?),
    };

    value.map_err(|e| DaemonError::Config(e.to_string()))
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

/// Returns the configured local clone paths (`specs/conflict-resolver.md`).
fn handle_repos_list(state: &ServerState) -> Result<serde_json::Value> {
    let repos = state.config.lock().expect("config mutex poisoned").repos.clone();
    Ok(serde_json::to_value(repos).expect("Vec<RepoConfig> always serializes"))
}

/// Registers a local clone path for one repo. Validates the path (is a git
/// repo, `origin` points at `owner/name`) before it's stored, so a typo'd
/// path can't silently disable conflict resolution later. Replaces any
/// previous entry for the same repo; writes the config through before
/// responding so the change survives a restart.
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

    let mut config = state.config.lock().expect("config mutex poisoned");
    config.repos.retain(|entry| entry.repo != params.repo);
    config.repos.push(RepoConfig {
        repo: params.repo,
        path: params.path,
    });
    config.save(&state.config_path)?;
    Ok(serde_json::to_value(&config.repos).expect("Vec<RepoConfig> always serializes"))
}

/// Removes a repo's local clone path. Idempotent: removing a repo that isn't
/// configured is a no-op rather than an error, so a UI retry can't wedge.
async fn handle_repos_remove(state: &ServerState, params: serde_json::Value) -> Result<serde_json::Value> {
    let params: RepoRemoveParams =
        serde_json::from_value(params).map_err(|e| DaemonError::InvalidParams(e.to_string()))?;
    let mut config = state.config.lock().expect("config mutex poisoned");
    let before = config.repos.len();
    config.repos.retain(|entry| entry.repo != params.repo);
    if config.repos.len() != before {
        config.save(&state.config_path)?;
    }
    Ok(serde_json::to_value(&config.repos).expect("Vec<RepoConfig> always serializes"))
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
    use gitsurveil_proto::{AccountRef, AuthKind};

    fn test_state() -> ServerState {
        ServerState {
            store: Arc::new(Store::open_in_memory().unwrap()),
            started_at: Instant::now(),
            rules: Vec::new(),
            config: Mutex::new(Config::default()),
            config_path: PathBuf::from(""),
        }
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
    async fn repos_remove_is_idempotent_and_persists() {
        let state = test_state();
        let params = serde_json::json!({ "repo": "acme/api" });
        let resp = dispatch(
            &state,
            Request { id: 8, method: "repos.remove".into(), params: params.clone() },
        )
        .await;
        let list = resp.result.unwrap();
        assert_eq!(list, serde_json::json!([]));
        // A second remove on the same repo must not error.
        let resp = dispatch(
            &state,
            Request { id: 9, method: "repos.remove".into(), params },
        )
        .await;
        assert!(resp.error.is_none());
    }
}

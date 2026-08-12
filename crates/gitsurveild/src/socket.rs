//! The local API server (`specs/daemon.md`): newline-delimited JSON over a
//! unix domain socket (macOS/Linux) or a named pipe (Windows). Phase 1
//! implements `status`, `items.list`, and `accounts.{add,list}` (PAT auth
//! only — OAuth device flow is Phase 5); later phases add more `match` arms
//! to [`dispatch`] without touching the transport code.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use gitsurveil_proto::{AccountRef, AuthKind, Request, Response, StatusResult};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{DaemonError, Result};
use crate::github::GitHubClient;
use crate::store::Store;
use crate::keychain;

/// Shared state every connection's [`dispatch`] call can read.
pub struct ServerState {
    pub store: Arc<Store>,
    pub started_at: Instant,
}

/// Params for `items.list`. All fields optional; an absent field means "no
/// filter on this dimension". Only account-independent listing (all open
/// items) is implemented in Phase 1 — filtering by kind/repo/severity is
/// added in `specs/desktop-ui.md`'s Phase 5 work.
#[derive(Debug, Default, Deserialize)]
struct ItemsListParams {}

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
        "accounts.list" => handle_accounts_list(state),
        "accounts.add" => handle_accounts_add(state, req.params).await,
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
    let open_item_count = state.store.open_items()?.len();
    let status = StatusResult {
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_secs: state.started_at.elapsed().as_secs(),
        account_count,
        open_item_count,
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
    Ok(serde_json::to_value(items).expect("Vec<ActionItem> always serializes"))
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
}

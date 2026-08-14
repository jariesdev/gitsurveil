//! Client for the daemon's local API (`specs/daemon.md`).
//!
//! The app is a thin client: it holds no domain state, it asks `gitsurveild`
//! for everything. Each call opens a short-lived connection, writes one
//! newline-delimited JSON request, and reads one response line — the daemon
//! handles each connection independently, so there's no session to manage and
//! nothing to keep open while the popover is closed.

use std::path::PathBuf;

use gitsurveil_proto::{
    AccountRef, CloneStatus, RepoCatalog, Repository, Request, Response, ScoredItem, StatusResult,
};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Something went wrong talking to the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonClientError {
    /// The socket/pipe couldn't be reached — almost always "daemon isn't
    /// running", which the UI surfaces as a distinct state rather than an
    /// error toast.
    #[error("cannot reach the gitsurveil service: {0}")]
    Unreachable(#[from] std::io::Error),
    /// The daemon replied, but with an error payload.
    #[error("{0}")]
    Daemon(String),
    /// The reply couldn't be decoded — a protocol mismatch between app and
    /// daemon versions.
    #[error("unexpected reply from the service: {0}")]
    Decode(#[from] serde_json::Error),
    /// The connection closed before a reply arrived.
    #[error("the service closed the connection without replying")]
    NoReply,
}

/// Result alias for daemon calls.
pub type Result<T> = std::result::Result<T, DaemonClientError>;

/// Where the daemon listens. Must stay in sync with `gitsurveild`'s
/// `local_api_address` — both derive it from the same platform rules in
/// `specs/architecture.md`.
fn address() -> String {
    #[cfg(windows)]
    {
        r"\\.\pipe\gitsurveil".to_string()
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "linux")]
        if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
            return PathBuf::from(runtime_dir)
                .join("gitsurveil.sock")
                .to_string_lossy()
                .into_owned();
        }
        data_dir()
            .join("daemon.sock")
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(not(windows))]
fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("io", "gitsurveil", "gitsurveil")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Sends one request and decodes the result into `T`.
async fn call<T: DeserializeOwned>(method: &str, params: serde_json::Value) -> Result<T> {
    let request = Request {
        // Single request per connection, so the id only needs to be echoed
        // back — there's no multiplexing to disambiguate.
        id: 1,
        method: method.to_string(),
        params,
    };
    let mut line = serde_json::to_vec(&request)?;
    line.push(b'\n');

    let reply = exchange(&line).await?;
    let response: Response = serde_json::from_str(&reply)?;
    if let Some(err) = response.error {
        return Err(DaemonClientError::Daemon(err.message));
    }
    let value = response.result.unwrap_or(serde_json::Value::Null);
    Ok(serde_json::from_value(value)?)
}

#[cfg(not(windows))]
async fn exchange(request_line: &[u8]) -> Result<String> {
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(address()).await?;
    let (read_half, mut write_half) = stream.into_split();
    write_half.write_all(request_line).await?;
    let mut reader = BufReader::new(read_half);
    let mut reply = String::new();
    if reader.read_line(&mut reply).await? == 0 {
        return Err(DaemonClientError::NoReply);
    }
    Ok(reply)
}

#[cfg(windows)]
async fn exchange(request_line: &[u8]) -> Result<String> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let stream = ClientOptions::new().open(address())?;
    let (read_half, mut write_half) = tokio::io::split(stream);
    write_half.write_all(request_line).await?;
    let mut reader = BufReader::new(read_half);
    let mut reply = String::new();
    if reader.read_line(&mut reply).await? == 0 {
        return Err(DaemonClientError::NoReply);
    }
    Ok(reply)
}

/// Fetches the daemon's health/status summary.
pub async fn status() -> Result<StatusResult> {
    call("status", serde_json::Value::Null).await
}

/// Fetches every currently open action item, already scored and ordered by
/// the daemon.
pub async fn list_items() -> Result<Vec<ScoredItem>> {
    call("items.list", serde_json::Value::Null).await
}

/// Fetches resolved and dismissed items for the history view.
pub async fn list_history(limit: Option<usize>) -> Result<Vec<ScoredItem>> {
    let params = match limit {
        Some(limit) => serde_json::json!({ "limit": limit }),
        None => serde_json::Value::Null,
    };
    call("items.history", params).await
}

/// Sets an item's locally-dismissed state.
pub async fn set_dismissed(id: &str, dismissed: bool) -> Result<()> {
    let method = if dismissed {
        "items.dismiss"
    } else {
        "items.undismiss"
    };
    let _: serde_json::Value = call(method, serde_json::json!({ "id": id })).await?;
    Ok(())
}

/// Validates a token against `host` and registers the account.
pub async fn add_account(host: &str, token: &str, api_base: Option<&str>) -> Result<AccountRef> {
    let mut params = serde_json::json!({ "host": host, "token": token });
    if let Some(api_base) = api_base {
        params["api_base"] = serde_json::Value::String(api_base.to_string());
    }
    call("accounts.add", params).await
}

/// Removes an account, its items, and its stored token.
pub async fn remove_account(id: &str) -> Result<()> {
    let _: serde_json::Value = call("accounts.remove", serde_json::json!({ "id": id })).await?;
    Ok(())
}

/// Lists configured accounts.
pub async fn list_accounts() -> Result<Vec<AccountRef>> {
    call("accounts.list", serde_json::Value::Null).await
}

/// Lists the active priority rules.
pub async fn list_rules() -> Result<serde_json::Value> {
    call("rules.list", serde_json::Value::Null).await
}

/// Lists the repository catalog: every discovered repo with its tracked/clone
/// state, plus the orgs each account can filter by (`specs/desktop-ui.md`).
pub async fn repos_list() -> Result<RepoCatalog> {
    call("repos.list", serde_json::Value::Null).await
}

/// Registers a local clone path for one repo (validated by the daemon).
/// Marks the repo tracked and acks it as seen.
pub async fn repos_set(repo: &str, path: &str) -> Result<Repository> {
    call("repos.set", serde_json::json!({ "repo": repo, "path": path })).await
}

/// Removes a repo's local clone path; idempotent. The catalog row survives.
pub async fn repos_remove(repo: &str) -> Result<()> {
    let _: serde_json::Value = call("repos.remove", serde_json::json!({ "repo": repo })).await?;
    Ok(())
}

/// Repositories discovered but never acknowledged, newest-first.
pub async fn repos_new() -> Result<Vec<Repository>> {
    call("repos.new", serde_json::Value::Null).await
}

/// Dismisses every currently-new repository (`specs/desktop-ui.md`), returning
/// how many rows were acknowledged. `first_seen_at` is the dismissal watermark.
pub async fn repos_ack_new(first_seen_at: &str) -> Result<u64> {
    call("repos.ack_new", serde_json::json!({ "first_seen_at": first_seen_at })).await
}

/// Forces a discovery cycle for every account, returning the fresh catalog.
pub async fn repos_refresh() -> Result<RepoCatalog> {
    call("repos.refresh", serde_json::Value::Null).await
}

/// Starts a background clone of `repo` into `target`, returning a `job_id` the
/// UI polls via [`repos_clone_status`]. HTTPS only; the account's keychain
/// token is the credential.
pub async fn repos_clone(repo: &str, target: &str) -> Result<String> {
    call("repos.clone", serde_json::json!({ "repo": repo, "target": target })).await
}

/// One clone job's current status, or `None` for an unknown job id.
pub async fn repos_clone_status(job_id: &str) -> Result<Option<CloneStatus>> {
    call("repos.clone_status", serde_json::json!({ "job_id": job_id })).await
}

/// Asks the daemon to poll now rather than waiting for the next cycle.
pub async fn poll_now() -> Result<()> {
    let _: serde_json::Value = call("poll.now", serde_json::Value::Null).await?;
    Ok(())
}

/// Forwards one `pr.*` call to the daemon.
///
/// Returns the raw JSON: the app is a pass-through for these, and typing the
/// eight response shapes twice (here and in TypeScript) would buy nothing the
/// UI doesn't already get from the generated types.
pub async fn pr_call(method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    call(method, params).await
}

/// Forwards one `conflicts.*` call to the daemon (`specs/conflict-resolver.md`).
///
/// Like `pr_call`, a raw pass-through: the UI renders the session and file
/// payloads straight off the wire.
pub async fn conflicts_call(method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    call(method, params).await
}

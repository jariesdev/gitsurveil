//! Client for the daemon's local API (`specs/daemon.md`).
//!
//! The app is a thin client: it holds no domain state, it asks `gitsurveild`
//! for everything. Each call opens a short-lived connection, writes one
//! newline-delimited JSON request, and reads one response line — the daemon
//! handles each connection independently, so there's no session to manage and
//! nothing to keep open while the popover is closed.

use std::path::PathBuf;

use gitsurveil_proto::{
    AccountRef, CloneStatus, KindPref, RegisteredApp, RepoCatalog, Repository, Request, Response,
    ScoredItem, StatusResult, WorktreeInfo, WorktreesResult,
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

/// Archives every resolved and dismissed item: they leave the Dashboard and
/// history permanently and never come back, even if still open on GitHub.
pub async fn clear_history() -> Result<()> {
    let _: serde_json::Value = call("items.clear_history", serde_json::Value::Null).await?;
    Ok(())
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

/// Validates a new token against the existing account's GitHub instance, then
/// replaces the old token in the OS keychain.
pub async fn update_account_token(id: &str, token: &str) -> Result<AccountRef> {
    call(
        "accounts.update_token",
        serde_json::json!({ "id": id, "token": token }),
    )
    .await
}

/// Lists configured accounts.
pub async fn list_accounts() -> Result<Vec<AccountRef>> {
    call("accounts.list", serde_json::Value::Null).await
}

/// Lists the active priority rules.
pub async fn list_rules() -> Result<serde_json::Value> {
    call("rules.list", serde_json::Value::Null).await
}

/// Lists every item kind's current notification preference.
pub async fn notifications_prefs() -> Result<Vec<KindPref>> {
    call("notifications.prefs", serde_json::Value::Null).await
}

/// Sets whether `kind` may produce a notification.
pub async fn notifications_set_pref(kind: &str, enabled: bool) -> Result<()> {
    let _: serde_json::Value = call(
        "notifications.set_pref",
        serde_json::json!({ "kind": kind, "enabled": enabled }),
    )
    .await?;
    Ok(())
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

/// Sets whether a repo's items feed notifications and the Pull Requests
/// view, independent of its clone-tracking state.
pub async fn repos_set_notify(account_id: &str, repo: &str, enabled: bool) -> Result<Repository> {
    call(
        "repos.set_notify",
        serde_json::json!({ "account_id": account_id, "repo": repo, "enabled": enabled }),
    )
    .await
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

/// A repo's user-created worktrees plus the branches a new one can be created
/// from (`specs/desktop-ui.md`). Derived from the clone's git metadata on each
/// call, so worktrees made outside gitsurveil show up too.
pub async fn repos_worktrees(repo: &str) -> Result<WorktreesResult> {
    call("repos.worktrees", serde_json::json!({ "repo": repo })).await
}

/// Creates a worktree for `branch` at `path` and returns its info. `branch`
/// may be an existing local/remote branch or a brand-new name; `path` may be
/// relative to the clone's parent. Errors if the target is non-empty or the
/// branch is checked out elsewhere — nothing pre-existing is ever touched.
pub async fn repos_worktree_add(repo: &str, branch: &str, path: &str) -> Result<WorktreeInfo> {
    call(
        "repos.worktree_add",
        serde_json::json!({ "repo": repo, "branch": branch, "path": path }),
    )
    .await
}

/// Removes a worktree (unregisters it and deletes its working directory),
/// keeping the checked-out branch. Refuses dirty worktrees and conflict
/// sessions. Pass `force` to skip the dirty-check.
pub async fn repos_worktree_remove(repo: &str, name: &str, force: bool) -> Result<()> {
    let _: serde_json::Value = call(
        "repos.worktree_remove",
        serde_json::json!({ "repo": repo, "name": name, "force": force }),
    )
    .await?;
    Ok(())
}

/// Asks the daemon to poll now rather than waiting for the next cycle.
pub async fn poll_now() -> Result<()> {
    let _: serde_json::Value = call("poll.now", serde_json::Value::Null).await?;
    Ok(())
}

/// Lists the registered "Open with" applications (`specs/desktop-ui.md`),
/// sorted by display name.
pub async fn apps_list() -> Result<Vec<RegisteredApp>> {
    call("apps.list", serde_json::Value::Null).await
}

/// Registers an application for the worktree "Open with" menu. `command` is a
/// bare executable name on `PATH`; the daemon rejects anything else.
pub async fn apps_add(name: &str, command: &str) -> Result<RegisteredApp> {
    call("apps.add", serde_json::json!({ "name": name, "command": command })).await
}

/// Forgets a registered application; idempotent.
pub async fn apps_remove(command: &str) -> Result<()> {
    let _: serde_json::Value = call("apps.remove", serde_json::json!({ "command": command })).await?;
    Ok(())
}

/// Opens `path` with a registered application (`apps.open`). The daemon
/// validates the app is registered and launches `command <path>` — no shell.
pub async fn apps_open(command: &str, path: &str) -> Result<()> {
    let _: serde_json::Value =
        call("apps.open", serde_json::json!({ "command": command, "path": path })).await?;
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

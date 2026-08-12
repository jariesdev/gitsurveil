//! Client for the daemon's local API (`specs/daemon.md`).
//!
//! The app is a thin client: it holds no domain state, it asks `gitsurveild`
//! for everything. Each call opens a short-lived connection, writes one
//! newline-delimited JSON request, and reads one response line — the daemon
//! handles each connection independently, so there's no session to manage and
//! nothing to keep open while the popover is closed.

use std::path::PathBuf;

use gitsurveil_proto::{Request, Response, ScoredItem, StatusResult};
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

//! The local API envelope described in `specs/daemon.md`: newline-delimited
//! JSON request/response over a unix socket (or Windows named pipe).
//!
//! Only a generic envelope lives here — per-method `params`/`result` shapes
//! are added as each method is implemented (KISS: no point pre-declaring the
//! full v1 surface before the phases that need it exist).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request sent by a client (the Tauri app, or `curl --unix-socket` in dev)
/// to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Caller-assigned id, echoed back on the matching [`Response`].
    pub id: u64,
    /// Method name, e.g. `"status"` or `"items.list"`.
    pub method: String,
    /// Method-specific parameters, or `null` for parameterless methods.
    #[serde(default)]
    pub params: Value,
}

/// The daemon's reply to a [`Request`]. Exactly one of `result`/`error` is set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Echoes [`Request::id`].
    pub id: u64,
    /// The method's result on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Present instead of `result` on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorPayload>,
}

/// Error detail attached to a failed [`Response`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    /// Short machine-readable code, e.g. `"auth_error"`, `"not_found"`.
    pub code: String,
    /// Human-readable message safe to show in a UI.
    pub message: String,
}

/// Result payload for the `status` method: what "is the daemon healthy"
/// means per `specs/daemon.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResult {
    /// Daemon crate version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// Seconds since the daemon process started.
    pub uptime_secs: u64,
    /// Number of configured accounts.
    pub account_count: usize,
    /// Number of currently open (non-dismissed, non-done) items across all accounts.
    pub open_item_count: usize,
}

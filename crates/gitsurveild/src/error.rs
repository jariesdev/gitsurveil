//! The daemon's single error type. Kept flat (no per-module error enums) —
//! KISS: every fallible operation in this crate ends up surfaced either as a
//! log line or as an [`gitsurveil_proto::ErrorPayload`], and both only need
//! a code + message, not a rich type hierarchy.

use gitsurveil_proto::ErrorPayload;

/// Errors that can occur anywhere in the daemon.
#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    /// SQLite storage failure.
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),
    /// GitHub API call failed.
    #[error("github api error: {0}")]
    GitHub(#[from] octocrab::Error),
    /// GitHub rejected a REST request. Carries GitHub's own message, which is
    /// the part that tells the user what to do ("Validation Failed", which
    /// scope is missing), so it is surfaced verbatim.
    #[error("{0}")]
    GitHubApi(String),
    /// OS keychain access failed.
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
    /// Config file could not be read or parsed.
    #[error("config error: {0}")]
    Config(String),
    /// I/O failure (socket, file).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Request referenced an account id that isn't configured.
    #[error("unknown account: {0}")]
    UnknownAccount(String),
    /// A client sent a request the daemon doesn't understand.
    #[error("unknown method: {0}")]
    UnknownMethod(String),
    /// Request params didn't match what the method expects.
    #[error("invalid params: {0}")]
    InvalidParams(String),
}

impl DaemonError {
    /// Map this error to the machine-readable code sent to clients. The
    /// message stays as `Display` output — every variant above is already
    /// written to be safe to show in a UI.
    pub fn code(&self) -> &'static str {
        match self {
            DaemonError::Storage(_) => "storage_error",
            DaemonError::GitHub(_) => "github_error",
            DaemonError::GitHubApi(_) => "github_error",
            DaemonError::Keychain(_) => "keychain_error",
            DaemonError::Config(_) => "config_error",
            DaemonError::Io(_) => "io_error",
            DaemonError::UnknownAccount(_) => "unknown_account",
            DaemonError::UnknownMethod(_) => "unknown_method",
            DaemonError::InvalidParams(_) => "invalid_params",
        }
    }
}

impl From<DaemonError> for ErrorPayload {
    fn from(err: DaemonError) -> Self {
        ErrorPayload {
            code: err.code().to_string(),
            message: err.to_string(),
        }
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, DaemonError>;

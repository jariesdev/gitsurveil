//! Daemon configuration: the human-editable TOML file living next to the
//! SQLite database (`specs/daemon.md`). Holds polling/behavior settings and
//! (from Phase 4 on) priority rules. Accounts live in SQLite
//! (`crate::store`) since they're mutated via the local API, not hand-edited;
//! tokens are never here — only in the OS keychain (`crate::keychain`).

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::error::{DaemonError, Result};
use crate::priority::Rule;

/// One configured local clone path from pre-catalog configs (`specs/desktop-ui.md`).
/// Repos now live in SQLite (`crate::store`); this struct survives only so
/// startup can import the old `repos` block into the catalog once. The path
/// was validated on `repos.set` back then (is a git repo, `origin` matches).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoConfig {
    /// `"owner/name"` exactly as it appears on GitHub.
    pub repo: String,
    /// Absolute path to a local clone of that repository. Never resolved
    /// against the daemon's working directory — kept as given.
    pub path: PathBuf,
}

/// Default polling interval, in seconds. GitHub's own `x-poll-interval` on
/// `/notifications` floors at 60s; we default to matching it exactly and the
/// poller raises it dynamically if GitHub asks for a longer interval.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 60;

/// The full daemon configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// How often to poll each account, in seconds.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Priority rules (`specs/priority-engine.md`). Hand-editable; a graphical
    /// editor arrives in Phase 5 and will write through this same file.
    #[serde(default = "crate::priority::default_rules")]
    pub rules: Vec<Rule>,
}

/// The `repos` block written by pre-catalog versions. `Config::load` ignores
/// unknown keys, so the main config keeps parsing even when this key is
/// present; startup reads it out of the raw file once, imports it, and
/// rewrites the config without it.
#[derive(Debug, Deserialize)]
pub struct LegacyConfig {
    /// Clone paths registered before the catalog existed.
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

fn default_poll_interval() -> u64 {
    DEFAULT_POLL_INTERVAL_SECS
}

impl Default for Config {
    fn default() -> Self {
        Config {
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            rules: crate::priority::default_rules(),
        }
    }
}

/// Resolves the platform-appropriate data directory for gitsurveil
/// (`~/Library/Application Support/gitsurveil` on macOS,
/// `~/.local/share/gitsurveil` on Linux, `%APPDATA%\gitsurveil` on Windows),
/// creating it if it doesn't exist.
pub fn data_dir() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("io", "gitsurveil", "gitsurveil")
        .ok_or_else(|| DaemonError::Config("could not resolve a home directory".into()))?;
    let dir = dirs.data_dir().to_path_buf();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Config {
    /// Loads the config from `path`, or returns [`Config::default`] if the
    /// file doesn't exist yet (first run).
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = std::fs::read_to_string(path)?;
        toml::from_str(&raw).map_err(|e| DaemonError::Config(e.to_string()))
    }

    /// Reads just the legacy `repos` block out of `path`, returning `None`
    /// when the file is missing, has no `repos` key, or that block is empty.
    /// Callers import whatever comes back into the catalog and then rewrite
    /// the file via [`Config::save`], so the stale block is seen only once.
    pub fn load_legacy_repos(path: &Path) -> Option<Vec<RepoConfig>> {
        let raw = std::fs::read_to_string(path).ok()?;
        let legacy: LegacyConfig = toml::from_str(&raw).ok()?;
        if legacy.repos.is_empty() {
            None
        } else {
            Some(legacy.repos)
        }
    }

    /// Writes the config back to `path`. Startup uses this after importing a
    /// legacy `repos` block, to rewrite the file without the stale key.
    pub fn save(&self, path: &Path) -> Result<()> {
        let raw = toml::to_string_pretty(self).map_err(|e| DaemonError::Config(e.to_string()))?;
        std::fs::write(path, raw)?;
        Ok(())
    }
}

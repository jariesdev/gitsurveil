//! Registered "Open with" applications (`apps.*` methods).
//!
//! A registered app is a command-line executable on `PATH` that can open a
//! directory — VS Code (`code`), PhpStorm (`phpstorm`), Sublime Merge
//! (`smerge`). The daemon launches `command <path>` when the user picks an app
//! from a worktree's context menu, so the record is just the display name and
//! the bare executable; no shell is involved.

use serde::{Deserialize, Serialize};

/// An application the user has registered to open worktrees with.
///
/// Keyed by `command` — the bare executable name resolved on `PATH` — so the
/// same command can't be registered twice under a different display name.
/// `name` is what the UI shows in the "Open with" submenu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisteredApp {
    /// Display name shown in the "Open with" submenu.
    pub name: String,
    /// Bare command-line executable on `PATH`, e.g. `code`. The daemon runs it
    /// as `command <path>` (never through a shell).
    pub command: String,
}

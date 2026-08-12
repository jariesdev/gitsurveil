//! Priority scoring output shared with the UIs (`specs/priority-engine.md`).
//!
//! Scores and severities are *computed*, never stored: the daemon recalculates
//! them every poll because age escalation means an untouched item's priority
//! changes with nothing but the passage of time. They therefore live here as a
//! view over an [`crate::ActionItem`] rather than as fields on it.

use serde::{Deserialize, Serialize};

use crate::ActionItem;

/// Coarse urgency band derived from a numeric score. Drives the tray icon
/// color and the grouping headers in the UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Nothing open at all — tray shows its resting icon.
    Idle,
    /// Visible in lists, but never worth an interruption on its own.
    Info,
    /// Ordinary work.
    Normal,
    /// Someone is blocked on you.
    High,
    /// Broken build or equivalent; always interrupts.
    Critical,
}

impl Severity {
    /// Maps a score to its band, per the table in `specs/priority-engine.md`.
    ///
    /// A score of zero means "no open items"; any real item scores at least 1
    /// because negative rule modifiers are clamped.
    pub fn from_score(score: u32) -> Severity {
        match score {
            0 => Severity::Idle,
            1..=29 => Severity::Info,
            30..=59 => Severity::Normal,
            60..=99 => Severity::High,
            _ => Severity::Critical,
        }
    }
}

/// An [`ActionItem`] together with the priority the engine assigned it.
///
/// Serializes flat, so the TypeScript side sees one object with the item's
/// fields plus `score`, `severity`, and `muted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoredItem {
    /// The underlying item.
    #[serde(flatten)]
    pub item: ActionItem,
    /// Numeric priority; higher is more urgent.
    pub score: u32,
    /// Band derived from `score`.
    pub severity: Severity,
    /// Whether a rule suppressed desktop notifications for this item. It still
    /// appears in lists — muting silences, it doesn't hide.
    pub muted: bool,
}

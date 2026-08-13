//! Conflict-resolution types shared with the UI (`specs/conflict-resolver.md`).
//!
//! The three-pane editor is built from these: a file is a list of ordered
//! [`ConflictSegment`]s — plain context or a conflict hunk — and the UI
//! resolves hunks by replacing a `Conflict` segment with a `Context` one. Line
//! content is kept **verbatim** (line terminators included) so serializing
//! segments back to text is byte-exact: that's what makes "the center pane
//! showed exactly what ends up on GitHub" (AC-6.5) hold by construction.

use serde::{Deserialize, Serialize};

/// One ordered piece of a conflicted file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConflictSegment {
    /// Unconflicted lines. Shown identically in all three panes and never
    /// edited; only conflict hunks change during resolution.
    Context {
        /// Verbatim lines (terminators included).
        lines: Vec<String>,
    },
    /// One `<<<<<<<` … `>>>>>>>` conflict block.
    Conflict {
        /// The parsed hunk: panes' sides plus the verbatim marker block.
        hunk: ConflictHunk,
    },
}

/// One conflict block in a file. `ours` is the PR branch's side, `theirs` the
/// base branch's; in the three-pane UI ours renders left, theirs right, and
/// the raw marker block renders (editable) in the center.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictHunk {
    /// 1-based line number of the `<<<<<<<` marker in the original file.
    pub start_line: usize,
    /// 1-based line number of the `>>>>>>>` marker (inclusive).
    pub end_line: usize,
    /// The verbatim marker block: `<<<<<<<` … `>>>>>>>` exactly as git wrote
    /// it, terminators included. The center pane's initial content.
    pub raw: Vec<String>,
    /// Text after `<<<<<<<`, e.g. `HEAD` or a branch name. Git's own label;
    /// shown in the panes' gutter.
    pub ours_label: Option<String>,
    /// The PR branch's version of the lines (between `<<<<<<<` and the next
    /// marker).
    pub ours: Vec<String>,
    /// The base version, present only when git wrote a diff3-style conflict
    /// (`|||||||` section). Rendered in the center pane under a divider so
    /// the user can see what each side changed relative to.
    pub base: Option<Vec<String>>,
    /// Text after `|||||||`, when present.
    pub base_label: Option<String>,
    /// The base branch's version of the lines (between `=======` and
    /// `>>>>>>>`).
    pub theirs: Vec<String>,
    /// Text after `>>>>>>>`, when present.
    pub theirs_label: Option<String>,
}

/// One conflicted file, as served by `conflicts.file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictFile {
    /// Path relative to the repository root.
    pub path: String,
    /// Binary file: no marker parsing, whole-file pick only.
    pub binary: bool,
    /// Larger than the text-editing threshold (5 MB): whole-file pick only.
    pub large: bool,
    /// Ordered segments of the file. Empty when `binary` or `large` — those
    /// files are resolved whole-file or not at all.
    pub segments: Vec<ConflictSegment>,
}

/// One row of the file list returned by `conflicts.prepare`, so the UI can
/// show counts before the first file is opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictFileSummary {
    /// Path relative to the repository root.
    pub path: String,
    /// Number of conflict hunks in the file.
    pub conflicts: usize,
    /// Binary file: whole-file pick only.
    pub binary: bool,
    /// Larger than the text-editing threshold: whole-file pick only.
    pub large: bool,
}

/// Result of `conflicts.prepare`: the session the rest of the resolver API
/// addresses, plus the files that need resolving.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictSession {
    /// Opaque session id, passed to `conflicts.file`/`save`/`commit`/`abort`.
    pub session_id: String,
    /// `"owner/name"`.
    pub repo: String,
    /// PR number.
    pub number: u64,
    /// Base branch being merged in (the "theirs" side).
    pub base: String,
    /// Head branch being merged into (the "ours" side).
    pub head: String,
    /// Absolute path of the temporary worktree. Exists so an operator can
    /// verify resolution never happens in the user's clone; the UI does not
    /// act on it.
    pub worktree_path: String,
    /// Conflicted files, each with its conflict count.
    pub files: Vec<ConflictFileSummary>,
}

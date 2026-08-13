//! Conflict resolution: pure marker parsing and the temp-worktree session
//! lifecycle (`specs/conflict-resolver.md`).
//!
//! [`parse`] turns conflicted file text into ordered segments (the three-pane
//! editor's data); [`session`] owns the temporary worktree a resolution lives
//! in. The socket layer (`crate::socket`) stays thin — it only translates
//! requests into these calls.

pub mod parse;
pub mod session;

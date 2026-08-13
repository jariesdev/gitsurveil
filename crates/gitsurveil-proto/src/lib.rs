//! Shared wire types between `gitsurveild` (the daemon) and the Tauri app.
//!
//! This crate is the single source of truth for the local API described in
//! `specs/daemon.md`. Rust callers use these types directly; TypeScript
//! callers consume a generated equivalent so the two sides can never drift.
//! Kept dependency-light (serde only) since it's linked into both the daemon
//! and, eventually, the Tauri shell.

#![warn(missing_docs)]

mod item;
mod pr;
mod priority;
mod rpc;

pub use item::{AccountRef, ActionItem, AuthKind, CiStatus, ItemKind, ItemState};
pub use pr::{Check, Comment, MergeMethod, Mergeability, PullRequestDetail, Reviewer};
pub use priority::{ScoredItem, Severity};
pub use rpc::{ErrorPayload, Request, Response, StatusResult};

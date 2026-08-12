//! Everything that talks to GitHub: the polling client and the pure diff
//! logic that turns two snapshots into a set of changes.

pub mod client;
pub mod diff;

pub use client::GitHubClient;

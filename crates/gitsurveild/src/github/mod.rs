//! Everything that talks to GitHub: the polling client and the pure diff
//! logic that turns two snapshots into a set of changes.

pub mod client;
pub mod diff;
pub mod pr;

pub use client::GitHubClient;
pub use pr::PrPatch;

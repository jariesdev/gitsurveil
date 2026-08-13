//! Local git operations for the conflict resolver (`specs/conflict-resolver.md`).
//!
//! Everything in here runs against a configured clone path and, from Phase 7
//! on, temporary worktrees attached to it. Two hard rules shape the module:
//!
//! - **Never touch the user's working tree.** Conflict resolution happens in
//!   worktrees the daemon creates and owns; the user's checkout is only ever
//!   read (and, for pushes, read in the form of the worktree's own remotes).
//! - **`git2` objects are `!Send`.** Every call in this module is cheap and
//!   self-contained on purpose so callers can wrap them in
//!   `tokio::task::spawn_blocking` from async contexts.

use std::path::Path;

use git2::Repository;

use crate::error::{DaemonError, Result};

/// Validates that `path` is a usable clone of GitHub repo `repo`
/// (`"owner/name"`), the prerequisite check for `repos.set`.
///
/// Checks, in order: the path opens as a git repository, it has an `origin`
/// remote, and that remote's URL mentions `owner/name`. Failures carry a
/// message that tells the user what to fix rather than just what broke.
pub fn validate_clone(repo: &str, path: &Path) -> Result<()> {
    let repository = Repository::open(path)
        .map_err(|e| DaemonError::Config(format!("{} is not a git repository: {e}", path.display())))?;
    let origin = repository
        .find_remote("origin")
        .map_err(|_| DaemonError::Config(format!("{} has no `origin` remote", path.display())))?;
    let url = origin
        .url()
        .map_err(|e| DaemonError::Config(format!("{}'s `origin` remote has no URL: {e}", path.display())))?;
    if !url.contains(repo) {
        return Err(DaemonError::Config(format!(
            "{}'s `origin` remote (`{url}`) does not point at `{repo}`",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Builds a throwaway git repo in `dir` with `origin` set to a fake GitHub
    /// URL, so validation can be tested without network access.
    fn scaffold_repo(dir: &std::path::Path, origin_url: &str) {
        let git = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-b", "main", "."]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("file.txt"), "hello\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        git(&["remote", "add", "origin", origin_url]);
    }

    #[test]
    fn accepts_a_clone_whose_origin_matches() {
        let dir = std::env::temp_dir().join(format!("gs-validate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        scaffold_repo(&dir, "https://github.com/acme/api.git");
        let result = validate_clone("acme/api", &dir);
        std::fs::remove_dir_all(&dir).ok();
        result.expect("valid clone should pass");
    }

    #[test]
    fn rejects_an_origin_that_points_elsewhere() {
        let dir = std::env::temp_dir().join(format!("gs-validate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        scaffold_repo(&dir, "https://github.com/other/project.git");
        let err = validate_clone("acme/api", &dir).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(err.to_string().contains("does not point at `acme/api`"));
    }

    #[test]
    fn rejects_a_directory_that_is_not_a_repo() {
        let dir = std::env::temp_dir().join(format!("gs-validate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = validate_clone("acme/api", &dir).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(err.to_string().contains("not a git repository"));
    }
}

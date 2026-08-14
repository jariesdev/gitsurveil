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

/// Clones `clone_url` into `target` using `token` as an HTTPS credential
/// (`specs/desktop-ui.md`). Used by the daemon's background `repos.clone`
/// jobs, which own the target path only when they created it — cleanup on a
/// failed clone is the caller's job and must never touch a pre-existing path.
///
/// `on_progress` is called with `(received_bytes, total_bytes)` as git
/// fetches; `total` stays 0 (git2 can't predict the final pack size). The
/// callback runs on the cloning thread, so it must stay cheap.
///
/// Refuses to clone into a directory that already exists with content: a
/// fresh clone there would merge with whatever is already in it.
pub fn clone_repo<F>(
    clone_url: &str,
    login: &str,
    token: &str,
    target: &Path,
    mut on_progress: F,
) -> Result<()>
where
    F: FnMut(u64, u64),
{
    if target.exists() {
        let non_empty = std::fs::read_dir(target)?.next().is_some();
        if non_empty {
            return Err(DaemonError::Config(format!(
                "{} already exists and is not empty; pick a new folder or clear it",
                target.display()
            )));
        }
    }

    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(|_url, username_from_url, _allowed| {
        // GitHub accepts the token as the HTTPS password; the username is
        // ignored but the account login is the conventional value.
        git2::Cred::userpass_plaintext(username_from_url.unwrap_or(login), token)
    });
    callbacks.transfer_progress(|stats| {
        on_progress(stats.received_bytes() as u64, 0);
        true
    });

    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(callbacks);
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_options);

    builder.clone(clone_url, target)?;
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

    #[test]
    fn clone_refuses_a_preexisting_non_empty_target_without_touching_it() {
        // Regression: a failed clone must never delete content that was in
        // the target before the job started.
        let dir = std::env::temp_dir().join(format!("gs-clone-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let survivor = dir.join("keep.txt");
        std::fs::write(&survivor, "do not delete\n").unwrap();
        let mut sentinel_ok = true;
        let result = clone_repo("https://github.com/acme/api.git", "me", "tok", &dir, |_, _| {
            sentinel_ok = false;
        });
        assert!(result.is_err(), "clone must refuse a non-empty target");
        assert!(sentinel_ok, "clone must fail before any transfer");
        assert!(survivor.exists(), "preexisting file must survive a failed clone");
        assert_eq!(std::fs::read_to_string(&survivor).unwrap(), "do not delete\n");
        std::fs::remove_dir_all(&dir).ok();
    }
}

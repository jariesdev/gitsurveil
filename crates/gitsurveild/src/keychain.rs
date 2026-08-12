//! Thin wrapper around the OS keychain for GitHub tokens.
//!
//! This is the *only* place a token is allowed to touch memory outside the
//! GitHub client itself — never SQLite, never the config file, never a log
//! line (`CLAUDE.md` hard rule). Keyed by account id so it lines up 1:1 with
//! [`gitsurveil_proto::AccountRef::id`].

use keyring::v1::Entry;

use crate::error::Result;

const SERVICE: &str = "io.gitsurveil.daemon";

/// Stores `token` in the OS keychain for `account_id`, overwriting any
/// existing value.
pub fn set_token(account_id: &str, token: &str) -> Result<()> {
    let entry = Entry::new(SERVICE, account_id)?;
    entry.set_password(token)?;
    Ok(())
}

/// Retrieves the token stored for `account_id`, if any.
pub fn get_token(account_id: &str) -> Result<Option<String>> {
    let entry = Entry::new(SERVICE, account_id)?;
    match entry.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Removes the token stored for `account_id` (used by `accounts.remove`).
pub fn delete_token(account_id: &str) -> Result<()> {
    let entry = Entry::new(SERVICE, account_id)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

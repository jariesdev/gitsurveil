//! Login registration for the daemon (`specs/architecture.md`).
//!
//! Until this exists the user has to start `gitsurveild --foreground` in a
//! terminal at every login, which is the difference between a demo and a tool
//! you actually rely on. Registration is per-user and needs no elevation on
//! any of the three platforms: a launchd *user* agent, a systemd *user* unit,
//! or the per-user `Run` registry key.
//!
//! Deliberately writes plain text files (or one registry value) rather than
//! taking a dependency: each platform's format is a dozen lines, and a crate
//! that abstracts all three would hide exactly the details a user needs when
//! something does not start.

use std::path::PathBuf;

use crate::error::{DaemonError, Result};

/// Reverse-DNS identifier, used as the launchd label and the systemd unit
/// name. Matches the app's bundle identifier so the two are recognisably one
/// product in system tooling.
pub const SERVICE_ID: &str = "io.gitsurveil.daemon";

/// Whether the daemon is registered to start at login, and where that
/// registration lives so the user can inspect or delete it by hand.
#[derive(Debug)]
pub struct ServiceStatus {
    /// True when the registration exists.
    pub registered: bool,
    /// The plist / unit file / registry value backing it.
    pub location: String,
    /// The binary the registration points at, when registered. A stale path
    /// here (after `cargo build` into a new location, say) is the most common
    /// reason a registered service silently fails to start.
    pub program: Option<String>,
}

/// Absolute path to the currently running binary, which is what gets
/// registered. Resolved rather than assumed: registering `gitsurveild` by
/// name would depend on the login shell's `PATH`, which for a GUI login
/// session is not the user's interactive `PATH`.
fn current_exe() -> Result<PathBuf> {
    std::env::current_exe()
        .map_err(|e| DaemonError::Config(format!("cannot resolve own path: {e}")))?
        .canonicalize()
        .map_err(|e| DaemonError::Config(format!("cannot canonicalize own path: {e}")))
}

// ---- macOS ---------------------------------------------------------------

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    /// The current user id, which names the launchd GUI domain.
    ///
    /// Read from the owner of the home directory rather than via `libc`:
    /// std already exposes it, and one fewer dependency for one integer is
    /// the right trade in a crate that budgets its dependencies.
    fn uid() -> Result<u32> {
        use std::os::unix::fs::MetadataExt;
        let home = directories::UserDirs::new()
            .ok_or_else(|| DaemonError::Config("cannot resolve home directory".into()))?;
        Ok(std::fs::metadata(home.home_dir())?.uid())
    }

    fn plist_path() -> Result<PathBuf> {
        let home = directories::UserDirs::new()
            .ok_or_else(|| DaemonError::Config("cannot resolve home directory".into()))?;
        Ok(home
            .home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{SERVICE_ID}.plist")))
    }

    /// Writes the launchd agent and loads it.
    ///
    /// `KeepAlive` restarts the daemon if it dies; `RunAtLoad` starts it at
    /// login. Both are what make this a service rather than a shortcut.
    pub fn install() -> Result<String> {
        let exe = current_exe()?;
        let path = plist_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{SERVICE_ID}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
            exe = exe.display(),
            log = crate::config::data_dir()?.join("daemon.log").display(),
        );
        std::fs::write(&path, plist)?;

        // Boot out first so re-installing over an existing registration picks
        // up the new binary path instead of silently keeping the old one.
        let domain = format!("gui/{}", uid()?);
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{SERVICE_ID}")])
            .output();

        // `bootstrap` is the supported verb; `load` is deprecated and on
        // recent macOS fails with an empty message, which is worse than no
        // message at all. Fall back to it only for older systems.
        let boot = std::process::Command::new("launchctl")
            .args(["bootstrap", &domain, &path.to_string_lossy()])
            .output()
            .map_err(|e| DaemonError::Config(format!("could not run launchctl: {e}")))?;

        if !boot.status.success() {
            let detail = String::from_utf8_lossy(&boot.stderr).trim().to_string();
            let legacy = std::process::Command::new("launchctl")
                .args(["load", &path.to_string_lossy()])
                .output();
            let legacy_ok = legacy.map(|o| o.status.success()).unwrap_or(false);
            if !legacy_ok {
                // The plist is already written and valid, so say so: the user
                // can load it by hand, and on next login launchd picks it up
                // regardless. Failing to mention that would imply nothing
                // happened.
                return Err(DaemonError::Config(format!(
                    "wrote {path} but launchctl would not start it now: {detail}\n\
                     It will start at your next login. To start it immediately, run:\n\
                     \x20   launchctl bootstrap {domain} {path}",
                    path = path.display(),
                )));
            }
        }
        Ok(path.to_string_lossy().into_owned())
    }

    /// Unloads and removes the agent. Idempotent.
    pub fn uninstall() -> Result<String> {
        let path = plist_path()?;
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{}/{SERVICE_ID}", uid()?)])
            .output();
        // Older systems only understand `unload`; harmless if bootout worked.
        let _ = std::process::Command::new("launchctl")
            .args(["unload", &path.to_string_lossy()])
            .output();
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(path.to_string_lossy().into_owned())
    }

    pub fn status() -> Result<ServiceStatus> {
        let path = plist_path()?;
        let program = std::fs::read_to_string(&path).ok().and_then(|s| {
            // The first <string> after ProgramArguments is the binary path.
            let start = s.find("<key>ProgramArguments</key>")?;
            let rest = &s[start..];
            let open = rest.find("<string>")? + "<string>".len();
            let close = rest[open..].find("</string>")?;
            Some(rest[open..open + close].to_string())
        });
        Ok(ServiceStatus {
            registered: path.exists(),
            location: path.to_string_lossy().into_owned(),
            program,
        })
    }
}

// ---- Linux ---------------------------------------------------------------

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    fn unit_path() -> Result<PathBuf> {
        let dirs = directories::BaseDirs::new()
            .ok_or_else(|| DaemonError::Config("cannot resolve config directory".into()))?;
        Ok(dirs
            .config_dir()
            .join("systemd/user")
            .join("gitsurveild.service"))
    }

    /// Writes a systemd *user* unit and enables it.
    ///
    /// A user unit (not a system one) needs no root and starts with the user's
    /// session, which is what a per-user monitor wants.
    pub fn install() -> Result<String> {
        let exe = current_exe()?;
        let path = unit_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let unit = format!(
            "[Unit]\n\
             Description=gitsurveil GitHub action-item monitor\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exe} --foreground\n\
             Restart=on-failure\n\
             RestartSec=5\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            exe = exe.display(),
        );
        std::fs::write(&path, unit)?;

        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", "gitsurveild.service"])?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Disables and removes the unit. Idempotent.
    pub fn uninstall() -> Result<String> {
        let path = unit_path()?;
        let _ = systemctl(&["disable", "--now", "gitsurveild.service"]);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let _ = systemctl(&["daemon-reload"]);
        Ok(path.to_string_lossy().into_owned())
    }

    pub fn status() -> Result<ServiceStatus> {
        let path = unit_path()?;
        let program = std::fs::read_to_string(&path).ok().and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("ExecStart="))
                .map(|l| l.trim_end_matches(" --foreground").to_string())
        });
        Ok(ServiceStatus {
            registered: path.exists(),
            location: path.to_string_lossy().into_owned(),
            program,
        })
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let out = std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .map_err(|e| DaemonError::Config(format!("systemctl {args:?} failed: {e}")))?;
        if !out.status.success() {
            return Err(DaemonError::Config(format!(
                "systemctl {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }
}

// ---- Windows -------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use super::*;

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "gitsurveil";

    /// Registers under the per-user `Run` key.
    ///
    /// Not a true Windows service: that survives logout but needs elevation to
    /// install, and a per-user monitor holding per-user credentials has no
    /// business running when that user is logged out. Revisit only if someone
    /// actually wants it (`specs/architecture.md`, open questions).
    pub fn install() -> Result<String> {
        let exe = current_exe()?;
        let out = std::process::Command::new("reg")
            .args([
                "add",
                &format!(r"HKCU\{RUN_KEY}"),
                "/v",
                VALUE,
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\" --foreground", exe.display()),
                "/f",
            ])
            .output()
            .map_err(|e| DaemonError::Config(format!("reg add failed: {e}")))?;
        if !out.status.success() {
            return Err(DaemonError::Config(format!(
                "reg add failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(format!(r"HKCU\{RUN_KEY}\{VALUE}"))
    }

    /// Removes the value. Idempotent — a missing value is not an error.
    pub fn uninstall() -> Result<String> {
        let _ = std::process::Command::new("reg")
            .args(["delete", &format!(r"HKCU\{RUN_KEY}"), "/v", VALUE, "/f"])
            .output();
        Ok(format!(r"HKCU\{RUN_KEY}\{VALUE}"))
    }

    pub fn status() -> Result<ServiceStatus> {
        let out = std::process::Command::new("reg")
            .args(["query", &format!(r"HKCU\{RUN_KEY}"), "/v", VALUE])
            .output();
        let (registered, program) = match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout).to_string();
                let program = text
                    .lines()
                    .find(|l| l.contains(VALUE))
                    .and_then(|l| l.split_whitespace().last())
                    .map(str::to_string);
                (true, program)
            }
            _ => (false, None),
        };
        Ok(ServiceStatus {
            registered,
            location: format!(r"HKCU\{RUN_KEY}\{VALUE}"),
            program,
        })
    }
}

/// Registers the daemon to start at login. Returns where the registration was
/// written. Safe to run repeatedly; re-running repoints an existing
/// registration at the current binary.
pub fn install() -> Result<String> {
    platform::install()
}

/// Removes the registration. Idempotent.
pub fn uninstall() -> Result<String> {
    platform::uninstall()
}

/// Reports whether the daemon is registered, and at which binary.
pub fn status() -> Result<ServiceStatus> {
    platform::status()
}

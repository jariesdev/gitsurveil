# Changelog

All notable changes to GitSurveil are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [v0.2.1] - 2026-08-16

### Fixed

- The packaged app now registers and starts `gitsurveild` itself on launch,
  so a fresh install no longer needs a manual `gitsurveild install` before
  the popover stops reporting the service as unreachable. Self-healing on
  every launch, not just first run.

## [v0.2.0] - 2026-08-16

### Added

- First-run onboarding that walks you through adding your first GitHub account.
- Account provider picker: choose GitHub, GitHub Enterprise, or GitLab when
  adding an account.
- Add-application form is now a modal.
- Inbox curation: review requests are deduped, and the Authored/ReviewedByMe
  inbox views are gated on content.
- Item state is synced across the popover and desktop windows.
- Dismiss items straight from the popover row.
- "Clear all history", archiving items so they can never come back.

### Changed

- Refined PR detail pane: dedupe reviewers, show review-round counts, and
  highlight the selected row.

## [v0.1.1] - 2026-08-15

### Fixed

- Exclude `target/` from Vite's dev-server file watcher.

## [v0.1.0-alpha] - 2026-08-15

### Added

- **Daemon** (`gitsurveild`): a persistent local poller for GitHub action items
  with a local JSON API, SQLite storage, and event stream.
- **Priority engine**: scoring, severity, and a pure notification gate.
- **Desktop notifications** for actionable items.
- **Menubar app**: tray icon and a notifications-only popover.
- **Desktop UI**: dashboard, history, rules, accounts, and settings.
- **Pull requests view**: role-merged listing, filters, review threads with
  resolve and threaded replies, markdown rendering, and a "ready to merge"
  notification.
- **Conflict resolution**: three-pane resolver on temp worktrees, resolved
  directly from the pull request list.
- **Repository management**: local clone paths, worktree Open-with apps, and a
  file picker for apps.
- Notification scoping: per-repo mute and per-kind preferences.
- "Copy URL" on notification and pull request row context menus.
- First-run setup niceties: daemon registered to start at login, daemon shipped
  inside the packaged app, and a live GitHub verification harness.
- Rebranded display labels from gitsurveil to **GitSurveil**, with a new icon
  (a git branch growing from soil).

### Security

- XSS audit of the markdown renderer.

### Build

- Vendored OpenSSL, Linux build prerequisites, and CI and tag-driven release
  workflows.

[Unreleased]: https://github.com/anomalyco/gitsurveil/compare/v0.2.1...HEAD
[v0.2.1]: https://github.com/anomalyco/gitsurveil/compare/v0.2.0...v0.2.1
[v0.2.0]: https://github.com/anomalyco/gitsurveil/compare/v0.1.1...v0.2.0
[v0.1.1]: https://github.com/anomalyco/gitsurveil/compare/v0.1.0-alpha...v0.1.1
[v0.1.0-alpha]: https://github.com/anomalyco/gitsurveil/releases/tag/v0.1.0-alpha

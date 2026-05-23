# Changelog

All notable changes to **gitcycle** will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Dirty-files diff view** on the Repo tab. Press `d` to focus the dirty-files panel, `↑/↓` to select a file, and `Enter` to view its diff inline — the recent-commits and open-PRs panels are replaced by a color-coded unified diff (green/red/cyan for `+`/`-`/`@@` lines). Untracked files render as a full-file addition; tracked files run `git diff HEAD --`. `Esc` closes the diff and restores the original panels.

### Changed

- Repo tab footer now advertises both `c` (focus commits) and `d` (focus dirty files) hotkeys.

## [0.1.0] — 2026-05-22

Initial public release on [crates.io](https://crates.io/crates/gitcycle).

### Added

- **Dashboard tab** — your GitHub at a glance, regardless of cwd:
  - Awaiting your review (PRs in the active org where you're a requested reviewer)
  - Your open PRs (authored by you across all visible orgs)
  - Notifications (`gh api /notifications`, with unread indicators and reason tags)
  - Your recent commits (across all visible repos)
  - Per-panel focus hotkeys (`v` / `p` / `n` / `c`), `Enter` opens contextual detail screens, `o` opens the resource in your default browser.
- **Repo tab** — when launched inside a git repository:
  - Summary (branch, upstream, ahead/behind, last fetch)
  - Dirty files list (color-coded status codes for `M`/`A`/`D`/`??`/`R`)
  - Recent commits (focus with `c`, `Enter` opens commit detail with full message and per-file `+/-` stats)
  - Open pull requests
  - Friendly placeholder when run outside a git repo.
- **Org tab** — organization-wide views with three sub-views (cycled with `[` / `]`):
  - **Activity** — recent PRs and issues grouped by author
  - **Repos** — searchable list of non-archived org repos with drill-in detail (top contributors, recent commits, recent PRs)
  - **Users** — searchable list of org members with drill-in detail (commits, submitted PRs, reviewed PRs)
- Org data (activity, repos, members) is prefetched in the background on startup; switching tabs is instant.
- Archived-repository results are filtered out of every `gh search prs`/`gh search issues` call.
- Cross-platform browser open (`open` on macOS, `xdg-open` on Linux, `start` on Windows).
- Tron-inspired ASCII banner in the README.

[Unreleased]: https://github.com/wjgilmore/gitcycle/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/wjgilmore/gitcycle/releases/tag/v0.1.0

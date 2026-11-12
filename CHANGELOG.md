# Changelog

All notable changes to AgentSync are documented in this file.

## Unreleased

### Added

- Terminal skill trees with individual file reading, plain-text conversations, mouse selection and clipboard support.
- Noninteractive `agentsync sync-all` with timestamp-based bidirectional synchronization.
- SQLite, MySQL/MariaDB and libSQL/Turso backends alongside Postgres.
- Turso authentication through `TURSO_AUTH_TOKEN` or the connection URL.
- Cargo installation from GitHub source, replacing GUI installers and release downloads.

### Changed

- Replaced the Tauri/web GUI with a Rust terminal interface and URL-only connection setup.
- Preserved Postgres skill-version archives when synchronizing partial skill changes.
- Limited legacy skill snapshot seeding to newly inserted versions so repeated schema initialization preserves history.

## Earlier desktop development

### Added

- Local and remote skill removal, including deletion of individual remote versions.
- Immutable remote skill versions with version browsing, content previews, and version-specific pulls.
- One-click pulling of the latest remote versions for all skills already installed locally.
- Upload and download indicators for local skills and workspace history that differs from remote state.
- Skill cleanup that removes versions older than one month while preserving each skill's newest version.
- Workspace history cleanup that removes local and remote sessions older than three months.

### Changed

- Simplified the workspace navigation header by removing the redundant Projects label.

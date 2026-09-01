# AgentSync

AgentSync is a cross-platform Tauri desktop app for browsing and synchronizing coding-agent resources through Postgres. Codex is the first supported agent; the adapter boundary is designed for adding Claude and other agents later.

## Features

- Browse local and remote global or project skills, including every file in multi-file skills, with rendered Markdown.
- Filter remote skills by author, choose visibility, and push or pull one skill without leaving the reader.
- Browse Codex conversation history with metadata-aware indexing, incremental Markdown rendering, and caches that survive tab changes.
- Synchronize every conversation in a selected project with latest-modified history winning automatically.
- Choose public or private visibility per skill and see each resource's author and sync version.
- Keep chat history private at both the application and database layers.
- Adjust the font family, reading font size, and UI zoom.

AgentSync does not synchronize `AGENTS.md` files.

## Install

Download the installer or application bundle for your operating system from [GitHub Releases](https://github.com/nhanhoang1405/agentsync/releases):

- Linux: AppImage or Debian package
- macOS: DMG or application bundle
- Windows: MSI or NSIS installer

Release artifacts are produced by the cross-platform Tauri release workflow whenever a version tag is pushed.

## Configure

Open **Settings** on first launch and enter:

- your author email;
- the Postgres connection URL; and
- an optional PEM CA certificate for a private or company certificate authority.

AgentSync validates the connection, applies the idempotent migrations in [`migrations`](migrations), registers the author, and stores the configuration in the operating system's per-user application config directory. On Linux this is normally `~/.config/agentsync/config.toml`, protected with mode `0600`.

Certificate verification is always enabled. If the connection reports `UnknownIssuer`, select the server's CA certificate in Settings.

## Resource mapping

| Scope | Resource | Local source or destination |
| --- | --- | --- |
| global | skills | `$CODEX_HOME/skills`, excluding Codex's `.system` skills |
| global | histories | `$CODEX_HOME/history.jsonl` and `$CODEX_HOME/sessions/**/*.jsonl` |
| project | skills | `<project>/.agents/skills` and `<project>/.codex/skills` |
| project | histories | Codex sessions whose recorded working directory is inside the project |

`CODEX_HOME` defaults to `~/.codex`. Project keys are derived from the Git remote when available, with a local directory fallback.

## Visibility and security

Every stored resource includes its author, visibility, and monotonic sync version. Private resources are returned only to their author; public resources can be discovered by other users of the same AgentSync database. Histories are always private, regardless of the requested visibility.

Skills can execute commands or expose environment details. Review public resources before pulling them. Application permissions do not replace Postgres access controls: use separate least-privilege database roles when the database crosses trust boundaries.

Remote paths are normalized before writes, content is checked with SHA-256, symlinks are not followed, and differing local files require explicit overwrite confirmation. Filesystem modification times are preserved through history and resource sync.

## Development

Install the JavaScript dependencies and start the app:

```bash
npm install
npm run tauri dev
```

Tauri uses the operating system webview. Linux development additionally requires the packages listed in the [official Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), including WebKitGTK 4.1.

Start the included development database:

```bash
docker compose up -d --wait postgres
```

The default development connection is:

```text
postgres://agentsync:agentsync-dev-only@localhost:55432/agentsync
```

Run the checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
npm ci
npm run build
```

Build a native installer or application bundle for the current platform:

```bash
npm run tauri build
```

Remove the disposable database and its volume when no longer needed:

```bash
docker compose down -v
```

The Rust services and Tauri application now live in one root package. The extension boundary for future agent support is [`src/agent/mod.rs`](src/agent/mod.rs). A new adapter supplies discovery, destination validation, and resource-specific writes while reusing the desktop services, Postgres schema, visibility rules, and conflict handling.

# AgentSync

AgentSync is a Rust terminal app for browsing and synchronizing Codex skills and conversation history through Postgres, SQLite, MySQL/MariaDB, or libSQL/Turso. It includes a noninteractive `sync-all` command. No GUI, settings screen, or Markdown rendering is used.

## Install

Requires Rust 1.88 or newer, Cargo, and a C compiler for bundled SQLite. Install from the GitHub `main` branch:

```sh
cargo install --git https://github.com/nhanhoang1405/agentsync --branch main --locked agentsync
```

Ensure Cargo's binary directory (`~/.cargo/bin` on Linux/macOS, `%USERPROFILE%\.cargo\bin` on Windows) is on your `PATH`.

Update an existing installation with:

```sh
cargo install --git https://github.com/nhanhoang1405/agentsync --branch main --locked --force agentsync
```

## Run

```sh
agentsync
```

Browse local skills and project conversations without a database. Select **Remote skills**, push, pull, or sync to enter your SQL connection URL. This is the only required input. AgentSync validates the connection, applies the existing idempotent schema migrations, and saves the URL in the per-user config directory (`~/.config/agentsync/config.toml` on Linux). The file contains credentials in plain text and uses mode `0600` on Unix.

Alternatively, provide the URL without saving it:

```sh
export AGENTSYNC_DATABASE_URL='postgres://user:password@localhost:5432/agentsync'
agentsync
agentsync sync-all
# Or supply --database-url to either command.
```

`agentsync sync-all` also works with a previously saved connection and exits with a nonzero status on failure. It prints upload, download, metadata, unchanged, and conflict counts. It never opens the terminal UI or prompts for input.

Supported database URLs:

| Backend | URL example |
| --- | --- |
| Postgres | `postgres://user:password@host:5432/agentsync` (also `postgresql://`) |
| libSQL/Turso | `libsql://your-database.turso.io` |
| SQLite | `sqlite://agentsync.db` or `sqlite:///absolute/path/agentsync.db` |
| MySQL 8.0.16+ | `mysql://user:password@host:3306/agentsync` |
| MariaDB 10.6+ | `mariadb://user:password@host:3306/agentsync` |

For Turso, use the database URL and a database auth token:

```sh
export TURSO_AUTH_TOKEN='your-turso-database-auth-token'
agentsync sync-all --database-url 'libsql://agentsync-chinhan1405.aws-ap-southeast-1.turso.io'
```

The same URL works in the TUI. `TURSO_AUTH_TOKEN` is read at connection time and is not saved in config.toml. Alternatively, include `?authToken=TOKEN` in the URL (`auth_token` is also accepted); this takes precedence over the environment token and is saved with the URL in the private config file. Use a token with write permissions: sync initializes tables and uploads data. `libsql://` uses verified HTTPS through the official [libSQL Rust client](https://docs.rs/libsql/latest/libsql/struct.Builder.html). HTTPS libSQL endpoints are also accepted; plain HTTP is allowed only for loopback development servers.

Remote libSQL uses the SQLite schema and a stable archive identity (`libsql@remote`). Tokens granting access to the same archive share that identity; rotating a token does not create another author. History stays private in application queries and schema constraints, while the database token controls access to the archive itself.

For a local SQLite archive:

```sh
agentsync sync-all --database-url 'sqlite://agentsync.db'
```

SQLite creates the database file if missing; its parent directory must exist. Newly created files use Unix mode `0600`. `?mode=rw` opens an existing file only. In-memory and read-only SQLite URLs are rejected because sync needs persistent writable storage. Saved SQLite URLs use absolute paths, so changing directories does not silently select a different archive. SQLite uses a single archive identity (`sqlite@local`); anyone with filesystem access to that archive can access its contents. Move or share the archive file to use it on another device; it does not provide a network server.

Postgres and MySQL/MariaDB derive author identity from the authenticated database login. Existing saved author emails are preserved and a unique legacy author registered to the login is reused. New identities use `<hex-encoded-role>@postgres` or `<hex-encoded-login>@mysql`. Multiple legacy authors on one login require the original config file to disambiguate. Devices sharing a database login share its resources; use distinct logins for distinct users.

Postgres retains its existing schema, migrations, and immutable skill-version archives. Partial skill sync snapshots include unchanged files; repeated migrations preserve older snapshots. SQLite/libSQL and MySQL/MariaDB use their corresponding versioned schemas, storing timestamps in UTC microseconds. Schema initialization is idempotent; selecting another backend does not copy data between databases.

MySQL/MariaDB verifies TLS certificates and hostnames by default. For a local server configured without TLS, explicitly use `?ssl-mode=DISABLED`. MySQL/MariaDB uses the driver's bundled public certificate roots; private CAs can be specified with `ssl-ca` in the URL. Postgres uses operating-system trust and supports legacy saved CA paths. Legacy saved CA paths are also accepted for MySQL/MariaDB. SQLite requires no server or TLS configuration.

## Terminal controls

The minimum terminal size is **72 × 26** cells.

| Key | Action |
| --- | --- |
| Alt+1 / Alt+2 / Alt+3 | Local skills / remote skills / project history |
| Tab / Shift+Tab | Switch browser and reader; cycle controls in a dialog |
| Arrows, Enter, Esc | Select, open, go back or dismiss a dialog |
| F5 | Sync all, or sync history for the selected/open project |
| Ctrl+S | Sync all from any workbench tab |
| F6 / F7 | Push local skill / pull remote skill |
| F8 | Toggle push visibility (private by default) |
| F3 / `/` | Refresh / filter by name, scope, or author |
| F1 | Help |
| Ctrl+A / Shift+arrows / Ctrl+C | Select all / select text / copy |
| Ctrl+V, Ctrl+Z / Ctrl+Y | Paste, undo / redo in URL and filter fields |
| Ctrl+Q | Quit after a running operation completes |

Skills use an expandable tree: Enter or Right opens a skill or directory, Left collapses it or selects its parent, and selecting a file displays only that file. Click folders to toggle them and click files to read them. F6/F7 still synchronize the entire selected skill. Click tabs and buttons, click a project or conversation to select it, and click it again to open it. Click or drag text in the reader to select it. Mouse wheel scrolls the panel under the pointer; Shift+wheel scrolls text horizontally. All skill files and chat messages are shown as plain text. Output is read-only. The clipboard uses a desktop provider with an internal fallback (`AGENTSYNC_CLIPBOARD=internal` forces the fallback). Bracketed paste is supported; single-line inputs strip newlines.

Database and filesystem work runs in the background. Operations are allowed to finish before quitting. Errors can leave earlier uploads or writes committed; rerun sync to converge.

## Sync behavior

`sync-all` synchronizes **your own skills and histories**, in global scope and every discovered local project. Projects come from Codex configuration, session working directories, and the current directory. It does not create missing project checkouts or synchronize `AGENTS.md` or MCP configuration. Remote resources from other authors require explicit skill pull.

Missing files are copied in either direction; newer modification time wins when both sides exist. Equal timestamps with different content prefer the remote file. Existing skill visibility is preserved; newly uploaded skills default to private. Histories are always private, enforced in both the application and database. Deletions are not propagated. Project history may also appear in the global history archive.

Individual skill push uploads every file in that skill, using the chosen visibility. Individual pull skips differing local files and offers an explicit overwrite confirmation. Nonconflicting files may already be written before this prompt. Remote paths are validated, SHA-256 checksums are checked, symlinks are rejected, and modification times are preserved.

| Scope | Resource | Local source or destination |
| --- | --- | --- |
| global | skills | `$CODEX_HOME/skills`, excluding `.system` |
| global | histories | `$CODEX_HOME/history.jsonl` and `$CODEX_HOME/sessions/**/*.jsonl` |
| project | skills | `<project>/.agents/skills` and `<project>/.codex/skills` |
| project | histories | Codex sessions recorded inside the project |

`CODEX_HOME` defaults to `~/.codex`. Project keys use the Git remote when available, with a directory-name fallback. Server database access controls remain necessary when crossing trust boundaries.

## Development

To install from a local checkout:

```sh
cargo install --path . --locked
```

```sh
docker compose up -d --wait postgres
export AGENTSYNC_DATABASE_URL='postgres://agentsync:agentsync-dev-only@localhost:55432/agentsync'
cargo run --locked
cargo run --locked -- sync-all
```

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --locked
python3 scripts/tui_smoke.py
python3 scripts/sqlite_smoke.py
```

The optional Postgres integration smoke test needs a disposable, empty database and uses isolated local fixtures:

```sh
AGENTSYNC_TEST_DATABASE_URL='postgres://user:password@localhost/testdb' python3 scripts/sync_smoke.py
# Verify legacy skill archives using a separate empty database:
AGENTSYNC_TEST_DATABASE_URL='postgres://user:password@localhost/versiontestdb' python3 scripts/postgres_version_smoke.py
```

For a disposable, empty MySQL/MariaDB test database:

```sh
AGENTSYNC_TEST_MYSQL_URL='mysql://user:password@localhost/testdb?ssl-mode=DISABLED' cargo test --locked mysql_contract -- --ignored
# Run the full TUI/CLI smoke test against a separate empty test database:
AGENTSYNC_TEST_MYSQL_URL='mysql://user:password@localhost/smokedb?ssl-mode=DISABLED' python3 scripts/sql_server_smoke.py
```

For a disposable, empty local libSQL server (Turso credentials are not needed for these fixtures):

```sh
AGENTSYNC_TEST_LIBSQL_URL='http://127.0.0.1:8080' cargo test --locked libsql_storage_contract -- --ignored
# Use a separate empty server/database for the CLI/TUI smoke test:
AGENTSYNC_TEST_LIBSQL_URL='http://127.0.0.1:8081' python3 scripts/libsql_smoke.py
```

Distribution uses Cargo installation from GitHub source. The terminal CI checks Linux, macOS, Windows, and the minimum Rust version, with database integration jobs for Postgres, MySQL/MariaDB and libSQL. Agent adapters remain behind `src/agent/mod.rs`; shared services live in `src/service.rs`.

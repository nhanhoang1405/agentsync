"""End-to-end SQLite CLI and TUI sync using isolated files and no server."""
import hashlib
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import tomllib
from urllib.parse import unquote

from fixtures import fixture
from pty_session import TuiSession

BINARY = Path(os.environ.get("AGENTSYNC_BINARY", "target/debug/agentsync")).resolve()


def main():
    with tempfile.TemporaryDirectory(prefix="agentsync-sqlite-") as directory:
        home, project, skill, session = fixture(directory)
        archive = Path(directory) / "archive.db"
        url = f"sqlite://{archive}"
        env = {**os.environ, "CODEX_HOME": str(home), "XDG_CONFIG_HOME": str(Path(directory) / "config"), "AGENTSYNC_DATABASE_URL": url}

        def sync():
            return subprocess.check_output([BINARY, "sync-all"], env=env, cwd=project, text=True)

        assert "Uploaded: 5" in sync()
        with sqlite3.connect(archive) as db:
            assert db.execute("SELECT count(*) FROM agentsync_resources").fetchone()[0] == 5
            assert db.execute("SELECT email FROM agentsync_users").fetchone()[0] == "sqlite@local"
            assert db.execute("SELECT count(*) FROM agentsync_resources WHERE kind='histories' AND visibility<>'private'").fetchone()[0] == 0
        assert "Uploaded: 0" in sync()
        newer = b"newer remote SQLite skill\n"
        with sqlite3.connect(archive) as db:
            db.execute("UPDATE agentsync_resources SET content=?,content_sha256=?,source_modified_at=?,sync_version=sync_version+1 WHERE scope='global' AND path='skills/sample/SKILL.md'", (newer, hashlib.sha256(newer).hexdigest(), int((time.time() + 10) * 1_000_000)))
        assert "Downloaded: 1" in sync()
        assert (skill / "SKILL.md").read_bytes() == newer
        (skill / "SKILL.md").write_text("newer local SQLite skill")
        stamp = time.time() + 30
        os.utime(skill / "SKILL.md", (stamp, stamp))
        assert "Uploaded: 1" in sync()
        session.unlink()
        sync()
        assert session.is_file()
        # A relative URL saved by the TUI must still target the same archive in another cwd.
        with TuiSession(BINARY, env={"CODEX_HOME": str(home), "AGENTSYNC_DATABASE_URL": "", "AGENTSYNC_CLIPBOARD": "internal"}, cwd=project) as s:
            s.wait_for("sample [global]")
            s.send(b"\x13")
            s.wait_for("SQL connection URL")
            s.paste("sqlite://../archive.db")
            s.send(b"\r")
            s.wait_absent("SQL connection URL", timeout=20)
            s.wait_for("Uploaded: 0", timeout=20)
            saved = s.config_dir / "agentsync/config.toml"
            assert saved.exists()
            saved_url = tomllib.loads(saved.read_text())["database_url"]
            saved_path = unquote(saved_url.removeprefix("sqlite://").split("?", 1)[0])
            assert Path(saved_path).is_absolute() and Path(saved_path).resolve() == archive
            saved_env = {**env, "XDG_CONFIG_HOME": str(s.config_dir), "AGENTSYNC_DATABASE_URL": ""}
            result = subprocess.check_output([BINARY, "sync-all"], env=saved_env, cwd=directory, text=True)
            # Changing cwd may discover an additional local project history scope.
            assert "Uploaded: 0" in result or "Uploaded: 1" in result, result
            s.send(b"\x11")
            s.wait_exit()
        # Hash checks also apply to the new storage backend.
        (skill / "helper.txt").unlink()
        with sqlite3.connect(archive) as db:
            db.execute("UPDATE agentsync_resources SET content_sha256=? WHERE path='skills/sample/helper.txt'", ("0" * 64,))
        failed = subprocess.run([BINARY, "sync-all"], env=env, cwd=project, text=True, capture_output=True)
        assert failed.returncode != 0 and "integrity check" in failed.stderr
        assert not (skill / "helper.txt").exists()
    print("SQLite smoke passed: CLI/TUI sync, timestamps, history privacy, saved relative URL, integrity rejection")


if __name__ == "__main__":
    main()

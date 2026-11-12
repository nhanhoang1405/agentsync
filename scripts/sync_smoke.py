"""Integration checks against a disposable, initially empty Postgres database.

Requires psql and AGENTSYNC_TEST_DATABASE_URL. Leaves fixture database rows for inspection.
"""
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import time

from fixtures import fixture
from pty_session import TuiSession

BINARY = Path(os.environ.get("AGENTSYNC_BINARY", "target/debug/agentsync")).resolve()
URL = os.environ["AGENTSYNC_TEST_DATABASE_URL"]


def sql(query):
    return subprocess.check_output(["psql", URL, "-XAt", "-v", "ON_ERROR_STOP=1", "-c", query], text=True).strip()


def main():
    assert sql("SELECT to_regclass('agentsync_resources') IS NULL") == "t", "Use an empty disposable database"
    with tempfile.TemporaryDirectory(prefix="agentsync-sync-") as directory:
        home, project, skill, session = fixture(directory)
        config = Path(directory) / "config"
        env = {**os.environ, "CODEX_HOME": str(home), "XDG_CONFIG_HOME": str(config), "AGENTSYNC_DATABASE_URL": URL}

        def sync():
            return subprocess.check_output([BINARY, "sync-all"], env=env, cwd=project, text=True)

        assert "Uploaded: 5" in sync()
        assert sql("SELECT count(*) FROM agentsync_resources") == "5"
        role = sql("SELECT current_user")
        assert sql("SELECT email FROM agentsync_users") == role.encode().hex() + "@postgres"
        assert "Uploaded: 0" in sync(), "Second sync must be idempotent at Postgres timestamp precision"
        assert sql("SELECT count(*) FROM agentsync_resources WHERE kind='histories' AND visibility <> 'private'") == "0"
        remote = b"# Newer remote skill\nremote payload\n"
        sql(f"UPDATE agentsync_resources SET content=decode('{remote.hex()}','hex'), content_sha256='{hashlib.sha256(remote).hexdigest()}', source_modified_at=now()+interval '10 seconds', sync_version=sync_version+1 WHERE scope='global' AND path='skills/sample/SKILL.md'")
        assert "Downloaded: 1" in sync()
        assert (skill / "SKILL.md").read_bytes() == remote
        assert "Uploaded: 0" in sync()
        (skill / "SKILL.md").write_text("newer local payload")
        stamp = time.time() + 30
        os.utime(skill / "SKILL.md", (stamp, stamp))
        assert "Uploaded: 1" in sync()
        assert sql("SELECT convert_from(content,'UTF8') FROM agentsync_resources WHERE scope='global' AND path='skills/sample/SKILL.md'") == "newer local payload"
        (skill / "helper.txt").unlink()
        session.unlink()
        sync()
        assert (skill / "helper.txt").read_text() == "helper payload\n"
        assert session.is_file()
        sql("INSERT INTO agentsync_users(email,database_role) VALUES ('other@example.com','other-role')")
        public = b"other author"
        sql(f"INSERT INTO agentsync_resources(agent,kind,scope,path,content,content_sha256,visibility,author_email) VALUES ('codex','skills','global','skills/other/SKILL.md',decode('{public.hex()}','hex'),'{hashlib.sha256(public).hexdigest()}','public','other@example.com')")
        sync()
        assert not (home / "skills/other").exists(), "sync-all must not import others' public resources"
        # Connect through the real TUI, then reuse its privately saved URL in CLI.
        with TuiSession(BINARY, env={"CODEX_HOME": str(home), "AGENTSYNC_DATABASE_URL": "", "AGENTSYNC_CLIPBOARD": "internal"}, cwd=project) as s:
            s.wait_for("sample [global]")
            s.send(b"\r")
            s.wait_for("helper.txt")
            s.click_text("SKILL.md", within=(0,4,40,33))
            s.wait_for("newer local payload")
            s.send(b"\x13")
            s.wait_for("SQL connection URL")
            s.paste(URL)
            s.send(b"\r")
            s.wait_absent("SQL connection URL", timeout=20)
            s.wait_for("Uploaded: 0", timeout=20)
            saved = s.config_dir / "agentsync/config.toml"
            assert saved.exists()
            assert saved.stat().st_mode & 0o777 == 0o600
            saved_env = {**env, "XDG_CONFIG_HOME": str(s.config_dir), "AGENTSYNC_DATABASE_URL": ""}
            result = subprocess.check_output([BINARY, "sync-all"], env=saved_env, cwd=project, text=True)
            assert "Uploaded: 0" in result
            # Push every skill file with chosen visibility through the terminal.
            s.send(b"\x1b[19~\x1b[17~")  # F8 visibility, F6 push.
            s.wait_for("Uploaded: 2", timeout=20)
            assert sql("SELECT count(*) FROM agentsync_resources WHERE scope='global' AND path LIKE 'skills/sample/%' AND visibility='public'") == "2"
            s.send(b"\x1b2")
            s.wait_for("other@example.com", timeout=20)
            s.send(b"/")
            s.wait_for("Filter the browser")
            s.paste("other@example.com")
            s.send(b"\r")
            s.wait_absent("Filter the browser")
            s.send(b"\x1b[18~")  # F7 explicit public skill pull.
            s.wait_for("Downloaded: 1", timeout=20)
            assert (home / "skills/other/SKILL.md").read_bytes() == public
            s.send(b"/")
            s.wait_for("Filter the browser")
            s.send(b"\x01")
            s.paste("sample")
            s.send(b"\r")
            s.wait_absent("Filter the browser")
            (skill / "SKILL.md").write_text("keep local draft")
            s.send(b"\x1b[18~")
            s.wait_for("Replace differing local files", timeout=20)
            s.send(b"\x1b")
            s.wait_absent("Replace differing local files")
            assert (skill / "SKILL.md").read_text() == "keep local draft"
            s.send(b"\x1b[18~")
            s.wait_for("Replace differing local files", timeout=20)
            s.send(b"\r")
            s.wait_absent("Replace differing local files", timeout=20)
            s.wait_for("Downloaded: 1", timeout=20)
            assert (skill / "SKILL.md").read_text() == "newer local payload"
            s.send(b"\x11")
            s.wait_exit()
        # Corrupt remote content is rejected before writing it.
        (skill / "helper.txt").unlink()
        sql("UPDATE agentsync_resources SET content_sha256=repeat('0',64) WHERE path='skills/sample/helper.txt'")
        failed = subprocess.run([BINARY, "sync-all"], env=env, cwd=project, text=True, capture_output=True)
        assert failed.returncode != 0 and "integrity check" in failed.stderr
        assert not (skill / "helper.txt").exists()
    print("Sync integration passed: upload/download, idempotency, history privacy, author identity, saved connection, skill push/pull and overwrite, integrity rejection")


if __name__ == "__main__":
    main()

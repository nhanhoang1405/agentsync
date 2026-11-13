"""Use AGENTSYNC_TEST_LIBSQL_URL with a disposable empty libSQL server."""
import os
from pathlib import Path
import subprocess
import tempfile

from fixtures import fixture
from pty_session import TuiSession

BINARY = Path(os.environ.get("AGENTSYNC_BINARY", "target/debug/agentsync")).resolve()
URL = os.environ["AGENTSYNC_TEST_LIBSQL_URL"]


def main():
    with tempfile.TemporaryDirectory(prefix="agentsync-sql-server-") as directory:
        home, project, skill, session = fixture(directory)
        env = {**os.environ, "CODEX_HOME": str(home), "XDG_CONFIG_HOME": str(Path(directory) / "config"), "AGENTSYNC_DATABASE_URL": URL}

        def sync():
            return subprocess.check_output([BINARY, "sync-all"], env=env, cwd=project, text=True)

        assert "Uploaded: 5" in sync()
        assert "Uploaded: 0" in sync()
        (skill / "helper.txt").unlink()
        session.unlink()
        assert "Downloaded: 2" in sync()
        assert (skill / "helper.txt").read_text() == "helper payload\n"
        assert session.is_file()
        with TuiSession(BINARY, env={"CODEX_HOME": str(home), "AGENTSYNC_DATABASE_URL": "", "AGENTSYNC_CLIPBOARD": "internal", "TURSO_AUTH_TOKEN": os.environ.get("TURSO_AUTH_TOKEN", "")}, cwd=project) as s:
            s.wait_for("sample [global]")
            s.send(b"\x13")
            s.wait_for("SQL connection URL")
            s.paste(URL)
            s.send(b"\r")
            s.wait_absent("SQL connection URL", timeout=20)
            s.wait_for("Uploaded: 0", timeout=20)
            s.send(b"\x1b[17~")  # F6 whole-skill push.
            s.wait_for("Uploaded: 2", timeout=20)
            s.send(b"\x1b2")
            s.wait_for("sample [global]", timeout=20)
            s.send(b"\r")
            s.wait_for("helper.txt")
            s.click_text("helper.txt", within=(0,4,40,33))
            s.wait_for("helper payload")
            (skill / "helper.txt").unlink()
            s.send(b"\x1b[18~")
            s.wait_for("Downloaded: 1", timeout=20)
            assert (skill / "helper.txt").read_text() == "helper payload\n"
            saved_file = s.config_dir / "agentsync/config.toml"
            token = os.environ.get("TURSO_AUTH_TOKEN", "")
            if token and "authToken=" not in URL:
                assert token not in saved_file.read_text(), "Environment tokens must not be persisted"
            saved_env = {**env, "AGENTSYNC_DATABASE_URL": "", "XDG_CONFIG_HOME": str(s.config_dir)}
            assert "Uploaded: 0" in subprocess.check_output([BINARY, "sync-all"], env=saved_env, cwd=project, text=True)
            s.send(b"\x11")
            s.wait_exit()
        if os.environ.get("AGENTSYNC_TEST_LIBSQL_REQUIRE_AUTH") == "1":
            for token in ["", "invalid-test-token"]:
                failed = subprocess.run([BINARY, "sync-all"], env={**env, "TURSO_AUTH_TOKEN": token}, cwd=project, text=True, capture_output=True)
                assert failed.returncode != 0 and "TURSO_AUTH_TOKEN" in failed.stderr
                assert "invalid-test-token" not in failed.stderr
            from urllib.parse import quote
            inline_url = URL + "?authToken=" + quote(os.environ["TURSO_AUTH_TOKEN"], safe="")
            output = subprocess.check_output([BINARY, "sync-all", "--database-url", inline_url], env={**env,"TURSO_AUTH_TOKEN":"invalid-test-token"}, cwd=project, text=True)
            assert "Uploaded: 0" in output, "Inline token should take precedence over environment"
    print("libSQL smoke passed: CLI/TUI sync, idempotency, history/file restore, skill push/pull, saved URL")


if __name__ == "__main__":
    main()

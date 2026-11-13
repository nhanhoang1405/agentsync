"""Verify legacy Postgres archives and partial skill sync in an empty disposable database."""
import os
from pathlib import Path
import subprocess
import tempfile
import time

from fixtures import fixture

BINARY = Path(os.environ.get("AGENTSYNC_BINARY", "target/debug/agentsync")).resolve()
URL = os.environ["AGENTSYNC_TEST_DATABASE_URL"]


def sql(query):
    return subprocess.check_output(["psql", URL, "-XAt", "-v", "ON_ERROR_STOP=1", "-c", query], text=True).strip()


def main():
    assert sql("SELECT to_regclass('agentsync_resources') IS NULL") == "t", "Use an empty disposable database"
    with tempfile.TemporaryDirectory(prefix="agentsync-pg-versions-") as directory:
        home, project, skill, _ = fixture(directory)
        env = {**os.environ, "CODEX_HOME": str(home), "XDG_CONFIG_HOME": str(Path(directory) / "config"), "AGENTSYNC_DATABASE_URL": URL}

        def sync():
            return subprocess.check_output([BINARY, "sync-all"], env=env, cwd=project, text=True)

        def files(version):
            return sql(f"SELECT count(*) FROM agentsync_skill_version_files f JOIN agentsync_skill_versions v ON f.skill_version_id=v.id WHERE v.scope='global' AND v.skill_name='sample' AND v.version={version}")

        assert "Uploaded: 5" in sync()
        assert files(1) == "2"
        (skill / "SKILL.md").write_text("changed one file\n")
        stamp = time.time() + 30
        os.utime(skill / "SKILL.md", (stamp, stamp))
        assert "Uploaded: 1" in sync()
        assert files(1) == "2" and files(2) == "2", "Partial sync must snapshot unchanged files"
        assert sql("SELECT convert_from(f.content,'UTF8') FROM agentsync_skill_version_files f JOIN agentsync_skill_versions v ON f.skill_version_id=v.id WHERE v.scope='global' AND v.skill_name='sample' AND v.version=1 AND f.path='skills/sample/helper.txt'") == "helper payload"
        (skill / "extra.txt").write_text("added file\n")
        assert "Uploaded: 1" in sync()
        assert files(3) == "3"
        assert "Uploaded: 0" in sync()
        assert files(1) == "2" and files(2) == "2", "Repeated migrations must not add newer files to old snapshots"
        # Simulate a pre-versioning database while preserving its current resource rows.
        before = sql("SELECT path || ':' || content_sha256 FROM agentsync_resources ORDER BY path")
        sql("DROP TABLE agentsync_skill_version_files; DROP TABLE agentsync_skill_versions; UPDATE agentsync_resources SET sync_version=8 WHERE scope='global' AND kind='skills'")
        assert "Uploaded: 0" in sync()
        assert files(8) == "3"
        assert sql("SELECT path || ':' || content_sha256 FROM agentsync_resources ORDER BY path") == before
        assert "Uploaded: 0" in sync() and files(8) == "3"
    print("Postgres version smoke passed: complete partial snapshots, immutable history, legacy migration and idempotency")


if __name__ == "__main__":
    main()

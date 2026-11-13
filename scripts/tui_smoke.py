"""Real Linux/Unix terminal acceptance checks; no database required."""
import os
from pathlib import Path
import tempfile

from fixtures import fixture
from pty_session import TuiSession

BINARY = Path(os.environ.get("AGENTSYNC_BINARY", "target/debug/agentsync")).resolve()


def main():
    with tempfile.TemporaryDirectory(prefix="agentsync-tui-") as directory:
        home, project, _, _ = fixture(directory)
        env = {"CODEX_HOME": str(home), "AGENTSYNC_DATABASE_URL": "", "AGENTSYNC_CLIPBOARD": "internal"}
        for width, height in [(72, 26), (80, 26), (100, 30), (120, 40), (160, 48)]:
            with TuiSession(BINARY, env=env, width=width, height=height, cwd=project) as s:
                s.wait_for("sample [global]")
                assert "Browse | /" not in s.screen.text
                s.send(b"\r")
                s.wait_for("helper.txt")
                s.click_text("SKILL.md", within=(0, 4, max(24, width // 3), height - 7))
                s.wait_for("Fixture skill")
                assert "helper payload" not in s.screen.text, "Reader must show only the selected file"
                s.click_text("helper.txt", within=(0, 4, max(24, width // 3), height - 7))
                s.wait_for("helper payload")
                assert "Fixture skill" not in s.screen.text
                s.click_text("SKILL.md", within=(0, 4, max(24, width // 3), height - 7))
                s.wait_for("Fixture skill")
                for label in ["F5 Sync", "F6 Push", "F7 Pull", "F1 Help", "Alt+1 Local skills", "Alt+2 Remote skills", "Alt+3 History"]:
                    assert label in s.screen.text, s.screen.text
                s.send(b"\t")  # Reader focus.
                s.send(b"\x01\x03")  # Select all and copy full document.
                s.send(b"/")
                s.wait_for("Filter the browser")
                s.send(b"\x16")  # Internal clipboard round trip into editable filter.
                s.send(b"\x01\x03")
                s.send(b"\x1b")
                s.wait_absent("Filter the browser")
                # Exact substring mouse selection, then copy/paste into the filter.
                lines = s.screen.text.splitlines()
                y = next(i for i, line in enumerate(lines) if "Fixture skill" in line)
                x = lines[y].index("Fixture skill")
                s.drag(x, y, x + 7, y)
                s.send(b"\x03/")
                s.wait_for("Filter the browser")
                s.send(b"\x16")
                s.wait_for("Fixture")
                s.send(b"\r")
                s.wait_absent("Filter the browser")
                s.wait_for("Filter: Fixture")
                assert "sample [global]" not in s.screen.text, s.screen.text
                s.send(b"/")
                s.wait_for("Filter the browser")
                s.send(b"\x01\x7f\r")  # Clear filter.
                s.wait_absent("Filter the browser")
                s.wait_for("sample [global]")
                s.send(b"\x1b3")
                s.wait_for("sample-project")
                s.send(b"\r")
                s.wait_for("Fixture conversation")
                s.send(b"\r")
                s.wait_for("Plain answer **without rendering**")
                # Output rejects paste, typed edits and undo.
                s.paste("MUTATION_SENTINEL")
                s.send(b"\x16\x1a")
                s.settle()
                assert "MUTATION_SENTINEL" not in s.screen.text
                s.send(b"\x1b2")
                s.wait_for("SQL connection URL")
                s.paste("invalid-url")
                s.send(b"\r")
                s.wait_for("database URL must start")
                assert "Connect" in s.screen.text and "Esc Cancel" in s.screen.text
                s.send(b"\x1b")
                s.wait_absent("SQL connection URL")
                s.send(b"\x11")
                s.wait_exit()
        print("TUI smoke passed: five sizes, browse/read, mouse selection, clipboard, filtering, URL validation, clean exit")


if __name__ == "__main__":
    main()

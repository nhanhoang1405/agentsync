"""Small isolated Codex fixture shared by the terminal and sync smoke tests."""
import json
from pathlib import Path


def fixture(root):
    root = Path(root)
    home = root / "codex"
    project = root / "sample-project"
    project.mkdir(parents=True)
    (project / ".git").mkdir()  # Keep root detection inside the isolated fixture.
    skill = home / "skills" / "sample"
    skill.mkdir(parents=True)
    (skill / "SKILL.md").write_text("# Fixture skill\nhello 世界\nplain **text**\n")
    (skill / "helper.txt").write_text("helper payload\n")
    project_skill = project / ".agents" / "skills" / "project-skill"
    project_skill.mkdir(parents=True)
    (project_skill / "SKILL.md").write_text("# Project skill\nproject payload\n")
    (home / "config.toml").write_text(f"[projects.{json.dumps(str(project))}]\ntrust_level = 'trusted'\n")
    session = home / "sessions" / "fixture.jsonl"
    session.parent.mkdir()
    rows = [
        {"timestamp": "2026-01-01T00:00:00Z", "type": "session_meta", "payload": {"id": "fixture-chat", "cwd": str(project)}},
        {"timestamp": "2026-01-01T00:00:01Z", "type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Fixture conversation"}]}},
        {"timestamp": "2026-01-01T00:00:02Z", "type": "response_item", "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Plain answer **without rendering**"}]}},
    ]
    session.write_text("\n".join(map(json.dumps, rows)) + "\n")
    return home, project, skill, session

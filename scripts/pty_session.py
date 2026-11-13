"""Unix-only, stdlib PTY helper for application-specific Rust TUI smoke tests.

Copy beside terminal_screen.py. Use native widget tests for exact Unicode layout;
this small recorder is intended for integrated flows and ASCII control labels.
"""
import fcntl
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time

from terminal_screen import Screen


class TuiSession:
    def __init__(self, binary, args=(), *, env=None, width=120, height=40, cwd=None):
        self._config = tempfile.TemporaryDirectory(prefix="rust-tui-smoke-")
        self.config_dir = Path(self._config.name)
        self.screen = Screen(width, height)
        self.width, self.height = width, height
        self.master, slave = pty.openpty()
        try:
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
            self.process = subprocess.Popen(
                [str(Path(binary).resolve()), *map(str, args)],
                stdin=slave, stdout=slave, stderr=slave,
                env={**os.environ, "TERM": "xterm-256color", **(env or {}),
                     "XDG_CONFIG_HOME": str(self.config_dir)},
                start_new_session=True, cwd=cwd,
            )
        except BaseException:
            os.close(self.master)
            self._config.cleanup()
            raise
        finally:
            os.close(slave)
        self._closed = False
        self._query_tail = b""

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def send(self, data):
        if isinstance(data, str):
            data = data.encode()
        remaining = memoryview(data)
        while remaining:
            written = os.write(self.master, remaining)
            remaining = remaining[written:]

    def paste(self, text):
        self.send(b"\x1b[200~" + text.encode() + b"\x1b[201~")

    def pump(self, timeout=0.1):
        ready, _, _ = select.select([self.master], [], [], timeout)
        if not ready:
            return False
        try:
            chunk = os.read(self.master, 65536)
        except OSError:
            return False
        if not chunk:
            return False
        self.screen.feed(chunk)
        pending = self._query_tail + chunk
        if b"\x1b[6n" in pending:
            self.send(b"\x1b[1;1R")
            self._query_tail = b""
        else:
            self._query_tail = pending[-3:]
        return True

    def settle(self, timeout=0.3):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and self.pump(0.03):
            pass

    def _wait(self, text, present, timeout):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            if (text in self.screen.text) == present:
                self.settle()
                if (text in self.screen.text) == present:
                    return
            if self.process.poll() is not None:
                break
        state = "appear" if present else "disappear"
        raise AssertionError(
            f"Expected {text!r} to {state}; exit={self.process.poll()}\n{self.screen.text}"
        )

    def wait_for(self, text, timeout=8):
        self._wait(text, True, timeout)

    def wait_absent(self, text, timeout=8):
        self._wait(text, False, timeout)

    def _mouse(self, code, x, y, *, release=False):
        if not (0 <= x < self.width and 0 <= y < self.height):
            raise ValueError(f"Mouse point outside terminal: {(x, y)}")
        self.send(f"\x1b[<{code};{x + 1};{y + 1}{'m' if release else 'M'}")

    def click_at(self, x, y):
        self._mouse(0, x, y)
        self._mouse(0, x, y, release=True)

    def drag(self, start_x, start_y, end_x, end_y):
        self._mouse(0, start_x, start_y)
        self._mouse(32, end_x, end_y)
        self._mouse(0, end_x, end_y, release=True)

    def click_text(self, label, *, within=None):
        """Click a unique ASCII label, optionally scoped to an (x, y, w, h) rectangle."""
        if not label or not label.isascii():
            raise ValueError("Use an ASCII label or exact coordinates")
        self.wait_for(label)
        x, y, width, height = within or (0, 0, self.width, self.height)
        matches = []
        for row in range(y, min(y + height, self.height)):
            line = self.screen.text.splitlines()[row]
            start, limit = max(x, 0), min(x + width, self.width)
            while start < limit:
                column = line.find(label, start, limit)
                if column < 0:
                    break
                matches.append((column, row))
                start = column + len(label)
        if len(matches) != 1:
            raise AssertionError(
                f"Expected a unique {label!r}; found {matches}. Scope the label or use coordinates.\n"
                + self.screen.text
            )
        self.click_at(*matches[0])

    def save_screen(self, path):
        self.settle()
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(self.screen.text)

    def wait_exit(self, timeout=5, expected_code=0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline and self.process.poll() is None:
            self.pump()
        if self.process.poll() is None:
            raise AssertionError(f"App did not exit\n{self.screen.text}")
        if self.process.returncode != expected_code:
            raise AssertionError(f"Exit={self.process.returncode}\n{self.screen.text}")

    def close(self):
        if self._closed:
            return
        self._closed = True
        try:
            if self.process.poll() is None:
                os.killpg(self.process.pid, signal.SIGTERM)
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(self.process.pid, signal.SIGKILL)
                    self.process.wait(timeout=5)
        finally:
            os.close(self.master)
            self._config.cleanup()

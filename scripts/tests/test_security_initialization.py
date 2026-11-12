"""Execute the Compose initialization shell in temporary directories, without Docker."""

from __future__ import annotations

import re
import stat
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path


def initialization_script() -> str:
    source = (Path(__file__).parents[2] / "platforms/linux/compose.yaml").read_text()
    block = re.search(r"\n    command:\n      - \|\n((?:        [^\n]*\n|\n)+)", source)
    if block is None:
        raise AssertionError("Compose initialization command not found")
    return textwrap.dedent(block.group(1)).replace("$$", "$")


def run_initialization(directory: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["/bin/sh", "-ec", initialization_script()],
        cwd=directory,
        capture_output=True,
        text=True,
        check=False,
    )


class InitializationSecurityTests(unittest.TestCase):
    def test_password_is_never_logged_and_restart_preserves_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = run_initialization(root)
            self.assertEqual(first.returncode, 0)
            password = (root / "admin-password").read_bytes()
            config = (root / "config.json").read_bytes()
            self.assertEqual(len(password.strip()), 64)
            self.assertFalse(password.decode().strip() in first.stdout + first.stderr)
            self.assertIn("admin-password", first.stdout)
            self.assertEqual(stat.S_IMODE((root / "admin-password").stat().st_mode), 0o600)
            self.assertEqual(stat.S_IMODE((root / "config.json").stat().st_mode), 0o600)
            second = run_initialization(root)
            self.assertEqual(second.returncode, 0)
            self.assertEqual((root / "admin-password").read_bytes(), password)
            self.assertEqual((root / "config.json").read_bytes(), config)
            self.assertFalse(password.decode().strip() in second.stdout + second.stderr)
            self.assertEqual(list(root.glob(".sumpter-init.*")), [])

    def test_existing_files_are_not_replaced(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _ = (root / "admin-password").write_text("synthetic-existing-password\n")
            _ = (root / "config.json").write_text('{"synthetic":"preserve"}\n')
            result = run_initialization(root)
            self.assertEqual(result.returncode, 0)
            self.assertEqual((root / "admin-password").read_text(), "synthetic-existing-password\n")
            self.assertEqual((root / "config.json").read_text(), '{"synthetic":"preserve"}\n')
            self.assertNotIn("synthetic-existing-password", result.stdout + result.stderr)

    def test_symlink_password_is_rejected_before_creation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "admin-password").symlink_to(root / "missing")
            self.assertNotEqual(run_initialization(root).returncode, 0)
            self.assertFalse((root / "config.json").exists())


if __name__ == "__main__":
    _ = unittest.main()

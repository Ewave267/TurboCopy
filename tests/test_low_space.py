"""Real ENOSPC checks in a private, size-limited Linux tmpfs mount."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("TCOPY_BIN", ROOT / "target/debug/tcopy")).resolve()


class LowSpace(unittest.TestCase):
    def test_full_filesystem_keeps_destination_and_removes_stages(self):
        result = subprocess.run(
            ["unshare", "--user", "--map-root-user", "--mount", sys.executable,
             str(Path(__file__).resolve()), "--inside"],
            capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


def inside():
    with tempfile.TemporaryDirectory(prefix="tcopy-enospc-") as directory:
        root = Path(directory)
        source = root / "source"
        source.write_bytes(b"x" * (2 << 20))
        destination = root / "limited"
        destination.mkdir()
        subprocess.run(["mount", "-t", "tmpfs", "-o", "size=1m", "tmpfs", str(destination)], check=True)
        try:
            target = destination / "file"
            for options in ([], ["--workers", "4", "--chunk-size", "512K"], ["--gpfs"]):
                target.write_bytes(b"old destination")
                result = subprocess.run([str(BINARY), *options, str(source), str(target)],
                                        capture_output=True, text=True, timeout=10)
                assert result.returncode == 1, result.stdout + result.stderr
                assert "No space left on device" in result.stderr, result.stderr
                assert target.read_bytes() == b"old destination"
                assert sorted(p.name for p in destination.iterdir()) == ["file"]
        finally:
            subprocess.run(["umount", str(destination)], check=True)


if __name__ == "__main__":
    if sys.argv[1:] == ["--inside"]:
        inside()
    else:
        unittest.main()

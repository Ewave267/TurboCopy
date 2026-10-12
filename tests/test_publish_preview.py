import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "publish_preview", Path(__file__).resolve().parents[1] / "tools/publish_preview.py")
preview = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preview)


class PublishTests(unittest.TestCase):
    def test_publication_precedes_retention_and_covers_all_pages(self):
        with tempfile.TemporaryDirectory() as name:
            folder = Path(name)
            for arch in ("x86_64", "aarch64"):
                (folder / f"tcopy-linux-{arch}.tar.gz").write_bytes(b"package")
            calls = []

            def gh(*args):
                calls.append(args)
                if args[:2] == ("api", "repos/owner/repo/commits/master"):
                    return json.dumps({"sha": "abc"})
                if args[:3] == ("api", "--paginate", "--slurp"):
                    return json.dumps([[{"tag_name": "stable"}, {"tag_name": "preview-1-1"}],
                                       [{"tag_name": "preview-2-1"}]])
                return ""

            with patch.object(preview, "gh", side_effect=gh):
                preview.publish(folder, "owner/repo", "master", "abc", "2", "1")
            self.assertEqual([call[2] for call in calls if call[:2] == ("release", "delete")],
                             ["stable", "preview-1-1"])
            self.assertTrue(all("--cleanup-tag" in call for call in calls
                                if call[:2] == ("release", "delete")))
            edit = next(i for i, call in enumerate(calls) if call[:2] == ("release", "edit"))
            self.assertTrue(all(i > edit for i, call in enumerate(calls)
                                if call[:2] == ("release", "delete")))
            self.assertEqual(len((folder / "SHA256SUMS").read_text().splitlines()), 2)

    def test_stale_build_never_mutates_releases(self):
        with patch.object(preview, "gh", return_value=json.dumps({"sha": "new"})) as gh:
            preview.publish(Path("unused"), "owner/repo", "master", "old", "2", "1")
            self.assertEqual(gh.call_count, 1)

    def test_missing_package_never_mutates_releases(self):
        with tempfile.TemporaryDirectory() as name:
            with patch.object(preview, "gh", return_value=json.dumps({"sha": "abc"})) as gh:
                with self.assertRaises(ValueError):
                    preview.publish(Path(name), "owner/repo", "master", "abc", "2", "1")
                self.assertEqual(gh.call_count, 1)

    def test_upload_or_publication_failure_never_deletes_previous_release(self):
        for failure in ("upload", "edit"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as name:
                folder = Path(name)
                for arch in ("x86_64", "aarch64"):
                    (folder / f"tcopy-linux-{arch}.tar.gz").write_bytes(b"package")

                def gh(*args):
                    if args[0] == "api":
                        return json.dumps({"sha": "abc"})
                    if args[:2] == ("release", failure):
                        raise subprocess.CalledProcessError(1, args)
                    return ""

                with patch.object(preview, "gh", side_effect=gh) as mocked:
                    with self.assertRaises(subprocess.CalledProcessError):
                        preview.publish(folder, "owner/repo", "master", "abc", "2", "1")
                    self.assertFalse(any(call.args[:2] == ("release", "delete")
                                         for call in mocked.call_args_list))

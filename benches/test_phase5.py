"""Small checks for Phase 5 safety, cache residency and subprocess accounting."""
import os
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import phase5


class Phase5Tests(unittest.TestCase):
    def test_btrfs_device_discovery_uses_mount_point(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            mount = {"target": "/home", "source": "/dev/mapper/test[/home]",
                     "fstype": "btrfs", "maj:min": "0:36"}

            def capture(command):
                output = ""
                if command[0] == "findmnt":
                    output = json.dumps({"filesystems": [mount]})
                elif command[1:3] == ["filesystem", "show"]:
                    self.assertEqual(command[-1], "/home")
                    output = f"Total devices 1\n devid 1 size 1 used 1 path {parent}\n"
                return {"returncode": 0, "stdout": output, "stderr": ""}

            with patch.object(phase5, "capture", side_effect=capture), \
                 patch.object(phase5, "physical_devices", return_value=["nvme0n1"]):
                fs = phase5.filesystem(parent)
            self.assertEqual(fs["physical_devices"], ["nvme0n1"])
            self.assertEqual(fs["path"], str(parent))

    def test_same_device_space_is_added_before_reserve(self):
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            usage = phase5.shutil._ntuple_diskusage(total=1000, used=300, free=700)
            with patch.object(phase5.shutil, "disk_usage", return_value=usage), \
                 patch.object(phase5, "memory_available", return_value=phase5.GIB):
                phase5.check_space([parent, parent], [200, 200], 0, .1)
                with self.assertRaises(phase5.StopBenchmark):
                    phase5.check_space([parent, parent], [350, 350], 0, .1)

    def test_dense_data_and_metadata_detect_corruption(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "source"
            report = phase5.make_source(source, 3, phase5.MIB, lambda: None)
            self.assertGreaterEqual(report["allocated_bytes"], 3 * phase5.MIB)
            self.assertTrue(all(r > .99 for r in report["compression_sample_ratios"]))
            expected = phase5.checked_manifest(source)
            before = (source / "file-00000").stat()
            with (source / "file-00000").open("r+b") as handle:
                handle.write(b"corrupt")
            os.utime(source / "file-00000", ns=(before.st_atime_ns, before.st_mtime_ns))
            self.assertNotEqual(expected, phase5.checked_manifest(source))
            warm = phase5.prepare_cache(sorted(source.iterdir()), "warm")
            self.assertEqual(warm["status"], "controlled")

    def test_process_io_includes_waited_child_and_stops_on_pressure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            command = ["python3", "-c", "import subprocess; subprocess.run(['python3', '-c', "
                       "'import sys; sys.stdout.write(chr(120)*1048576)'], check=True)"]
            metrics, output = phase5.run_tool(command, root, lambda: None)
            self.assertEqual(len(output), phase5.MIB)
            self.assertGreaterEqual(metrics["process_io"]["wchar"], phase5.MIB)
            self.assertGreater(metrics["user_cpu_seconds"], 0)

            def stop():
                raise phase5.StopBenchmark("test pressure stop")

            with self.assertRaisesRegex(phase5.StopBenchmark, "test pressure stop"):
                phase5.run_tool(["sleep", "10"], root, stop)

    def test_uncontrolled_and_incomplete_results_cannot_make_complete_groups(self):
        sample = dict(pair=0, workload="test", scenario="dense", tool="test", status="verified",
                      cache={"mode": "storage", "status": "uncontrolled"}, seconds=1,
                      copy_seconds=.9, sync_seconds=.1, cpu_percent_one_core=20,
                      throughput_mib_s=5, files_per_second=1)
        result = {"samples": [sample], "runs": 3}
        phase5.summarize(result)
        self.assertEqual(result["summaries"], [])
        sample["cache"]["status"] = "controlled"
        phase5.summarize(result)
        self.assertFalse(result["summaries"][0]["complete"])


if __name__ == "__main__":
    unittest.main()

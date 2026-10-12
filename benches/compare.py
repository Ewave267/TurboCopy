#!/usr/bin/env python3
"""Warm-cache copy and unchanged-incremental benchmark; no global cache changes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import shutil
import statistics
import subprocess
import tempfile
import time


def durable_tree(root):
    directories = []
    for current, _, files in os.walk(root):
        directories.append(Path(current))
        for name in files:
            with (Path(current) / name).open("rb") as handle:
                os.fsync(handle.fileno())
    for directory in reversed(directories):
        fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def manifest(root):
    result = {}
    for path in [root, *root.rglob("*")]:
        stat = path.stat()
        digest = None
        if path.is_file():
            with path.open("rb") as handle:
                digest = hashlib.file_digest(handle, "sha256").hexdigest()
        result[str(path.relative_to(root))] = (stat.st_mode & 0o7777, stat.st_mtime_ns, digest)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/tcopy"))
    parser.add_argument("--directory", type=Path, help="Parent filesystem for owned temporary dataset")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--small-files", type=int, default=1000)
    parser.add_argument("--medium-files", type=int, default=16)
    parser.add_argument("--large-mib", type=int, default=64)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 1 or min(args.small_files, args.medium_files) < 0 or args.large_mib < 1:
        parser.error("runs and large-mib must be positive; file counts must be nonnegative")
    binary = args.binary.resolve(strict=True)
    tools = {"tcopy": [str(binary), "--json"], "tcopy --sync": [str(binary), "--sync", "--json"],
             "tcopy chunks": [str(binary), "--workers=4", "--chunk-size=8M", "--json"],
             "tcopy chunks --sync": [str(binary), "--workers=4", "--chunk-size=8M", "--sync", "--json"],
             "tcopy auto": [str(binary), "--auto", "--json"],
             "tcopy auto --sync": [str(binary), "--auto", "--sync", "--json"],
             "tcopy tune": [str(binary), "--tune", "--json"],
             "tcopy tune --sync": [str(binary), "--tune", "--sync", "--json"]}
    if shutil.which("cp"):
        tools["cp"] = ["cp", "-rp", "--reflink=never", "--sparse=never"]
    if shutil.which("rsync"):
        tools["rsync"] = ["rsync", "-rltp"]
    result = {"platform": platform.platform(), "cpus": os.cpu_count(),
              "cache": "warm (source read before each sample)",
              "durability": "tool completion plus file and directory fsync for every tool",
              "runs": args.runs, "tools": tools, "samples": []}
    result["tool_versions"] = {name: subprocess.check_output(
        [command[0], "--version"], text=True).splitlines()[0]
        for name, command in tools.items()}
    if shutil.which("rustc"):
        result["rustc"] = subprocess.check_output(["rustc", "--version"], text=True).strip()
    with tempfile.TemporaryDirectory(prefix="tcopy-bench-", dir=args.directory) as owned:
        root = Path(owned)
        source = root / "source"
        source.mkdir()
        rng = random.Random(2026)
        block = rng.randbytes(1024 * 1024)
        for index in range(args.small_files):
            (source / f"small-{index:06d}").write_bytes(rng.randbytes(4096))
        for index in range(args.medium_files):
            (source / f"medium-{index:02d}").write_bytes(block)
        with (source / "large").open("wb") as handle:
            for _ in range(args.large_mib):
                handle.write(block)
        (source / "empty").mkdir()
        durable_tree(source)
        expected = manifest(source)
        files = [path for path in source.rglob("*") if path.is_file()]
        result["files"] = len(files)
        result["bytes"] = sum(path.stat().st_size for path in files)
        if shutil.which("findmnt"):
            result["filesystem"] = subprocess.check_output(
                ["findmnt", "-T", str(root), "-n", "-o", "FSTYPE,SOURCE"], text=True).strip()
        for run in range(args.runs):
            order = list(tools)
            rng.shuffle(order)
            for index, name in enumerate(order):
                destination = root / f"run-{run}-{index}"
                destination.mkdir()
                command = [*tools[name], str(source), str(destination)]
                for scenario in ["first-copy", "unchanged-incremental"]:
                    for path in files:
                        with path.open("rb") as handle:
                            while handle.read(1024 * 1024):
                                pass
                    started = time.perf_counter()
                    completed = subprocess.run(command, check=True, capture_output=True, text=True)
                    durable_tree(destination)
                    elapsed = time.perf_counter() - started
                    assert manifest(destination / "source") == expected, f"{name}: content/metadata mismatch"
                    sample = {"tool": name, "scenario": scenario, "run": run + 1,
                              "seconds": elapsed, "stdout": completed.stdout.strip()}
                    if tools[name][0] == str(binary):
                        metrics = json.loads(completed.stdout)
                        assert metrics["failures"] == 0
                        assert metrics["peak_active_workers"] <= metrics["workers"]
                        if "--sync" in tools[name] and scenario == "unchanged-incremental":
                            assert metrics["skipped"] == len(files) and metrics["transferred_bytes"] == 0
                        else:
                            assert metrics["bytes"] == result["bytes"] == metrics["transferred_bytes"]
                        sample["metrics"] = metrics
                    result["samples"].append(sample)
                shutil.rmtree(destination)
    result["medians_seconds"] = {
        name: {scenario: statistics.median(sample["seconds"] for sample in result["samples"]
                                            if sample["tool"] == name and sample["scenario"] == scenario)
               for scenario in ["first-copy", "unchanged-incremental"]}
        for name in tools}
    serialized = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(serialized)
    print(serialized)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Verified Phase 4 warm-cache benchmark across source/destination filesystems.

Use --directory once per writable filesystem. Temporary trees are owned and removed.
Cold-cache measurements require external cache control and are not attempted here.
"""
import argparse
import errno
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


def fs_type(path):
    if shutil.which("findmnt"):
        return subprocess.check_output(
            ["findmnt", "-T", str(path), "-n", "-o", "FSTYPE,SOURCE"], text=True
        ).strip()
    return "unknown"


def durable_tree(root):
    for current, _, files in os.walk(root, topdown=False):
        for name in files:
            with (Path(current) / name).open("rb") as handle:
                os.fsync(handle.fileno())
        fd = os.open(current, os.O_RDONLY | os.O_DIRECTORY)
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
        xattrs = {name: os.getxattr(path, name) for name in os.listxattr(path)
                  if name.startswith("user.")}
        result[str(path.relative_to(root))] = (stat.st_mode & 0o7777, stat.st_mtime_ns,
                                               digest, xattrs)
    return result


def make_source(source, small_files, sparse_mib, xattrs):
    source.mkdir()
    rng = random.Random(2026)
    (source / "empty").mkdir()
    for index in range(small_files):
        (source / f"small-{index:04d}").write_bytes(rng.randbytes(4096))
    with (source / "dense").open("wb") as handle:
        handle.write(rng.randbytes(4 * 1024 * 1024))
    with (source / "sparse").open("wb") as handle:
        handle.write(b"head")
        handle.seek(sparse_mib * 1024 * 1024)
        handle.write(b"tail")
    if xattrs:
        os.setxattr(source / "dense", "user.tcopy_phase4", b"phase4")
    for path in reversed([source, *source.rglob("*")]):
        os.utime(path, ns=(1_600_000_000_000_000_000, 1_600_000_000_000_000_000))
    durable_tree(source)


def reports_sparse_hole(path):
    stat = path.stat()
    if stat.st_blocks * 512 >= stat.st_size // 4:
        return False
    with path.open("rb") as handle:
        try:
            hole = os.lseek(handle.fileno(), 0, os.SEEK_HOLE)
            return hole < stat.st_size and os.lseek(handle.fileno(), hole, os.SEEK_DATA) > hole
        except OSError as error:
            if error.errno in (errno.EINVAL, errno.ENXIO, errno.EOPNOTSUPP):
                return False
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/tcopy"))
    parser.add_argument("--directory", type=Path, action="append", required=True,
                        help="Writable filesystem parent; repeat for a source/destination matrix")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--small-files", type=int, default=100)
    parser.add_argument("--sparse-mib", type=int, default=32)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.runs < 1 or args.small_files < 0 or args.sparse_mib < 1:
        parser.error("runs and sparse-mib must be positive; small-files must be nonnegative")
    binary = args.binary.resolve(strict=True)
    parents = [path.resolve(strict=True) for path in args.directory]
    if any(not path.is_dir() or not os.access(path, os.W_OK) for path in parents):
        parser.error("every --directory must be writable")
    tools = {"tcopy": [str(binary), "--json"],
             "tcopy sparse": [str(binary), "--sparse", "--json"],
             "tcopy sparse --sync": [str(binary), "--sparse", "--sync", "--json"],
             "tcopy sparse --checksum --sync": [str(binary), "--sparse", "--checksum", "--sync", "--json"],
             "tcopy extended": [str(binary), "--sparse", "--preserve-extended", "--json"],
             "tcopy reflink": [str(binary), "--reflink", "--sparse", "--json"]}
    if shutil.which("cp"):
        tools["cp sparse"] = ["cp", "-a", "--reflink=never", "--sparse=always"]
        tools["cp reflink"] = ["cp", "-a", "--reflink=always"]
    if shutil.which("rsync"):
        tools["rsync sparse"] = ["rsync", "-aXS"]
    result = {"platform": platform.platform(), "cpus": os.cpu_count(),
              "cache": "warm: source read before each sample", "runs": args.runs,
              "durability": "tool completion plus file and directory fsync for every tool",
              "dataset": {"small_files": args.small_files, "sparse_mib": args.sparse_mib,
                          "dense_mib": 4}, "tools": tools, "samples": [], "pairs": []}
    with binary.open("rb") as handle:
        result["binary_sha256"] = hashlib.file_digest(handle, "sha256").hexdigest()
    result["tool_versions"] = {name: subprocess.check_output(
        [command[0], "--version"], text=True).splitlines()[0]
        for name, command in tools.items()}
    for source_parent in parents:
        for destination_parent in parents:
            with tempfile.TemporaryDirectory(prefix="tcopy-phase4-src-", dir=source_parent) as src_owned, \
                 tempfile.TemporaryDirectory(prefix="tcopy-phase4-dst-", dir=destination_parent) as dst_owned:
                source = Path(src_owned) / "source"
                root = Path(dst_owned)
                xattrs = True
                try:
                    for path in (source.parent, root):
                        os.setxattr(path, "user.tcopy_phase4_probe", b"1")
                        os.removexattr(path, "user.tcopy_phase4_probe")
                except OSError as error:
                    if error.errno not in (errno.ENOTSUP, errno.EOPNOTSUPP, errno.ENOSYS):
                        raise
                    xattrs = False
                make_source(source, args.small_files, args.sparse_mib, xattrs)
                check_holes = reports_sparse_hole(source / "sparse")
                expected = manifest(source)
                files = [path for path in source.rglob("*") if path.is_file()]
                pair = {"source_parent": str(source_parent), "destination_parent": str(destination_parent),
                        "source_filesystem": fs_type(source), "destination_filesystem": fs_type(root),
                        "files": len(files), "logical_bytes": sum(path.stat().st_size for path in files),
                        "user_xattrs": xattrs, "seek_hole_reports_sparse_extents": check_holes}
                result["pairs"].append(pair)
                pair_tools = {name: command for name, command in tools.items()
                              if xattrs or name != "tcopy extended"}
                if not xattrs:
                    pair["omitted"] = {"tcopy extended": "user xattrs unsupported by source or destination"}
                for run in range(args.runs):
                    order = list(pair_tools)
                    random.Random(2026 + run).shuffle(order)
                    for index, name in enumerate(order):
                        destination = root / f"run-{run}-{index}"
                        destination.mkdir()
                        command = [*pair_tools[name], str(source), str(destination)]
                        for scenario in ("first-copy", "unchanged-incremental"):
                            for path in files:
                                with path.open("rb") as handle:
                                    while handle.read(1024 * 1024):
                                        pass
                            started = time.perf_counter()
                            completed = subprocess.run(command, capture_output=True, text=True)
                            if completed.returncode:
                                if name == "cp reflink" and any(message in completed.stderr.lower() for message in
                                        ("operation not supported", "invalid cross-device link", "cross-device link",
                                         "not supported", "invalid argument")):
                                    result["samples"].append({"pair": len(result["pairs"]) - 1,
                                        "tool": name, "scenario": scenario, "run": run + 1,
                                        "status": "unsupported", "returncode": completed.returncode,
                                        "stderr": completed.stderr.strip()})
                                    break
                                raise RuntimeError(f"{name} failed ({completed.returncode}): {completed.stderr}")
                            durable_tree(destination)
                            elapsed = time.perf_counter() - started
                            copied = destination / "source"
                            actual = manifest(copied)
                            if not (name.startswith("tcopy") and name not in ("tcopy extended",)):
                                mismatch = [(path, expected[path], actual.get(path)) for path in expected
                                            if actual.get(path) != expected[path]]
                                assert not mismatch, f"{name} {scenario}: content/metadata mismatch: {mismatch[:3]}"
                            else:
                                assert {path: data[:3] for path, data in actual.items()} == {
                                    path: data[:3] for path, data in expected.items()
                                }, f"{name}: content/metadata mismatch"
                            sparse = (copied / "sparse").stat()
                            if check_holes and name != "tcopy":
                                assert sparse.st_blocks * 512 < sparse.st_size // 4, f"{name}: sparse holes lost"
                            sample = {"pair": len(result["pairs"]) - 1, "tool": name,
                                      "scenario": scenario, "run": run + 1, "seconds": elapsed,
                                      "sparse_allocated_bytes": sparse.st_blocks * 512,
                                      "status": "verified"}
                            if command[0] == str(binary):
                                metrics = json.loads(completed.stdout)
                                assert metrics["failures"] == 0
                                assert metrics["peak_active_workers"] <= metrics["workers"]
                                if "--sync" in command and scenario == "unchanged-incremental":
                                    assert metrics["skipped"] == len(files) and metrics["transferred_bytes"] == 0
                                sample["metrics"] = metrics
                            result["samples"].append(sample)
                        shutil.rmtree(destination)
    result["medians_seconds"] = {
        str(pair): {name: {scenario: statistics.median(
            sample["seconds"] for sample in result["samples"]
            if sample["pair"] == pair and sample["tool"] == name and sample["scenario"] == scenario
            and sample["status"] == "verified")
            for scenario in ("first-copy", "unchanged-incremental")}
            for name in tools if all(any(sample["pair"] == pair and sample["tool"] == name
                                      and sample["scenario"] == scenario and sample["status"] == "verified"
                                      for sample in result["samples"])
                                      for scenario in ("first-copy", "unchanged-incremental"))}
        for pair in range(len(result["pairs"]))}
    serialized = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(serialized)
    print(serialized)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Verified warm-cache Linux copy-syscall experiment, including durability.

Python loop overhead is included. copy_file_range may clone/offload on Btrfs;
these results do not select a production engine or establish Rust throughput.
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


METHODS = ["read_write", "pread_pwrite", "sendfile", "copy_file_range"]
UNSUPPORTED = {errno.ENOSYS, errno.EOPNOTSUPP, errno.EINVAL, errno.EXDEV}


def transfer(method, source, destination, length, buffer_size):
    offset = 0
    while offset < length:
        limit = min(buffer_size, length - offset)
        try:
            if method in ("read_write", "pread_pwrite"):
                block = (os.read(source, limit) if method == "read_write"
                         else os.pread(source, limit, offset))
                if not block:
                    raise EOFError("source shrank")
                written = 0
                while written < len(block):
                    try:
                        count = (os.write(destination, memoryview(block)[written:])
                                 if method == "read_write" else os.pwrite(
                                     destination, memoryview(block)[written:], offset + written))
                    except InterruptedError:
                        continue
                    if count == 0:
                        raise OSError(errno.EIO, "write returned zero")
                    written += count
                count = len(block)
            elif method == "sendfile":
                count = os.sendfile(destination, source, offset, limit)
            elif method == "copy_file_range":
                count = os.copy_file_range(source, destination, limit, offset, offset)
            else:
                raise ValueError(method)
        except InterruptedError:
            continue
        if count == 0:
            raise EOFError("copy returned zero before observed source length")
        offset += count
    assert offset == length


def digest(path):
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--mib", type=int, default=128)
    parser.add_argument("--buffer-kib", type=int, default=128)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if min(args.runs, args.mib, args.buffer_kib) < 1 or args.buffer_kib > 65536:
        parser.error("runs/mib must be positive; buffer-kib must be 1..65536")
    result = {"platform": platform.platform(), "python": platform.python_version(),
              "cache": "warm: source read before every sample", "runs": args.runs,
              "durability": "create, transfer, mode/mtime, file fsync, directory fsync included",
              "caveats": ["Python loop overhead included", "copy_file_range may clone/offload; not a physical-copy speed comparison"],
              "buffer_bytes": args.buffer_kib * 1024, "samples": []}
    rng = random.Random(2026)
    with tempfile.TemporaryDirectory(prefix="tcopy-bench-io-", dir=args.directory) as owned:
        root = Path(owned)
        source = root / "source"
        block = rng.randbytes(1024 * 1024)
        with source.open("wb") as handle:
            for _ in range(args.mib):
                handle.write(block)
            handle.write(b"partial last tail")
            os.fchmod(handle.fileno(), 0o640)
            os.fsync(handle.fileno())
        metadata = source.stat()
        expected = digest(source)
        result["bytes"] = metadata.st_size
        result["dataset"] = "repeated seeded random 1 MiB block plus 17-byte tail"
        if shutil.which("findmnt"):
            result["filesystem"] = subprocess.check_output(
                ["findmnt", "-T", str(root), "-n", "-o", "FSTYPE,SOURCE"], text=True).strip()
        for run in range(args.runs):
            order = METHODS.copy()
            rng.shuffle(order)
            for method in order:
                destination = root / "destination"
                with source.open("rb") as warm:
                    while warm.read(1024 * 1024):
                        pass
                sample = {"method": method, "run": run + 1}
                started = time.perf_counter()
                try:
                    with source.open("rb", buffering=0) as incoming, destination.open("xb", buffering=0) as outgoing:
                        transfer(method, incoming.fileno(), outgoing.fileno(), metadata.st_size, args.buffer_kib * 1024)
                        os.fchmod(outgoing.fileno(), metadata.st_mode & 0o7777)
                        os.utime(outgoing.fileno(), ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
                        os.fsync(outgoing.fileno())
                    directory_fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
                    try:
                        os.fsync(directory_fd)
                    finally:
                        os.close(directory_fd)
                    sample["seconds"] = time.perf_counter() - started
                    actual = destination.stat()
                    assert actual.st_size == metadata.st_size
                    assert actual.st_mode & 0o7777 == metadata.st_mode & 0o7777
                    assert actual.st_mtime_ns == metadata.st_mtime_ns
                    assert digest(destination) == expected, f"{method}: content mismatch"
                    sample["status"] = "verified"
                except OSError as error:
                    if error.errno not in UNSUPPORTED:
                        raise
                    sample.update(status="unsupported", errno=error.errno, error=str(error))
                finally:
                    destination.unlink(missing_ok=True)
                result["samples"].append(sample)
    result["medians_seconds"] = {}
    for method in METHODS:
        samples = [s["seconds"] for s in result["samples"] if s["method"] == method and s["status"] == "verified"]
        result["medians_seconds"][method] = statistics.median(samples) if len(samples) == args.runs else None
    serialized = json.dumps(result, indent=2) + "\n"
    if args.output:
        args.output.write_text(serialized)
    print(serialized)


if __name__ == "__main__":
    main()

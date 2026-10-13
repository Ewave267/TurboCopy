"""Real loopback SSH tests. Requires OpenSSH client/server and a built tcopy."""

import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import signal
import socket
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("TCOPY_BIN", ROOT / "target/debug/tcopy")).resolve()


class NativeSSH(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # CI uses noexec /dev/shm for Rust chunk tests; SSH wrappers need executable storage.
        cls.temp = tempfile.TemporaryDirectory(prefix="tcopy-real-ssh-", dir="/tmp")
        cls.addClassCleanup(cls.temp.cleanup)
        cls.root = Path(cls.temp.name)
        sshd = shutil.which("sshd")
        if not sshd or not shutil.which("ssh-keygen") or not BINARY.is_file():
            raise RuntimeError("build tcopy and install openssh-client/openssh-server before running these tests")
        cls.ssh = shutil.which("ssh")
        for name in ("host_key", "client_key"):
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(cls.root / name)], check=True)
        (cls.root / "authorized_keys").write_bytes((cls.root / "client_key.pub").read_bytes())
        cls.daemons = []
        cls.ports = []
        for index in range(2):
            with socket.socket() as reserved:
                reserved.bind(("127.0.0.1", 0))
                port = reserved.getsockname()[1]
            cls.ports.append(port)
            config = cls.root / f"sshd-{index}.conf"
            config.write_text(
                f"Port {port}\nListenAddress 127.0.0.1\nHostKey {cls.root / 'host_key'}\n"
                f"PidFile {cls.root / f'sshd-{index}.pid'}\nAuthorizedKeysFile {cls.root / 'authorized_keys'}\n"
                "StrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n"
                "PubkeyAuthentication yes\nAllowTcpForwarding local\nAllowAgentForwarding no\n"
                "X11Forwarding no\nPrintMotd no\nLogLevel ERROR\n"
            )
            log = open(cls.root / f"sshd-{index}.log", "wb")
            cls.addClassCleanup(log.close)
            daemon = subprocess.Popen([sshd, "-D", "-e", "-f", str(config)], stderr=log, stdout=subprocess.DEVNULL)
            cls.daemons.append(daemon)
            cls.addClassCleanup(cls.stop, daemon)
            deadline = time.monotonic() + 5
            while True:
                if daemon.poll() is not None:
                    raise RuntimeError((cls.root / f"sshd-{index}.log").read_text())
                try:
                    with socket.create_connection(("127.0.0.1", port), 0.1):
                        break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise RuntimeError("loopback SSH daemon did not start")
                    time.sleep(0.02)
        cls.config = cls.root / "ssh.conf"
        known_hosts = cls.root / "known_hosts"
        public = (cls.root / "host_key.pub").read_text().split()
        known_hosts.write_text("".join(f"[127.0.0.1]:{port} {public[0]} {public[1]}\n" for port in cls.ports))
        cls.config.write_text(
            "".join(f"Host source\n  HostName 127.0.0.1\n  Port {port}\n" if index == 0
                    else f"Host destination\n  HostName 127.0.0.1\n  Port {port}\n"
                    for index, port in enumerate(cls.ports))
            + f"Host *\n  User {pwd.getpwuid(os.getuid()).pw_name}\n  IdentityFile {cls.root / 'client_key'}\n"
            f"  UserKnownHostsFile {known_hosts}\n  StrictHostKeyChecking yes\n  IdentitiesOnly yes\n"
            "  ControlMaster no\n  LogLevel ERROR\n"
        )
        cls.bin = cls.root / "bin"
        cls.bin.mkdir()
        (cls.bin / "ssh").write_text(f'#!/bin/sh\nprintf "%s\\n" "$@" >> "$TCOPY_TEST_SSH_TRACE"\nexec "{cls.ssh}" -F "$TCOPY_TEST_SSH_CONFIG" "$@"\n')
        for name in ("rsync", "scp"):
            (cls.bin / name).write_text('#!/bin/sh\ntouch "$TCOPY_TEST_FORBIDDEN"\nexit 99\n')
        for path in cls.bin.iterdir():
            path.chmod(0o700)

    @staticmethod
    def stop(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()

    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory(prefix="data-", dir=self.root)
        self.addCleanup(self.fixture.cleanup)
        self.data = Path(self.fixture.name)
        self.trace = self.data / "ssh-trace"
        self.env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}",
                        TCOPY_TEST_SSH_CONFIG=str(self.config), TCOPY_TEST_SSH_TRACE=str(self.trace),
                        TCOPY_TEST_FORBIDDEN=str(self.data / "FORBIDDEN"))

    def remote(self, host, name):
        return f"{host}:{self.data}/{name}"

    def command(self, args):
        return [str(BINARY), "--ssh-program", str(BINARY), *args]

    def run_copy(self, *args, success=True):
        result = subprocess.run(self.command(args), env=self.env, cwd=self.data,
                                capture_output=True, text=True, timeout=45)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse((self.data / "FORBIDDEN").exists())
        return result

    def test_upload_download_and_two_remote_hosts(self):
        payload = bytes(range(251)) * 1000
        (self.data / "source").write_bytes(payload)
        (self.data / "uploaded").write_bytes(b"old")
        os.link(self.data / "uploaded", self.data / "alias")
        self.run_copy("source", self.remote("destination", "uploaded"))
        self.assertEqual((self.data / "uploaded").read_bytes(), payload)
        self.assertEqual((self.data / "alias").read_bytes(), b"old")
        self.run_copy(self.remote("source", "uploaded"), "downloaded")
        self.assertEqual((self.data / "downloaded").read_bytes(), payload)
        self.run_copy(self.remote("source", "source"), self.remote("destination", "relayed"))
        self.assertEqual((self.data / "relayed").read_bytes(), payload)
        self.assertEqual(self.trace.read_text().count("--ssh-agent"), 4)

    def test_tree_mapping_multiple_sources_metadata_and_literal_paths(self):
        (self.data / "tree/empty").mkdir(parents=True)
        name = "space ' $(touch INJECTED); [literal]"
        (self.data / "tree" / name).write_bytes(b"contents")
        (self.data / "tree" / name).chmod(0o640)
        os.symlink(name, self.data / "tree/link")
        self.run_copy("tree/", self.remote("destination", "new/"))
        self.assertTrue((self.data / "new/empty").is_dir())
        self.assertEqual(os.readlink(self.data / "new/link"), name)
        self.assertEqual((self.data / "new" / name).stat().st_mode & 0o777, 0o640)
        self.assertEqual((self.data / "new" / name).stat().st_mtime_ns, (self.data / "tree" / name).stat().st_mtime_ns)
        self.run_copy(self.remote("source", "tree/."), self.remote("destination", "contents"))
        self.assertEqual((self.data / "contents" / name).read_bytes(), b"contents")
        (self.data / "other").write_bytes(b"other")
        self.run_copy("other", self.remote("source", "tree"), self.remote("destination", "new"))
        self.assertTrue((self.data / "new/tree" / name).is_file())
        self.assertEqual((self.data / "new/other").read_bytes(), b"other")
        self.assertFalse((self.data / "INJECTED").exists())
        program = self.data / "agent ' $(touch INJECTED)"
        program.symlink_to(BINARY)
        self.run_copy("--ssh-program", str(program), "other", self.remote("destination", "quoted"))
        self.assertFalse((self.data / "INJECTED").exists())

    def test_sync_checksum_sparse_dry_run_and_json(self):
        (self.data / "source").write_bytes(b"new")
        (self.data / "dest").write_bytes(b"old")
        destination = self.remote("destination", "dest")
        report = json.loads(self.run_copy("--json", "--sync", "source", destination).stdout)
        self.assertEqual(report["backend"], "native-ssh")
        self.assertEqual(report["skipped"], 1)
        self.assertEqual((self.data / "dest").read_bytes(), b"old")
        output = self.run_copy("--dry-run", "--json", "--progress", "source", destination)
        self.assertEqual(len(output.stdout.splitlines()), 1)
        self.assertTrue(json.loads(output.stdout)["dry_run"])
        self.assertEqual((self.data / "dest").read_bytes(), b"old")
        self.run_copy("--sync", "--checksum", "source", destination)
        self.assertEqual((self.data / "dest").read_bytes(), b"new")
        with open(self.data / "zeros", "wb") as file:
            file.truncate(8 << 20)
        self.run_copy("--sparse", "zeros", self.remote("destination", "sparse"))
        self.assertEqual((self.data / "sparse").stat().st_size, 8 << 20)
        self.assertEqual((self.data / "sparse").stat().st_blocks, 0)

    def test_parallel_chunks_and_native_byte_integrity(self):
        block = bytes(range(251)) * 4096
        digest = hashlib.sha256()
        with open(self.data / "large", "wb") as file:
            for _ in range(70):
                file.write(block)
                digest.update(block)
            file.write(b"tail")
            digest.update(b"tail")
        result = self.run_copy("--json", "--workers", "4", "--chunk-size", "4M",
                               self.remote("source", "large"), self.remote("destination", "large-copy"))
        report = json.loads(result.stdout)
        self.assertEqual(report["chunked_files"], 1)
        self.assertLessEqual(report["peak_active_workers"], 4)
        self.assertGreater(report["peak_active_workers"], 1)
        copied = hashlib.sha256()
        with open(self.data / "large-copy", "rb") as file:
            for chunk in iter(lambda: file.read(1 << 20), b""):
                copied.update(chunk)
        self.assertEqual(copied.digest(), digest.digest())

    def test_many_small_files_empty_files_and_extra_destination_data(self):
        (self.data / "tree").mkdir()
        for index in range(150):
            (self.data / "tree" / str(index)).write_bytes(bytes([index % 251]) * 37)
        (self.data / "tree/empty").touch()
        (self.data / "target").mkdir()
        (self.data / "target/extra").write_bytes(b"keep")
        report = json.loads(self.run_copy("--json", "--workers", "4", self.remote("source", "tree/."), self.remote("destination", "target")).stdout)
        self.assertEqual(report["copied"], 151)
        for index in range(150):
            self.assertEqual((self.data / "target" / str(index)).read_bytes(), bytes([index % 251]) * 37)
        self.assertEqual((self.data / "target/empty").stat().st_size, 0)
        self.assertEqual((self.data / "target/extra").read_bytes(), b"keep")

    def test_rejections_missing_agent_and_interrupted_transfer(self):
        (self.data / "source").write_bytes(b"payload")
        os.mkfifo(self.data / "fifo")
        self.run_copy(self.remote("source", "fifo"), self.remote("destination", "fifo-copy"), success=False)
        self.assertFalse((self.data / "fifo-copy").exists())
        (self.data / "outside").mkdir()
        os.symlink("outside", self.data / "link")
        self.run_copy("source", self.remote("destination", "link/escape"), success=False)
        self.run_copy(self.remote("source", "outside"), self.remote("destination", "outside/nested"), success=False)
        self.run_copy("--resume", "source", self.remote("destination", "dest"), success=False)
        self.run_copy("--ssh-program", "/missing-tcopy", "source", self.remote("destination", "dest"), success=False)
        self.assertFalse((self.data / "outside/escape").exists())
        with open(self.data / "huge", "wb") as file:
            file.truncate(8 << 30)
        (self.data / "dest").write_bytes(b"old destination")
        process = subprocess.Popen(self.command(["huge", self.remote("destination", "dest")]),
                                   cwd=self.data, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(self.stop, process)
        deadline = time.monotonic() + 15
        while not list(self.data.glob(".tcopy-*.tmp")):
            if process.poll() is not None or time.monotonic() >= deadline:
                self.fail("transfer did not reach native staging")
            time.sleep(0.01)
        process.send_signal(signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 128 + signal.SIGTERM, stderr.decode())
        self.assertEqual((self.data / "dest").read_bytes(), b"old destination")
        self.assertFalse(list(self.data.glob(".tcopy-*.tmp")))

    def test_user_multiplexing_configuration_cannot_leave_persistent_tunnels(self):
        config = self.data / "multiplex.conf"
        config.write_text(f"Host *\n  ControlMaster auto\n  ControlPersist 10m\n  ControlPath {self.data}/control-%C\n" + self.config.read_text())
        self.env["TCOPY_TEST_SSH_CONFIG"] = str(config)
        (self.data / "source").write_bytes(b"native")
        self.run_copy("source", self.remote("destination", "dest"))
        self.assertEqual((self.data / "dest").read_bytes(), b"native")
        self.assertFalse(list(self.data.glob("control-*")))

    def test_disconnected_ssh_tunnel_keeps_destination_and_cleans_stage(self):
        with open(self.data / "huge", "wb") as file:
            file.truncate(8 << 30)
        for upload in (True, False):
            with self.subTest(upload=upload):
                (self.data / "dest").write_bytes(b"old destination")
                args = (["huge", self.remote("destination", "dest")] if upload else
                        [self.remote("source", "huge"), "dest"])
                process = subprocess.Popen(self.command(args), cwd=self.data, env=self.env,
                                           stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.addCleanup(self.stop, process)
                deadline = time.monotonic() + 15
                while not any(p.stat().st_blocks for p in self.data.glob(".tcopy-*.tmp")):
                    if process.poll() is not None or time.monotonic() >= deadline:
                        self.fail("transfer did not write native staging data")
                    time.sleep(0.01)
                children = Path(f"/proc/{process.pid}/task/{process.pid}/children").read_text().split()
                tunnels = [int(pid) for pid in children
                           if b"-N" in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")]
                self.assertEqual(len(tunnels), 1)
                os.kill(tunnels[0], signal.SIGKILL)
                stdout, stderr = process.communicate(timeout=40)
                self.assertNotEqual(process.returncode, 0, stdout.decode())
                self.assertEqual((self.data / "dest").read_bytes(), b"old destination", stderr.decode())
                # Remote sessions can observe tunnel EOF after the coordinator exits.
                deadline = time.monotonic() + 5
                while list(self.data.glob(".tcopy-*.tmp")) and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertFalse(list(self.data.glob(".tcopy-*.tmp")))


if __name__ == "__main__":
    unittest.main()

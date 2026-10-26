"""Unit checks for the shell's embedded service/Swap recovery primitives.

These exercise real temporary files. Service calls are injected only in rollback
unit tests; they do not claim to validate a running sshd, Docker, or systemd.
"""

import base64
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import unittest
from unittest import mock


SOURCE = Path(os.environ.get("CQS_TEST_SHELL_SOURCE", Path(__file__).resolve().parents[1] / "cqs.sh"))


def load_backend():
    text = SOURCE.read_text(encoding="utf-8")
    match = re.search(r"<<'CQS_SYSTEM_PY'\n(.*?)\nCQS_SYSTEM_PY\n", text, re.S)
    if not match:
        raise AssertionError("cqs.sh is missing the embedded system backend")
    namespace = {"__name__": "cqs_system_test"}
    exec(compile(match[1], str(SOURCE) + ":CQS_SYSTEM_PY", "exec"), namespace)
    return namespace


class ShellSystemTests(unittest.TestCase):
    def setUp(self):
        self.backend = load_backend()
        self.case = tempfile.TemporaryDirectory(prefix="cqs-shell-system-")
        self.addCleanup(self.case.cleanup)
        self.root = Path(self.case.name)

    def call(self, name, *args, **kwargs):
        return self.backend[name](*args, **kwargs)

    def test_ssh_candidate_preserves_include_and_match_and_effective_rejects_conflicts(self):
        original = "# Port 22\nPort 22\nInclude /etc/ssh/sshd_config.d/*.conf\nMatch User deploy\n Port 2222\n"
        settings = [("Port", "55520")]
        candidate = self.call("ssh_candidate", original, settings)
        self.assertTrue(candidate.startswith("Port 55520\n"))
        self.assertNotIn("\nPort 22\n", candidate)
        self.assertIn("Include /etc/ssh/sshd_config.d/*.conf", candidate)
        self.assertIn("Match User deploy\n Port 2222\n", candidate)
        self.call("ssh_effective", "port 55520\nlistenaddress [::]:55520\n", settings)
        for invalid in ("port 22\n", "port 55520\nport 22\n", "port 55520\nlistenaddress 0.0.0.0:22\n"):
            with self.assertRaises(ValueError):
                self.call("ssh_effective", invalid, settings)

    def test_key_settings_disable_both_password_methods(self):
        settings = [("PubkeyAuthentication", "yes"), ("PasswordAuthentication", "no"), ("KbdInteractiveAuthentication", "no")]
        original = "PasswordAuthentication yes\nKbdInteractiveAuthentication yes\nMatch User keep\n PasswordAuthentication yes\n"
        candidate = self.call("ssh_candidate", original, settings)
        self.assertEqual(candidate.count("PasswordAuthentication no\n"), 1)
        self.assertIn("KbdInteractiveAuthentication no\n", candidate)
        self.assertIn("Match User keep\n PasswordAuthentication yes\n", candidate)

    def test_docker_mirror_preserves_other_keys_and_rejects_unsafe_urls(self):
        original = '{"log-driver":"json-file","ipv6":true,"registry-mirrors":["https://old.example"]}'
        value = json.loads(self.call("docker_candidate", original, "mirror", "https://mirror.example/v2"))
        self.assertEqual(value["log-driver"], "json-file")
        self.assertTrue(value["ipv6"])
        self.assertEqual(value["registry-mirrors"], ["https://mirror.example/v2"])
        for invalid in ("https://", "ftp://mirror.example", "https://user:password@mirror.example", "https://mirror.example?query=1", "https://mirror.example#fragment", "https://mirror.example:99999", "https://mirror.example\ninvalid"):
            with self.subTest(url=invalid), self.assertRaises(ValueError):
                self.call("docker_candidate", original, "mirror", invalid)
        with self.assertRaises(ValueError):
            self.call("docker_candidate", "[]", "mirror", "https://mirror.example")

    def test_docker_ipv6_preserves_existing_subnet_and_removes_only_ipv6_fields(self):
        original = '{"fixed-cidr-v6":"fd12:3456:789a:1::/64","bip6":"fd12:3456:789a:1::1/64","default-gateway-v6":"fd12:3456:789a:1::1","log-driver":"local"}'
        on = json.loads(self.call("docker_candidate", original, "ipv6", "on", "fdab:cdef:1234:1::/64"))
        self.assertEqual(on["fixed-cidr-v6"], "fd12:3456:789a:1::/64")
        off = json.loads(self.call("docker_candidate", original, "ipv6", "off"))
        self.assertEqual(off, {"ipv6": False, "log-driver": "local"})
        generated = json.loads(self.call("docker_candidate", "{}", "ipv6", "on"))
        self.assertRegex(generated["fixed-cidr-v6"], r"^fd[0-9a-f]{2}:[0-9a-f]{4}:[0-9a-f]{4}:1::/64$")

    def test_swap_boundaries_and_fstab_only_remove_exact_swapfile(self):
        for memory, expected in ((512, 1024), (2048, 4096), (4096, 4096), (5120, 5120), (8192, 8192), (9216, 8192), (65536, 8192), (66560, 16384)):
            self.assertEqual(self.call("swap_size", memory), expected)
        text = "# /swapfile none swap defaults 0 0\n/swapfile2 none swap defaults 0 0\n  /swapfile none swap defaults 0 0\nUUID=x none swap defaults 0 0\n"
        output = self.call("fstab_without_swap", text)
        self.assertEqual(output, "# /swapfile none swap defaults 0 0\n/swapfile2 none swap defaults 0 0\nUUID=x none swap defaults 0 0\n")

    def test_atomic_saved_format_and_invalid_snapshot(self):
        target = self.root / "config"
        self.call("atomic_write", target, b"original\x00bytes", 0o640)
        saved = self.call("saved", target)
        self.assertEqual(base64.b64decode(saved["data"]), b"original\x00bytes")
        self.assertEqual(set(saved), {"data", "mode"})
        if os.name == "posix":
            self.assertEqual(saved["mode"], 0o640)
        self.call("atomic_write", target, b"changed", 0o600)
        self.call("saved_write", target, saved)
        self.assertEqual(target.read_bytes(), b"original\x00bytes")
        for invalid in ({"data": "!", "mode": 0o600}, {"data": "", "mode": 0o10000}, {"data": "", "mode": True}):
            with self.assertRaises((ValueError, TypeError)):
                self.call("saved_write", target, invalid)
        self.assertEqual(target.read_bytes(), b"original\x00bytes")

    def test_pending_blocks_a_new_transaction_before_mutation(self):
        target = self.root / "config"
        target.write_bytes(b"original")
        pending = Path(str(target) + ".cqs-pending.json")
        pending.write_bytes(b"unfinished")
        with self.assertRaises(ValueError):
            self.call("service_update", target, b"new", "test", False, lambda _: self.fail("validated despite pending"), lambda: self.fail("activated despite pending"))
        self.assertEqual(target.read_bytes(), b"original")
        self.assertEqual(pending.read_bytes(), b"unfinished")

    def test_candidate_validation_failure_never_replaces_file(self):
        target = self.root / "daemon.json"
        target.write_text('{"existing":true}', encoding="utf-8")
        def validate(candidate):
            json.loads(candidate.read_bytes())
        with self.assertRaises(ValueError):
            self.call("service_update", target, b"invalid json", "docker", True, validate, lambda: self.fail("activated invalid candidate"))
        self.assertEqual(target.read_text(), '{"existing":true}')
        self.assertFalse(Path(str(target) + ".cqs-pending.json").exists())

    def test_service_activation_failure_restores_file_and_old_inactive_state(self):
        target = self.root / "daemon.json"
        target.write_bytes(b'{"original":true}')
        calls = []
        def run(*args, **kwargs):
            calls.append(args)
            if args == ("systemctl", "is-active", "--quiet", "docker"):
                return 3
            if args == ("systemctl", "stop", "docker"):
                return 0
            raise AssertionError(args)
        def activate():
            self.assertEqual(target.read_bytes(), b'{"new":true}')
            snapshot = json.loads(Path(str(target) + ".cqs-pending.json").read_bytes())
            self.assertEqual(snapshot["service"], "docker")
            self.assertFalse(snapshot["was_active"])
            self.assertEqual(base64.b64decode(snapshot["file"]["data"]), b'{"original":true}')
            raise RuntimeError("injected service activation failure")
        with mock.patch.dict(self.backend, {"run": run}):
            with self.assertRaisesRegex(RuntimeError, "injected service activation failure"):
                self.call("service_update", target, b'{"new":true}', "docker", False, lambda path: json.loads(path.read_bytes()), activate)
        self.assertEqual(target.read_bytes(), b'{"original":true}')
        self.assertIn(("systemctl", "stop", "docker"), calls)
        self.assertFalse(Path(str(target) + ".cqs-pending.json").exists())

    def test_failed_service_recovery_retains_rust_compatible_pending_snapshot(self):
        target = self.root / "sshd_config"
        target.write_bytes(b"Port 22\n")
        def fail_run(*_args, **_kwargs):
            raise RuntimeError("injected service recovery failure")
        def fail_activate():
            raise RuntimeError("injected activation failure")
        with mock.patch.dict(self.backend, {"run": fail_run}):
            with self.assertRaisesRegex(RuntimeError, "recovery failed"):
                self.call("service_update", target, b"Port 55520\n", "ssh", True, lambda _: None, fail_activate)
        self.assertEqual(target.read_bytes(), b"Port 22\n")
        snapshot = json.loads(Path(str(target) + ".cqs-pending.json").read_bytes())
        self.assertEqual(set(snapshot), {"file", "service", "was_active"})
        self.assertEqual(base64.b64decode(snapshot["file"]["data"]), b"Port 22\n")

    @unittest.skipUnless(os.name == "posix", "symlink/Unix file mode checks require POSIX")
    def test_symlink_ancestors_rejected_and_saved_mode_retained(self):
        real = self.root / "real"
        real.mkdir()
        link = self.root / "link"
        link.symlink_to(real, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.call("atomic_write", link / "config", b"unsafe")
        self.assertFalse((real / "config").exists())
        target = real / "config"
        self.call("atomic_write", target, b"original", 0o640)
        self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o640)

    @unittest.skipUnless(os.name == "posix", "flock requires POSIX")
    def test_lock_excludes_concurrent_writers_and_releases_on_failure(self):
        target = self.root / "nested" / "config.cqs-lock"
        with self.call("lock", target):
            with self.assertRaises(BlockingIOError):
                with self.call("lock", target):
                    self.fail("Concurrent writer acquired the lock")
        with self.call("lock", target):
            self.assertTrue(target.is_file())

    @unittest.skipUnless(os.name == "posix" and shutil.which("ssh-keygen"), "real SSH public-key validation requires POSIX ssh-keygen")
    def test_real_public_key_import_is_idempotent_and_keeps_private_key_out_of_authorized_keys(self):
        key = self.root / "client-key"
        subprocess.run(["ssh-keygen", "-t", "ed25519", "-N", "", "-f", str(key)],
                       check=True, capture_output=True)
        public_key = Path(str(key) + ".pub")
        home = self.root / "server-home"
        home.mkdir()
        with mock.patch.dict(os.environ, {"HOME": str(home)}):
            self.call("ssh_key_install", str(public_key))
            self.call("ssh_key_install", str(public_key))
        installed = home / ".ssh" / "authorized_keys"
        self.assertEqual(installed.read_text(), public_key.read_text())
        self.assertNotIn("PRIVATE KEY", installed.read_text())
        self.assertEqual(stat.S_IMODE(installed.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(installed.parent.stat().st_mode), 0o700)


if __name__ == "__main__":
    unittest.main()

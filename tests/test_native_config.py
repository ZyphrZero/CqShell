"""Optional native configuration parsers; services are never started or reloaded."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from shell_support import load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run native validation in Linux/WSL")


class NativeConfigTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-native-config-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.conf = self.root / "service.conf"

    def run_native(self, binary, function, argument):
        # Service operations are failure sentinels. Reaching them proves only
        # that the real parser accepted the candidate, not service readiness.
        body = load_script() + r'''
require_root() { :; }
sshd() { "$NATIVE" "$@"; }
dockerd() { "$NATIVE" "$@"; }
ss() { return 99; }
systemctl() {
    case "$1" in
        is-active) [[ "$3" == ssh ]]; return $? ;;
        reload|restart|reload-or-restart)
            if [[ ! -e "$TEST_ROOT/accepted" ]]; then cp -- "$CONF" "$TEST_ROOT/accepted"; fi
            return 99 ;;
        *) return 99 ;;
    esac
}
"$FUNCTION" "$ARGUMENT" "$CONF"
'''
        binary_dir = Path(binary).parent
        native_path = os.pathsep.join((str(binary_dir), str(binary_dir.parent / "bin"), os.environ["PATH"]))
        return subprocess.run(
            ["bash", "-c", body], text=True, capture_output=True, timeout=15,
            env=dict(os.environ, PATH=native_path, NATIVE=str(binary), FUNCTION=function, ARGUMENT=argument,
                     CONF=str(self.conf), TEST_ROOT=str(self.root)),
        )

    def ssh_config(self, listen):
        binary = os.environ.get("CQS_TEST_SSHD") or shutil.which("sshd")
        if not binary or not shutil.which("ssh-keygen"):
            self.skipTest("Set CQS_TEST_SSHD or install native sshd/ssh-keygen")
        key = self.root / "host_key"
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key)],
                       check=True, capture_output=True)
        self.conf.write_text(f"HostKey {key}\nUsePAM no\nPort 22\nListenAddress {listen}\n")
        return binary

    def test_native_sshd_accepts_new_global_port(self):
        binary = self.ssh_config("127.0.0.1")
        original = self.conf.read_bytes()
        result = self.run_native(binary, "system_ssh", "2222")
        self.assertNotEqual(result.returncode, 0)  # Explicit reload failure.
        accepted = self.root / "accepted"
        self.assertTrue(accepted.exists(), result.stderr)
        self.assertTrue(accepted.read_text().startswith("Port 2222\n"))
        self.assertEqual(self.conf.read_bytes(), original)

    def test_native_sshd_rejects_explicit_old_listen_port_before_reload(self):
        binary = self.ssh_config("127.0.0.1:22")
        original = self.conf.read_bytes()
        result = self.run_native(binary, "system_ssh", "2222")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ListenAddress", result.stderr)
        self.assertFalse((self.root / "accepted").exists())
        self.assertEqual(self.conf.read_bytes(), original)

    def test_native_dockerd_accepts_both_ipv6_modes(self):
        binary = os.environ.get("CQS_TEST_DOCKERD") or shutil.which("dockerd")
        if not binary:
            self.skipTest("Set CQS_TEST_DOCKERD or install native dockerd with --validate")
        original = '{"data-root":"/srv/docker","log-driver":"journald","ipv6":true,"fixed-cidr-v6":"fd42:1234::/64"}'
        for mode in ("on", "off"):
            with self.subTest(mode=mode):
                self.conf.write_text(original)
                accepted = self.root / "accepted"
                accepted.unlink(missing_ok=True)
                result = self.run_native(binary, "docker_set_ipv6", mode)
                self.assertNotEqual(result.returncode, 0)  # Explicit restart failure.
                self.assertTrue(accepted.exists(), result.stderr)
                data = json.loads(accepted.read_text())
                self.assertEqual(data["ipv6"], mode == "on")
                self.assertEqual(data["data-root"], "/srv/docker")
                self.assertEqual(self.conf.read_text(), original)

"""Real recovery files and a sysctl boundary double; never modify host sysctls."""

import base64
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "cqs.sh"
CC = "net.ipv4.tcp_congestion_control"
QDISC = "net.core.default_qdisc"

SYSCTL = r'''
import json
import os
from pathlib import Path
import signal
import sys

root = Path(os.environ["NETWORK_TEST_ROOT"])
state = root / "runtime.json"
values = json.loads(state.read_text())
args = sys.argv[1:]
with (root / "calls.jsonl").open("a") as output:
    output.write(json.dumps(args) + "\n")

def write(key, value):
    if key == os.environ.get("FAIL_WRITE") or f"{key}={value}" == os.environ.get("FAIL_PAIR"):
        print(f"sysctl: permission denied: {key}", file=sys.stderr)
        return False
    if key != os.environ.get("IGNORE_WRITE"):
        values[key] = value
        state.write_text(json.dumps(values))
    return True

if args[0] == "-n":
    if args[1] == os.environ.get("FAIL_READ"):
        sys.exit(42)
    print(values[args[1]])
elif args[0] == "-w":
    key, value = args[1].split("=", 1)
    sys.exit(0 if write(key, value) else 42)
elif args[0] == "-p":
    failed = False
    for line in Path(args[1]).read_text().splitlines():
        key, value = (part.strip() for part in line.split("=", 1))
        failed |= not write(key, value)
        if os.environ.get("CANCEL_APPLY"):
            os.kill(os.getppid(), signal.SIGTERM)
            break
    sys.exit(42 if failed else 0)
else:
    print(f"unexpected sysctl invocation: {args}", file=sys.stderr)
    sys.exit(99)
'''


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run network transactions on Linux/WSL")


class NetworkTransactionsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-shell-network-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        command = self.bin / "sysctl"
        command.write_text(f"#!{sys.executable}\n" + SYSCTL, encoding="utf-8")
        command.chmod(0o755)
        self.config = self.root / "config/bbr.conf"
        self.config.parent.mkdir()
        self.backup = Path(str(self.config) + ".cqs-backup")
        self.pending = Path(str(self.config) + ".cqs-pending.json")
        self.runtime_file = self.root / "runtime.json"
        self.initial = {QDISC: "fq_codel", CC: "cubic"}
        self.runtime_file.write_text(json.dumps(self.initial))
        self.env = dict(os.environ, HOME=str(self.root), NETWORK_TEST_ROOT=str(self.root),
                        PATH=str(self.bin) + os.pathsep + os.environ["PATH"])
        self.env.pop("BASH_ENV", None)

    def run_network(self, mode="apply", *pairs, config=None, **env):
        # The optional fragment lets an implementation be tested before assembly.
        fragment = os.environ.get("CQS_TEST_NETWORK_FRAGMENT")
        sources = f"source {shlex.quote(str(SCRIPT))}\n"
        if fragment:
            sources += f"source {shlex.quote(str(Path(fragment).resolve()))}\n"
        function = {"apply": "network_apply_config", "restore": "network_restore_config"}[mode]
        if mode == "apply" and not pairs:
            pairs = (f"{QDISC}=fq", f"{CC}=bbr")
        return subprocess.run(
            ["bash", "-c", sources + f'{function} "$@"', "_", str(config or self.config), *pairs],
            stdin=subprocess.DEVNULL, text=True, capture_output=True, timeout=15,
            cwd=self.root, env=dict(self.env, **env),
        )

    def runtime(self):
        return json.loads(self.runtime_file.read_text())

    def calls(self):
        path = self.root / "calls.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def assert_no_mutation(self):
        self.assertEqual(self.runtime(), self.initial)
        self.assertFalse(any(args[0] in ("-w", "-p") for args in self.calls()))

    def test_apply_preserves_first_baseline_and_restores_bytes_and_mode(self):
        original = b"# user config without final newline"
        self.config.write_bytes(original)
        self.config.chmod(0o640)
        self.assert_success(self.run_network())
        baseline = (self.backup / "runtime.conf").read_bytes()
        self.assertEqual(self.runtime(), {QDISC: "fq", CC: "bbr"})
        self.assertEqual(self.config.stat().st_mode & 0o777, 0o644)
        self.assertEqual((self.backup / "runtime.conf").stat().st_mode & 0o777, 0o600)
        self.assertFalse(self.pending.exists())
        self.assert_success(self.run_network("apply", f"{QDISC}=cake", f"{CC}=bbr"))
        self.assertEqual((self.backup / "runtime.conf").read_bytes(), baseline)
        self.assert_success(self.run_network("restore"))
        self.assertEqual(self.runtime(), self.initial)
        self.assertEqual(self.config.read_bytes(), original)
        self.assertEqual(self.config.stat().st_mode & 0o777, 0o640)
        self.assertFalse(self.backup.exists())
        self.assertFalse(self.pending.exists())

    def test_restore_removes_an_originally_absent_configuration(self):
        self.assert_success(self.run_network())
        self.assertTrue((self.backup / "no-original").is_file())
        self.assert_success(self.run_network("restore"))
        self.assertFalse(self.config.exists())
        self.assertEqual(self.runtime(), self.initial)

    def test_rust_format_baseline_is_accepted(self):
        self.backup.mkdir(mode=0o700)
        (self.backup / "runtime.conf").write_text(f"{QDISC} = cake\n{CC} = cubic\n")
        (self.backup / "original.conf").write_bytes(b"original from Rust\n")
        (self.backup / "original.conf").chmod(0o640)
        self.config.write_text("current\n")
        self.assert_success(self.run_network("restore"))
        self.assertEqual(self.runtime(), {QDISC: "cake", CC: "cubic"})
        self.assertEqual(self.config.read_bytes(), b"original from Rust\n")
        self.assertEqual(self.config.stat().st_mode & 0o777, 0o640)

    def test_partial_apply_failure_rolls_back_and_removes_new_baseline(self):
        original = b"# preserve\n"
        self.config.write_bytes(original)
        result = self.run_network(FAIL_WRITE=CC)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("permission denied", result.stderr)
        self.assertEqual(self.runtime(), self.initial)
        self.assertEqual(self.config.read_bytes(), original)
        self.assertFalse(self.backup.exists())
        self.assertFalse(self.pending.exists())

    def test_failed_recovery_keeps_rust_compatible_pending_record(self):
        original = b"# before\n"
        self.config.write_bytes(original)
        result = self.run_network(FAIL_WRITE=CC, FAIL_PAIR=f"{QDISC}=fq_codel")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(str(self.pending), result.stderr)
        pending = json.loads(self.pending.read_text())
        self.assertEqual(pending["runtime"], self.initial)
        self.assertEqual(base64.b64decode(pending["file"]["data"], validate=True), original)
        self.assertTrue(pending["new_baseline"])
        self.assertEqual(self.pending.stat().st_mode & 0o777, 0o600)
        calls = self.calls()
        self.assertNotEqual(self.run_network().returncode, 0)
        self.assertNotEqual(self.run_network("restore").returncode, 0)
        self.assertEqual(self.calls(), calls)

    def test_readback_mismatch_is_a_failure_with_recovery(self):
        result = self.run_network(IGNORE_WRITE=CC)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.runtime(), self.initial)
        self.assertFalse(self.config.exists())
        self.assertFalse(self.pending.exists())

    def test_failed_snapshot_read_performs_no_writes(self):
        result = self.run_network(FAIL_READ=QDISC)
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_mutation()
        self.assertFalse(self.backup.exists())
        self.assertFalse(self.pending.exists())

    def test_restore_failure_retains_current_state_and_baseline_for_retry(self):
        self.assert_success(self.run_network())
        current = self.config.read_bytes()
        result = self.run_network("restore", FAIL_PAIR=f"{CC}=cubic")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.runtime(), {QDISC: "fq", CC: "bbr"})
        self.assertEqual(self.config.read_bytes(), current)
        self.assertTrue((self.backup / "runtime.conf").is_file())
        self.assertFalse(self.pending.exists())
        self.assert_success(self.run_network("restore"))

    def test_cancelled_apply_recovers_before_returning_130(self):
        result = self.run_network(CANCEL_APPLY="1")
        self.assertEqual(result.returncode, 130, result.stdout + result.stderr)
        self.assertEqual(self.runtime(), self.initial)
        self.assertFalse(self.config.exists())
        self.assertFalse(self.pending.exists())
        self.assertFalse(self.backup.exists())

    def test_real_lock_and_pending_file_prevent_runtime_access(self):
        import fcntl

        with Path(str(self.config) + ".cqs-lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertNotEqual(self.run_network().returncode, 0)
        self.assertEqual(self.calls(), [])
        self.pending.write_text("{}")
        self.assertNotEqual(self.run_network().returncode, 0)
        self.assertEqual(self.calls(), [])
        self.assertEqual(self.pending.read_text(), "{}")

    def test_invalid_keys_duplicates_and_multiline_values_are_rejected(self):
        for pairs in (("kernel.hostname=bad",), (f"{CC}=bbr", f"{CC}=cubic"),
                      (f"{CC}=bbr\n{QDISC}=fq",), (f"{CC}=bbr\r",), (f"{CC}=",)):
            with self.subTest(pairs=pairs):
                self.assertNotEqual(self.run_network("apply", *pairs).returncode, 0)
                self.assertEqual(self.calls(), [])
                self.assertFalse(self.config.exists())

    def test_parent_links_and_backup_member_links_are_rejected(self):
        linked = self.root / "linked"
        linked.symlink_to(self.config.parent, target_is_directory=True)
        self.assertNotEqual(self.run_network(config=linked / "bbr.conf").returncode, 0)
        self.assertEqual(self.calls(), [])
        self.assert_success(self.run_network())
        baseline = self.backup / "runtime.conf"
        copied = self.root / "copy.conf"
        copied.write_bytes(baseline.read_bytes())
        baseline.unlink()
        baseline.symlink_to(copied)
        calls = self.calls()
        self.assertNotEqual(self.run_network("restore").returncode, 0)
        self.assertNotEqual(self.run_network().returncode, 0)
        self.assertEqual(self.calls(), calls)

    def test_incomplete_baseline_and_different_key_sets_do_not_change_runtime(self):
        self.assert_success(self.run_network())
        runtime = self.runtime()
        calls = self.calls()
        result = self.run_network("apply", f"{CC}=cubic")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.runtime(), runtime)
        self.assertEqual(self.calls(), calls)
        (self.backup / "runtime.conf").write_text("")
        self.assertNotEqual(self.run_network("restore").returncode, 0)
        self.assertEqual(self.runtime(), runtime)
        self.assertEqual(self.calls(), calls)

    def test_file_publish_failure_restores_current_file_and_runtime(self):
        self.config.write_bytes(b"keep original\n")
        hooks = self.root / "hooks"
        hooks.mkdir()
        (hooks / "sitecustomize.py").write_text('''
import os
from pathlib import Path
replace = os.replace
def fail_once(src, dst):
    marker = Path(os.environ["FAIL_MARKER"])
    if str(dst) == os.environ["FAIL_TARGET"] and not marker.exists():
        marker.touch()
        raise OSError("injected publish failure")
    return replace(src, dst)
os.replace = fail_once
''')
        result = self.run_network(PYTHONPATH=str(hooks), FAIL_TARGET=str(self.config),
                                  FAIL_MARKER=str(self.root / "failed"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("injected publish failure", result.stderr)
        self.assertEqual(self.runtime(), self.initial)
        self.assertEqual(self.config.read_bytes(), b"keep original\n")
        self.assertFalse(self.pending.exists())
        self.assertFalse(self.backup.exists())


if __name__ == "__main__":
    unittest.main()

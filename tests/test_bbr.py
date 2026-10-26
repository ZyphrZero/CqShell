"""BBR control-flow regressions; no host sysctl values or /etc files are changed."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "yuju.sh"
SOURCE = SCRIPT.read_text(encoding="utf-8")
CC = "net.ipv4.tcp_congestion_control"
QDISC = "net.core.default_qdisc"
AVAILABLE = "net.ipv4.tcp_available_congestion_control"


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest(
            "Run these shell tests with Linux Python inside Debian/Ubuntu or WSL"
        )


def functions(*names):
    # Extract only named, top-level functions: sourcing yuju.sh starts the menu
    # and performs installation/authorization work outside any function.
    definitions = []
    for name in names:
        match = re.search(
            rf"^{re.escape(name)}\(\) [{{(]\n.*?^[}})]$", SOURCE, re.M | re.S
        )
        if match is None:
            raise AssertionError(f"Missing function: {name}")
        definitions.append(match.group())
    return "\n".join(definitions)


# This test double models the OS boundary, including partial writes and denied
# writes. Native procps parsing is tested separately below with --dry-run.
SYSCTL_DOUBLE = r'''
sysctl() {
    printf '%s\n' "$*" >> "$TEST_ROOT/calls"
    local key value line result=0
    case "$1" in
        -n)
            [[ "$2" != "${READ_FAIL_KEY:-}" ]] || return 1
            cat "$TEST_ROOT/$2"
            ;;
        -w)
            key=${2%%=*}
            value=${2#*=}
            if [[ "$key" == "${WRITE_FAIL_KEY:-}" || "$2" == "${FAIL_PAIR:-}" ]]; then
                echo "sysctl: permission denied: $key" >&2
                return 1
            fi
            [[ "$key" == "${IGNORE_KEY:-}" ]] && return 0
            printf '%s\n' "$value" > "$TEST_ROOT/$key"
            ;;
        -p)
            # Only the generated two-assignment BBR file goes through this double.
            while IFS= read -r line || [[ -n "$line" ]]; do
                [[ "$line" == net.* ]] || continue
                read -r key _ value <<< "$line"
                sysctl -w "$key=$value" || result=1
            done < "$2"
            return "$result"
            ;;
        *) echo "Unexpected sysctl arguments: $*" >&2; return 2 ;;
    esac
}
'''


class BbrTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="yuju-bbr-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.conf = self.root / "99-bbr.conf"
        self.write(CC, "cubic")
        self.write(QDISC, "fq_codel")
        self.write(AVAILABLE, "reno cubic bbr")

    def write(self, key, value):
        (self.root / key).write_text(value + "\n", encoding="utf-8")

    def read(self, key):
        return (self.root / key).read_text(encoding="utf-8").strip()

    def run_shell(self, body, **settings):
        env = dict(os.environ, TEST_ROOT=str(self.root), CONF=str(self.conf))
        env.update(settings)
        definitions = functions(
            "check_bbr_status", "check_bbr_available", "load_bbr_module",
            "sysctl_apply_file", "restore_bbr_runtime", "apply_bbr_config",
            "require_root", "enable_bbr",
        )
        return subprocess.run(
            ["bash"], input="red= white= yellow= green= cyan=\n"
            + definitions + "\n" + SYSCTL_DOUBLE + "\n" + body,
            text=True, capture_output=True, env=env, cwd=self.root,
            encoding="utf-8", timeout=10,
        )

    def apply(self, **settings):
        return self.run_shell('apply_bbr_config "$CONF"', **settings)

    def assert_restored(self):
        self.assertEqual(self.read(CC), "cubic")
        self.assertEqual(self.read(QDISC), "fq_codel")
        self.assertEqual(list(self.root.glob("99-bbr.conf.*")), [])

    def test_full_script_parses_in_linux_bash(self):
        result = subprocess.run(["bash", "-n", str(SCRIPT)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_algorithm_detection_uses_exact_whitespace_delimited_name(self):
        for available, expected in [
            ("reno cubic bbr", 0), ("reno\tbbr\tcubic", 0),
            ("reno bbr2", 1), ("bbrplus cubic", 1), ("", 1),
        ]:
            with self.subTest(available=available):
                self.write(AVAILABLE, available)
                self.assertEqual(self.run_shell("check_bbr_available").returncode, expected)

    def test_builtin_or_loaded_bbr_does_not_call_modprobe(self):
        result = self.run_shell('modprobe() { echo unexpected >&2; return 99; }; load_bbr_module')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("unexpected", result.stderr)

    def test_module_loading_rechecks_available_algorithms(self):
        self.write(AVAILABLE, "reno cubic")
        result = self.run_shell('''
modprobe() {
    [[ "$1" == tcp_bbr ]] || return 2
    printf 'reno cubic bbr\n' > "$TEST_ROOT/net.ipv4.tcp_available_congestion_control"
}
load_bbr_module
''')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_module_command_success_without_bbr_is_failure(self):
        self.write(AVAILABLE, "reno cubic")
        result = self.run_shell('modprobe() { return 0; }; load_bbr_module')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("仍未检测到", result.stderr)

    def test_module_load_error_is_visible(self):
        self.write(AVAILABLE, "reno cubic")
        result = self.run_shell('modprobe() { echo "module denied" >&2; return 1; }; load_bbr_module')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("module denied", result.stderr)

    def test_success_persists_only_bbr_and_fq(self):
        result = self.apply()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read(CC), "bbr")
        self.assertEqual(self.read(QDISC), "fq")
        assignments = [line for line in self.conf.read_text().splitlines() if line.startswith("net.")]
        self.assertEqual(assignments, [f"{QDISC} = fq", f"{CC} = bbr"])
        self.assertEqual(list(self.root.glob("99-bbr.conf.*")), [])

    def test_already_enabled_bbr_still_creates_persistent_config(self):
        self.write(CC, "bbr")
        result = self.apply()
        self.assertEqual(result.returncode, 0, result.stderr)
        first = self.conf.read_bytes()
        self.assertEqual(self.apply().returncode, 0)
        self.assertEqual(self.conf.read_bytes(), first)
        self.assertEqual(self.read(QDISC), "fq")

    def test_partial_apply_failure_restores_runtime_and_preserves_file(self):
        self.conf.write_text("# existing user config\n")
        for key in [CC, QDISC]:
            with self.subTest(key=key):
                result = self.apply(WRITE_FAIL_KEY=key)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("permission denied", result.stderr)
                self.assertEqual(self.conf.read_text(), "# existing user config\n")
                self.assert_restored()

    def test_failure_does_not_create_persistent_config(self):
        self.assertNotEqual(self.apply(WRITE_FAIL_KEY=CC).returncode, 0)
        self.assertFalse(self.conf.exists())
        self.assert_restored()

    def test_snapshot_read_failure_performs_no_writes(self):
        self.assertNotEqual(self.apply(READ_FAIL_KEY=QDISC).returncode, 0)
        self.assertNotIn("-w ", self.read("calls"))
        self.assertFalse(self.conf.exists())
        self.assert_restored()

    def test_readback_mismatch_is_failure_and_rolls_back(self):
        self.assertNotEqual(self.apply(IGNORE_KEY=CC).returncode, 0)
        self.assertFalse(self.conf.exists())
        self.assert_restored()

    def test_persistence_failure_rolls_back_runtime(self):
        self.conf.mkdir()
        result = self.apply()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("保存失败", result.stderr)
        self.assertTrue(self.conf.is_dir())
        self.assert_restored()

    def test_rollback_failure_reports_previous_values(self):
        result = self.apply(WRITE_FAIL_KEY=CC, FAIL_PAIR=f"{QDISC}=fq_codel")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("恢复原参数失败", result.stderr)
        self.assertIn("fq_codel", result.stderr)
        self.assertEqual(self.read(QDISC), "fq")
        self.assertFalse(self.conf.exists())

    def test_enable_stops_before_system_changes_when_root_check_fails(self):
        result = self.run_shell('clear() { :; }; require_root() { return 1; }; enable_bbr')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "calls").exists())

    def test_sysctl_exit_code_is_preserved(self):
        self.conf.write_text(f"{CC} = bbr\n")
        result = self.run_shell('sysctl() { return 42; }; sysctl_apply_file "$CONF"')
        self.assertEqual(result.returncode, 42)

    def test_missing_config_returns_failure(self):
        result = self.run_shell('sysctl_apply_file "$CONF"')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "calls").exists())


class NativeSysctlTests(unittest.TestCase):
    def setUp(self):
        self.sysctl = shutil.which("sysctl")
        if not self.sysctl and Path("/usr/sbin/sysctl").is_file():
            self.sysctl = "/usr/sbin/sysctl"
        if not self.sysctl:
            self.skipTest("Native procps sysctl is unavailable")
        help_text = subprocess.run([self.sysctl, "--help"], capture_output=True, text=True).stdout
        if "--dry-run" not in help_text:
            self.skipTest("Native sysctl lacks --dry-run; refusing to change host values")

    def run_native(self, content):
        with tempfile.TemporaryDirectory(prefix="yuju-procps-") as directory:
            conf = Path(directory) / "tcp.conf"
            conf.write_bytes(content)
            return subprocess.run(
                ["bash"], text=True, capture_output=True,
                env=dict(os.environ, NATIVE_SYSCTL=self.sysctl, CONF=str(conf)),
                input="red= white=\n" + functions("sysctl_apply_file") + '''
sysctl() { "$NATIVE_SYSCTL" --dry-run "$@"; }
sysctl_apply_file "$CONF"
''', timeout=10,
            )

    def test_native_parser_preserves_multivalue_whitespace_and_last_line(self):
        result = self.run_native(
            b"# comment\r\n\r\nnet.ipv4.tcp_rmem = 4096 87380 33554432\r\n"
            b"net.ipv4.ip_local_port_range = 1024 65535"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("4096 87380 33554432", result.stdout)
        self.assertIn("1024 65535", result.stdout)

    def test_native_unknown_key_returns_failure(self):
        result = self.run_native(b"net.ipv4.yuju_nonexistent_test_key = 1\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("yuju_nonexistent_test_key", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)

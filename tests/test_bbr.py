"""BBR control-flow regressions; no host sysctl values or /etc files are changed."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


from shell_support import SCRIPT, load_script
CC = "net.ipv4.tcp_congestion_control"
QDISC = "net.core.default_qdisc"
AVAILABLE = "net.ipv4.tcp_available_congestion_control"


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest(
            "Run these shell tests with Linux Python inside Debian/Ubuntu or WSL"
        )


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
            # Only generated key = value network configs go through this double.
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
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-bbr-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.conf = self.root / "99-cqs-bbr.conf"
        self.tcp_conf = self.root / "99-cqs-tcp.conf"
        self.write(CC, "cubic")
        self.write(QDISC, "fq_codel")
        self.write(AVAILABLE, "reno cubic bbr")
        for key, value in {
            "net.core.rmem_max": "212992", "net.core.wmem_max": "212992",
            "net.ipv4.tcp_rmem": "4096 131072 6291456",
            "net.ipv4.tcp_wmem": "4096 16384 4194304",
            "net.ipv4.tcp_moderate_rcvbuf": "1", "net.ipv4.tcp_mtu_probing": "0",
            "net.ipv4.tcp_limit_output_bytes": "1048576",
            "net.ipv4.tcp_slow_start_after_idle": "1",
        }.items():
            self.write(key, value)

    def write(self, key, value):
        (self.root / key).write_text(value + "\n", encoding="utf-8")

    def read(self, key):
        return (self.root / key).read_text(encoding="utf-8").strip()

    def run_shell(self, body, **settings):
        env = dict(os.environ, TEST_ROOT=str(self.root), CONF=str(self.conf),
                   CQS_BBR_CONFIG=str(self.conf), CQS_TCP_CONFIG=str(self.tcp_conf))
        env.update(settings)
        definitions = load_script()
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
        self.assertEqual(list(self.root.glob("99-cqs-bbr.conf.work.*")), [])

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
        self.assertEqual(list(self.root.glob("99-cqs-bbr.conf.work.*")), [])
        self.assertTrue((self.root / "99-cqs-bbr.conf.cqs-backup/runtime.conf").is_file())

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
        self.conf.write_text("# existing config\n")
        result = self.run_shell('''
mv() {
    if [[ "$*" == *desired.conf* ]]; then return 1; fi
    command mv "$@"
}
apply_bbr_config "$CONF"
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("保存失败", result.stderr)
        self.assertEqual(self.conf.read_text(), "# existing config\n")
        self.assert_restored()

    def test_rollback_failure_reports_previous_values(self):
        result = self.apply(WRITE_FAIL_KEY=CC, FAIL_PAIR=f"{QDISC}=fq_codel")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("恢复原参数失败", result.stderr)
        snapshot = self.root / "99-cqs-bbr.conf.cqs-backup/runtime.conf"
        self.assertIn("fq_codel", snapshot.read_text())
        self.assertEqual(self.read(QDISC), "fq")
        self.assertFalse(self.conf.exists())

    def test_enable_stops_before_system_changes_when_root_check_fails(self):
        result = self.run_shell('clear() { :; }; require_root() { return 1; }; enable_bbr')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "calls").exists())

    def test_bbr_entrypoints_apply_and_restore_the_configured_path(self):
        self.conf = self.root / "network settings" / "bbr.conf"
        boundary = "clear() { :; }; require_root() { :; };\n"
        result = self.run_shell(boundary + "enable_bbr fq")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.conf.is_file())
        self.assertIn(str(self.conf), result.stdout)
        self.assertEqual(self.read(CC), "bbr")
        self.assertEqual(self.read(QDISC), "fq")
        result = self.run_shell(boundary + "restore_bbr <<< y")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.conf.exists())
        self.assertFalse(Path(str(self.conf) + ".cqs-backup").exists())
        self.assert_restored()

    def test_tcp_entrypoints_apply_and_restore_the_configured_path(self):
        self.tcp_conf = self.root / "network settings" / "tcp.conf"
        boundary = "clear() { :; }; require_root() { :; }; network_memory_kb() { echo 1048576; };\n"
        result = self.run_shell(boundary + "tcp_tune --auto")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(self.tcp_conf.is_file())
        self.assertIn(str(self.tcp_conf), result.stdout)
        self.assertFalse(self.conf.exists())
        self.assertEqual(self.read("net.ipv4.tcp_rmem"), "4096 131072 12582912")
        result = self.run_shell(boundary + "tcp_tune <<< $'3\\ny\\n'")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.tcp_conf.exists())
        self.assertFalse(Path(str(self.tcp_conf) + ".cqs-backup").exists())
        self.assertEqual(self.read("net.ipv4.tcp_rmem"), "4096 131072 6291456")

    def test_sysctl_exit_code_is_preserved(self):
        self.conf.write_text(f"{CC} = bbr\n")
        result = self.run_shell('sysctl() { return 42; }; sysctl_apply_file "$CONF"')
        self.assertEqual(result.returncode, 42)

    def test_missing_config_returns_failure(self):
        result = self.run_shell('sysctl_apply_file "$CONF"')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "calls").exists())

    def test_all_queue_choices_are_supported_and_other_names_rejected(self):
        for qdisc in ["fq", "fq_codel", "fq_pie", "cake"]:
            with self.subTest(qdisc=qdisc):
                result = self.run_shell(f'apply_bbr_config "$CONF" {qdisc}')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.read(QDISC), qdisc)
        self.assertNotEqual(self.run_shell('apply_bbr_config "$CONF" invalid').returncode, 0)
        self.assertEqual(self.read(QDISC), "cake")

    def test_undo_preserves_first_runtime_snapshot_across_repeated_changes(self):
        self.conf.write_text("# original configuration\n")
        self.assertEqual(self.apply().returncode, 0)
        self.assertEqual(self.run_shell('apply_bbr_config "$CONF" cake').returncode, 0)
        result = self.run_shell('network_restore_config "$CONF"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assert_restored()
        self.assertEqual(self.conf.read_text(), "# original configuration\n")
        self.assertFalse((self.root / "99-cqs-bbr.conf.cqs-backup").exists())

    def test_undo_removes_new_config_and_refuses_without_snapshot(self):
        self.assertNotEqual(self.run_shell('network_restore_config "$CONF"').returncode, 0)
        self.assertEqual(self.apply().returncode, 0)
        self.assertEqual(self.run_shell('network_restore_config "$CONF"').returncode, 0)
        self.assertFalse(self.conf.exists())
        self.assert_restored()

    def test_failed_undo_retains_snapshot_for_retry(self):
        self.assertEqual(self.apply().returncode, 0)
        result = self.run_shell('network_restore_config "$CONF"', FAIL_PAIR=f"{CC}=cubic")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.read(CC), "bbr")
        self.assertEqual(self.read(QDISC), "fq")
        self.assertTrue((self.root / "99-cqs-bbr.conf.cqs-backup/runtime.conf").exists())

    def test_running_version_is_not_guessed_from_kernel_name(self):
        result = self.run_shell('get_bbr_version "$TEST_ROOT/missing"')
        self.assertIn("未知", result.stdout)
        self.write("version", "3")
        self.assertEqual(self.run_shell('get_bbr_version "$TEST_ROOT/version"').stdout.strip(), "v3")
        self.write("version", "not-a-version")
        self.assertIn("未知", self.run_shell('get_bbr_version "$TEST_ROOT/version"').stdout)

    def test_bandwidth_and_rtt_both_affect_buffer_with_memory_cap(self):
        for bw, rtt, memory, expected_mib in [
            (100, 10, 1048576, 1), (1000, 10, 1048576, 3),
            (1000, 80, 1048576, 20), (1000, 200, 1048576, 32),
            (10000, 200, 8388608, 64), (1000, 80, 262144, 8),
        ]:
            with self.subTest(bw=bw, rtt=rtt, memory=memory):
                result = self.run_shell(f"calculate_tcp_buffer {bw} {rtt} {memory}")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(int(result.stdout), expected_mib * 1048576)

    def test_invalid_buffer_inputs_are_rejected(self):
        for values in ["0 50 1048576", "1000 -1 1048576", "1e9 10 1048576",
                       "100001 10 1048576", "1000 10001 1048576", "1000 50 32768"]:
            with self.subTest(values=values):
                self.assertNotEqual(self.run_shell(f"calculate_tcp_buffer {values}").returncode, 0)

    def test_tcp_profile_is_transactional_and_does_not_modify_global_memory(self):
        result = self.run_shell('apply_tcp_profile "$CONF" 8388608')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read("net.ipv4.tcp_rmem"), "4096 131072 8388608")
        self.assertNotIn("tcp_mem", self.conf.read_text())
        self.assertNotIn("nf_conntrack", self.conf.read_text())
        result = self.run_shell('network_restore_config "$CONF"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.read("net.ipv4.tcp_rmem"), "4096 131072 6291456")

    def test_tcp_failure_restores_all_applied_values(self):
        result = self.run_shell('apply_tcp_profile "$CONF" 8388608',
                                WRITE_FAIL_KEY="net.ipv4.tcp_mtu_probing")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.read("net.core.rmem_max"), "212992")
        self.assertEqual(self.read("net.ipv4.tcp_rmem"), "4096 131072 6291456")
        self.assertFalse(self.conf.exists())

    def test_concurrent_update_is_rejected_before_runtime_writes(self):
        import fcntl
        with (self.root / "99-cqs-bbr.conf.cqs-lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.apply()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("另一个进程", result.stderr)
        self.assertFalse((self.root / "calls").exists())

    def test_new_parameter_set_cannot_overwrite_an_existing_recovery_baseline(self):
        self.assertEqual(self.apply().returncode, 0)
        original = self.conf.read_bytes()
        result = self.run_shell('apply_tcp_profile "$CONF" 8388608')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("参数集合不同", result.stderr)
        self.assertEqual(self.conf.read_bytes(), original)
        self.assertEqual(self.read("net.core.rmem_max"), "212992")

    def test_empty_recovery_snapshot_is_rejected(self):
        self.assertEqual(self.apply().returncode, 0)
        (self.root / "99-cqs-bbr.conf.cqs-backup/runtime.conf").write_text("")
        self.assertNotEqual(self.run_shell('network_restore_config "$CONF"').returncode, 0)
        self.assertEqual(self.read(CC), "bbr")

    def test_failed_undo_file_restore_rolls_back_current_runtime(self):
        self.conf.write_text("# original configuration\n")
        self.assertEqual(self.apply().returncode, 0)
        active = self.conf.read_bytes()
        result = self.run_shell('''
mv() { return 1; }
network_restore_config "$CONF"
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.read(CC), "bbr")
        self.assertEqual(self.read(QDISC), "fq")
        self.assertEqual(self.conf.read_bytes(), active)
        self.assertTrue((self.root / "99-cqs-bbr.conf.cqs-backup/runtime.conf").is_file())

    def test_invalid_network_key_and_multiline_value_are_rejected(self):
        for argument in ["=bbr", "kernel.hostname=bad", "net.core.rmem_max=1\nnet.core.wmem_max=1"]:
            with self.subTest(argument=argument):
                result = self.run_shell('network_apply_config "$CONF" "$ARGUMENT"', ARGUMENT=argument)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.conf.exists())


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
        with tempfile.TemporaryDirectory(prefix="cqs-procps-") as directory:
            conf = Path(directory) / "tcp.conf"
            conf.write_bytes(content)
            return subprocess.run(
                ["bash"], text=True, capture_output=True,
                env=dict(os.environ, NATIVE_SYSCTL=self.sysctl, CONF=str(conf)),
                input="red= white=\n" + load_script() + '''
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
        result = self.run_native(b"net.ipv4.cqs_nonexistent_test_key = 1\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("cqs_nonexistent_test_key", result.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)

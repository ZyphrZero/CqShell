"""Exercise the real selection/dispatch flow with isolated system-action boundaries."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from shell_support import load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run CqShell shell tests in Linux/WSL")


class OnekeyOptimizationTests(unittest.TestCase):
    ACTIONS = (
        "system_update", "system_clean", "tcp_tune --auto", "enable_bbr fq",
        "system_time", "swap_create_auto", "system_fail2ban", "download_all",
        "docker_install", "system_ssh 55520", "system_keygen",
    )

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-onekey-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.actions = self.root / "actions"
        key = self.root / ".ssh/id_rsa"
        key.parent.mkdir()
        key.write_text("PRIVATE_KEY_FIXTURE_DO_NOT_DISPLAY\n")

    def run_optimization(self, stdin, fail_action="", root_status=0):
        self.actions.unlink(missing_ok=True)
        boundaries = "\n".join(
            name + '() { record ' + name + ' "$@"; }'
            for name in (action.split()[0] for action in self.ACTIONS)
        )
        body = load_script() + r'''
clear() { :; }
require_root() { return "$ROOT_STATUS"; }
white= green= red= yellow= pink= cyan=
record() {
    printf '%s\n' "$*" >> "$TEST_ROOT/actions"
    [[ "$1" != "$FAIL_ACTION" ]]
}
''' + boundaries + "\nonekey_optimization\n"
        result = subprocess.run(
            ["bash", "-c", body], input=stdin, text=True, capture_output=True, timeout=10,
            cwd=self.root,
            env=dict(os.environ, HOME=str(self.root), TEST_ROOT=str(self.root),
                     FAIL_ACTION=fail_action, ROOT_STATUS=str(root_status)),
        )
        calls = self.actions.read_text().splitlines() if self.actions.exists() else []
        self.assertNotIn("PRIVATE_KEY_FIXTURE_DO_NOT_DISPLAY", result.stdout)
        self.assertNotIn("您现在的SSH端口为55520", result.stdout)
        return result, calls

    def test_each_item_executes_independently_with_fixed_arguments(self):
        for number, action in enumerate(self.ACTIONS, 1):
            with self.subTest(number=number):
                result, calls = self.run_optimization(f"{number}\ns\ny\n")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, [action])
                self.assertIn("[OK] 1/1.", result.stdout)
                self.assertIn("已完成所选 1 项优化", result.stdout)

    def test_defaults_to_empty_and_rejects_execution_without_selection(self):
        result, calls = self.run_optimization("s\n0\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, [])
        self.assertIn("已选 0/11 项", result.stdout)
        self.assertIn("请至少选择一项", result.stdout)
        self.assertNotIn("[x]", result.stdout)

    def test_batch_selection_toggle_and_execution_order(self):
        result, calls = self.run_optimization("4,1，3\n3\ns\ny\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["system_update", "enable_bbr fq"])
        self.assertIn("即将按顺序执行以下 2 项", result.stdout)
        self.assertIn("[OK] 2/2.", result.stdout)
        self.assertNotIn("涉及 SSH 登录设置", result.stdout)

    def test_repeated_number_in_one_input_toggles_only_once(self):
        result, calls = self.run_optimization("3 3 3 3\ns\ny\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["tcp_tune --auto"])

    def test_select_all_runs_each_action_once(self):
        result, calls = self.run_optimization("A\nS\nY\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, list(self.ACTIONS))
        self.assertIn("[OK] 11/11.", result.stdout)
        self.assertIn("涉及 SSH 登录设置", result.stdout)

    def test_clear_selection_then_select_again(self):
        result, calls = self.run_optimization("a\nn\ns\n5\ns\ny\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(calls, ["system_time"])
        self.assertIn("请至少选择一项", result.stdout)

    def test_return_and_eof_do_not_execute_pending_selections(self):
        for stdin in ("", "0\n", "1\n", "1\n0\n", "1\ns\n", "1\ns\nn\n0\n"):
            with self.subTest(stdin=stdin):
                result, calls = self.run_optimization(stdin)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, [])

    def test_confirmation_defaults_to_cancel_and_allows_reselection(self):
        for confirm in ("", "n", "yes", "invalid"):
            with self.subTest(confirm=confirm):
                result, calls = self.run_optimization(f"1\ns\n{confirm}\n1 5\ns\ny\n")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, ["system_time"])
                self.assertIn("已取消执行，可继续调整选择", result.stdout)

    def test_invalid_batch_leaves_existing_selection_unchanged(self):
        for invalid in ("", " ", "12", "01", "1 99", "1 a", "1 0", "-1",
                        "1; touch UNEXPECTED", "$(touch UNEXPECTED)", "1[0]"):
            with self.subTest(invalid=invalid):
                result, calls = self.run_optimization(f"5\n{invalid}\ns\ny\n")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(calls, ["system_time"])
                self.assertIn("无效输入", result.stdout)
                self.assertFalse((self.root / "UNEXPECTED").exists())

    def test_network_failure_continues_but_returns_failure(self):
        for failed in ("tcp_tune", "enable_bbr"):
            with self.subTest(failed=failed):
                result, calls = self.run_optimization("3 4 5\ns\ny\n", fail_action=failed)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, ["tcp_tune --auto", "enable_bbr fq", "system_time"])
                self.assertIn("成功 2 项，失败 1 项", result.stdout)
                self.assertNotIn("已完成所选", result.stdout)

    def test_other_failures_stop_remaining_actions(self):
        for number in (1, 2, 5, 6, 7, 8, 9, 10):
            with self.subTest(number=number):
                failed = self.ACTIONS[number - 1]
                result, calls = self.run_optimization(
                    f"{number} 11\ns\ny\n", fail_action=failed.split()[0],
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [failed])
                self.assertIn("成功 0 项，失败 1 项，未执行 1 项", result.stdout)
                self.assertNotIn("已完成所选", result.stdout)

    def test_last_action_failure_is_not_reported_as_success(self):
        result, calls = self.run_optimization("11\ns\ny\n", fail_action="system_keygen")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, ["system_keygen"])
        self.assertIn("[FAIL] 1/1.", result.stdout)
        self.assertNotIn("已完成所选", result.stdout)

    def test_permission_failure_does_not_execute_anything(self):
        result, calls = self.run_optimization("a\ns\ny\n", root_status=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(calls, [])

"""Entrypoint, shortcut and menu behavior without host installation or services."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from shell_support import SCRIPT, load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run CqShell shell tests in Linux/WSL")


class AppStructureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-app-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.shortcut = self.root / "cqs"
        self.state = self.root / "state"
        self.env = dict(os.environ, TEST_ROOT=str(self.root), CQS_INSTALL_PATH=str(self.shortcut),
                        CQS_STATE_DIR=str(self.state))

    def run_shell(self, body, stdin=""):
        return subprocess.run(
            ["bash", "-c", load_script() + '\nclear() { :; }; break_end() { :; }\n' + body],
            input=stdin, text=True, capture_output=True, timeout=10, env=self.env, cwd=self.root,
        )

    def test_source_only_loads_definitions(self):
        guard = 'cp() { touch "$TEST_ROOT/UNEXPECTED"; return 99; }; apt() { return 99; };\n'
        result = subprocess.run(["bash", "-c", guard + load_script() + 'printf "loaded\\n"'],
                                input="", text=True, capture_output=True, timeout=10,
                                env=self.env, cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "loaded\n")
        self.assertEqual(list(self.root.iterdir()), [])

    def test_help_and_version_do_not_install_or_prompt(self):
        for flag in ("--help", "--version"):
            with self.subTest(flag=flag):
                result = subprocess.run(["bash", str(SCRIPT), flag], input="", text=True,
                                        capture_output=True, timeout=5, env=self.env, cwd=self.root)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("CqShell", result.stdout)
                self.assertFalse(self.shortcut.exists())
                self.assertFalse(self.state.exists())

    def test_unknown_cli_argument_returns_failure(self):
        result = subprocess.run(["bash", str(SCRIPT), "--unknown"], capture_output=True,
                                text=True, env=self.env, timeout=5)
        self.assertEqual(result.returncode, 2)

    def test_shortcut_installs_the_source_not_a_cwd_file(self):
        decoy = self.root / "cqs.sh"
        decoy.write_text("not the trusted source\n")
        result = self.run_shell("require_root() { :; }; install_shortcut")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.shortcut.read_bytes(), SCRIPT.read_bytes())
        self.assertEqual(self.shortcut.stat().st_mode & 0o777, 0o755)
        original_mtime = self.shortcut.stat().st_mtime_ns
        self.assertEqual(self.run_shell("require_root() { :; }; install_shortcut").returncode, 0)
        self.assertEqual(self.shortcut.stat().st_mtime_ns, original_mtime)
        result = subprocess.run([str(self.shortcut), "--version"], capture_output=True, text=True,
                                env=self.env, timeout=5, cwd=self.root)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(result.stdout.startswith("CqShell "))

    def test_shortcut_refuses_a_symlink_target(self):
        target = self.root / "target"
        target.write_text("preserve\n")
        self.shortcut.symlink_to(target)
        result = self.run_shell("require_root() { :; }; install_shortcut")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(target.read_text(), "preserve\n")

    def test_shortcut_repairs_execute_mode_and_preserves_old_copy_on_failure(self):
        self.shortcut.write_bytes(SCRIPT.read_bytes())
        self.shortcut.chmod(0o644)
        result = self.run_shell("require_root() { :; }; install_shortcut")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.shortcut.stat().st_mode & 0o777, 0o755)
        self.shortcut.write_text("previous version\n")
        result = self.run_shell("require_root() { :; }; install() { return 23; }; install_shortcut")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.shortcut.read_text(), "previous version\n")
        self.assertEqual(list(self.root.glob("cqs.*")), [])

    def test_uninstall_keeps_download_and_recovery_material(self):
        self.shortcut.write_text("installed\n")
        download = self.root / "cqs.sh"
        download.write_text("download\n")
        self.state.mkdir()
        result = self.run_shell("require_root() { :; }; uninstall_shortcut")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.shortcut.exists())
        self.assertTrue(download.exists())
        self.assertTrue(self.state.is_dir())

    def test_agreement_uses_state_instead_of_rewriting_source(self):
        original = SCRIPT.read_bytes()
        result = self.run_shell('apt() { touch "$TEST_ROOT/UNEXPECTED"; return 99; }; user_agreement', "y\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.state / "agreement.accepted").read_text(), "accepted\n")
        self.assertEqual(SCRIPT.read_bytes(), original)
        self.assertFalse((self.root / "UNEXPECTED").exists())
        self.assertEqual(self.run_shell("user_agreement").returncode, 0)

    def test_declined_agreement_stops_startup(self):
        result = self.run_shell('cqs_menu() { touch "$TEST_ROOT/MENU"; }; cqs_main', "n\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.shortcut.exists())
        self.assertFalse(self.state.exists())
        self.assertFalse((self.root / "MENU").exists())

    def test_repeated_submenu_returns_do_not_grow_the_call_stack(self):
        result = self.run_shell(r'''
show_sysinfo() { printf '%s\n' "${#FUNCNAME[@]}" >> "$TEST_ROOT/depth"; }
cqs_menu
''', "1\n0\n" * 40 + "0\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        depths = (self.root / "depth").read_text().splitlines()
        self.assertEqual(len(depths), 41)
        self.assertEqual(len(set(depths)), 1)

    def test_submenus_exit_on_zero_or_eof(self):
        for menu in ("system_related", "test_script", "useful_tools", "docker_manage",
                     "bbr_manage", "bbrv3_kernel_menu", "system_swap", "terminal_menu"):
            for stdin in ("0\n", ""):
                with self.subTest(menu=menu, stdin=stdin):
                    result = self.run_shell('require_root() { :; }; ' + menu, stdin)
                    self.assertEqual(result.returncode, 0, result.stderr)

    def test_menu_dispatch_preserves_arguments_and_rejects_shell_input(self):
        result = self.run_shell(r'''
record() { printf '%s\n' "$*" >> "$TEST_ROOT/actions"; }
menu_loop Demo '1|Install|record curl' '2|Queue|record cake'
''', '1; touch UNEXPECTED\n2\n1\n0\n')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "actions").read_text().splitlines(), ["cake", "curl"])
        self.assertFalse((self.root / "UNEXPECTED").exists())

    def test_menu_reports_failed_actions(self):
        result = self.run_shell("fail() { return 42; }; menu_loop Demo '1|Fail|fail'", "1\n0\n")
        self.assertEqual(result.returncode, 0)
        self.assertIn("操作未完成", result.stderr)

    def test_all_declared_menu_actions_resolve_to_functions(self):
        result = self.run_shell(r'''
require_root() { :; }
menu_loop() {
    local item key label action
    local -a args
    shift
    for item in "$@"; do
        IFS='|' read -r key label action <<< "$item"
        [[ -n "$key" && -n "$label" && -n "$action" ]] || return 1
        read -ra args <<< "$action"
        declare -F "${args[0]}" >/dev/null || return 1
    done
}
for menu in system_related test_script useful_tools docker_manage bbr_manage bbrv3_kernel_menu system_swap terminal_menu; do
    "$menu" || exit 1
done
''')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_cancelling_docker_mirror_input_does_not_reenter_menu_or_write(self):
        config = self.root / "daemon.json"
        original = '{"data-root":"/srv/docker"}\n'
        config.write_text(original)
        body = r'''
docker_manage() { touch "$TEST_ROOT/REENTERED"; }
sudo() { touch "$TEST_ROOT/WRITE_ATTEMPTED"; return 99; }
docker_mirrors "$TEST_ROOT/daemon.json"
'''
        result = self.run_shell(body, "q\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(config.read_text(), original)
        self.assertFalse((self.root / "REENTERED").exists())
        self.assertFalse((self.root / "WRITE_ATTEMPTED").exists())

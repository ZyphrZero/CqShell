"""Terminal configuration uses real files; only installation and failing I/O are isolated."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

try:
    import tomllib
except ImportError:
    tomllib = None

from shell_support import SCRIPT, load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run terminal tests in Linux/WSL")


class TerminalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-terminal-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home with 'quote"
        self.home.mkdir()
        self.state = self.root / "state"
        self.config = self.home / ".config/cqs/terminal.toml"
        self.bashrc = self.home / ".bashrc"
        self.zshrc = self.home / ".zshrc"
        self.baseline = self.state / "terminal/original.json"
        self.options = self.state / "terminal/options.json"
        self.env = dict(os.environ, HOME=str(self.home), ZDOTDIR=str(self.home),
                        XDG_CONFIG_HOME=str(self.home / ".config"),
                        XDG_STATE_HOME=str(self.state), CQS_LANG="en-US", CQS_YES="0",
                        CQS_KERNEL_LOCK=str(self.root / "package.lock"),
                        CQS_STATE_DIR=str(self.state), CQS_TERMINAL_CONFIG=str(self.config))
        self.env.pop("BASH_ENV", None)

    def run_shell(self, body, stdin="", **env):
        return subprocess.run(["bash", "-c", load_script() + "\n" + body],
                              input=stdin, text=True, capture_output=True, timeout=15,
                              env=dict(self.env, **env), cwd=self.root)

    def apply(self, args="both full cyan off on", **env):
        # Exercise file transactions without claiming an installed prompt engine.
        return self.run_shell("terminal_config apply " + args, **env)

    def cli(self, *args, confirmation=None):
        command = ["bash", str(SCRIPT), *args]
        if confirmation is None:
            return subprocess.run(command, stdin=subprocess.DEVNULL, text=True,
                                  capture_output=True, timeout=15, env=self.env, cwd=self.root)
        import pty

        master, slave = pty.openpty()
        try:
            child = subprocess.Popen(command, stdin=slave, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, text=True, env=self.env, cwd=self.root)
            try:
                os.write(master, confirmation)
                stdout, stderr = child.communicate(timeout=15)
            except BaseException:
                child.kill()
                child.wait()
                raise
            return subprocess.CompletedProcess(command, child.returncode, stdout, stderr)
        finally:
            os.close(master)
            os.close(slave)

    def test_repeated_apply_and_restore_preserve_first_baseline_and_modes(self):
        original_bash = b"# existing\nexport KEEP=1"  # No final newline.
        original_zsh = b"# existing zsh\r\neval \"$(starship init zsh)\"\r\n" * 2
        self.bashrc.write_bytes(original_bash)
        self.bashrc.chmod(0o640)
        self.zshrc.write_bytes(original_zsh)
        self.config.parent.mkdir(parents=True)
        self.config.write_text("# original custom file\n")
        unmanaged = self.home / ".config/starship.toml"
        unmanaged.write_text("# other Starship configuration\n")
        result = self.apply()
        self.assertEqual(result.returncode, 0, result.stderr)
        baseline = self.baseline.read_bytes()
        self.assertEqual(self.baseline.stat().st_mode & 0o777, 0o600)
        for args in ("both full cyan off on", "both compact green on off"):
            result = self.apply(args)
            self.assertEqual(result.returncode, 0, result.stderr)
            for rc, shell in ((self.bashrc, "bash"), (self.zshrc, "zsh")):
                self.assertEqual(rc.read_text().count(f"starship init {shell}"), 1)
            self.assertEqual(self.baseline.read_bytes(), baseline)
        self.assertEqual(self.bashrc.stat().st_mode & 0o777, 0o640)
        result = self.run_shell("terminal_config restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.bashrc.read_bytes(), original_bash)
        self.assertEqual(self.zshrc.read_bytes(), original_zsh)
        self.assertEqual(self.bashrc.stat().st_mode & 0o777, 0o640)
        self.assertEqual(self.config.read_text(), "# original custom file\n")
        self.assertEqual(unmanaged.read_text(), "# other Starship configuration\n")
        self.assertFalse(self.baseline.exists())
        self.assertFalse(self.options.exists())

    def test_restore_removes_originally_absent_files_and_allows_new_baseline(self):
        result = self.apply("bash compact magenta on off")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.zshrc.exists())
        result = self.run_shell("terminal_config restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.bashrc.exists())
        self.assertFalse(self.config.exists())
        self.bashrc.write_text("# new baseline\n")
        self.assertEqual(self.apply().returncode, 0)
        self.assertEqual(self.run_shell("terminal_config restore").returncode, 0)
        self.assertEqual(self.bashrc.read_text(), "# new baseline\n")

    def test_switch_shell_removes_previous_managed_initialization(self):
        self.bashrc.write_text("export KEEP=bash\n")
        self.assertEqual(self.apply().returncode, 0)
        result = self.apply("zsh compact green on off")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.bashrc.read_text(), "export KEEP=bash\n")
        self.assertEqual(self.zshrc.read_text().count("starship init zsh"), 1)

    def test_every_theme_is_valid_toml_with_requested_options(self):
        if tomllib is None:
            self.skipTest("TOML parser validation requires Python 3.11+")
        for style in ("compact", "full"):
            for color in ("cyan", "green", "magenta"):
                with self.subTest(style=style, color=color):
                    result = self.apply(f"both {style} {color} on off")
                    self.assertEqual(result.returncode, 0, result.stderr)
                    config = tomllib.loads(self.config.read_text())
                    self.assertEqual(config["directory"]["style"], f"bold {color}")
                    self.assertEqual(config["line_break"]["disabled"], style == "compact")
                    self.assertEqual(config["cmd_duration"]["disabled"], style == "compact")
                    self.assertFalse(config["time"]["disabled"])
                    self.assertTrue(config["git_branch"]["disabled"])
                    self.assertTrue(config["git_status"]["disabled"])

    def test_rc_is_valid_bash_and_quotes_paths_without_executing_them(self):
        self.assertEqual(self.apply().returncode, 0)
        result = subprocess.run(["bash", "-n", str(self.bashrc)], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        # A boundary double checks the generated shell wiring, not Starship's parser.
        result = self.run_shell('starship() { printf ":\\n"; }; source "$HOME/.bashrc"; printf "%s" "$STARSHIP_CONFIG"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, str(self.config))

    def test_invalid_input_and_malformed_blocks_preserve_files(self):
        self.bashrc.write_text("# keep\n")
        for args in ("bash full cyan off", "bash full cyan off on extra", "fish full cyan off on",
                     "bash full red off on", "bash full cyan yes on", "bash invalid cyan off on"):
            with self.subTest(args=args):
                result = self.apply(args)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.bashrc.read_text(), "# keep\n")
                self.assertFalse(self.baseline.exists())
        self.bashrc.write_text("# >>> cqs terminal >>>\n# truncated\n")
        result = self.apply()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("结束标记", result.stderr)
        self.assertFalse(self.baseline.exists())

    def test_symlink_targets_and_parents_are_rejected(self):
        other = self.root / "other"
        other.write_text("keep\n")
        self.bashrc.symlink_to(other)
        result = self.apply()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(other.read_text(), "keep\n")
        self.bashrc.unlink()
        other_dir = self.root / "other-dir"
        other_dir.mkdir()
        (self.home / ".config").symlink_to(other_dir, target_is_directory=True)
        self.assertNotEqual(self.apply().returncode, 0)
        self.assertEqual(list(other_dir.iterdir()), [])

    def test_cannot_restore_under_changed_paths_or_corrupted_baseline(self):
        self.assertEqual(self.apply().returncode, 0)
        baseline = self.baseline.read_bytes()
        result = self.run_shell("terminal_config restore", CQS_TERMINAL_CONFIG=str(self.root / "elsewhere"))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.baseline.read_bytes(), baseline)
        for broken in ("{broken", "null", "[]", "{}"):
            with self.subTest(broken=broken):
                self.baseline.write_text(broken)
                self.assertNotEqual(self.apply().returncode, 0)
                self.assertEqual(self.baseline.read_text(), broken)
                self.assertTrue(self.config.exists())

    def test_config_cannot_replace_the_shared_lock(self):
        result = self.apply(CQS_TERMINAL_CONFIG=str(self.state / "terminal/config.lock"))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bashrc.exists())
        self.assertFalse(self.baseline.exists())

    def test_persistent_options_are_data_and_can_be_read_without_execution(self):
        self.assertEqual(self.apply("bash compact green on off").returncode, 0)
        result = self.run_shell("terminal_config defaults")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "bash compact green on off")
        self.options.write_text('$(touch UNEXPECTED)')
        result = self.run_shell("terminal_config status")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "UNEXPECTED").exists())
        self.assertEqual(self.run_shell("terminal_config restore").returncode, 0)

    def test_cancel_and_eof_do_not_apply_or_restore(self):
        for confirmation in (b"no\n", b"\x04"):
            result = self.cli("run", "terminal.apply", confirmation=confirmation)
            self.assertEqual(result.returncode, 130, result.stdout + result.stderr)
            self.assertFalse(self.bashrc.exists())
            self.assertFalse(self.baseline.exists())
        self.assertEqual(self.apply().returncode, 0)
        for confirmation in (b"no\n", b"YES\n", b"\x04"):
            result = self.cli("run", "terminal.restore", confirmation=confirmation)
            self.assertEqual(result.returncode, 130, result.stdout + result.stderr)
            self.assertTrue(self.baseline.exists())
        result = self.cli("run", "terminal.restore", confirmation=b"RESTORE\n")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.baseline.exists())

    def inject_write_failure(self, target):
        hooks = self.root / "hooks"
        hooks.mkdir(exist_ok=True)
        # Raise a real OSError at one atomic replacement; every other I/O is real.
        (hooks / "sitecustomize.py").write_text('''
import os
from pathlib import Path
original_replace = os.replace
def replace(src, dst, *args, **kwargs):
    if str(dst) == os.environ.get("FAIL_TARGET"):
        marker = Path(os.environ["FAIL_MARKER"])
        if not marker.exists():
            marker.touch()
            raise OSError("injected write failure")
    return original_replace(src, dst, *args, **kwargs)
os.replace = replace
''')
        return {"PYTHONPATH": str(hooks), "FAIL_TARGET": str(target),
                "FAIL_MARKER": str(self.root / "failed")}

    def test_partial_apply_failure_rolls_back_and_cleans_new_baseline(self):
        self.bashrc.write_text("# original bash\n")
        self.zshrc.write_text("# original zsh\n")
        result = self.apply(**self.inject_write_failure(self.zshrc))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("injected write failure", result.stderr)
        self.assertEqual(self.bashrc.read_text(), "# original bash\n")
        self.assertEqual(self.zshrc.read_text(), "# original zsh\n")
        self.assertFalse(self.config.exists())
        self.assertFalse(self.baseline.exists())
        self.assertFalse((self.state / "terminal/pending.json").exists())

    def test_partial_restore_failure_keeps_active_config_and_baseline_for_retry(self):
        self.bashrc.write_text("# original bash\n")
        self.zshrc.write_text("# original zsh\n")
        self.assertEqual(self.apply().returncode, 0)
        before = {p: p.read_bytes() for p in (self.bashrc, self.zshrc, self.config, self.options, self.baseline)}
        result = self.run_shell("terminal_config restore", **self.inject_write_failure(self.zshrc))
        self.assertNotEqual(result.returncode, 0)
        for path, data in before.items():
            self.assertEqual(path.read_bytes(), data)
        self.assertEqual(self.run_shell("terminal_config restore").returncode, 0)

    def test_pending_transaction_and_real_lock_prevent_overlapping_writes(self):
        directory = self.state / "terminal"
        directory.mkdir(parents=True)
        (directory / "pending.json").write_text("{}")
        self.assertNotEqual(self.apply().returncode, 0)
        self.assertFalse(self.bashrc.exists())
        (directory / "pending.json").unlink()
        result = self.run_shell('exec 9>"$CQS_STATE_DIR/terminal/config.lock"; flock -n 9; terminal_config apply both full cyan off on')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bashrc.exists())

    def test_failed_install_does_not_enable_prompt_or_report_success(self):
        result = self.run_shell('''
apt-get() { echo "apt installation failed" >&2; return 42; }
cqs_terminal_install
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("apt installation failed", result.stderr)
        self.assertFalse(self.bashrc.exists())
        self.assertNotIn("已安装", result.stdout)

    def test_starship_install_failure_stops_before_version_check(self):
        result = self.run_shell('''
apt-get() { return 0; }
command() {
    if [[ "$1" == -v && "$2" == starship ]]; then return 1; fi
    builtin command "$@"
}
cqs_starship_install() { echo "Starship install failed" >&2; return 42; }
starship() { echo UNEXPECTED_SUCCESS; return 99; }
cqs_terminal_install || exit 42
''')
        self.assertEqual(result.returncode, 42)
        self.assertIn("Starship install failed", result.stderr)
        self.assertNotIn("UNEXPECTED_SUCCESS", result.stdout)
        self.assertFalse(self.bashrc.exists())

    def test_missing_starship_is_reported_before_writing_configuration(self):
        result = self.run_shell('''
require_commands() {
    if [[ "$1" == starship ]]; then echo 'missing Starship' >&2; return 1; fi
}
terminal_apply bash full cyan off on
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.bashrc.exists())
        self.assertFalse(self.baseline.exists())

    def test_native_starship_can_render_all_generated_themes(self):
        starship = os.environ.get("CQS_TEST_STARSHIP") or shutil.which("starship")
        if not starship:
            self.skipTest("Native Starship not installed; set CQS_TEST_STARSHIP")
        subprocess.run(["git", "init", "-q", "-b", "main", str(self.root)], check=True, capture_output=True)
        for style in ("compact", "full"):
            for color in ("cyan", "green", "magenta"):
                for toggle in ("on", "off"):
                    with self.subTest(style=style, color=color, toggle=toggle):
                        self.assertEqual(self.apply(f"bash {style} {color} {toggle} {toggle}").returncode, 0)
                        result = subprocess.run([starship, "prompt", "--status", "1", "--cmd-duration", "2500"],
                                                capture_output=True, text=True, timeout=10,
                                                env=dict(self.env, STARSHIP_CONFIG=str(self.config), TERM="xterm-256color"),
                                                cwd=self.root)
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stderr, "")
                        self.assertIn(">", result.stdout)
                        self.assertEqual("git:" in result.stdout, toggle == "on")
                        self.assertEqual("took" in result.stdout, style == "full")

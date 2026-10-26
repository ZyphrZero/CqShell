"""Compare the standalone Bash CLI with the real Rust CLI without host changes.

Run on Linux/WSL after building cqs, for example:
    CQS_TEST_BINARY=target/release/cqs python3 -m unittest discover -s tests -p test_shell_cli.py -v
"""

import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "cqs.sh"


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run standalone Shell tests on Linux/WSL")


class ShellTestCase(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-shell-cli-")
        self.addCleanup(self.temp.cleanup)
        self.home = Path(self.temp.name)
        self.env = dict(os.environ)
        for key in tuple(self.env):
            if key.startswith("CQS_") or key in ("BASH_ENV", "ENV", "ZDOTDIR"):
                self.env.pop(key)
        self.env.update(
            HOME=str(self.home),
            USERPROFILE=str(self.home),
            XDG_CONFIG_HOME=str(self.home / "config"),
            XDG_STATE_HOME=str(self.home / "state"),
            CQS_INSTALL_PATH=str(self.home / "cqs"),
            CQS_STATE_DIR=str(self.home / "state/cqs"),
            CQS_TERMINAL_CONFIG=str(self.home / "config/cqs/terminal.toml"),
            LC_ALL="C",
            LANG="C",
            TERM="dumb",
        )

    def command(self, *args, env=None):
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            stdin=subprocess.DEVNULL, text=True, capture_output=True,
            timeout=15, cwd=self.home, env=dict(self.env, **(env or {})),
        )

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def assert_untouched(self):
        self.assertEqual(list(self.home.iterdir()), [])


class ShellCliTests(ShellTestCase):
    def test_help_version_and_metadata_do_not_write_or_prompt(self):
        for args in (("--help",), ("--version",), ("list",),
                     ("list", "--json"), ("languages",),
                     ("languages", "--json")):
            with self.subTest(args=args):
                result = self.command(*args)
                self.assert_success(result)
                self.assertTrue(result.stdout.strip())
                self.assert_untouched()
        manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        version = re.search(r'^version = "([^"]+)"$', manifest, re.MULTILINE).group(1)
        self.assertIn(version, self.command("--version").stdout)

    def test_source_loads_definitions_without_running_startup(self):
        result = subprocess.run(
            ["bash", "-c", f"source {shlex.quote(str(SCRIPT))}\nprintf 'loaded\\n'"],
            stdin=subprocess.DEVNULL, text=True, capture_output=True,
            timeout=10, cwd=self.home, env=self.env,
        )
        self.assert_success(result)
        self.assertEqual(result.stdout, "loaded\n")
        self.assert_untouched()

    def test_noninteractive_start_fails_without_writes(self):
        result = self.command()
        self.assertNotEqual(result.returncode, 0)
        self.assert_untouched()

    def test_invalid_cli_and_parameters_fail_before_execution(self):
        cases = (
            ("--unknown",), ("--lang",), ("run",), ("batch",),
            ("run", "unknown"), ("run", "system.info", "extra"),
            ("run", "bbr.enable", "--set", "qdisc=invalid"),
            ("run", "bbr.enable", "--set", "qdisc=fq", "--set", "qdisc=cake"),
            ("run", "system.info", "--set", "unused=1"),
            ("run", "system.info", "--set", "=value"),
            ("run", "system.info", "--set", "missing-equals"),
            ("run", "ssh.port", "--set", "port=0"),
            ("run", "ssh.port", "--set", "port=65536"),
            ("run", "ssh.port", "--set", "port=22;id"),
            ("run", "ssh.key"),
            ("run", "ssh.key", "--set", "public_key=relative.pub"),
            ("run", "swap.create", "--set", "size_mb=1048577"),
            ("run", "swap.create", "--set", "size_mb=-1"),
            ("run", "tcp.tune", "--set", "bandwidth=0"),
            ("run", "tcp.tune", "--set", "rtt=NaN"),
            ("run", "kernel.install", "--set", "version=6.12"),
            ("run", "kernel.remove", "--set", "release=6.12.0-generic"),
            ("run", "system.timezone", "--set", "timezone=../UTC"),
            ("run", "terminal.apply", "--set", "color=red"),
            ("run", "terminal.apply", "--set", "shell=bash\nzsh"),
            ("run", "docker.install", "--set", "source=auto"),
            ("run", "docker.mirror", "--set", "mirror=file:///tmp/mirror"),
            ("run", "test.ip", "--set", "ip_family=ipv4;id"),
            ("run", "test.ip", "--set", "full_ip=yes"),
            ("run", "test.io", "--set", "directory=/tmp/../var/tmp"),
            ("--json", "run", "system.info"),
            ("--install", "--uninstall"),
            ("--install", "list"),
        )
        for args in cases:
            with self.subTest(args=args):
                result = self.command(*args)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assert_untouched()

    def test_batch_validates_the_whole_plan_before_any_write(self):
        # Even an accidental first action can only write the temporary shortcut.
        cases = (
            ("batch", "app.install", "unknown"),
            ("batch", "app.install", "app.install"),
            ("batch", "app.install", "ssh.key"),
            ("batch", "app.install", "bbr.enable", "--set", "bbr.enable.qdisc=invalid"),
            ("batch", "app.install", "--set", "unknown.parameter=1"),
            ("batch", "app.install", "bbr.enable", "--set", "qdisc=fq"),
            ("batch", "app.install", "bbr.enable", "--set", "bbr.enable.qdisc=fq",
             "--set", "bbr.enable.qdisc=cake"),
        )
        for args in cases:
            with self.subTest(args=args):
                result = self.command("--yes", *args)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assert_untouched()

    def test_language_precedence_and_explicit_unknown_locale(self):
        config = self.home / "preferences.toml"
        config.write_text('language = "zh-CN"\n', encoding="utf-8")
        zh = self.command("--config", str(config), "list", "--json")
        self.assert_success(zh)
        en = self.command("--config", str(config), "list", "--json", env={"CQS_LANG": "en-US"})
        self.assert_success(en)
        override = self.command("--config", str(config), "--lang", "zh-CN", "list", "--json",
                                env={"CQS_LANG": "en-US"})
        self.assert_success(override)
        self.assertEqual(json.loads(zh.stdout), json.loads(override.stdout))
        self.assertNotEqual(json.loads(zh.stdout)[0]["label"], json.loads(en.stdout)[0]["label"])
        invalid = self.command("--lang", "missing-language", "list")
        self.assertEqual(invalid.returncode, 2, invalid.stdout + invalid.stderr)
        self.assertEqual(list(self.home.iterdir()), [config])

    def test_explicit_missing_or_invalid_config_is_an_error(self):
        missing = self.command("--config", str(self.home / "missing.toml"), "--help")
        self.assertEqual(missing.returncode, 2, missing.stdout + missing.stderr)
        self.assert_untouched()
        config = self.home / "invalid.toml"
        for content in ('language = [', 'unknown = "value"\n'):
            config.write_text(content, encoding="utf-8")
            result = self.command("--config", str(config), "--help")
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertEqual(list(self.home.iterdir()), [config])


class ShellRustParityTests(ShellTestCase):
    @classmethod
    def setUpClass(cls):
        explicit = os.environ.get("CQS_TEST_BINARY")
        candidates = [Path(explicit)] if explicit else [ROOT / "target/release/cqs", ROOT / "target/debug/cqs"]
        cls.binary = next((path.resolve() for path in candidates if path.is_file()), None)
        if cls.binary is None:
            if explicit:
                raise RuntimeError(f"CQS_TEST_BINARY does not exist: {explicit}")
            raise unittest.SkipTest("Build Rust cqs or set CQS_TEST_BINARY for cross-implementation checks")

    def reference(self, *args):
        result = subprocess.run(
            [str(self.binary), *args], stdin=subprocess.DEVNULL,
            text=True, capture_output=True, timeout=15,
            cwd=self.home, env=self.env,
        )
        self.assert_success(result)
        return result

    def test_action_registry_matches_rust_for_both_languages(self):
        for locale in ("zh-CN", "en-US"):
            with self.subTest(locale=locale):
                args = ("--lang", locale, "list", "--json")
                shell = self.command(*args)
                self.assert_success(shell)
                actual = json.loads(shell.stdout)
                expected = json.loads(self.reference(*args).stdout)
                self.assertEqual(len({row["id"] for row in actual}), len(actual))
                self.assertEqual({row["id"]: row for row in actual},
                                 {row["id"]: row for row in expected})
                self.assert_untouched()

    def test_language_registry_matches_rust(self):
        shell = self.command("languages", "--json")
        self.assert_success(shell)
        self.assertEqual(json.loads(shell.stdout), json.loads(self.reference("languages", "--json").stdout))
        self.assert_untouched()


if __name__ == "__main__":
    unittest.main()

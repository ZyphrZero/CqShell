"""Standalone tool flows with real shell processes, files, hashes and locks."""

import ast
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
SCRIPT = Path(os.environ.get("CQS_SHELL_TOOLS_SOURCE", ROOT / "cqs.sh")).resolve()
CLI_SCRIPT = Path(os.environ.get("CQS_SHELL_CLI_SOURCE", ROOT / "cqs.sh")).resolve()
MENU_SCRIPT = Path(os.environ.get("CQS_SHELL_MENU_SOURCE", ROOT / "cqs.sh")).resolve()


@unittest.skipUnless(sys.platform == "linux", "Run Shell tools tests in Linux/WSL")
class ShellToolsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-shell-tools-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, HOME=str(self.root), CQS_KERNEL_LOCK=str(self.root / "packages.lock"),
                        CQS_LANG="en-US", CQS_YES="1", CQS_TEST_LOG=str(self.root / "commands"))

    def shell(self, body, tty=False, **env):
        command = "source " + shlex.quote(str(SCRIPT)) + "\n" + body
        args = ["bash", "-c", command]
        if tty:
            # util-linux script supplies a real terminal, preserving the production guard.
            args = ["script", "-q", "-e", "-c", shlex.join(args), "/dev/null"]
        return subprocess.run(args, input="", text=True, capture_output=True, timeout=15,
                              cwd=self.root, env=dict(self.env, **env))

    def test_embedded_python_compiles(self):
        checked = 0
        for path in {SCRIPT, CLI_SCRIPT, MENU_SCRIPT}:
            text = path.read_text(encoding="utf-8")
            blocks = re.findall(r"<<'([A-Z_]+)'[^\n]*\n(.*?)\n\1\n", text, re.DOTALL)
            blocks += [("-c", source) for source in re.findall(r"python3 -c '([^']*)'", text)]
            for marker, source in blocks:
                if not source.lstrip().startswith(("import ", "from ", "#")):
                    continue
                with self.subTest(file=str(path), marker=marker, start=source[:60]):
                    ast.parse(source)
                checked += 1
        self.assertGreater(checked, 0)

    def test_ip_arguments_keep_private_mode_and_follow_language(self):
        for language, expected in (("en-US", "en"), ("zh-CN", "cn"), ("fr-FR", "en")):
            result = self.shell("cqs_ip_arguments ipv6 on", CQS_LANG=language)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.splitlines(), ["-p", "-l", expected, "-6", "-f"])
        for args in ("invalid off", "dual invalid"):
            self.assertEqual(self.shell("cqs_ip_arguments " + args).returncode, 2)

    def test_ip_digest_rejects_download_before_execution(self):
        result = self.shell('''
cqs_confirm() { return 0; }
cqs_download() { printf '%s\n' 'touch "$HOME/executed"' > "$2"; }
cqs_ip_test dual off
''', tty=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("SHA-256 mismatch", result.stdout)
        self.assertFalse((self.root / "executed").exists())

    def test_download_requires_https_and_disallows_insecure_redirects(self):
        result = self.shell('''
curl() { printf '%s\n' "$@" > "$CQS_TEST_LOG"; return 23; }
cqs_download http://example.test "$HOME/download"
''')
        self.assertEqual(result.returncode, 1)
        self.assertFalse((self.root / "commands").exists())
        result = self.shell('''
curl() { printf '%s\n' "$@" > "$CQS_TEST_LOG"; return 23; }
cqs_download https://example.test "$HOME/download"
''')
        self.assertEqual(result.returncode, 23)
        args = (self.root / "commands").read_text().splitlines()
        self.assertEqual(args[args.index("--proto") + 1], "=https")
        self.assertEqual(args[args.index("--proto-redir") + 1], "=https")

    def test_speedtest_converts_bytes_per_second_to_megabits(self):
        value = json.dumps({"download": {"bandwidth": 125000000}, "upload": {"bandwidth": 12500000},
                            "result": {"url": "https://example.test/result"}})
        result = self.shell('cqs_confirm() { return 0; }; speedtest() { printf "%s\\n" "$CQS_SPEED_DATA"; }; cqs_speedtest',
                            CQS_SPEED_DATA=value)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("download: 1000.00 Mbit/s", result.stdout)
        self.assertIn("upload: 100.00 Mbit/s", result.stdout)

    def test_speedtest_failure_and_invalid_output_stay_failures(self):
        for definition in ("return 42", "printf '%s\\n' '{}'", "printf '%s\\n' 'invalid'",
                           "printf '%s\\n' '{\"download\":{\"bandwidth\":true}}'",
                           "printf '%s\\n' '{\"download\":{\"bandwidth\":\"100\"}}'",
                           "printf '%s\\n' '{\"download\":{\"bandwidth\":NaN}}'"):
            result = self.shell("cqs_confirm() { return 0; }; speedtest() { " + definition + "; }; cqs_speedtest")
            self.assertNotEqual(result.returncode, 0)

    def test_packages_validate_selection_and_hold_lock_during_install(self):
        support = '''
acquire_lock() { local descriptor; exec {descriptor}> "$2"; flock -n "$descriptor" || return 1; printf -v "$1" '%s' "$descriptor"; }
apt-get() {
    printf '%s\\n' "$*" >> "$CQS_TEST_LOG"
    if flock -n "$CQS_KERNEL_LOCK" true; then return 99; fi
}
cqs_terminal_install_unlocked() { printf '%s\\n' terminal >> "$CQS_TEST_LOG"; }
'''
        result = self.shell(support + "cqs_packages arbitrary")
        self.assertEqual(result.returncode, 2)
        self.assertFalse((self.root / "commands").exists())
        result = self.shell(support + "cqs_packages curl")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "commands").read_text().splitlines(), ["install -y curl"])
        (self.root / "commands").unlink()
        result = self.shell(support + "cqs_packages all")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.root / "commands").read_text().splitlines(),
                         ["install -y curl wget nano unzip tar tmux iftop btop gdu fzf", "terminal"])
        result = self.shell(support + "exec 9> \"$CQS_KERNEL_LOCK\"; flock -n 9; cqs_packages curl")
        self.assertNotEqual(result.returncode, 0)

    def test_failed_package_install_does_not_run_terminal_installer(self):
        result = self.shell('''
acquire_lock() { return 0; }
apt-get() { return 37; }
cqs_terminal_install_unlocked() { touch "$HOME/executed"; }
cqs_packages all
''')
        self.assertEqual(result.returncode, 37)
        self.assertFalse((self.root / "executed").exists())

    def test_remote_download_and_syntax_failures_never_execute(self):
        for download in ("return 22", "printf '%s\\n' 'touch \"$HOME/executed\"' 'if' > \"$2\""):
            result = self.shell("cqs_confirm() { return 0; }; cqs_download() { " + download + "; }; cqs_remote https://example.test/script", tty=True)
            self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse((self.root / "executed").exists())
        result = self.shell('cqs_confirm() { return 130; }; cqs_remote https://example.test/script', tty=True)
        self.assertEqual(result.returncode, 130)

    def prepare(self, *args):
        return self.shell("source " + shlex.quote(str(ROOT / "cqs.sh")) + "\nsource "
                          + shlex.quote(str(CLI_SCRIPT)) + "\ncqs_prepare " + shlex.join(args))

    def test_cli_accepts_rust_rtt_format_and_rejects_malformed_mirrors(self):
        result = self.prepare("run", "tcp.tune", "--set", "rtt=000050")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for url in ("https://example.test?", "https://example.test#", "https://example.test:65536",
                    "https://example.test:invalid", "https://example test"):
            with self.subTest(url=url):
                result = self.prepare("run", "docker.mirror", "--set", "mirror=" + url)
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)

    def test_multiline_translation_keeps_one_plan_row_per_action(self):
        import tomllib
        catalog = tomllib.loads((ROOT / "locales/en-US.toml").read_text(encoding="utf-8"))
        catalog["messages"]["action.system.info"] = "System\tinfo\ncontinued"
        locale_dir = self.root / "locales"
        locale_dir.mkdir()
        lines = [f"locale = {json.dumps(catalog['locale'])}", f"name = {json.dumps(catalog['name'])}", "[messages]"]
        lines += [json.dumps(key) + " = " + json.dumps(value) for key, value in catalog["messages"].items()]
        (locale_dir / "en-US.toml").write_text("\n".join(lines), encoding="utf-8")
        result = self.prepare("--locale-dir", str(locale_dir), "run", "system.info")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(len(result.stdout.splitlines()), 4)
        self.assertEqual(result.stdout.splitlines()[-1].split("\t")[3], "System info continued")

    def test_menu_keeps_yes_and_cycles_external_languages(self):
        source = MENU_SCRIPT.read_text(encoding="utf-8").split("cqs_menu() {", 1)[1]
        source = source.split("3<<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
        fixture = self.root / "entry.sh"
        fixture.write_text('#!/bin/bash\nprintf "%s\\n" "$@" > "$CQS_TEST_LOG"\n')
        metadata = self.root / "menu.json"
        metadata.write_text(json.dumps({"rows": [], "messages": {}, "config": "/tmp/config.toml",
                                        "locale_dir": "/tmp/locales", "languages": [["en-US", "English"],
                                                                                   ["fr-FR", "French"], ["zh-CN", "Chinese"]]}))
        result = subprocess.run(["python3", "-c", source, str(fixture), str(metadata)], input="l\n",
                                text=True, capture_output=True, timeout=10, env=self.env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((self.root / "commands").read_text().splitlines(),
                         ["--lang", "fr-FR", "--config", "/tmp/config.toml", "--locale-dir", "/tmp/locales", "--yes"])


if __name__ == "__main__":
    unittest.main()

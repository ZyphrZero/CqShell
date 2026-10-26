"""Service transactions against temporary files and explicit OS-boundary doubles."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from shell_support import load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run service transaction tests in Linux/WSL")


class ConfigSafetyTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-config-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.conf = self.root / "service.conf"
        self.definitions = load_script()

    def run_shell(self, body, **settings):
        # sshd/dockerd below are explicit validators for control-flow tests,
        # not evidence that a real daemon accepts a configuration.
        boundary = r'''
require_root() { return 0; }
sshd() {
    printf 'sshd %s\n' "$*" >> "$TEST_ROOT/calls"
    [[ "${VALIDATE_FAIL:-0}" != 1 ]] || return 1
    if [[ "$1" == -T ]]; then
        awk 'tolower($1) == "port" { print "port " $2 }' "$3"
        [[ -z "${EXTRA_PORT:-}" ]] || echo "port $EXTRA_PORT"
        [[ -z "${LISTEN_PORT:-}" ]] || echo "listenaddress 127.0.0.1:$LISTEN_PORT"
    fi
    return 0
}
dockerd() {
    printf 'dockerd %s\n' "$*" >> "$TEST_ROOT/calls"
    [[ "$1" == --validate && "$2" == --config-file ]] || return 99
    [[ "${VALIDATE_FAIL:-0}" != 1 ]]
}
systemctl() {
    printf 'systemctl %s\n' "$*" >> "$TEST_ROOT/calls"
    if [[ "$1" == show ]]; then echo 12345; return 0; fi
    if [[ "$1" == is-active ]]; then
        if [[ "$3" == docker && "${DOCKER_INACTIVE:-0}" == 1 ]]; then return 1; fi
        if [[ "$3" == ssh && "${SSH_INACTIVE:-0}" == 1 ]]; then return 1; fi
        [[ "$3" != ssh.socket ]] || [[ "${SOCKET_ACTIVE:-0}" == 1 ]]
        return $?
    fi
    case "$1" in stop) return 0 ;; reload|reload-or-restart|restart) ;; *) return 99 ;; esac
    if [[ "${SERVICE_FAIL:-}" == always ]]; then return 1; fi
    if [[ "${SERVICE_FAIL:-}" == once && ! -e "$TEST_ROOT/failed" ]]; then
        touch "$TEST_ROOT/failed"
        return 1
    fi
}
ss() { printf 'LISTEN 0 128 127.0.0.1:2222 *:* users:(("sshd",pid=%s,fd=3))\n' "${LISTENER_PID:-12345}"; }
'''
        return subprocess.run(
            ["bash", "-c", self.definitions + "\n" + boundary + "\n" + body],
            capture_output=True, text=True, timeout=10,
            env=dict(os.environ, TEST_ROOT=str(self.root), CONF=str(self.conf), **settings),
        )

    def test_invalid_ssh_ports_do_not_touch_config_or_service(self):
        original = "Port 22\n"
        self.conf.write_text(original)
        for port in ["not-a-port", "0", "65536", "1; id", "-1", "123456"]:
            with self.subTest(port=port):
                result = self.run_shell('system_ssh "$PORT" "$CONF"', PORT=port)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.conf.read_text(), original)
                self.assertFalse((self.root / "calls").exists())

    def test_ssh_validation_failure_preserves_original(self):
        self.conf.write_text("Port 22\n")
        result = self.run_shell('system_ssh 2222 "$CONF"', VALIDATE_FAIL="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.conf.read_text(), "Port 22\n")
        self.assertNotIn("reload", (self.root / "calls").read_text())

    def test_ssh_reload_failure_restores_original_and_reports_failure(self):
        original = "# local config\nPort 22\nPasswordAuthentication yes\n"
        self.conf.write_text(original)
        result = self.run_shell('system_ssh 2222 "$CONF"', SERVICE_FAIL="once")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.conf.read_text(), original)
        calls = (self.root / "calls").read_text()
        self.assertIn("systemctl reload ssh", calls)
        self.assertIn("systemctl reload-or-restart ssh", calls)
        self.assertNotIn("已修改", result.stdout)
        self.assertEqual(list(self.root.glob("*.work.*")), [])

    def test_ssh_sets_global_port_without_changing_match_block(self):
        original = "#Port 22\nPort 2200\nMatch User backup\n    X11Forwarding no\n"
        self.conf.write_text(original)
        self.conf.chmod(0o600)
        result = self.run_shell('system_ssh 02222 "$CONF"')
        self.assertEqual(result.returncode, 0, result.stderr)
        content = self.conf.read_text()
        self.assertTrue(content.startswith("Port 2222\n"))
        self.assertNotIn("Port 2200", content)
        self.assertIn("Match User backup\n    X11Forwarding no\n", content)
        self.assertEqual(self.conf.stat().st_mode & 0o777, 0o600)

    def test_ssh_socket_or_include_conflict_is_rejected(self):
        original = "Include /etc/ssh/sshd_config.d/*.conf\n"
        for settings in ({"SOCKET_ACTIVE": "1"}, {"EXTRA_PORT": "2200"},
                         {"LISTEN_PORT": "22"}, {"SSH_INACTIVE": "1"}):
            with self.subTest(settings=settings):
                self.conf.write_text(original)
                result = self.run_shell('system_ssh 2222 "$CONF"', **settings)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.conf.read_text(), original)

    def test_ssh_does_not_accept_another_process_listening_on_port(self):
        self.conf.write_text("Port 22\n")
        result = self.run_shell('sleep() { :; }; system_ssh 2222 "$CONF"', LISTENER_PID="54321")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("未监听目标端口", result.stderr)
        self.assertEqual(self.conf.read_text(), "Port 22\n")

    def test_docker_toggle_preserves_unrelated_options(self):
        unrelated = {"data-root": "/srv/docker", "log-driver": "journald",
                     "registry-mirrors": ["https://mirror.example.invalid"]}
        self.conf.write_text(json.dumps(unrelated))
        self.conf.chmod(0o640)
        result = self.run_shell('docker_set_ipv6 on "$CONF"')
        self.assertEqual(result.returncode, 0, result.stderr)
        enabled = json.loads(self.conf.read_text())
        self.assertTrue(enabled["ipv6"])
        self.assertTrue(enabled["fixed-cidr-v6"].startswith("fd"))
        result = self.run_shell('docker_set_ipv6 off "$CONF"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.conf.read_text()), dict(unrelated, ipv6=False))
        self.assertEqual(self.conf.stat().st_mode & 0o777, 0o640)

    def test_docker_existing_ipv6_subnet_is_preserved_when_enabling(self):
        self.conf.write_text('{"fixed-cidr-v6":"fd42:1234::/64","data-root":"/srv/docker"}')
        result = self.run_shell('docker_set_ipv6 on "$CONF"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.conf.read_text())["fixed-cidr-v6"], "fd42:1234::/64")

    def test_docker_invalid_json_or_daemon_validation_failure_is_non_destructive(self):
        for original, settings in [("{bad", {}), ("[]", {}), ('{"debug":true}', {"VALIDATE_FAIL": "1"})]:
            with self.subTest(original=original):
                self.conf.write_text(original)
                result = self.run_shell('docker_set_ipv6 on "$CONF"', **settings)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.conf.read_text(), original)
                calls = self.root / "calls"
                if calls.exists():
                    self.assertNotIn("restart", calls.read_text())

    def test_docker_restart_failure_restores_original_bytes(self):
        original = '{ "data-root": "/srv/docker", "debug": true }\n'
        self.conf.write_text(original)
        result = self.run_shell('docker_set_ipv6 on "$CONF"', SERVICE_FAIL="once")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.conf.read_text(), original)
        self.assertEqual((self.root / "calls").read_text().count("systemctl restart docker"), 2)
        self.assertNotIn("已开启", result.stdout)

    def test_docker_failure_removes_new_config(self):
        result = self.run_shell('docker_set_ipv6 on "$CONF"', SERVICE_FAIL="once")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.conf.exists())

    def test_failed_docker_toggle_restores_inactive_service_state(self):
        self.conf.write_text('{"debug":true}')
        result = self.run_shell('docker_set_ipv6 on "$CONF"', SERVICE_FAIL="once", DOCKER_INACTIVE="1")
        self.assertNotEqual(result.returncode, 0)
        calls = (self.root / "calls").read_text()
        self.assertEqual(calls.count("systemctl restart docker"), 1)
        self.assertIn("systemctl stop docker", calls)
        self.assertEqual(self.conf.read_text(), '{"debug":true}')

    def test_failed_service_recovery_keeps_material(self):
        for function, args, original in [("system_ssh", "2222", "Port 22\n"),
                                         ("docker_set_ipv6", "on", '{"debug":true}')]:
            with self.subTest(function=function):
                self.conf.write_text(original)
                result = self.run_shell(f'{function} {args} "$CONF"', SERVICE_FAIL="always")
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("保留备份", result.stderr)
                self.assertEqual(self.conf.read_text(), original)
                self.assertTrue(list(self.root.glob("*.work.*")))

    def test_symlink_targets_are_not_overwritten(self):
        target = self.root / "target"
        target.write_text("original\n")
        self.conf.symlink_to(target)
        for command in ['system_ssh 2222 "$CONF"', 'docker_set_ipv6 on "$CONF"']:
            self.assertNotEqual(self.run_shell(command).returncode, 0)
        self.assertTrue(self.conf.is_symlink())
        self.assertEqual(target.read_text(), "original\n")

    def test_config_lock_blocks_service_changes(self):
        import fcntl
        for command, original in [('system_ssh 2222 "$CONF"', "Port 22\n"),
                                  ('docker_set_ipv6 on "$CONF"', '{"debug":true}')]:
            with self.subTest(command=command):
                self.conf.write_text(original)
                calls_file = self.root / "calls"
                calls_file.unlink(missing_ok=True)
                with Path(str(self.conf) + ".cqs-lock").open("w") as lock:
                    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    result = self.run_shell(command)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("另一个进程", result.stderr)
                self.assertEqual(self.conf.read_text(), original)
                calls = calls_file.read_text() if calls_file.exists() else ""
                self.assertNotIn("systemctl reload", calls)
                self.assertNotIn("systemctl restart", calls)
                self.assertNotIn("dockerd --validate", calls)

    def test_onekey_stops_before_disabling_password_login_when_ssh_fails(self):
        boundary = "\n".join(
            name + "() { :; }" for name in (
                "require_root", "clear", "system_update", "system_clean", "tcp_tune", "enable_bbr",
                "system_time", "swap_create_auto", "system_fail2ban", "download_all", "docker_install",
            )
        )
        body = load_script() + "\n" + boundary + r'''
system_ssh() { return 1; }
system_keygen() { touch "$TEST_ROOT/LOGIN_CHANGED"; }
onekey_optimization
'''
        result = subprocess.run(
            ["bash", "-c", body], input="10 11\ns\ny\n", text=True, capture_output=True, timeout=10,
            env=dict(os.environ, TEST_ROOT=str(self.root)),
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "LOGIN_CHANGED").exists())

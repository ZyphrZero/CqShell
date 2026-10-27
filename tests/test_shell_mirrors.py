"""Mirror logic, recovery, real curl/TLS, and optional pinned upstream generation.

All source files and HTTPS responses live in temporary directories. No host APT
configuration or package state is changed. CQS_LINUXMIRRORS_SCRIPT enables a
generator-only smoke test against the real, digest-verified upstream script.
"""
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import types
import unittest


ROOT = Path(__file__).resolve().parents[1]


def backend():
    text = (ROOT / "cqs.sh").read_text()
    source = text.split("3<<'CQS_MIRRORS_PY'\n", 1)[1].split("\nCQS_MIRRORS_PY", 1)[0]
    module = types.ModuleType("mirror_backend")
    exec(compile(source, str(ROOT / "cqs.sh") + ":mirrors", "exec"), module.__dict__)
    return module


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run mirror tests in Linux/WSL")


class MirrorTests(unittest.TestCase):
    def setUp(self):
        self.module = backend()
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-mirror-test-")
        self.addCleanup(self.temp.cleanup)
        self.apt = Path(self.temp.name) / "apt"
        (self.apt / "sources.list.d").mkdir(parents=True)
        self.host = self.module.Host("ID=debian\nVERSION_CODENAME=trixie", "amd64")

    def test_pin_and_candidate_catalog_match_rust(self):
        rust = (ROOT / "src/ops/mirrors.rs").read_text()
        for value in [self.module.REVISION, self.module.SCRIPT_SHA256, *self.module.SITES]:
            self.assertIn('"' + value + '"', rust)
        for distro, code, arch, branch in [("ubuntu", "noble", "arm64", "ubuntu-ports"),
                                          ("ubuntu", "jammy", "amd64", "ubuntu"),
                                          ("debian", "bookworm", "arm64", "debian")]:
            host = self.module.Host(f"ID={distro}\nVERSION_CODENAME={code}", arch)
            args = self.module.arguments(host, "tuna")
            self.assertEqual(args[args.index("--branch") + 1], branch)
            self.assertEqual(args[args.index("--source-security") + 1], host.security[0])
            for option in ["--backup", "--upgrade-software", "--clean-cache"]:
                self.assertEqual(args[args.index(option) + 1], "false")

    def test_unknown_and_mixed_configuration_is_rejected(self):
        self.module.check_source(self.host, "sources.list", "deb https://deb.debian.org/debian trixie main\n")
        for line in ["deb https://download.docker.com/linux/debian trixie stable",
                     "deb [trusted=yes] https://deb.debian.org/debian trixie main",
                     "deb https://deb.debian.org/debian stable main"]:
            with self.assertRaises(self.module.Failure):
                self.module.check_source(self.host, "sources.list", line)
        with self.assertRaises(self.module.Failure):
            self.module.Host("ID=debian\nVERSION_CODENAME=sid", "amd64")

    def test_transaction_preserves_bytes_permissions_and_third_party_on_failure(self):
        source = self.apt / "sources.list"
        source.write_bytes(b"# original\r\n")
        source.chmod(0o640)
        third_party = self.apt / "sources.list.d/docker.list"
        third_party.write_bytes(b"unrelated")
        before = self.module.capture(self.apt, self.host)
        def fail():
            source.write_text("broken")
            (self.apt / "sources.list.d/debian.sources").write_text("partial")
            raise OSError("injected operation failure")
        with self.assertRaises(OSError):
            self.module.transaction(self.apt, self.host, before, fail)
        self.assertEqual(self.module.capture(self.apt, self.host), before)
        self.assertEqual(third_party.read_bytes(), b"unrelated")
        self.assertFalse((self.apt / self.module.STATE / "pending.json").exists())

    def test_cancelled_second_change_preserves_last_undo_and_manual_edits_block_restore(self):
        source = self.apt / "sources.list"
        source.write_bytes(b"# original\r\n")
        before = self.module.capture(self.apt, self.host)
        self.module.transaction(self.apt, self.host, before, lambda: source.write_text("changed"))
        state = self.apt / self.module.STATE
        last = (state / "last.json").read_bytes()
        current = self.module.capture(self.apt, self.host)
        def cancel():
            source.write_text("partial")
            self.module.CANCELLED = True
        with self.assertRaises(self.module.Failure):
            self.module.transaction(self.apt, self.host, current, cancel)
        self.module.CANCELLED = False
        self.assertEqual(self.module.capture(self.apt, self.host), current)
        self.assertEqual((state / "last.json").read_bytes(), last)
        source.write_text("manual edit")
        with self.assertRaises(self.module.Failure):
            self.module.restore_files(self.apt, self.host)
        source.write_text("changed")
        self.module.restore_files(self.apt, self.host)
        self.assertEqual(self.module.capture(self.apt, self.host), before)

    def test_pending_recovery_rejects_arbitrary_paths_and_corrupt_snapshots(self):
        state = self.apt / self.module.STATE
        state.mkdir()
        original = self.module.capture(self.apt, self.host)
        record = dict(version=1, distro="debian", before=original, after=None, previous=None)
        pending = state / "pending.json"
        for invalid in [dict(record, before={"../../escape": None}), dict(record, version=2),
                        dict(record, before={name: dict(data="!invalid", mode=420) for name in original})]:
            pending.write_text(json.dumps(invalid))
            with self.assertRaises((self.module.Failure, ValueError)):
                self.module.restore_files(self.apt, self.host)
            self.assertTrue(pending.exists())
        pending.write_text(json.dumps(record))
        (self.apt / "sources.list").write_text("interrupted")
        self.module.restore_files(self.apt, self.host)
        self.assertEqual(self.module.capture(self.apt, self.host), original)

    def test_wrong_or_comment_only_result_is_not_success(self):
        source = self.apt / "sources.list"
        source.write_text("# https://mirrors.tuna.tsinghua.edu.cn/debian/\n# https://security.debian.org/debian-security/\n")
        with self.assertRaises(self.module.Failure):
            self.module.verify_result(self.apt, self.host, "tuna")

    def test_cli_invalid_choice_fails_without_creating_state(self):
        result = subprocess.run(["bash", str(ROOT / "cqs.sh"), "run", "system.mirror.apply", "--set", "mirror_site=bad;id"],
                                env=dict(os.environ, HOME=self.temp.name, CQS_STATE_DIR=str(self.apt / "state")),
                                capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertFalse((self.apt / "state").exists())

    def test_external_locale_reaches_benchmark_and_all_failed_samples_stay_errors(self):
        directory = Path(self.temp.name)
        locales = directory / "locales"
        locales.mkdir()
        text = (ROOT / "locales/en-US.toml").read_text()
        text = text.replace('locale = "en-US"', 'locale = "xx"')
        lines = text.splitlines()
        lines = ['"error.mirror_speed" = "CUSTOM ALL FAILED: {detail}"' if line.startswith('"error.mirror_speed" =') else line for line in lines]
        (locales / "xx.toml").write_text("\n".join(lines) + "\n")
        binary = directory / "bin"
        binary.mkdir()
        (binary / "curl").write_text("#!/bin/sh\nexit 77\n")  # Explicit network failure, never a fake success.
        (binary / "curl").chmod(0o755)
        result = subprocess.run(["bash", str(ROOT / "cqs.sh"), "--locale-dir", str(locales), "--lang", "xx",
                                 "run", "system.mirror.test"], capture_output=True, text=True, timeout=15,
                                env=dict(os.environ, PATH=str(binary) + ":" + os.environ["PATH"], HOME=self.temp.name))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CUSTOM ALL FAILED", result.stderr)

    @unittest.skipUnless(os.environ.get("CQS_LINUXMIRRORS_SCRIPT"), "Set CQS_LINUXMIRRORS_SCRIPT for pinned upstream generation")
    def test_pinned_upstream_generates_supported_formats_and_architectures(self):
        original = Path(os.environ["CQS_LINUXMIRRORS_SCRIPT"]).read_bytes()
        self.assertEqual(hashlib.sha256(original).hexdigest(), self.module.SCRIPT_SHA256)
        text = original.decode()
        self.assertTrue(text.endswith("\nmain\n"))
        # Remove only the final entrypoint in this TEST harness. Call the actual
        # upstream generator with temporary paths; never run host mutation/apt.
        definitions = text[:-len("main\n")]
        harness = Path(self.temp.name) / "upstream-generator.sh"
        harness.write_text(definitions + r'''
Dir_AptAdditionalSources="$CQS_FIXTURE_ROOT/sources.list.d"
File_AptSourceList="$CQS_FIXTURE_ROOT/sources.list"
File_DebianSources="$Dir_AptAdditionalSources/debian.sources"
File_UbuntuSources="$Dir_AptAdditionalSources/ubuntu.sources"
File_ArmbianRelease="$CQS_FIXTURE_ROOT/no-armbian"
File_ProxmoxVersion="$CQS_FIXTURE_ROOT/no-proxmox"
SYSTEM_FACTIONS=Debian
SYSTEM_JUDGMENT="$CQS_FIXTURE_SYSTEM"
SYSTEM_VERSION_CODENAME="$CQS_FIXTURE_CODE"
SYSTEM_VERSION_ID_MAJOR="$CQS_FIXTURE_MAJOR"
DEVICE_ARCH_RAW="$CQS_FIXTURE_ARCH"
USE_DEB822_FORMAT="$CQS_FIXTURE_DEB822"
change_mirrors_Debian
''')
        for distro, code, major in [("debian", "bookworm", "12"), ("debian", "trixie", "13"),
                                     ("ubuntu", "jammy", "22"), ("ubuntu", "noble", "24"), ("ubuntu", "resolute", "26")]:
            for arch in ["amd64", "arm64"]:
                for deb822 in ["true", "false"]:
                    with self.subTest(distro=distro, code=code, arch=arch, deb822=deb822):
                        with tempfile.TemporaryDirectory(dir=self.temp.name) as directory:
                            apt = Path(directory)
                            (apt / "sources.list.d").mkdir()
                            host = self.module.Host(f"ID={distro}\nVERSION_CODENAME={code}", arch)
                            env = dict(os.environ, CQS_FIXTURE_ROOT=directory, CQS_FIXTURE_SYSTEM=distro.capitalize(),
                                       CQS_FIXTURE_CODE=code, CQS_FIXTURE_MAJOR=major,
                                       CQS_FIXTURE_ARCH="x86_64" if arch == "amd64" else "aarch64", CQS_FIXTURE_DEB822=deb822)
                            result = subprocess.run(["bash", str(harness), *self.module.arguments(host, "tuna")],
                                                    env=env, capture_output=True, text=True, timeout=20)
                            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                            self.module.verify_result(apt, host, "tuna")


class CurlTlsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not shutil.which("openssl") or not shutil.which("curl"):
            raise unittest.SkipTest("Native curl and openssl are required")
        cls.temp = tempfile.TemporaryDirectory(prefix="cqs-mirror-tls-")
        root = Path(cls.temp.name)
        cls.cert = root / "cert.pem"
        key = root / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
                        "-keyout", str(key), "-out", str(cls.cert)], check=True, capture_output=True)
        cls.body = b"\xfd7zXZ\0" + bytes(1024 * 1024 - 6)
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass
            def do_GET(self):
                if self.path == "/missing":
                    self.send_error(404)
                    return
                self.send_response(200 if self.path == "/no-range" else 206)
                self.send_header("Content-Length", str(len(cls.body)))
                self.send_header("Content-Range", "bytes 0-1048575/9000000")
                self.end_headers()
                body = cls.body if self.path != "/html" else b"<html>" + cls.body[6:]
                if self.path == "/truncated":
                    body = body[:512]
                self.wfile.write(body)
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cls.cert, key)
        cls.server.socket = context.wrap_socket(cls.server.socket, server_side=True)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join()
        cls.temp.cleanup()

    def test_real_https_range_and_failures(self):
        module = backend()
        previous = {key: os.environ.get(key) for key in ["CURL_CA_BUNDLE", "NO_PROXY"]}
        os.environ.update(CURL_CA_BUNDLE=str(self.cert), NO_PROXY="localhost,127.0.0.1")
        try:
            base = f"https://localhost:{self.server.server_port}"
            speed, first = module.probe(base + "/valid")
            self.assertGreater(speed, 0)
            self.assertGreaterEqual(first, 0)
            for path in ["/missing", "/no-range", "/html", "/truncated"]:
                with self.subTest(path=path), self.assertRaises(module.Failure):
                    module.probe(base + path)
        finally:
            for key, value in previous.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value


if __name__ == "__main__":
    unittest.main()

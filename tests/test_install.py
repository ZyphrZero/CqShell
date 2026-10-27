"""Exercise the real installer, curl/TLS, tar and a real built cqs executable."""

import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import platform
import ssl
import subprocess
import tarfile
import tempfile
import threading
import unittest


INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"


class ReleaseHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.requests.append(self.path)
        status, headers, body = self.server.responses.get(self.path, (404, {}, b"Not found"))
        self.send_response(status)
        for name, value in headers.items():
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


class InstallerTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        binary_path = os.environ.get("CQS_TEST_BINARY")
        if not binary_path:
            raise RuntimeError("Set CQS_TEST_BINARY to a real built Linux cqs executable.")
        cls.binary = Path(binary_path).resolve()
        cls.binary_bytes = cls.binary.read_bytes()
        cls.version_output = subprocess.check_output([str(cls.binary), "--version"], text=True).strip()
        if not cls.version_output.startswith("cqs "):
            raise RuntimeError(f"Unexpected binary version: {cls.version_output}")
        cls.tag = "v" + cls.version_output.removeprefix("cqs ")
        arch = {"x86_64": "x86_64", "aarch64": "aarch64"}[platform.machine()]
        cls.asset = f"cqs-{arch}-unknown-linux-musl.tar.gz"
        if os.environ.get("CQS_TEST_ARCHIVE"):
            cls.archive = Path(os.environ["CQS_TEST_ARCHIVE"]).read_bytes()
        else:
            cls.archive = cls.make_archive({"cqs/cqs": cls.binary_bytes})

        cls.fixture = tempfile.TemporaryDirectory(prefix="cqs-installer-tls-")
        cls.addClassCleanup(cls.fixture.cleanup)
        cert = Path(cls.fixture.name) / "cert.pem"
        key = Path(cls.fixture.name) / "key.pem"
        subprocess.run(
            ["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
             "-keyout", str(key), "-out", str(cert), "-subj", "/CN=localhost",
             "-addext", "subjectAltName=DNS:localhost"],
            check=True, capture_output=True,
        )
        cls.cert = cert
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ReleaseHandler)
        cls.addClassCleanup(cls.server.server_close)
        cls.server.socket = context.wrap_socket(cls.server.socket, server_side=True)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.addClassCleanup(cls.thread.join)
        cls.addClassCleanup(cls.server.shutdown)
        cls.base = f"https://localhost:{cls.server.server_port}/releases"

    @staticmethod
    def make_archive(entries):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as archive:
            for name, content in entries.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(content)
                entry.mode = 0o755
                archive.addfile(entry, io.BytesIO(content))
        return output.getvalue()

    def setUp(self):
        self.case = tempfile.TemporaryDirectory(prefix="cqs-installer-test-")
        self.addCleanup(self.case.cleanup)
        self.root = Path(self.case.name)
        self.bin_dir = self.root / "bin with spaces"
        self.bin_dir.mkdir()
        self.destination = self.bin_dir / "cqs"
        self.temp_dir = self.root / "downloads"
        self.temp_dir.mkdir()
        self.server.requests = []
        self.server.responses = {
            "/releases/latest": (302, {"Location": f"{self.base}/tag/{self.tag}"}, b""),
            f"/releases/tag/{self.tag}": (200, {}, b"Release"),
        }
        self.asset_path = f"/releases/download/{self.tag}/{self.asset}"
        self.set_archive(self.archive)

    def set_archive(self, body):
        self.server.responses[self.asset_path] = (200, {}, body)
        checksum = f"{hashlib.sha256(body).hexdigest()}  {self.asset}\n".encode()
        self.server.responses[self.asset_path + ".sha256"] = (200, {}, checksum)

    def run_installer(self, *args, env=None, use_default_dir=False):
        environment = {
            **os.environ,
            "CQS_RELEASE_BASE_URL": self.base,
            "CURL_CA_BUNDLE": str(self.cert),
            "NO_PROXY": "localhost,127.0.0.1",
            "no_proxy": "localhost,127.0.0.1",
            "GH_PROXY": "",
            "TMPDIR": str(self.temp_dir),
            **(env or {}),
        }
        command = ["bash", str(INSTALLER)]
        if not use_default_dir:
            command.extend(["--install-dir", str(self.bin_dir)])
        result = subprocess.run(
            [*command, *args],
            env=environment, text=True, capture_output=True, timeout=30,
        )
        self.assertEqual(list(self.temp_dir.iterdir()), [], result.stderr)
        self.assertEqual(list(self.bin_dir.glob(".cqs.*")), [], result.stderr)
        return result

    def assert_installed(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.destination.read_bytes(), self.binary_bytes)
        self.assertEqual(self.destination.stat().st_mode & 0o777, 0o755)
        version = subprocess.check_output([str(self.destination), "--version"], text=True).strip()
        self.assertEqual(version, self.version_output)

    def assert_rejected(self, message, *args, env=None):
        self.destination.write_bytes(b"existing installation")
        result = self.run_installer(*args, env=env)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertIn(message, result.stderr)
        self.assertEqual(self.destination.read_bytes(), b"existing installation")

    def test_latest_installs_real_binary(self):
        self.assert_installed(self.run_installer())
        self.assertEqual(self.server.requests.count("/releases/latest"), 1)
        self.assertIn(self.asset_path, self.server.requests)

    def test_default_installs_the_global_cqs_command(self):
        boundary = self.root / "filesystem-boundary"
        boundary.mkdir()
        arguments = self.root / "mkdir-arguments"
        mkdir = boundary / "mkdir"
        # Stop before touching the host directory. Explicit-directory tests
        # exercise the real install, executable mode and atomic replacement.
        mkdir.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$CQS_TEST_MKDIR_ARGS"\nexit 73\n')
        mkdir.chmod(0o755)
        result = self.run_installer(use_default_dir=True, env={
            "PATH": str(boundary) + ":" + os.environ["PATH"],
            "CQS_TEST_MKDIR_ARGS": str(arguments),
        })
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cannot create the installation directory", result.stderr)
        self.assertEqual(arguments.read_text().splitlines(), ["-p", "--", "/usr/local/bin"])
        self.assertIn(self.asset_path, self.server.requests)
        self.assertIn(self.asset_path + ".sha256", self.server.requests)

    def test_installed_cqs_command_runs_from_path(self):
        environment = {"PATH": str(self.bin_dir) + ":" + os.environ["PATH"]}
        result = self.run_installer(env=environment)
        self.assert_installed(result)
        self.assertIn("Run: cqs --lang zh-CN", result.stdout)
        version = subprocess.check_output(["cqs", "--version"], text=True,
                                          env={**os.environ, **environment}).strip()
        self.assertEqual(version, self.version_output)

    def test_pinned_version_does_not_resolve_latest(self):
        del self.server.responses["/releases/latest"]
        self.assert_installed(self.run_installer("--version", self.tag))
        self.assertNotIn("/releases/latest", self.server.requests)

    def proxy_fixture(self):
        self.github = "https://github.com/ZyphrZero/CqShell/releases"
        self.proxy = self.base.removesuffix("/releases") + "/proxy/"
        self.proxy_api = "/proxy/https://api.github.com/repos/ZyphrZero/CqShell/releases/latest"
        self.proxy_asset = f"/proxy/{self.github}/download/{self.tag}/{self.asset}"
        self.server.requests = []
        self.server.responses = {
            self.proxy_api: (200, {}, json.dumps({"tag_name": self.tag, "draft": False, "prerelease": False}).encode()),
            self.proxy_asset: (200, {}, self.archive),
            self.proxy_asset + ".sha256": (200, {}, f"{hashlib.sha256(self.archive).hexdigest()}  {self.asset}\n".encode()),
        }
        return {"CQS_RELEASE_BASE_URL": self.github}

    def test_github_proxy_covers_latest_api_archive_and_checksum(self):
        for trailing in (False, True):
            with self.subTest(trailing_slash=trailing):
                env = self.proxy_fixture()
                proxy = self.proxy if trailing else self.proxy.rstrip("/")
                self.assert_installed(self.run_installer("--gh-proxy", proxy, env=env))
                self.assertEqual(self.server.requests, [self.proxy_api, self.proxy_asset, self.proxy_asset + ".sha256"])

    def test_github_proxy_environment_and_pinned_version_skip_latest(self):
        env = self.proxy_fixture()
        del self.server.responses[self.proxy_api]
        env["GH_PROXY"] = self.proxy
        self.assert_installed(self.run_installer("--version", self.tag, env=env))
        self.assertEqual(self.server.requests, [self.proxy_asset, self.proxy_asset + ".sha256"])

    def test_explicit_github_proxy_overrides_environment(self):
        env = self.proxy_fixture()
        env["GH_PROXY"] = "http://invalid.example"
        self.assert_installed(self.run_installer("--gh-proxy", self.proxy, env=env))

    def test_proxy_uses_the_configured_github_repository(self):
        env = self.proxy_fixture()
        env["CQS_RELEASE_BASE_URL"] = "https://github.com/fixture/fork/releases"
        self.server.responses = {path.replace("/ZyphrZero/CqShell/", "/fixture/fork/"): value
                                 for path, value in self.server.responses.items()}
        self.assert_installed(self.run_installer("--gh-proxy", self.proxy, env=env))
        self.assertEqual(len(self.server.requests), 3)
        self.assertTrue(all("/fixture/fork/" in path for path in self.server.requests))

    def test_invalid_github_proxy_fails_before_network(self):
        for value in ("http://example.com/", "https://", "https:///example.com", "https://user@example.com/",
                      "https://example.com/?token=value", "https://example.com/#fragment", "https://example.com/\\path",
                      "https://example.com/\n", "https://example.com:abc/", "https://example.com:0/", "https://example.com:65536/"):
            with self.subTest(proxy=value):
                self.assert_rejected("--gh-proxy", env={"GH_PROXY": value})
        self.assertEqual(self.server.requests, [])
        self.assert_rejected("--gh-proxy requires", "--gh-proxy")
        self.assert_rejected("--gh-proxy requires", "--gh-proxy", "")

    def test_proxy_and_non_github_release_mirror_cannot_be_combined(self):
        self.assert_rejected("GitHub releases URL", "--gh-proxy", self.base + "/proxy/")
        self.assertEqual(self.server.requests, [])

    def test_proxy_corruption_still_preserves_existing_installation(self):
        env = self.proxy_fixture()
        self.server.responses[self.proxy_asset] = (200, {}, self.archive + b"corrupt")
        self.assert_rejected("SHA-256 verification failed", "--gh-proxy", self.proxy, env=env)

    def test_proxy_cannot_downgrade_to_http(self):
        env = self.proxy_fixture()
        self.server.responses[self.proxy_asset] = (302, {"Location": "http://localhost/unsafe"}, b"")
        self.assert_rejected("Cannot download", "--gh-proxy", self.proxy, env=env)

    def test_proxy_latest_failures_do_not_fall_back_to_direct_github(self):
        env = self.proxy_fixture()
        self.server.responses[self.proxy_api] = (403, {}, b"API not supported")
        self.assert_rejected("--version", "--gh-proxy", self.proxy, env=env)
        self.assertEqual(self.server.requests, [self.proxy_api])

    def test_proxy_latest_requires_one_unescaped_valid_release_tag(self):
        invalid = [b'<html>Not a release</html>', b'{"message":"rate limited"}',
                   b'{"tag_name":"../../escape"}', b'{"tag_name":42}',
                   b'{"tag_name":"v1.2.3","tag_name":"v4.5.6"}',
                   b'{"tag_name":"v1.2.3","tag_name":null}',
                   b'{"tag_name":null,"tag_name":"v1.2.3"}',
                   b'{"tag_name":"v1.2.3"invalid}',
                   b'{"tag_name":"v1.2.3"', b'{"body":"tag_name: v1.2.3"}',
                   json.dumps({"body": '"tag_name":"v1.2.3"'}).encode()]
        for body in invalid:
            with self.subTest(body=body):
                env = self.proxy_fixture()
                self.server.responses[self.proxy_api] = (200, {}, body)
                self.assert_rejected("release metadata", "--gh-proxy", self.proxy, env=env)
                self.assertEqual(self.server.requests, [self.proxy_api])

    def test_proxy_latest_accepts_pretty_json_without_matching_escaped_body_text(self):
        env = self.proxy_fixture()
        body = {"body": 'Example: {"tag_name":"v9876.0.0"}', "tag_name": self.tag, "draft": False, "prerelease": False}
        self.server.responses[self.proxy_api] = (200, {}, json.dumps(body, indent=2).encode())
        self.assert_installed(self.run_installer("--gh-proxy", self.proxy, env=env))

    def test_upgrade_replaces_existing_binary(self):
        self.destination.write_bytes(b"previous version")
        self.assert_installed(self.run_installer())

    def test_symlink_is_replaced_without_writing_through_it(self):
        linked = self.root / "other-file"
        linked.write_bytes(b"preserve me")
        self.destination.symlink_to(linked)
        self.assert_installed(self.run_installer())
        self.assertFalse(self.destination.is_symlink())
        self.assertEqual(linked.read_bytes(), b"preserve me")

    def test_corrupt_download_preserves_existing_binary(self):
        self.server.responses[self.asset_path] = (200, {}, self.archive + b"corrupted")
        self.assert_rejected("SHA-256 verification failed")

    def test_untrusted_checksum_entries_are_rejected(self):
        digest = hashlib.sha256(self.archive).hexdigest()
        for checksum in (f"{digest}  ../../other-file\n", "invalid\n", f"{digest}  {self.asset}\nextra\n"):
            with self.subTest(checksum=checksum):
                self.server.responses[self.asset_path + ".sha256"] = (200, {}, checksum.encode())
                self.assert_rejected("cqs installer:")

    def test_missing_assets_preserve_existing_binary(self):
        for path in (self.asset_path, self.asset_path + ".sha256"):
            with self.subTest(path=path):
                saved = self.server.responses.pop(path)
                self.assert_rejected("Cannot download")
                self.server.responses[path] = saved

    def test_no_published_release_has_clear_error(self):
        del self.server.responses["/releases/latest"]
        self.assert_rejected("Cannot resolve the latest release")

    def test_invalid_archive_preserves_existing_binary(self):
        for body in (b"not a gzip archive", self.make_archive({"cqs/README.md": b"missing binary"})):
            with self.subTest(body_size=len(body)):
                self.set_archive(body)
                self.assert_rejected("Cannot extract")

    def test_other_archive_paths_are_never_extracted(self):
        escaped = self.temp_dir / "escaped"
        self.set_archive(self.make_archive({"../escaped": b"unwanted", "cqs/cqs": self.binary_bytes}))
        self.assert_installed(self.run_installer())
        self.assertFalse(escaped.exists())

    def test_binary_version_must_match_release_tag(self):
        wrong_tag = "v9876.0.0"
        for suffix in ("", ".sha256"):
            self.server.responses[self.asset_path.replace(self.tag, wrong_tag) + suffix] = \
                self.server.responses[self.asset_path + suffix]
        self.assert_rejected("Binary version mismatch", "--version", wrong_tag)

    def test_https_redirect_cannot_downgrade_to_http(self):
        self.server.responses[self.asset_path] = (302, {"Location": "http://localhost/unsafe"}, b"")
        self.assert_rejected("Cannot download")

    def test_certificate_verification_is_required(self):
        self.assert_rejected("Cannot resolve", env={"CURL_CA_BUNDLE": "/etc/ssl/certs/ca-certificates.crt"})

    def test_latest_redirect_must_identify_a_release(self):
        self.server.responses["/releases/latest"] = (200, {}, b"Not a redirect")
        self.assert_rejected("Unexpected release redirect")

    def test_invalid_options_fail_before_network_access(self):
        for args in (("--version", "../../main"), ("--version",), ("--install-dir", "relative"), ("--unknown",)):
            with self.subTest(args=args):
                self.assert_rejected("cqs installer:", *args)
        self.assertEqual(self.server.requests, [])

    def test_insecure_mirror_is_rejected(self):
        self.assert_rejected("must be an HTTPS URL", env={"CQS_RELEASE_BASE_URL": "http://localhost/releases"})
        self.assertEqual(self.server.requests, [])

    def test_destination_directory_is_not_overwritten(self):
        self.destination.mkdir()
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Destination is a directory", result.stderr)
        self.assertTrue(self.destination.is_dir())

    def test_help_does_not_download_or_install(self):
        result = self.run_installer("--help")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Usage:", result.stdout)
        self.assertEqual(self.server.requests, [])
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":
    unittest.main()

"""Exercise the real curl HTTPS/redirect boundary using a local TLS fixture."""

import http.server
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import unittest

from shell_support import load_script, function_source, SOURCE


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run HTTPS execution tests in Linux/WSL")
    if not shutil.which("curl") or not shutil.which("openssl"):
        raise unittest.SkipTest("Native curl and openssl are required")


class HttpsDownloadTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix="cqs-https-test-")
        cls.addClassCleanup(cls.temp.cleanup)
        cls.root = Path(cls.temp.name)
        cls.cert, key = cls.root / "cert.pem", cls.root / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
            "-subj", "/CN=localhost", "-addext", "subjectAltName=DNS:localhost",
            "-keyout", str(key), "-out", str(cls.cert),
        ], check=True, capture_output=True)

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path == "/redirect":
                    self.send_response(302)
                    self.send_header("Location", "http://127.0.0.1:1/unsafe")
                    self.end_headers()
                    return
                payload = b'touch "$MARKER"\nexit 7\n'
                if self.path == "/syntax-error":
                    payload = b'touch "$MARKER"\nif\n'
                self.send_response(200)
                self.send_header("Content-Length", str(len(payload) + (100 if self.path == "/truncated" else 0)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        cls.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cls.cert, key)
        cls.server.socket = context.wrap_socket(cls.server.socket, server_side=True)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.addClassCleanup(cls.close_server)

    @classmethod
    def close_server(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.thread.join(timeout=5)

    def run_download(self, endpoint):
        marker = self.root / self.id().rsplit(".", 1)[-1]
        url = endpoint if endpoint.startswith("http:") else f"https://localhost:{self.server.server_port}{endpoint}"
        env = dict(os.environ, CURL_CA_BUNDLE=str(self.cert), MARKER=str(marker),
                   URL=url, NO_PROXY="localhost,127.0.0.1", no_proxy="localhost,127.0.0.1")
        result = subprocess.run(
            ["bash", "-c", load_script() + '\nrun_https_script "$URL"'],
            text=True, capture_output=True, env=env, timeout=10,
        )
        return result, marker

    def test_complete_script_executes_and_preserves_exit_status(self):
        result, marker = self.run_download("/complete")
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertTrue(marker.exists())

    def test_http_url_is_rejected(self):
        result, marker = self.run_download("http://127.0.0.1:1/script")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())

    def test_https_redirect_cannot_downgrade_to_http(self):
        result, marker = self.run_download("/redirect")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("disabled", result.stderr)
        self.assertFalse(marker.exists())

    def test_truncated_download_is_never_executed(self):
        result, marker = self.run_download("/truncated")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())

    def test_syntax_error_is_detected_before_any_execution(self):
        result, marker = self.run_download("/syntax-error")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(marker.exists())

    def test_remote_execution_entrypoints_use_https_helper(self):
        for name in ("bandwidth_test", "ip_test", "router_test", "performance_test", "docker_install"):
            with self.subTest(name=name):
                body = function_source(name)
                self.assertIn("run_https_script https://", body)
                self.assertNotRegex(body, r"(?:curl|wget)[^\n]*\|\s*(?:sudo\s+)?bash")
        self.assertNotIn("bash <(curl", SOURCE)

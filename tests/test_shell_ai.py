"""AI release validation and publication use real files and executable fixtures."""

import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import types
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(os.environ.get("CQS_SHELL_AI_SOURCE", ROOT / "cqs.sh"))


def implementation():
    source = SCRIPT.read_text(encoding="utf-8").split("3<<'CQS_AI_PY'\n", 1)[1].split("\nCQS_AI_PY\n", 1)[0]
    module = types.ModuleType("cqs_shell_ai")
    exec(compile(source, str(SCRIPT) + "::AI", "exec"), module.__dict__)
    return module


class AiValidationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ai = implementation()
        cls.arch = cls.ai.ARCHITECTURES["x86_64"]

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cqs-ai-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def release(self, tool="codex"):
        prefix = "rust-v" if tool == "codex" else "v"
        name = "codex-package-x86_64-unknown-linux-musl.tar.gz" if tool == "codex" else "pi-linux-x64.tar.gz"
        tag = prefix + "1.2.3"
        return {"tag_name": tag, "draft": False, "prerelease": False, "assets": [{
            "name": name, "browser_download_url": f"https://github.com/{self.ai.REPOSITORIES[tool]}/releases/download/{tag}/{name}",
            "digest": "sha256:" + "a" * 64, "size": 123}]}

    def test_release_requires_unique_official_asset_digest_and_size(self):
        for tool in ("pi", "codex"):
            self.assertEqual(self.ai.github_package(tool, self.arch, self.release(tool))["version"], "1.2.3")
        for change in ("duplicate", "url", "digest", "size", "draft", "prerelease", "version"):
            release = self.release()
            asset = release["assets"][0]
            if change == "duplicate":
                release["assets"].append(asset.copy())
            elif change == "url":
                asset["browser_download_url"] = "https://example.com/package"
            elif change == "digest":
                asset["digest"] = "sha256:invalid"
            elif change == "size":
                asset["size"] = True
            elif change == "version":
                release["tag_name"] = "rust-v../escape"
            else:
                release[change] = True
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.ai.github_package("codex", self.arch, release)

    def test_claude_manifest_and_download_are_verified(self):
        manifest = {"version": "1.2.3", "platforms": {"linux-x64": {"checksum": "a" * 64}}}
        package = self.ai.claude_package(self.arch, "1.2.3", manifest)
        self.assertEqual(package["url"], self.ai.CLAUDE_RELEASES + "/1.2.3/linux-x64/claude")
        manifest["version"] = "1.2.4"
        with self.assertRaises(ValueError):
            self.ai.claude_package(self.arch, "1.2.3", manifest)
        archive = self.root / "archive"
        archive.write_bytes(b"verified package")
        package.update(size=archive.stat().st_size, digest=hashlib.sha256(archive.read_bytes()).hexdigest())
        self.ai.validate_download(package, archive)
        archive.write_bytes(b"tampered package")
        with self.assertRaises(ValueError):
            self.ai.validate_download(package, archive)

    def archive(self, entries):
        archive = self.root / "package.tar.gz"
        with tarfile.open(archive, "w:gz") as output:
            for name, kind in entries:
                entry = tarfile.TarInfo(name)
                entry.type = kind
                if kind == tarfile.REGTYPE:
                    entry.size = 4
                    output.addfile(entry, io.BytesIO(b"data"))
                else:
                    entry.linkname = "/tmp/outside"
                    output.addfile(entry)
        return archive

    def test_archive_rejects_traversal_links_duplicates_and_devices(self):
        cases = [
            [("../outside", tarfile.REGTYPE)], [("/absolute", tarfile.REGTYPE)],
            [("a\\b", tarfile.REGTYPE)], [("a:b", tarfile.REGTYPE)],
            [("file", tarfile.SYMTYPE)], [("file", tarfile.LNKTYPE)], [("file", tarfile.CHRTYPE)],
            [("same", tarfile.REGTYPE), ("same", tarfile.REGTYPE)],
        ]
        for index, entries in enumerate(cases):
            target = self.root / str(index)
            target.mkdir()
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                self.ai.unpack(self.archive(entries), target)
        self.assertFalse((self.root / "outside").exists())

    def test_archive_extracts_regular_files_and_validates_elf_architecture(self):
        target = self.root / "unpacked"
        target.mkdir()
        self.ai.unpack(self.archive([("nested/file", tarfile.REGTYPE)]), target)
        self.assertEqual((target / "nested/file").read_bytes(), b"data")
        binary = self.root / "binary"
        header = bytearray(20)
        header[:7] = b"\x7fELF\x02\x01\x01"
        header[16:18] = (2).to_bytes(2, "little")
        header[18:20] = (62).to_bytes(2, "little")
        binary.write_bytes(header)
        self.ai.elf(binary, self.arch)
        with self.assertRaises(ValueError):
            self.ai.elf(binary, self.ai.ARCHITECTURES["aarch64"])


@unittest.skipUnless(sys.platform == "linux", "Atomic entrypoint tests require Linux/WSL")
class AiPublicationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ai = implementation()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cqs-ai-publish-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def executable(self, name, version):
        path = self.root / name
        path.write_text(f"#!/bin/sh\nprintf '%s\\n' 'fixture {version}'\n")
        path.chmod(0o755)
        return path

    def test_publication_preserves_old_entrypoint_and_rolls_back_bad_version(self):
        old = self.executable("old", "0.9.0")
        target = self.root / "bin/tool"
        target.parent.mkdir()
        target.symlink_to(old)
        wrong = self.executable("wrong", "1.0.0")
        with self.assertRaises(ValueError):
            self.ai.publish(wrong, target, "1.2.3")
        self.assertEqual(os.readlink(target), str(old))
        self.assertIn("0.9.0", subprocess.check_output([str(target), "--version"], text=True))
        ready = self.executable("ready", "1.2.3")
        self.ai.publish(ready, target, "1.2.3")
        self.assertEqual(os.readlink(target), str(ready))
        backups = list(target.parent.glob(".cqs-ai-backup-*/previous"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(os.readlink(backups[0]), str(old))
        self.assertEqual(old.read_text(), "#!/bin/sh\nprintf '%s\\n' 'fixture 0.9.0'\n")

    def test_bad_first_publication_removes_new_entrypoint(self):
        wrong = self.executable("wrong", "1.0.0")
        target = self.root / "bin/tool"
        with self.assertRaises(ValueError):
            self.ai.publish(wrong, target, "1.2.3")
        self.assertFalse(os.path.lexists(target))

    def test_lock_and_symlink_paths_are_rejected(self):
        state = self.root / "state"
        with self.ai.install_lock(state):
            with self.assertRaises(BlockingIOError):
                with self.ai.install_lock(state):
                    self.fail("Overlapping installation acquired lock")
        elsewhere = self.root / "elsewhere"
        elsewhere.mkdir()
        linked = self.root / "linked"
        linked.symlink_to(elsewhere)
        with self.assertRaises(ValueError):
            self.ai.check_launcher(linked / "tool")

    def test_sudo_install_is_rejected_before_network_or_filesystem_changes(self):
        with patch.dict(os.environ, {"SUDO_USER": "ordinary-user"}), patch.object(self.ai.os, "geteuid", return_value=0):
            with self.assertRaisesRegex(ValueError, "sudo"):
                self.ai.install("claude")

    def test_confirmation_cancellation_does_not_publish(self):
        home = self.root / "home"
        home.mkdir()
        package = {"tool": "claude", "arch": self.ai.ARCHITECTURES[self.ai.platform.machine()],
                   "version": "1.2.3", "url": "https://downloads.claude.ai/fixture",
                   "size": 1, "digest": "a" * 64}
        environment = {"HOME": str(home), "CQS_STATE_DIR": str(self.root / "state"), "SUDO_USER": "",
                       "BASH_FUNC_cqs_confirm%%": "() { return 130; }"}
        with patch.dict(os.environ, environment), patch.object(self.ai, "resolve", return_value=package):
            with self.assertRaises(subprocess.CalledProcessError) as result:
                self.ai.install("claude")
        self.assertEqual(result.exception.returncode, 130)
        self.assertFalse((home / ".local/bin/claude").exists())

    def test_install_uses_real_elf_and_runs_both_version_checks(self):
        compiler = shutil.which("cc")
        if not compiler:
            self.skipTest("A C compiler is required for the executable release fixture")
        binary = self.root / "fixture"
        subprocess.run([compiler, "-x", "c", "-o", str(binary), "-"],
                       input='#include <stdio.h>\nint main(void) { puts("fixture 1.2.3"); return 0; }\n',
                       text=True, check=True, capture_output=True)
        home = self.root / "home"
        home.mkdir()
        package = {"tool": "claude", "arch": self.ai.ARCHITECTURES[self.ai.platform.machine()],
                   "version": "1.2.3", "url": "https://downloads.claude.ai/fixture",
                   "size": binary.stat().st_size, "digest": hashlib.sha256(binary.read_bytes()).hexdigest()}
        # Isolate the network boundary only; hashes, ELF parsing, process execution,
        # locking, files, symlinks, fsync and publication remain real.
        def fixture_download(_url, destination, _timeout):
            shutil.copyfile(binary, destination)
        environment = {"HOME": str(home), "CQS_STATE_DIR": str(self.root / "state"), "SUDO_USER": "",
                       "BASH_FUNC_cqs_confirm%%": "() { return 0; }"}
        with patch.dict(os.environ, environment), patch.object(self.ai, "resolve", return_value=package), \
                patch.object(self.ai, "download", side_effect=fixture_download), contextlib.redirect_stdout(io.StringIO()) as output:
            self.ai.install("claude")
        installed = home / ".local/bin/claude"
        self.assertTrue(installed.is_symlink())
        self.assertEqual(subprocess.check_output([str(installed), "--version"], text=True).strip(), "fixture 1.2.3")
        self.assertEqual(output.getvalue().count("fixture 1.2.3"), 2)


if __name__ == "__main__":
    unittest.main()

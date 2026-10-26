"""Linux recovery checks using temporary files and real .deb/hash parsers.

Kernel package installation is never run: package-manager calls are explicit
failure sentinels. Terminal tests write only beneath a temporary HOME.
"""

import base64
import hashlib
import json
import os
from pathlib import Path
import shlex
import stat
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "cqs.sh"


def release_fixture(version="7.3.0", arch="amd64", profile="standard"):
    prefix = "x86_64" if arch == "amd64" else "arm64"
    suffix = "-max" if profile == "max" else ""
    tag = f"{prefix}-{version}{suffix}"
    assets = []
    for kind in ("image", "headers"):
        name = f"linux-{kind}-{version}-joeyblog-bbrv3{suffix}_{version}-1_{arch}.deb"
        assets.append({"name": name, "digest": "sha256:" + "a" * 64,
                       "browser_download_url": f"https://github.com/byJoey/Actions-bbr-v3/releases/download/{tag}/{name}"})
    return {"tag_name": tag, "draft": False, "prerelease": False, "assets": assets}


@unittest.skipUnless(sys.platform == "linux", "Run recovery tests in Linux/WSL")
class ShellRecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-recovery-")
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
                        CQS_STATE_DIR=str(self.state), CQS_TERMINAL_CONFIG=str(self.config),
                        CQS_KERNEL_LOCK=str(self.root / "kernel.lock"), CQS_YES="1", TEST_ROOT=str(self.root))

    def run_shell(self, body, **env):
        return subprocess.run(["bash", "-c", f"source {shlex.quote(str(SCRIPT))}\n" + body],
                              text=True, capture_output=True, timeout=20,
                              env=dict(self.env, **env), cwd=self.root)

    def manifest(self, releases, arch="amd64", profile="standard", requested=""):
        (self.root / "release.json").write_text(json.dumps(releases))
        return self.run_shell('bbrv3_release_manifest "$TEST_ROOT/release.json" "$ARCH" "$PROFILE" "$REQUESTED"',
                              ARCH=arch, PROFILE=profile, REQUESTED=requested)

    def test_kernel_manifest_selects_numeric_stable_architecture_and_profile(self):
        preview = release_fixture("9.0.0")
        preview["prerelease"] = True
        releases = [release_fixture("7.2.9"), release_fixture("7.3.0"),
                    release_fixture("8.0.0", profile="max"), release_fixture("8.0.0", arch="arm64"), preview]
        self.assertEqual(self.manifest(releases).stdout.splitlines()[0], "x86_64-7.3.0")
        self.assertEqual(self.manifest(releases, profile="max").stdout.splitlines()[0], "x86_64-8.0.0-max")
        self.assertEqual(self.manifest(releases, arch="arm64").stdout.splitlines()[0], "arm64-8.0.0")
        self.assertEqual(self.manifest(releases, requested="x86_64-7.2.9").stdout.splitlines()[0], "x86_64-7.2.9")
        self.assertNotEqual(self.manifest(releases, requested="x86_64-6.0.0").returncode, 0)

    def test_kernel_manifest_requires_explicit_boolean_stability_fields(self):
        for field in ("draft", "prerelease"):
            for replacement in (None, 0, "", "missing"):
                with self.subTest(field=field, replacement=replacement):
                    release = release_fixture()
                    if replacement == "missing":
                        del release[field]
                    else:
                        release[field] = replacement
                    self.assertNotEqual(self.manifest(release).returncode, 0)

    def test_kernel_manifest_rejects_untrusted_and_duplicate_assets(self):
        for field, value in (("digest", "sha256:bad"), ("digest", None),
                             ("browser_download_url", "https://example.invalid/image.deb"),
                             ("name", "../../image.deb")):
            release = release_fixture()
            release["assets"][0][field] = value
            self.assertNotEqual(self.manifest(release).returncode, 0)
        release = release_fixture()
        release["assets"].append(dict(release["assets"][0]))
        self.assertNotEqual(self.manifest(release).returncode, 0)

    def build_packages(self):
        release = "7.3.0-joeyblog-bbrv3"
        for kind in ("image", "headers"):
            package = self.root / kind
            (package / "DEBIAN").mkdir(parents=True)
            (package / "DEBIAN/control").write_text(
                f"Package: linux-{kind}-{release}\nVersion: 7.3.0-1\nArchitecture: amd64\n"
                "Maintainer: Test <test@example.invalid>\nDescription: Test fixture, never installed\n")
            if kind == "image":
                (package / "boot").mkdir()
                (package / f"boot/config-{release}").write_text("\n".join(
                    f"CONFIG_{symbol}=y" for symbol in ("TCP_CONG_BBR", "NET_SCH_FQ", "HYPERV",
                    "HYPERV_VMBUS", "HYPERV_STORAGE", "HYPERV_NET", "PCI_HYPERV", "BLK_DEV_SD")) + "\n")
            subprocess.run(["dpkg-deb", "--build", "--root-owner-group", str(package), str(self.root / f"{kind}.deb")],
                           check=True, capture_output=True)

    def test_real_deb_metadata_architecture_and_corruption_validation(self):
        self.build_packages()
        valid = self.run_shell('bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 amd64')
        self.assertEqual(valid.returncode, 0, valid.stderr)
        self.assertNotEqual(self.run_shell('bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 arm64').returncode, 0)
        (self.root / "image.deb").write_bytes(b"not a Debian package")
        self.assertNotEqual(self.run_shell('bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 amd64').returncode, 0)

    def test_real_deb_without_bbr_support_is_rejected(self):
        self.build_packages()
        config = self.root / "image/boot/config-7.3.0-joeyblog-bbrv3"
        config.write_text(config.read_text().replace("CONFIG_TCP_CONG_BBR=y", "CONFIG_TCP_CONG_BBR=n"))
        subprocess.run(["dpkg-deb", "--build", "--root-owner-group", str(self.root / "image"), str(self.root / "image.deb")],
                       check=True, capture_output=True)
        result = self.run_shell('bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 amd64')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("BBR", result.stderr)

    def installer_fixture(self, corrupt=False):
        self.build_packages()
        release = release_fixture()
        for asset, kind in zip(release["assets"], ("image", "headers")):
            asset["digest"] = "sha256:" + hashlib.sha256((self.root / f"{kind}.deb").read_bytes()).hexdigest()
        if corrupt:
            release["assets"][0]["digest"] = "sha256:" + "0" * 64
        (self.root / "release.json").write_text(json.dumps([release]))
        return r'''
bbrv3_host_preflight() { :; }
curl() {
    local arg previous="" output="" url=""
    for arg in "$@"; do
        [[ "$previous" != -o && "$previous" != --output ]] || output="$arg"
        [[ "$arg" != https:* ]] || url="$arg"
        previous="$arg"
    done
    case "$url" in
        https://api.github.com/*) command cp "$TEST_ROOT/release.json" "$output" ;;
        */linux-image-*) command cp "$TEST_ROOT/image.deb" "$output" ;;
        */linux-headers-*) command cp "$TEST_ROOT/headers.deb" "$output" ;;
        *) return 99 ;;
    esac
}
bbrv3_backup_grub() { printf '%s\n' "$TEST_ROOT/boot-backup"; }
bbrv3_restore_grub() { touch "$TEST_ROOT/RESTORE_ATTEMPTED"; }
apt-get() { touch "$TEST_ROOT/INSTALL_ATTEMPTED"; return 99; }
'''

    def test_bad_digest_stops_before_install(self):
        result = self.run_shell(self.installer_fixture(corrupt=True) + "\nbbrv3_install_kernel standard\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256", result.stderr)
        self.assertFalse((self.root / "INSTALL_ATTEMPTED").exists())

    def test_cancel_after_real_package_validation_has_no_install_side_effect(self):
        result = self.run_shell(self.installer_fixture() + r'''
cqs_confirm() { [[ "$2" != INSTALL ]] || return 130; }
bbrv3_install_kernel standard
''')
        self.assertEqual(result.returncode, 130, result.stderr)
        self.assertIn("SHA-256", result.stdout)
        self.assertFalse((self.root / "INSTALL_ATTEMPTED").exists())
        self.assertFalse((self.root / "RESTORE_ATTEMPTED").exists())

    def test_install_failure_enters_recovery_and_is_never_reported_as_success(self):
        result = self.run_shell(self.installer_fixture() + "\nbbrv3_install_kernel standard\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.root / "INSTALL_ATTEMPTED").exists())
        self.assertTrue((self.root / "RESTORE_ATTEMPTED").exists())
        self.assertIn("暂勿重启", result.stderr)

    def test_running_kernel_cannot_be_reinstalled(self):
        result = self.run_shell(self.installer_fixture() + '\nuname() { echo 7.3.0-joeyblog-bbrv3; }; bbrv3_install_kernel standard\n')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "INSTALL_ATTEMPTED").exists())

    def test_shared_kernel_lock_is_held_before_package_preflight(self):
        import fcntl
        with (self.root / "kernel.lock").open("w") as locked:
            fcntl.flock(locked, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for operation in ("bbrv3_install_kernel standard", "bbrv3_remove_kernel 7.3.0-joeyblog-bbrv3"):
                with self.subTest(operation=operation):
                    result = self.run_shell('bbrv3_host_preflight() { touch "$TEST_ROOT/PREFLIGHT_ENTERED"; return 99; }; ' + operation)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertFalse((self.root / "PREFLIGHT_ENTERED").exists())

    def test_backup_refuses_symlink_directory_before_writing(self):
        destination = self.root / "elsewhere"
        destination.mkdir()
        link = self.root / "linked-backup"
        link.symlink_to(destination, target_is_directory=True)
        result = self.run_shell("bbrv3_backup_grub", CQS_KERNEL_BACKUP=str(link))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(list(destination.iterdir()), [])

    def test_backup_directory_and_metadata_are_private(self):
        backup = self.root / "backups"
        backup.mkdir(mode=0o777)
        backup.chmod(0o777)
        result = self.run_shell("umask 022; bbrv3_backup_grub", CQS_KERNEL_BACKUP=str(backup))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(stat.S_IMODE(backup.stat().st_mode), 0o700)
        snapshot = Path(result.stdout.strip())
        self.assertEqual(stat.S_IMODE((snapshot / "running-kernel").stat().st_mode), 0o600)
        for marker in snapshot.glob("no-*"):
            self.assertEqual(stat.S_IMODE(marker.stat().st_mode), 0o600)

    def test_incomplete_grub_snapshot_cannot_report_success(self):
        backup = self.root / "backup"
        backup.mkdir()
        (backup / "grub.cfg").write_text("temporary test menu\n")
        boot = self.root / "boot"
        boot.mkdir()
        (boot / "grub.cfg").write_text("existing menu\n")
        result = self.restore_grub(backup, boot)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual((boot / "grub.cfg").read_text(), "existing menu\n")

    def restore_grub(self, backup, boot):
        # Verify that this production helper supports isolated boot targets before
        # calling it; an old implementation must never ignore the second argument.
        definition = self.run_shell("declare -f bbrv3_restore_grub")
        self.assertEqual(definition.returncode, 0, definition.stderr)
        self.assertIn('${2:-/boot/grub}', definition.stdout,
                      "Update the fixture isolation before running this restore implementation")
        return self.run_shell('bbrv3_restore_grub "$GRUB_BACKUP" "$GRUB_BOOT"',
                              GRUB_BACKUP=str(backup), GRUB_BOOT=str(boot))

    def test_grub_restore_replaces_both_files_and_retains_saved_modes(self):
        backup = self.root / "backup"
        boot = self.root / "boot"
        backup.mkdir()
        boot.mkdir()
        for name, data, mode in (("grub.cfg", b"saved menu\n", 0o640),
                                 ("grubenv", b"saved environment\n", 0o600)):
            (backup / name).write_bytes(data)
            (backup / name).chmod(mode)
            (boot / name).write_bytes(b"changed\n")
        result = self.restore_grub(backup, boot)
        self.assertEqual(result.returncode, 0, result.stderr)
        for name, data, mode in (("grub.cfg", b"saved menu\n", 0o640),
                                 ("grubenv", b"saved environment\n", 0o600)):
            self.assertEqual((boot / name).read_bytes(), data)
            self.assertEqual(stat.S_IMODE((boot / name).stat().st_mode), mode)
            self.assertEqual((backup / name).read_bytes(), data)
        self.assertEqual(sorted(path.name for path in boot.iterdir()), ["grub.cfg", "grubenv"])

    def test_grub_restore_absence_marker_removes_only_saved_absent_grubenv(self):
        backup = self.root / "backup"
        boot = self.root / "boot"
        backup.mkdir()
        boot.mkdir()
        (backup / "grub.cfg").write_bytes(b"saved menu\n")
        (backup / "no-grubenv").touch()
        (boot / "grub.cfg").write_bytes(b"new menu\n")
        (boot / "grubenv").write_bytes(b"new environment\n")
        (boot / "keep").write_bytes(b"unrelated\n")
        result = self.restore_grub(backup, boot)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((boot / "grub.cfg").read_bytes(), b"saved menu\n")
        self.assertFalse((boot / "grubenv").exists())
        self.assertEqual((boot / "keep").read_bytes(), b"unrelated\n")
        self.assertTrue((backup / "no-grubenv").is_file())

    def test_terminal_preserves_first_baseline_modes_and_restores_absent_files(self):
        original = b"# keep\nexport KEEP=1"
        self.bashrc.write_bytes(original)
        self.bashrc.chmod(0o640)
        result = self.run_shell("terminal_config apply both full cyan off on")
        self.assertEqual(result.returncode, 0, result.stderr)
        baseline = json.loads(self.baseline.read_bytes())
        self.assertEqual(set(baseline), {"paths", "files"})
        self.assertEqual(base64.b64decode(baseline["files"]["bash"]["data"]), original)
        self.assertEqual(baseline["files"]["bash"]["mode"], 0o640)
        self.assertIsNone(baseline["files"]["zsh"])
        result = self.run_shell("terminal_config apply zsh compact green on off")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.baseline.read_bytes()), baseline)
        # Adding a managed block introduces a separator newline; disabling just
        # that shell removes the block, while restore uses the exact before-image.
        self.assertEqual(self.bashrc.read_bytes(), original + b"\n")
        result = self.run_shell("terminal_config restore")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.bashrc.read_bytes(), original)
        self.assertEqual(stat.S_IMODE(self.bashrc.stat().st_mode), 0o640)
        self.assertFalse(self.zshrc.exists())
        self.assertFalse(self.config.exists())
        self.assertFalse(self.baseline.exists())

    def inject_terminal_failure(self, target, signal_failure=False):
        hooks = self.root / "hooks"
        hooks.mkdir()
        (hooks / "sitecustomize.py").write_text('''
import os
from pathlib import Path
import signal
original_replace = os.replace
def replace(source, destination, *args, **kwargs):
    if str(destination) == os.environ.get("FAIL_TARGET"):
        marker = Path(os.environ["FAIL_MARKER"])
        if not marker.exists():
            marker.touch()
            if os.environ.get("FAIL_SIGNAL") == "1":
                original_replace(source, destination, *args, **kwargs)
                os.kill(os.getpid(), signal.SIGTERM)
                return
            raise OSError("injected atomic write failure")
    return original_replace(source, destination, *args, **kwargs)
os.replace = replace
''')
        return {"PYTHONPATH": str(hooks), "FAIL_TARGET": str(target),
                "FAIL_MARKER": str(self.root / "failed"), "FAIL_SIGNAL": "1" if signal_failure else "0"}

    def test_terminal_partial_apply_failure_restores_real_files(self):
        self.bashrc.write_bytes(b"original bash\n")
        self.zshrc.write_bytes(b"original zsh\n")
        result = self.run_shell("terminal_config apply both full cyan off on", **self.inject_terminal_failure(self.zshrc))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("injected atomic write failure", result.stderr)
        self.assertEqual(self.bashrc.read_bytes(), b"original bash\n")
        self.assertEqual(self.zshrc.read_bytes(), b"original zsh\n")
        self.assertFalse(self.baseline.exists())
        self.assertFalse(self.config.exists())
        self.assertFalse((self.state / "terminal/pending.json").exists())

    def test_terminal_signal_cancellation_rolls_back_and_returns_130(self):
        self.bashrc.write_bytes(b"original bash\n")
        self.zshrc.write_bytes(b"original zsh\n")
        result = self.run_shell("terminal_config apply both full cyan off on", **self.inject_terminal_failure(self.zshrc, signal_failure=True))
        self.assertEqual(result.returncode, 130, result.stderr)
        self.assertEqual(self.bashrc.read_bytes(), b"original bash\n")
        self.assertEqual(self.zshrc.read_bytes(), b"original zsh\n")
        self.assertFalse(self.baseline.exists())
        self.assertFalse((self.state / "terminal/pending.json").exists())

    def test_terminal_pending_and_lock_block_status_and_mutation(self):
        directory = self.state / "terminal"
        directory.mkdir(parents=True)
        pending = directory / "pending.json"
        pending.write_bytes(b"{}")
        for action in ("status", "apply bash full cyan off on", "restore"):
            self.assertNotEqual(self.run_shell("terminal_config " + action).returncode, 0)
        pending.unlink()
        result = self.run_shell('exec 9>"$CQS_STATE_DIR/terminal/config.lock"; flock -n 9; terminal_config status')
        self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_terminal_rejects_unknown_baseline_fields_and_saved_fields(self):
        self.bashrc.write_bytes(b"original\n")
        self.assertEqual(self.run_shell("terminal_config apply bash full cyan off on").returncode, 0)
        baseline = self.baseline.read_bytes()
        for kind in ("top", "saved"):
            data = json.loads(baseline)
            if kind == "top":
                data["unknown"] = True
            else:
                data["files"]["bash"]["unknown"] = True
            self.baseline.write_text(json.dumps(data))
            before = self.bashrc.read_bytes()
            result = self.run_shell("terminal_config restore")
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertEqual(self.bashrc.read_bytes(), before)
            self.baseline.write_bytes(baseline)

    def test_terminal_existing_state_directory_becomes_private(self):
        directory = self.state / "terminal"
        directory.mkdir(parents=True)
        directory.chmod(0o777)
        result = self.run_shell("terminal_config apply bash full cyan off on")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)


if __name__ == "__main__":
    unittest.main()

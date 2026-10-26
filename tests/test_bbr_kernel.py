"""Kernel management validation; package installation and reboot are never run."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

from shell_support import load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run kernel management tests in Linux/WSL")


def release_fixture(version="7.3.0", arch="amd64", profile="standard"):
    prefix = "x86_64" if arch == "amd64" else "arm64"
    suffix = "-max" if profile == "max" else ""
    tag = f"{prefix}-{version}{suffix}"
    assets = []
    for kind in ["image", "headers"]:
        name = f"linux-{kind}-{version}-joeyblog-bbrv3{suffix}_{version}-1_{arch}.deb"
        assets.append({"name": name, "digest": "sha256:" + "a" * 64,
                       "browser_download_url": f"https://github.com/byJoey/Actions-bbr-v3/releases/download/{tag}/{name}"})
    return {"tag_name": tag, "draft": False, "prerelease": False, "assets": assets}


class KernelManagementTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-kernel-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.metadata = self.root / "releases.json"
        self.definitions = load_script() + '\nCQS_KERNEL_LOCK="$TEST_ROOT/kernel.lock"\n'

    def run_shell(self, body, stdin="", **settings):
        env = dict(os.environ, TEST_ROOT=str(self.root))
        env.update(settings)
        return subprocess.run(
            ["bash", "-c", "red= white=\n" + self.definitions + "\n" + body],
            input=stdin, text=True, capture_output=True, env=env, timeout=20,
        )

    def manifest(self, releases, arch="amd64", profile="standard", requested=""):
        self.metadata.write_text(json.dumps(releases))
        return self.run_shell(
            'bbrv3_release_manifest "$TEST_ROOT/releases.json" "$ARCH" "$PROFILE" "$REQUESTED"',
            ARCH=arch, PROFILE=profile, REQUESTED=requested,
        )

    def test_latest_selection_separates_architecture_profile_and_prereleases(self):
        preview = release_fixture("9.0.0")
        preview["prerelease"] = True
        releases = [release_fixture("7.2.9"), release_fixture("7.3.0"),
                    release_fixture("8.0.0", profile="max"), release_fixture("8.0.0", arch="arm64"), preview]
        result = self.manifest(releases)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines()[0], "x86_64-7.3.0")
        self.assertEqual(len(result.stdout.splitlines()), 3)
        self.assertEqual(self.manifest(releases, profile="max").stdout.splitlines()[0], "x86_64-8.0.0-max")
        self.assertEqual(self.manifest(releases, arch="arm64").stdout.splitlines()[0], "arm64-8.0.0")

    def test_explicit_version_does_not_fall_back_to_latest(self):
        releases = [release_fixture("7.2.9"), release_fixture("7.3.0")]
        self.assertEqual(self.manifest(releases, requested="x86_64-7.2.9").stdout.splitlines()[0], "x86_64-7.2.9")
        self.assertNotEqual(self.manifest(releases, requested="x86_64-6.0.0").returncode, 0)

    def test_single_release_response_and_api_errors(self):
        self.assertEqual(self.manifest(release_fixture()).returncode, 0)
        result = self.manifest({"message": "API rate limit exceeded"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rate limit", result.stderr)

    def test_manifest_rejects_missing_hash_untrusted_url_and_mismatched_packages(self):
        for field, replacement in [
            ("digest", None), ("digest", "sha256:bad"),
            ("browser_download_url", "https://example.com/image.deb"),
            ("name", "../../image.deb"),
        ]:
            with self.subTest(field=field, replacement=replacement):
                release = release_fixture()
                release["assets"][0][field] = replacement
                self.assertNotEqual(self.manifest(release).returncode, 0)
        release = release_fixture()
        release["assets"].pop()
        self.assertNotEqual(self.manifest(release).returncode, 0)

    def test_libc_and_debug_packages_are_excluded(self):
        release = release_fixture()
        for name in ["linux-libc-dev_7.3.0-1_amd64.deb", "linux-image-7.3.0-joeyblog-bbrv3-dbg_7.3.0-1_amd64.deb"]:
            release["assets"].append({"name": name})
        result = self.manifest(release)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("libc", result.stdout)
        self.assertNotIn("dbg", result.stdout)

    def test_distribution_minimum_versions(self):
        for distro, version, accepted in [
            ("debian", "12", True), ("debian", "13", True), ("debian", "11", False),
            ("ubuntu", "24.04", True), ("ubuntu", "22.04", False),
            ("alpine", "3.22", False), ("debian", "", False), ("debian", "sid", False),
        ]:
            with self.subTest(distro=distro, version=version):
                result = self.run_shell('bbrv3_supported_system "$DISTRO" "$VERSION"', DISTRO=distro, VERSION=version)
                self.assertEqual(result.returncode == 0, accepted)

    def test_running_or_unrelated_kernel_cannot_be_removed(self):
        running = "6.12.0-amd64"
        third_party = "7.3.0-joeyblog-bbrv3"
        installed = f"linux-image-{running}\nlinux-image-{third_party}"
        for release, active, packages, accepted in [
            (third_party, running, installed, True),
            (third_party, third_party, installed, False),
            (running, third_party, installed, False),
            ("7.3.0-joeyblog-bbrv3-max", running, installed, False),
            (third_party, running, f"linux-image-{third_party}", False),
            ("--help", running, installed, False),
        ]:
            with self.subTest(release=release, active=active):
                result = self.run_shell('bbrv3_removable_kernel "$RELEASE" "$ACTIVE" "$INSTALLED"',
                                        RELEASE=release, ACTIVE=active, INSTALLED=packages)
                self.assertEqual(result.returncode == 0, accepted)

    def build_packages(self, architecture="amd64"):
        release = "7.3.0-joeyblog-bbrv3"
        for kind in ["image", "headers"]:
            package = self.root / kind
            (package / "DEBIAN").mkdir(parents=True)
            (package / "DEBIAN/control").write_text(
                f"Package: linux-{kind}-{release}\nVersion: 7.3.0-1\nArchitecture: {architecture}\n"
                "Maintainer: Test <test@example.invalid>\nDescription: Test fixture, never installed\n"
            )
            if kind == "image":
                (package / "boot").mkdir()
                (package / f"boot/config-{release}").write_text("\n".join(
                    f"CONFIG_{symbol}=y" for symbol in ["TCP_CONG_BBR", "NET_SCH_FQ", "HYPERV",
                    "HYPERV_VMBUS", "HYPERV_STORAGE", "HYPERV_NET", "PCI_HYPERV", "BLK_DEV_SD"]
                ) + "\n")
            subprocess.run(["dpkg-deb", "--build", "--root-owner-group", str(package),
                            str(self.root / f"{kind}.deb")], check=True, capture_output=True)

    def test_native_deb_metadata_and_architecture_validation(self):
        self.build_packages()
        result = self.run_shell('bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 amd64')
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.run_shell('bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 arm64')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("元数据", result.stderr)

    def test_corrupt_deb_is_rejected(self):
        (self.root / "image.deb").write_text("not a deb")
        self.assertNotEqual(self.run_shell(
            'bbrv3_validate_packages "$TEST_ROOT" 7.3.0-joeyblog-bbrv3 amd64'
        ).returncode, 0)

    def installer_fixture(self, corrupt=False):
        self.build_packages()
        release = release_fixture()
        for asset, kind in zip(release["assets"], ["image", "headers"]):
            asset["digest"] = "sha256:" + hashlib.sha256((self.root / f"{kind}.deb").read_bytes()).hexdigest()
        if corrupt:
            release["assets"][0]["digest"] = "sha256:" + "0" * 64
        self.metadata.write_text(json.dumps([release]))
        return r'''
# Only the environment gate and network are substituted. Checksums and .deb
# inspection use the real Linux tools; apt is an explicit failure sentinel.
bbrv3_host_preflight() { return 0; }
curl() {
    local arg previous="" output="" url=""
    for arg in "$@"; do
        [[ "$previous" != -o ]] || output="$arg"
        [[ "$arg" != https:* ]] || url="$arg"
        previous="$arg"
    done
    case "$url" in
        https://api.github.com/*) cp "$TEST_ROOT/releases.json" "$output" ;;
        */linux-image-*) cp "$TEST_ROOT/image.deb" "$output" ;;
        */linux-headers-*) cp "$TEST_ROOT/headers.deb" "$output" ;;
        *) return 99 ;;
    esac
}
apt-get() { touch "$TEST_ROOT/INSTALL_WAS_ATTEMPTED"; return 99; }
bbrv3_install_kernel standard
'''

    def test_download_verification_can_be_cancelled_before_any_install(self):
        result = self.run_shell(self.installer_fixture(), stdin="y\nno\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("SHA-256", result.stdout)
        self.assertIn("两个安装包", result.stdout)
        self.assertFalse((self.root / "INSTALL_WAS_ATTEMPTED").exists())

    def test_bad_download_digest_stops_before_any_install(self):
        result = self.run_shell(self.installer_fixture(corrupt=True), stdin="y\nINSTALL\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("SHA-256 校验失败", result.stderr)
        self.assertFalse((self.root / "INSTALL_WAS_ATTEMPTED").exists())

    def test_running_release_cannot_be_reinstalled(self):
        script = self.installer_fixture().replace(
            "bbrv3_install_kernel standard\n",
            "uname() { echo 7.3.0-joeyblog-bbrv3; }\nbbrv3_install_kernel standard\n",
        )
        result = self.run_shell(script, stdin="y\nINSTALL\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("拒绝覆盖正在运行", result.stderr)
        self.assertFalse((self.root / "INSTALL_WAS_ATTEMPTED").exists())

    def test_install_and_remove_share_a_real_lock(self):
        import fcntl
        script = self.installer_fixture()
        with (self.root / "kernel.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for body in (script, "bbrv3_host_preflight() { :; }; bbrv3_remove_kernel",
                         "require_root() { :; }; clear() { :; }; system_clean",
                         "require_root() { :; }; clear() { :; }; system_update"):
                with self.subTest(body=body[-30:]):
                    result = self.run_shell(body, stdin="y\nINSTALL\n")
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("另一个内核操作", result.stderr)
        self.assertFalse((self.root / "INSTALL_WAS_ATTEMPTED").exists())

    def test_remove_cannot_interleave_with_install_rollback(self):
        import fcntl
        script = self.installer_fixture().replace("bbrv3_install_kernel standard\n", r'''
bbrv3_backup_grub() { printf '%s\n' "$TEST_ROOT/backup"; }
bbrv3_restore_grub() {
    touch "$TEST_ROOT/ROLLBACK_STARTED"
    for ((i=0; i<500; i++)); do
        [[ ! -e "$TEST_ROOT/ALLOW_ROLLBACK" ]] || return 0
        sleep 0.01
    done
    return 1
}
bbrv3_install_kernel standard
''')
        env = dict(os.environ, TEST_ROOT=str(self.root))
        process = subprocess.Popen(
            ["bash", "-c", self.definitions + "\n" + script], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
        )
        try:
            process.stdin.write("y\nINSTALL\n")
            process.stdin.flush()
            deadline = time.monotonic() + 4
            while not (self.root / "ROLLBACK_STARTED").exists():
                if process.poll() is not None or time.monotonic() > deadline:
                    self.fail("installer did not enter the rollback boundary")
                time.sleep(0.01)
            result = self.run_shell(r'''
bbrv3_host_preflight() { :; }
bbrv3_list_kernels() { touch "$TEST_ROOT/REMOVE_ENTERED"; return 99; }
bbrv3_remove_kernel
''')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("另一个内核操作", result.stderr)
            self.assertFalse((self.root / "REMOVE_ENTERED").exists())
        finally:
            (self.root / "ALLOW_ROLLBACK").touch()
            process.communicate(timeout=10)
        self.assertNotEqual(process.returncode, 0)
        with (self.root / "kernel.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_package_install_failure_restores_grub_and_returns_failure(self):
        script = self.installer_fixture().replace(
            "bbrv3_install_kernel standard\n", r'''
bbrv3_backup_grub() { printf '%s\n' "$TEST_ROOT/backup"; }
bbrv3_restore_grub() { touch "$TEST_ROOT/RESTORE_WAS_ATTEMPTED"; }
bbrv3_install_kernel standard
''')
        result = self.run_shell(script, stdin="y\nINSTALL\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue((self.root / "INSTALL_WAS_ATTEMPTED").exists())
        self.assertTrue((self.root / "RESTORE_WAS_ATTEMPTED").exists())
        self.assertIn("暂勿重启", result.stderr)

    def test_boot_validation_failure_is_not_reported_as_success(self):
        script = self.installer_fixture().replace(
            "bbrv3_install_kernel standard\n", r'''
bbrv3_backup_grub() { printf '%s\n' "$TEST_ROOT/backup"; }
bbrv3_restore_grub() { touch "$TEST_ROOT/RESTORE_WAS_ATTEMPTED"; }
apt-get() { return 0; }
bbrv3_boot_ready() { echo 'initramfs validation failed' >&2; return 1; }
bbrv3_install_kernel standard
''')
        result = self.run_shell(script, stdin="y\nINSTALL\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("initramfs validation failed", result.stderr)
        self.assertTrue((self.root / "RESTORE_WAS_ATTEMPTED").exists())

    def test_container_environment_is_rejected_before_boot_or_package_changes(self):
        result = self.run_shell(r'''
require_root() { return 0; }
command() { if [[ "$1" == -v ]]; then return 0; fi; builtin command "$@"; }
systemd-detect-virt() { echo lxc; return 0; }
bbrv3_host_preflight
''')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("容器/WSL", result.stderr)

    def test_missing_grub_backup_is_not_reported_as_restored(self):
        result = self.run_shell('bbrv3_restore_grub "$TEST_ROOT/missing"')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("备份不完整", result.stderr)

    def test_uninstall_plan_that_removes_other_packages_is_rejected(self):
        result = self.run_shell(r'''
bbrv3_host_preflight() { return 0; }
bbrv3_list_kernels() { :; }
uname() { echo 6.12.0-amd64; }
dpkg-query() {
    if [[ "$*" == *linux-headers-* ]]; then return 1; fi
    if [[ "$*" == *'${Status}'* ]]; then echo 'install ok installed'; return 0; fi
    printf 'ii\tlinux-image-6.12.0-amd64\nii\tlinux-image-7.3.0-joeyblog-bbrv3\n'
}
apt-get() {
    if [[ "$1" == -s ]]; then echo 'Remv important-system-package [1.0]'; return 0; fi
    touch "$TEST_ROOT/REMOVE_WAS_ATTEMPTED"
    return 99
}
bbrv3_remove_kernel
''', stdin="7.3.0-joeyblog-bbrv3\nREMOVE\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("额外软件包", result.stderr)
        self.assertFalse((self.root / "REMOVE_WAS_ATTEMPTED").exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)

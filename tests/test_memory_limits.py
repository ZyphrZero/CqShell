"""Real cgroup path parsing against a synthetic proc/mount tree."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from shell_support import load_script


def setUpModule():
    if sys.platform != "linux":
        raise unittest.SkipTest("Run cgroup tests in Linux/WSL")


class MemoryLimitTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cqs-memory-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.proc = self.root / "proc"
        (self.proc / "self").mkdir(parents=True)
        (self.proc / "meminfo").write_text("MemTotal:       8388608 kB\n")
        self.mount = self.root / "cgroup with space"
        self.mount.mkdir()

    def hierarchy(self, kind, membership, root="/"):
        v2 = kind == "cgroup2"
        (self.proc / "self/cgroup").write_text(f'0::{membership}\n' if v2 else f'5:memory:{membership}\n')
        mount = str(self.mount).replace(" ", r"\040")
        options = "rw" if v2 else "rw,memory"
        (self.proc / "self/mountinfo").write_text(f"29 23 0:26 {root} {mount} rw - {kind} cgroup {options}\n")

    def calculate(self):
        return subprocess.run(
            ["bash", "-c", load_script() + '''
memory=$(network_memory_kb "$PROC") || exit 1
printf '%s\n' "$memory"
calculate_tcp_buffer 10000 200 "$memory"
'''], text=True, capture_output=True, timeout=10, env=dict(os.environ, PROC=str(self.proc)),
        )

    def test_v2_ancestor_limit_caps_buffer_despite_unlimited_leaf(self):
        self.hierarchy("cgroup2", "/tenant/app")
        (self.mount / "tenant/app").mkdir(parents=True)
        (self.mount / "memory.max").write_text("max\n")
        (self.mount / "tenant/memory.max").write_text("268435456\n")
        (self.mount / "tenant/app/memory.max").write_text("max\n")
        result = self.calculate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["262144", "8388608"])

    def test_v1_mount_root_and_ancestor_limit(self):
        self.hierarchy("cgroup", "/tenant/app", root="/tenant")
        (self.mount / "app").mkdir()
        (self.mount / "memory.limit_in_bytes").write_text("134217728\n")
        (self.mount / "app/memory.limit_in_bytes").write_text("9223372036854771712\n")
        result = self.calculate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), ["131072", "4194304"])

    def test_namespaced_root_reads_root_limit(self):
        self.hierarchy("cgroup2", "/")
        (self.mount / "memory.max").write_text("536870912\n")
        self.assertEqual(self.calculate().stdout.splitlines(), ["524288", "16777216"])

    def test_no_memory_controller_uses_physical_memory(self):
        (self.proc / "self/cgroup").write_text("3:cpu:/user\n")
        (self.proc / "self/mountinfo").write_text("")
        self.assertEqual(self.calculate().stdout.splitlines(), ["8388608", "67108864"])

    def test_unlimited_root_uses_physical_memory(self):
        self.hierarchy("cgroup2", "/")
        self.assertEqual(self.calculate().stdout.splitlines(), ["8388608", "67108864"])

    def test_invalid_limit_or_unresolvable_group_fails(self):
        self.hierarchy("cgroup2", "/")
        (self.mount / "memory.max").write_text("broken\n")
        self.assertNotEqual(self.calculate().returncode, 0)
        self.hierarchy("cgroup2", "/missing")
        self.assertNotEqual(self.calculate().returncode, 0)

    def test_group_path_cannot_escape_mount(self):
        self.hierarchy("cgroup2", "/../outside")
        self.assertNotEqual(self.calculate().returncode, 0)

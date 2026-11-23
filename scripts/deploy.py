#!/usr/bin/env python3
"""Build CqShell Linux release (via WSL on Windows or natively on Linux) and deploy atomically.

Supports two transports:
  1. Motty MCP saved session (--motty-session / .deploy.local.json)
  2. Standard SSH / SCP (--ssh user@host)

Personal defaults live in gitignored `.deploy.local.json` at the repo root.
"""

from __future__ import annotations

import argparse
import base64
import gzip
import hashlib
import json
import os
import platform
import shlex
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
LOCAL_CONFIG = REPO_ROOT / ".deploy.local.json"


def win_to_wsl_path(path: Path) -> str:
    resolved = str(path.resolve())
    if len(resolved) >= 2 and resolved[1] == ":":
        drive = resolved[0].lower()
        rest = resolved[2:].replace("\\", "/")
        return f"/mnt/{drive}{rest}"
    return resolved.replace("\\", "/")


def load_local_config() -> dict:
    if not LOCAL_CONFIG.is_file():
        return {}
    with LOCAL_CONFIG.open("r", encoding="utf-8") as fh:
        data = json.load(fh)
    if not isinstance(data, dict):
        raise SystemExit(f"Invalid JSON object in {LOCAL_CONFIG}")
    return data


def run_step(cmd: list[str], label: str) -> None:
    print(f"\n==> {label}")
    t0 = time.monotonic()
    res = subprocess.run(cmd)
    if res.returncode != 0:
        raise SystemExit(f"Step failed ({res.returncode}): {label}")
    print(f"    done in {time.monotonic() - t0:.1f}s")


def build_linux_binary(
    out_path: Path,
    wsl_distro: str,
    target_dir: str,
    do_fmt: bool,
    do_check: bool,
    skip_build: bool,
) -> Path:
    is_windows = platform.system() == "Windows"
    repo_posix = win_to_wsl_path(REPO_ROOT) if is_windows else str(REPO_ROOT)
    out_posix = win_to_wsl_path(out_path) if is_windows else str(out_path)
    bin_posix = f"{target_dir.rstrip('/')}/release/cqs"

    cmds: list[str] = [
        "set -euo pipefail",
        'export PATH="$HOME/.cargo/bin:$PATH"',
        f"export CARGO_TARGET_DIR={shlex.quote(target_dir)}",
        f"cd {shlex.quote(repo_posix)}",
    ]
    if do_fmt:
        cmds.append('echo "[fmt] cargo fmt --all" && cargo fmt --all')
    if do_check:
        cmds.append('echo "[fmt] cargo fmt --all --check" && cargo fmt --all --check')
        cmds.append(
            'echo "[clippy] cargo clippy --locked --all-targets -- -D warnings" '
            "&& cargo clippy --locked --all-targets -- -D warnings"
        )
        cmds.append('echo "[test] cargo test --locked" && cargo test --locked')
    if not skip_build:
        cmds.append(
            'echo "[build] cargo build --release --locked --bin cqs" '
            "&& cargo build --release --locked --bin cqs"
        )
    if do_check:
        cmds.append(
            f'echo "[test] tests/test_install.py" && '
            f"CQS_TEST_BINARY={shlex.quote(bin_posix)} python3 -m unittest tests/test_install.py"
        )
    cmds.append(f"test -f {shlex.quote(bin_posix)}")
    cmds.append(f"cp -f {shlex.quote(bin_posix)} {shlex.quote(out_posix)}")

    script = "\n".join(cmds)
    if is_windows:
        cmd = ["wsl", "-d", wsl_distro, "--", "bash", "-c", script]
        label = f"Build Linux release in WSL ({wsl_distro})"
    else:
        cmd = ["bash", "-c", script]
        label = "Build Linux release"

    run_step(cmd, label)
    if not out_path.is_file():
        raise SystemExit(f"Expected binary not found at {out_path}")
    return out_path


def deploy_via_motty(
    local_bin: Path,
    session: str,
    remote_path: str,
    motty_exe: str,
    motty_data_dir: str | None,
) -> None:
    raw = local_bin.read_bytes()
    local_sha = hashlib.sha256(raw).hexdigest()
    local_size = len(raw)

    print(f"\n==> Compress & upload via Motty session ({session})")
    t0 = time.monotonic()
    gz = gzip.compress(raw, compresslevel=9)
    b64 = base64.b64encode(gz).decode("ascii")
    chunk_size = 200_000
    chunks = [b64[i : i + chunk_size] for i in range(0, len(b64), chunk_size)]
    print(
        f"    binary={local_size:,} B  gzip={len(gz):,} B  chunks={len(chunks)}  sha256={local_sha[:16]}..."
    )

    cmd = [motty_exe, "mcp-serve"]
    if motty_data_dir:
        cmd.extend(["--data-dir", motty_data_dir])

    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    assert proc.stdin is not None and proc.stdout is not None and proc.stderr is not None
    req_id = 0

    def call(method: str, params: dict) -> dict:
        nonlocal req_id
        req_id += 1
        proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params}) + "\n"
        )
        proc.stdin.flush()
        while True:
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError("Motty MCP exited unexpectedly: " + proc.stderr.read())
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            if obj.get("id") == req_id:
                return obj

    def tool(name: str, args: dict) -> dict:
        res = call("tools/call", {"name": name, "arguments": args})
        result = res.get("result", {})
        if result.get("isError"):
            raise RuntimeError(f"Motty tool `{name}` failed: {result}")
        return result

    try:
        call(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "cqs-deploy", "version": "1.0"},
            },
        )
        proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
            + "\n"
        )
        proc.stdin.flush()

        stage_dir = f"/tmp/cqs-deploy-{os.getpid()}"
        remote_dir = os.path.dirname(remote_path) or "/usr/local/bin"
        remote_tmp = f"{remote_dir}/.{os.path.basename(remote_path)}.new.{os.getpid()}"

        tool(
            "terminal_exec",
            {"session": session, "command": f"rm -rf {shlex.quote(stage_dir)} && mkdir -p {shlex.quote(stage_dir)}"},
        )
        for idx, ch in enumerate(chunks):
            tool(
                "sftp_write",
                {
                    "session": session,
                    "path": f"{stage_dir}/part_{idx:03d}.b64",
                    "content": ch,
                    "overwrite": True,
                },
            )

        install_cmd = (
            f"set -euo pipefail; "
            f"cat {shlex.quote(stage_dir)}/part_*.b64 | base64 -d | gzip -d > {shlex.quote(stage_dir)}/cqs; "
            f"install -m 0755 {shlex.quote(stage_dir)}/cqs {shlex.quote(remote_tmp)}; "
            f"mv -f {shlex.quote(remote_tmp)} {shlex.quote(remote_path)}; "
            f"rm -rf {shlex.quote(stage_dir)}; "
            f"ls -la {shlex.quote(remote_path)}; "
            f"{shlex.quote(remote_path)} --version"
        )
        res = tool("terminal_exec", {"session": session, "command": install_cmd})
        chk = tool("sftp_checksum", {"session": session, "path": remote_path, "algorithm": "sha256"})
        digest_data = json.loads(chk["content"][0]["text"])
        if digest_data.get("bytes") != local_size or digest_data.get("digest") != local_sha:
            raise RuntimeError(f"Checksum mismatch: remote={digest_data} local=({local_size}, {local_sha})")

        print(f"    deployed in {time.monotonic() - t0:.1f}s")
        print(f"==> VERIFIED OK: {remote_path}")
        print(f"    BYTES : {digest_data['bytes']}")
        print(f"    SHA256: {digest_data['digest']}")
        stdout = res.get("structuredContent", {}).get("stdout", "").strip()
        if stdout:
            for line in stdout.splitlines():
                print(f"    {line}")
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)


def deploy_via_ssh(
    local_bin: Path,
    ssh_target: str,
    remote_path: str,
    port: int | None,
    identity: str | None,
) -> None:
    raw = local_bin.read_bytes()
    local_sha = hashlib.sha256(raw).hexdigest()
    local_size = len(raw)
    gz = gzip.compress(raw, compresslevel=9)

    print(f"\n==> Compress & upload via SSH ({ssh_target})")
    print(f"    binary={local_size:,} B  gzip={len(gz):,} B  sha256={local_sha[:16]}...")
    t0 = time.monotonic()

    remote_dir = os.path.dirname(remote_path) or "/usr/local/bin"
    remote_tmp = f"{remote_dir}/.{os.path.basename(remote_path)}.new.$$"
    remote_cmd = (
        f"set -euo pipefail; "
        f"gzip -d > {remote_tmp}; "
        f"chmod 0755 {remote_tmp}; "
        f"mv -f {remote_tmp} {shlex.quote(remote_path)}; "
        f"sha256sum {shlex.quote(remote_path)}; "
        f"stat -c '%s' {shlex.quote(remote_path)}; "
        f"{shlex.quote(remote_path)} --version"
    )
    ssh_cmd = ["ssh"]
    if port:
        ssh_cmd.extend(["-p", str(port)])
    if identity:
        ssh_cmd.extend(["-i", identity])
    ssh_cmd.extend([ssh_target, remote_cmd])

    res = subprocess.run(ssh_cmd, input=gz, capture_output=True)
    if res.returncode != 0:
        sys.stderr.write(res.stderr.decode("utf-8", "replace"))
        raise SystemExit(f"SSH deploy failed with exit code {res.returncode}")

    out_lines = res.stdout.decode("utf-8", "replace").strip().splitlines()
    if len(out_lines) < 3:
        raise RuntimeError(f"Unexpected remote output: {out_lines}")
    remote_sha = out_lines[0].split()[0]
    remote_size = int(out_lines[1].strip())
    version = out_lines[2].strip()
    if remote_sha != local_sha or remote_size != local_size:
        raise RuntimeError(
            f"Checksum mismatch: remote=({remote_size}, {remote_sha}) local=({local_size}, {local_sha})"
        )

    print(f"    deployed in {time.monotonic() - t0:.1f}s")
    print(f"==> VERIFIED OK: {ssh_target}:{remote_path}")
    print(f"    BYTES : {remote_size}")
    print(f"    SHA256: {remote_sha}")
    print(f"    VER   : {version}")


def parse_args() -> argparse.Namespace:
    cfg = load_local_config()
    parser = argparse.ArgumentParser(
        description="Build CqShell Linux release and deploy atomically with SHA-256 verification."
    )
    parser.add_argument("--fmt", action="store_true", help="Run `cargo fmt --all` before building")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Run fmt --check, clippy -D warnings, cargo test, and tests/test_install.py",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="Skip cargo build and deploy the existing release binary",
    )
    parser.add_argument(
        "--wsl-distro",
        default=cfg.get("wsl_distro", "debian13"),
        help="WSL distro name on Windows (default from .deploy.local.json or 'debian13')",
    )
    parser.add_argument(
        "--target-dir",
        default=cfg.get("target_dir", "/tmp/cqs-linux-target"),
        help="CARGO_TARGET_DIR inside Linux/WSL (default: /tmp/cqs-linux-target)",
    )
    parser.add_argument(
        "--remote-path",
        default=cfg.get("remote_path", "/usr/local/bin/cqs"),
        help="Remote install path (default: /usr/local/bin/cqs)",
    )
    parser.add_argument(
        "--ssh",
        default=cfg.get("ssh"),
        help="Deploy via standard SSH (e.g. root@example.com); overrides Motty if passed on CLI",
    )
    parser.add_argument("--port", type=int, default=cfg.get("port"), help="SSH port")
    parser.add_argument("--identity", default=cfg.get("identity"), help="SSH private key path")
    parser.add_argument(
        "--motty-session",
        default=cfg.get("motty_session"),
        help="Deploy via Motty MCP saved session ID",
    )
    parser.add_argument(
        "--motty-exe",
        default=cfg.get("motty_exe", "motty.exe"),
        help="Path to motty.exe when using --motty-session",
    )
    parser.add_argument(
        "--motty-data-dir",
        default=cfg.get("motty_data_dir"),
        help="Motty data directory when using --motty-session",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    cli_ssh = any(a == "--ssh" or a.startswith("--ssh=") for a in sys.argv[1:])
    cli_motty = any(
        a == "--motty-session" or a.startswith("--motty-session=") for a in sys.argv[1:]
    )

    use_ssh = bool(args.ssh) and (cli_ssh or not args.motty_session)
    use_motty = bool(args.motty_session) and (cli_motty or not use_ssh)
    if not use_ssh and not use_motty:
        raise SystemExit(
            "No deploy target configured. Pass `--ssh user@host` or `--motty-session <id>`, "
            f"or create `{LOCAL_CONFIG.name}`."
        )

    with tempfile.TemporaryDirectory(prefix="cqs-deploy-") as tmpdir:
        staged = Path(tmpdir) / "cqs"
        build_linux_binary(
            out_path=staged,
            wsl_distro=args.wsl_distro,
            target_dir=args.target_dir,
            do_fmt=args.fmt,
            do_check=args.check,
            skip_build=args.skip_build,
        )
        if use_ssh:
            deploy_via_ssh(
                local_bin=staged,
                ssh_target=args.ssh,
                remote_path=args.remote_path,
                port=args.port,
                identity=args.identity,
            )
        else:
            deploy_via_motty(
                local_bin=staged,
                session=args.motty_session,
                remote_path=args.remote_path,
                motty_exe=args.motty_exe,
                motty_data_dir=args.motty_data_dir,
            )


if __name__ == "__main__":
    main()

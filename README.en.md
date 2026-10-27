# CqShell

[简体中文](README.md)

A server toolbox for Debian and Ubuntu, available as a Rust/Ratatui application and a standalone `cqs.sh` Shell application, with Simplified Chinese / English action catalogs.

The Rust application does not load the Shell application or require Python. The Shell application uses Bash and embedded Python to execute the same operations independently. Native system tools and explicitly selected third-party benchmark/install scripts remain external dependencies.

## Shell version

Copy and run `cqs.sh` with **Bash 4.4+ and Python 3.11+** (available in Debian 12+ / Ubuntu 24.04+). No Rust compiler, Rust executable, or Node.js is required. Downloading requires curl and CA certificates.

Download the Shell version from the repository's `main` branch and start it:

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL \
  https://raw.githubusercontent.com/ZyphrZero/CqShell/main/cqs.sh \
  -o cqs.sh && bash cqs.sh --lang en-US
```

After downloading, run individual actions or explicitly install the global command:

```bash
bash cqs.sh list --json
bash cqs.sh run ai.status
sudo bash cqs.sh run bbr.enable --set qdisc=fq
sudo bash cqs.sh batch tcp.tune bbr.enable --set tcp.tune.rtt=80
sudo bash cqs.sh --install
```

Both implementations expose the same 55 action IDs, parameters, defaults and choices. Shell supports `--lang`, `--config`, `--locale-dir`, `--yes`, `languages`, `list`, `run` and `batch`. Its numbered menu uses `o` for optional optimization, `r` for local status, `n` for public IP/ISP and `l` for language selection. Ratatui mouse controls and its live dashboard remain specific to the Rust UI. Startup does not install a shortcut or contact public IP services.

Configuration paths, network/terminal baselines and pending recovery records are compatible. Both versions target `/usr/local/bin/cqs` for explicit `--install`; installing one replaces that command. Do not modify the same configuration concurrently. See [Shell implementation notes](docs/SHELL.md). The `install.sh` instructions below continue to install the Rust binary.

## Download and install

End users do not need Rust. Releases provide **x86_64 (AMD64) and ARM64** Linux binaries for Debian / Ubuntu. The installer detects the architecture, downloads over HTTPS, checks SHA-256, verifies the executable version, and defaults to `/root/cqs`. Use `--install-dir` to choose another directory.

Download the latest stable release by running the following on the target server:

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL \
  https://github.com/ZyphrZero/CqShell/releases/latest/download/install.sh \
  -o install.sh && sudo bash install.sh
sudo /root/cqs --lang en-US
```

Dependencies are Bash, curl, CA certificates, tar, gzip and coreutils. If needed, install download dependencies with `sudo apt-get update && sudo apt-get install -y curl ca-certificates tar gzip coreutils`.

Pin a version (including prereleases), or choose another directory:

```bash
sudo bash install.sh --version v0.1.0
sudo bash install.sh --install-dir /opt/cqs
bash install.sh --install-dir "$HOME/.local/bin"
export PATH="$HOME/.local/bin:$PATH"
```

For a prerelease-only project, download `install.sh` from that specific release. The default selects the latest stable release. Repeat installation to upgrade, or use `--version` to select an older release. The installer replaces only the executable, preserves application configuration, and runs no system tuning. Download, checksum or version failures leave an existing installation intact. Built-in translations are included in the binary.

Run the default installation using `/root/cqs` (with sudo for non-root users). Uninstall by deleting the installed file, for example `sudo rm -- /root/cqs`, or `cqs` in your selected directory. Configuration and recovery files are preserved. The program's own `--install` / `--uninstall` options manage the global `/usr/local/bin/cqs` command described below. You can also download the architecture-specific `.tar.gz` and `.sha256` assets from GitHub Releases, verify with `sha256sum --check`, extract, and run `./cqs/cqs` directly. See [release instructions](docs/RELEASING.md) for publishing and trusted mirrors.

## Build from source and run

With Rust 1.88+ and a C toolchain on Debian / Ubuntu:

```bash
cargo build --release --locked
./target/release/cqs --lang en-US
sudo ./target/release/cqs --install
cqs
```

Starting the interface does not install the command or change system settings. System changes require root. Prompt customization operates on the current HOME, including when using sudo.

Windows PowerShell can also build and open the interface. `cargo build` produces the executable but does not launch it:

```powershell
cargo build --release --locked
.\target\release\cqs.exe --lang en-US
```

The Windows artifact is `cqs.exe`, not a Linux binary. System actions remain limited to Debian / Ubuntu; build and run inside the target Linux environment or WSL for those actions. The Windows build supports UI development and CLI metadata checks.

Windows locks running executables. If rebuilding the same output reports access denied while removing `cqs.exe`, quit the existing interface with Q first. Debug and release builds live in `target\debug` and `target\release`, respectively.

The persistent dashboard shows host/OS/kernel, CPU, memory, system disk, swap, uptime, timezone, and Linux BBR/TCP state. Local measurements refresh in the background every three seconds. N explicitly queries public IPv4/IPv6 and ISP, then retains the results across pages; startup does not contact public IP services. Windows shows real local resources and marks Linux-only fields/actions unavailable.

Ordinary pages are single-action menus. Tab switches focus between categories and actions; arrows navigate, Enter opens an action, and Esc returns. O opens Optimization, the only page with checkboxes: Space selects, Enter configures and saves an item, and F5 reviews/runs the selected plan. R refreshes local metrics; G toggles the task log, F2 opens the full log, L switches language, and Q quits while idle. Mouse clicks select categories/items, toggle optimization checkboxes, and operate form fields/buttons.

Forms have explicit Continue / Save to selection / Cancel buttons. Tab moves focus, arrows edit choices/cursor, Enter advances, and Ctrl+Enter submits. Ordinary confirmations default to Cancel; sensitive confirmation tokens still require exact input. Esc cancels a running task after closing any modal view. Q / Ctrl+C requests cancellation and exits after cleanup, including from the full task log. Interactive third-party tools temporarily take over the terminal. Logs expand when needed, and cancelled operations are distinguished from failures. Layouts support 80×24 or larger terminals.

Version 0.1.1 fixes package upgrades hanging on hidden debconf/ucf dialogs. Built-in apt operations run noninteractively, retain locally edited configuration and disable dpkg's private PTY. needrestart lists restart requirements. Captured commands cannot read the TUI's controlling terminal, and complete ANSI control sequences are removed from logs. After interrupting an older version's upgrade, wait for apt/dpkg to exit and inspect `dpkg --audit` from a separate SSH session before completing pending configuration. Do not delete lock files.

## CLI

```bash
cqs list
cqs list --json
cqs run bbr.status
sudo cqs run bbr.enable --set qdisc=fq
sudo cqs run tcp.tune --set bandwidth=1000 --set rtt=80
sudo cqs run kernel.install --set profile=standard
cqs run terminal.apply --set shell=bash --set style=full
sudo cqs batch tcp.tune bbr.enable --set tcp.tune.rtt=80
```

Action IDs are language-independent. In `list --json`, `available` indicates that the action is implemented; execution still validates platform, permissions and parameters. `--yes` accepts operation confirmations automatically. `sudo cqs --uninstall` removes the installed command while preserving software and recovery files.

Use **Network → Enable BBR in the running kernel** to configure its BBR implementation, including BBRv3 when supplied by that kernel. TCP tuning is available on the same page. **Kernel → Install BBRv3 kernel** supports Standard / Max and the latest matching or a specified version. Reboot into the installed kernel, then inspect BBR status and configure the queue.

After installation and boot checks succeed, both Rust and Shell show the running kernel, target kernel and verification steps, then ask whether to reboot now. Cancel or leave the answer blank to keep the installation and run `sudo reboot` later. `--yes` never automatically approves a reboot; noninteractive runs only print manual instructions. After reconnecting, check the target kernel with `uname -r`; if the old kernel is still running, check the GRUB boot selection.

IP quality checks use [xykt/IPQuality](https://github.com/xykt/IPQuality): open **Tests → IP quality · xykt/IPQuality**, or run `cqs run test.ip --set ip_family=ipv4`. `ip_family` accepts `dual` (default), `ipv4`, or `ipv6`; `full_ip=on` explicitly reveals the full IP in the report. Output follows the Chinese/English UI language. Privacy mode (`-p`) disables online report upload, while checks still contact external providers. CqShell does not pass upstream `-y` or `-n`; dependency installation remains an upstream prompt. The pinned entry script is checksum- and syntax-checked before taking over the interactive Debian/Ubuntu terminal. See [integration details](docs/IPQUALITY.md).

## System mirrors

System management includes mirror benchmarking, switching through pinned LinuxMirrors Lite, and restoring the previous sources. Choose official, Tsinghua (`tuna`), USTC (`ustc`), Aliyun (`aliyun`), or `auto` to benchmark on the server and confirm the recommendation. Security repositories use official HTTPS endpoints. Failed changes restore the configuration; packages are not upgraded. See [mirror integration and recovery](docs/MIRRORS.md) for supported versions, custom-source restrictions and validation.

```bash
cqs run system.mirror.test
sudo cqs run system.mirror.apply --set mirror_site=auto
sudo cqs run system.mirror.restore
```

The standalone Shell version accepts the same commands after `bash cqs.sh`.

## Languages

Save `${XDG_CONFIG_HOME:-$HOME/.config}/cqs/config.toml`:

```toml
language = "en-US"
# locale_dir = "/absolute/path/to/locales"
```

Precedence: `--lang`, `CQS_LANG`, configuration, OS locale, English. An unsupported explicitly requested language is an error. External catalogs must have the same keys and placeholders as `locales/en-US.toml`; missing translations are not silently substituted. Built-in TOML catalogs are embedded in the binary.

```bash
cqs --locale-dir ./locales languages
```

## Operations and recovery

The **Tools** category includes individual installers for curl, wget, nano, unzip, tar, tmux, iftop, btop, gdu and fzf. **Install all / choose a package** defaults to `all`, including zsh and Starship components. Prompt configuration and restoration remain in **Prompt**.

```bash
sudo cqs run tools.tmux
sudo cqs run tools.install
sudo cqs run tools.install --set package=fzf
cqs run ai.pi.install
cqs run ai.claude.install
cqs run ai.codex.install
cqs run ai.status
```

The **Agent tools** category downloads official **Linux x86_64 / ARM64 standalone binaries** for all three tools. No Node.js/npm or remote install scripts are used. Pi retains its complete archive, including themes and WASM resources; Codex uses the complete `codex-package` with helper programs and sandbox resources; Claude Code uses its platform binary. Only curl, CA certificates and network access are needed for installation. Run as the intended user without `sudo`; direct root logins can install for root. TUI installs run with progress logs; unattended CLI installation is supported, for example `cqs --yes run ai.pi.install`.

The confirmation shows the stable version, source, SHA-256 and command path. Downloads are checked against GitHub Release asset digests for Pi/Codex and the official manifest for Claude Code. Size, ELF architecture, package layout and actual `--version` output must pass before switching the command. Extraction rejects escaping paths, links and special files. Failures are reported without falling back to npm.

Binaries and resources live in separate version directories under `~/.local/share/cqs/ai/<tool>/`. Commands default to `~/.local/bin`; Codex also respects `CODEX_INSTALL_DIR`. Shell configuration is unchanged: use the printed full path or add the command directory to PATH. Existing entrypoints are backed up as `.cqs-ai-backup-*/previous` next to the command and restored if verification after switching fails. Old versions and backups are retained. Use the same action to update. Status checks also support older Pi entrypoints under `PI_CODING_AGENT_DIR` / `~/.pi/agent/bin` and commands on PATH.

Credentials, sessions and project configuration are retained. Sign in through each tool. The tool itself needs no Node runtime; projects, extensions and MCP services may have their own dependencies.

Distribution sources checked on 2026-10-27: [Pi releases](https://github.com/earendil-works/pi/releases), [Claude Code binary integrity](https://code.claude.com/docs/en/setup#binary-integrity-and-code-signing), and [OpenAI Codex CLI](https://developers.openai.com/codex/cli).

Features include system overview, public IP/ISP lookup, package updates, cleanup, timezone, swap, fail2ban, BBR/TCP tuning, third-party kernel management, SSH configuration, Docker, prompt customization, and server benchmarks.

Configuration operations retain snapshots, locks, validation and recovery handling. Kernel installation supports Debian 12+ / Ubuntu 24.04+ on amd64 / arm64, with an independent kernel environment and GRUB. After installation, confirm a reboot or reboot manually later. WSL and containers cannot replace the host kernel; removal targets only a specified non-running kernel.

See [migration notes](docs/MIGRATION.md), [architecture/testing](docs/TECHNICAL.md), and [localization](docs/I18N.md). Existing network/prompt recovery baselines remain readable. Do not run old and new applications concurrently.

## Development

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Linux tests use Bash, curl, openssl, Python 3 for local TLS/download fixtures, cc to compile ELF fixtures, dpkg-deb and sysctl. They never install kernels or alter live services. Native sshd/dockerd parser tests are explicit opt-in checks. Real boots, external SSH access and container IPv6 connectivity require separate VM/hardware acceptance testing.

Branch pushes and pull requests run Rust checks and installer integration tests. Pushing a `v*` tag matching `Cargo.toml` runs those checks, builds both musl targets, and publishes a GitHub Release. Manual builds upload Actions artifacts only. See [release instructions](docs/RELEASING.md).

License: the existing [GNU GPL v3 text](LICENSE). Original project references and attribution remain in the [Chinese README](README.md#许可证与参考). Rust migration dated 2026-10-26.

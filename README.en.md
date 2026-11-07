# CqShell

[简体中文](README.md)

A Rust/Ratatui server toolbox for Debian and Ubuntu, with Simplified Chinese / English action catalogs.

Native system tools and explicitly selected third-party benchmark/install scripts remain external dependencies.

The `install.sh` instructions below install the Rust binary.

## Download and install

End users do not need Rust. Releases provide **x86_64 (AMD64) and ARM64** Linux binaries for Debian / Ubuntu. The installer detects the architecture, downloads over HTTPS, checks SHA-256, and verifies the executable version. When a terminal is available, choose `/usr/local/bin`, `$HOME/.local/bin`, or a custom directory; press Enter to keep `/usr/local/bin`. Without a terminal it uses `/usr/local/bin`, and `--install-dir` selects a directory directly.

Run these two lines on the target server to install the latest stable release. **To change the download gateway, edit only the address on the first line**:

```bash
GH_PROXY="https://gh-proxy.org"
curl -fsSL "${GH_PROXY%/}/https://raw.githubusercontent.com/ZyphrZero/CqShell/main/install.sh" | sudo bash -s -- --gh-proxy "$GH_PROXY"
```

A trailing `/` on the gateway address is optional. Remove `sudo` when already running as root. Choose a trusted HTTPS gateway: version lookup, archive and checksum downloads also use it. This option covers installation downloads and does not configure a system proxy.

Start CqShell after installation:

```bash
cqs --lang en-US
```

Dependencies are Bash, curl, CA certificates, tar, gzip and coreutils. If needed, install download dependencies with `sudo apt-get update && sudo apt-get install -y curl ca-certificates tar gzip coreutils`.

Append these options to the installation command above to select a version or installation directory:

| Purpose | Append |
| --- | --- |
| Pin a version, including when the gateway lacks API support or caches an older release | `--version v0.1.4` |
| Install into a specific directory | `--install-dir /opt/cqs` |
| Install for the current user (also remove `sudo`) | `--install-dir "$HOME/.local/bin"` |

For a user installation, add the directory to PATH with `export PATH="$HOME/.local/bin:$PATH"`.

For a prerelease-only project, download `install.sh` from that specific release. The default selects the latest stable release. Repeat installation to upgrade, or use `--version` to select an older release. The installer replaces only the executable, preserves application configuration, and runs no system tuning. Download, checksum or version failures leave an existing installation intact. Built-in translations are included in the binary.

Run `cqs` after the default installation, or `sudo cqs` for system changes. Remove the default command with `sudo cqs --uninstall`; for a custom installation, delete `cqs` in your selected directory. Configuration and recovery files are preserved. The program's own `--install` / `--uninstall` options also manage `/usr/local/bin/cqs`, as described below. You can also download the architecture-specific `.tar.gz` and `.sha256` assets from GitHub Releases, verify with `sha256sum --check`, extract, and run `./cqs/cqs` directly. See [release instructions](docs/RELEASING.md) for publishing and trusted mirrors.

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

The persistent dashboard has two side-by-side panels. System & resources shows CPU/memory/disk/swap gauges with host/OS/kernel, uptime and timezone; Network & proxy shows Linux BBR/TCP state, public IPv4/IPv6/ISP, terminal proxy and Mihomo status/version. Local measurements refresh in the background every three seconds or manually with R. Terminal proxy reflects the nonempty `http_proxy` / `https_proxy` / `all_proxy` variables (including uppercase variants) inherited when CqShell starts; it does not indicate Mihomo process state or connectivity. Mihomo independently reports not installed, stopped, running (PID), or unknown for the current CqShell data directory. The installed binary's `-v` output supplies its version, with a two-second coreutils `timeout` limit and no network request. Probe failures remain unknown with diagnostics rather than being reported as stopped or missing. N explicitly queries public IPv4/IPv6 and ISP, then retains the results across pages; startup does not contact public IP services. Windows shows real local resources and marks Linux-only fields, including Mihomo, and actions unavailable.

## TUI controls

Navigation has no category/action focus. Section headings are not selectable; each category remembers its selected action. Only Optimization has checkboxes. Mouse clicks select categories/items, exact checkbox clicks toggle selection, and the wheel operates the region beneath it. Overlays and the style panel own input and ignore background clicks.

| Key | Action |
| --- | --- |
| Up/Down, j/k, Home/End | Move through actions or to the first/last action |
| Left/Right, Tab/Shift+Tab | Switch category |
| 1–9, O/0 | Jump to numbered categories or pinned Optimization |
| Alt+Up/Down | Reorder ordinary categories while idle; save immediately |
| Enter | Open/configure/run an action; in Optimization configure or toggle selection |
| Space, X/F5, A | Optimization only: select, review/run the plan, or apply the recommended set |
| / | Search; typing edits, Up/Down/PgUp/PgDn select, Enter activates, Tab jumps, Esc closes |
| T | Style panel; Up/Down/j/k/Home/End or mouse previews, Enter saves, Esc restores |
| R / N | Refresh local metrics / query public IP |
| G / F2, PgUp/PgDn | Toggle inline / full log; scroll logs |
| L | Switch and save language |
| ? / F1 | Complete contextual key reference |
| Esc | Close the current layer or cancel a running task; no action while idle |
| Q / Ctrl+C | Quit, or cancel and quit after cleanup |

The plan shows current readings, expected changes and the equivalent CLI command. Batch progress retains each step's result and elapsed time until the checklist changes or a new task starts. The recommended set is package updates, standard cleanup, TCP tuning and BBR.

Forms have explicit Continue / Save to selection / Cancel buttons. Tab moves focus, arrows edit choices/cursor, Enter advances and submits from the last field, and Ctrl+Enter submits. Parameters that name an existing item (subscriptions, proxy groups and nodes, kernels to boot or remove) are never typed: the form reads the current items first and opens on a list where arrows move, typing filters, and Enter chooses; choosing the last one goes straight to confirmation. Left/Right also cycle through the items. A slow read shows a waiting dialog that Esc cancels, and when nothing is available a notice says why instead of opening an empty form. Ordinary confirmations default to Cancel; sensitive confirmation tokens still require exact input. Esc cancels a running task after closing any modal view. Q / Ctrl+C requests cancellation and exits after cleanup, including from the full task log. Interactive third-party tools temporarily take over the terminal. Logs expand when needed, and cancelled operations are distinguished from failures. When the terminal is too short for the log panel, the footer shows the task state and latest output, and a brief notice reports the outcome; language switches, category reordering and saved optimization items get the same notice. Resource gauges use the active theme's meter, warning and danger roles at the 70% and 90% thresholds, and the screen recedes behind dialogs. Layouts support 80×24 or larger terminals.

Version 0.1.1 fixes package upgrades hanging on hidden debconf/ucf dialogs. Built-in apt operations run noninteractively, retain locally edited configuration and disable dpkg's private PTY. needrestart lists restart requirements. Captured commands cannot read the TUI's controlling terminal, and complete ANSI control sequences are removed from logs. After interrupting an older version's upgrade, wait for apt/dpkg to exit and inspect `dpkg --audit` from a separate SSH session before completing pending configuration. Do not delete lock files.

Reversible Run-mode forms use Confirm execution and skip the generic prompt after review; irreversible forms Continue to that prompt. Specific confirmations still apply. 80×24 keeps all categories, the selected action, impact and primary hints visible. Meters use the theme's normal color below 70%, warning from 70%, and danger from 90%.

## Themes

Default `aurora` derives colors from the detected terminal background; unavailable detection uses classic dark colors and patterns. `cqs themes` and `cqs themes --json` list the eight built-ins and custom files.
Precedence is `--theme` > `CQS_THEME` > config.toml `theme` > aurora. The flag only affects the TUI. T previews immediately; Enter saves without changing other preferences, Esc restores, and a failed save restores the prior theme and reports the error. Flag/environment overrides apply again next launch.
Set `detect_background = false` to disable the query. Custom TOML files live in `$XDG_CONFIG_HOME/cqs/themes`, default `$HOME/.config/cqs/themes`; `--config` does not relocate them. See [theme format and validation](docs/THEMES.md).
Ambiguous-width Unicode can misalign on some terminals; use `--theme plain --lang en-US` for IPMI or serial consoles. Older releases reject `theme` and `detect_background`; remove both keys before downgrading.
Export JSON/HTML with `cargo run --release --example tui_snapshot -- target/previews/status.json 120 36 en-US aurora '#0b1119'`; backgrounds also accept `unavailable` and `disabled`.

## CLI

`cqs batch` asks for one generic confirmation before the batch. Interactive operation-specific confirmations remain per operation; existing `--yes` behavior is unchanged. Skipped steps are not reported as successes.

```bash
cqs list
cqs list --json
cqs run bbr.status
sudo cqs run bbr.enable --set qdisc=fq
sudo cqs run tcp.tune --set bandwidth=1000 --set rtt=80
sudo cqs run kernel.install --set profile=standard
cqs run terminal.apply --set shell=bash --set style=full
sudo cqs batch tcp.tune bbr.enable --set tcp.tune.rtt=80
cqs run clash.status
cqs run clash.start
cqs run clash.sub.add --set name=home --set url=https://example.invalid/sub --set use=on
cqs run clash.sub.list
cqs run clash.sub.update --set name=home
cqs run clash.sub.use --set name=home
cqs run clash.sub.remove --set name=home
eval "$(cqs proxy-env on --shell bash)"
eval "$(cqs proxy-env off --shell bash)"
```

### Mihomo core

The Rust application adds official Mihomo binary installation/update, configuration validation, start/stop/status, and subscription add/select/update/remove in the **Mihomo proxy** category. The default data directory is `${XDG_DATA_HOME:-$HOME/.local/share}/cqs/clash`; `CQS_CLASH_HOME`, `CQS_CLASH_DATA_DIR`, `CQS_CLASH_BINARY` and the other `CQS_CLASH_*` variables override paths. Runtime YAML, PID/process-start records and transaction recovery files live there.

The category now exposes 24 CLI/TUI actions, including controller configuration, policy group/node selection, secret management, the MetaCubeXD Web dashboard, bounded log viewing/following, TUN configuration, and systemd user/system management. Run `cqs --yes run clash.panel.install`, start Mihomo, then use `cqs run clash.panel.status` for the dashboard URL. Persistent settings require Mihomo to be stopped; TUN and systemd have explicit platform and privilege checks. See the [Mihomo integration guide (Chinese)](docs/MIHOMO.md) for commands, recovery behavior and remaining native Linux acceptance checks.

`cqs proxy-env` prints commands for the current parent shell. Bash/Zsh can use `eval "$(cqs proxy-env on)"`; Fish can use `source (cqs proxy-env on --shell fish | psub)`. `on` starts Mihomo when needed; `off` only clears proxy variables from the current shell and does not stop Mihomo. Subscription URLs must be HTTPS without credentials. Downloads pass YAML structure checks and, when Mihomo is installed, its `-t` validation. Multi-file subscription commits keep recovery before-images and roll back on failure.

For familiar `nelvko/clashctl` commands, initialize Bash with `eval "$(cqs clashctl shell-init --shell bash)"`, then use `clashctl on/off/start/stop/status`, `clashctl sub add/use/update/del/rename`, `clashctl node use`, and aliases such as `clashon` and `clashoff`. Use `--shell zsh` for Zsh or `cqs clashctl shell-init --shell fish | source` for Fish. Scripts may also call `cqs clashctl ...`; on/off specifically require parent-shell functions. Initialization does not edit shell configuration or import upstream data. This is a Rust-backed compatibility interface, not full behavioral parity: subscriptions require explicit stop/start, settings and upgrades require a stopped kernel, and conversion, delay testing, interactive selectors, Mixin editing and upstream script updates remain unsupported. Secrets are redacted and imported through `secret --file`, never positional arguments. See the [compatibility guide (Chinese)](docs/MIHOMO.md#clashctl-兼容命令).

Action IDs are language-independent. In `list --json`, `available` indicates that the action is implemented; execution still validates platform, permissions and parameters. `--yes` accepts operation confirmations automatically. `sudo cqs --uninstall` removes the installed command while preserving software and recovery files.

Use **Network → Enable BBR in the running kernel** to configure its BBR implementation, including BBRv3 when supplied by that kernel. TCP tuning is available on the same page. **Kernel → Install BBRv3 kernel** supports Standard / Max and the latest matching or a specified version. Reboot into the installed kernel, then inspect BBR status and configure the queue.

After installation and boot checks succeed, Rust shows the running kernel, target kernel and verification steps, then asks whether to reboot now. Cancel or leave the answer blank to keep the installation and run `sudo reboot` later. `--yes` never automatically approves a reboot; noninteractive runs only print manual instructions. After reconnecting, check the target kernel with `uname -r`; if the old kernel is still running, check the GRUB boot selection.

IP quality checks use [xykt/IPQuality](https://github.com/xykt/IPQuality): open **Tests → IP quality · xykt/IPQuality**, or run `cqs run test.ip --set ip_family=ipv4`. `ip_family` accepts `dual` (default), `ipv4`, or `ipv6`; `full_ip=on` explicitly reveals the full IP in the report. Output follows the Chinese/English UI language. Privacy mode (`-p`) disables online report upload, while checks still contact external providers. CqShell does not pass upstream `-y` or `-n`; dependency installation remains an upstream prompt. The pinned entry script is checksum- and syntax-checked before taking over the interactive Debian/Ubuntu terminal. See [integration details](docs/IPQUALITY.md).

## System mirrors

System management includes mirror benchmarking, switching through pinned LinuxMirrors Lite, and restoring the previous sources. Choose official, Tsinghua (`tuna`), USTC (`ustc`), Aliyun (`aliyun`), or `auto` to benchmark on the server and confirm the recommendation. Security repositories use official HTTPS endpoints. Failed changes restore the configuration; packages are not upgraded. See [mirror integration and recovery](docs/MIRRORS.md) for supported versions, custom-source restrictions and validation.

```bash
cqs run system.mirror.test
sudo cqs run system.mirror.apply --set mirror_site=auto
sudo cqs run system.mirror.restore
```

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

See [architecture/testing](docs/TECHNICAL.md) and [localization](docs/I18N.md). Existing network/prompt recovery baselines remain readable. Do not delete incomplete recovery records.

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

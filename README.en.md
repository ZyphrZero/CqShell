<div align="center">

# CqShell

**A modern Rust terminal toolbox (TUI & CLI) for Debian and Ubuntu servers**

[![Release](https://img.shields.io/github/v/release/ZyphrZero/CqShell?style=flat-square&color=5eead4)](https://github.com/ZyphrZero/CqShell/releases/latest)
[![Platform](https://img.shields.io/badge/Debian%20%7C%20Ubuntu-x86__64%20%26%20ARM64-7dd3fc?style=flat-square)](https://github.com/ZyphrZero/CqShell/releases)
[![Rust](https://img.shields.io/badge/Rust-1.88%2B%20%C2%B7%20Ratatui-f97316?style=flat-square)](Cargo.toml)
[![License](https://img.shields.io/github/license/ZyphrZero/CqShell?style=flat-square&color=a78bfa)](LICENSE)

[简体中文](README.md) | English

[Quick Install](#quick-install) · [Features](#feature-overview) · [Docker Workspace](#dedicated-docker-tui-workspace) · [TUI & Themes](#tui-controls--themes) · [CLI Automation](#cli--batch-automation) · [Community](#community--friend-links)

</div>

---

**CqShell (`cqs`)** is a single-binary Rust / Ratatui server operations toolbox for **Debian and Ubuntu**. It brings system maintenance, mirror benchmarking, BBRv3 kernel management, TCP tuning, SSH hardening, a dedicated Docker TUI workspace, full-stack Mihomo proxy management, standalone AI agent CLI installers, prompt customization, and server benchmarks into one responsive, recoverable bilingual TUI and scriptable CLI.

## Highlights

- **Single Static Binary**: Prebuilt **Linux x86_64 and ARM64** musl binaries with zero Rust, Python, or Node.js runtime requirements and embedded Simplified Chinese & English catalogs.
- **Modern Sidebar Shell TUI**: Dedicated Bento Overview dashboard, collapsible sidebar, fuzzy command palette (`/` / `Ctrl+K`), borderless plain-text copy view (`V`), full mouse/keyboard support, and 4 responsive breakpoints down to **`40×16`** (including mobile SSH clients such as iOS Termius).
- **Decoupled Themes & UI Styles**: **8 built-in color themes × 8 UI design languages**, automatic terminal light/dark detection, live preview drawer (`T`), transparent terminal background (`B`), reduced motion (`M`), and custom TOML themes.
- **Dedicated 6-Tab Docker TUI Workspace**: Launch directly via `cqs docker` with Focus / Embedded dual modes across Containers, Compose, Images, Volumes, Networks, and Daemon — powered by native CLI JSON output with zero idle daemon overhead.
- **97 Structured Operations**: 11 reorderable modules plus a pinned **One-Click Optimization** checklist sharing one validation, privilege, and execution engine across TUI and CLI.
- **Transactional Safety**: HTTPS-only downloads with SHA-256 verification, `flock` locks, config validation before commit, atomic replacements, before-image recovery journals, non-interactive `apt` safeguards, tiered confirmations, and zero implicit reboots.

---

## Quick Install

End users do not need Rust. `install.sh` detects CPU architecture, downloads over HTTPS, verifies SHA-256 and executable version, and interactively offers `/usr/local/bin`, `$HOME/.local/bin`, or a custom directory (press Enter to keep `/usr/local/bin`).

### Direct Install (GitHub)

```bash
curl -fsSL https://github.com/ZyphrZero/CqShell/releases/latest/download/install.sh | sudo bash
```

### Install via a Trusted GitHub Download Proxy

Edit only the first line to switch gateways (trailing `/` is optional; omit `sudo` if already root):

```bash
GH_PROXY="https://gh-proxy.org"
curl -fsSL "${GH_PROXY%/}/https://raw.githubusercontent.com/ZyphrZero/CqShell/main/install.sh" | sudo bash -s -- --gh-proxy "$GH_PROXY"
```

> [!NOTE]
> Choose a trusted HTTPS gateway. `--gh-proxy` covers version lookup, archive, and checksum downloads during installation without changing system proxy settings. You may also pass `--gh-proxy default` to use `https://gh-proxy.com/`.

### Launch, Update & Uninstall

```bash
cqs --lang en-US          # Launch interactive TUI (read-only / user-scoped actions)
sudo cqs --lang en-US     # Manage system-wide operations as root
cqs docker                # Open fullscreen Docker TUI workspace directly
sudo cqs update           # Verified atomic self-update to latest stable release
sudo cqs --uninstall      # Remove default command (preserves configs & recovery data)
```

<details>
<summary>Installer options, manual release download & building from source</summary>

#### Installer Dependencies & Flags

Requires `bash`, `curl`, `ca-certificates`, `tar`, `gzip`, and `coreutils` (`sudo apt-get update && sudo apt-get install -y curl ca-certificates tar gzip coreutils`).

Append options after `bash -s --`:

| Purpose | Append |
| --- | --- |
| Pin a specific release (including when a gateway lacks API support or for prereleases) | `--version v0.1.6` |
| Install into a custom absolute directory | `--install-dir /opt/cqs` |
| Install for the current user (also omit `sudo`) | `--install-dir "$HOME/.local/bin"` |

For per-user installs, add `export PATH="$HOME/.local/bin:$PATH"`. Re-running the installer or `sudo cqs update` upgrades in place while keeping configuration and recovery files intact on failure.

#### Manual Installation from GitHub Releases

```bash
sha256sum --check cqs-x86_64-unknown-linux-musl.tar.gz.sha256
tar -xzf cqs-x86_64-unknown-linux-musl.tar.gz
./cqs/cqs --lang en-US
```

#### Build from Source (Linux / Windows)

On Debian / Ubuntu with **Rust 1.88+** and a C toolchain:

```bash
cargo build --release --locked
./target/release/cqs --lang en-US
sudo ./target/release/cqs --install   # Explicitly install to /usr/local/bin/cqs
```

Windows PowerShell builds `cqs.exe` for UI preview/development and CLI metadata inspection (system actions remain Debian / Ubuntu only; quit an existing `cqs.exe` with `Q` before rebuilding the same target directory):

```powershell
cargo build --release --locked
.\target\release\cqs.exe --lang en-US
```

See [docs/RELEASING.md](docs/RELEASING.md) for publishing, CI workflows, and mirror details.

</details>

---

## Feature Overview

The sidebar pins **Overview (`H`)** and **Optimization (`O`)**, followed by 11 reorderable modules totaling **97** CLI/TUI actions:

| Module | Capabilities |
| --- | --- |
| **Optimization** | Checklist-based batch workflow; press `A` for the recommended preset (package updates + standard cleanup + TCP tuning + BBR) with step-by-step progress and timings |
| **System** | System overview, on-demand public IP/ISP lookup, non-interactive apt update & cleanup, **LinuxMirrors benchmark/switch/restore**, timezone, SWAP, `fail2ban` |
| **Network** | BBR status, 4 default qdiscs (`fq`, `fq_codel`, `fq_pie`, `cake`), bandwidth/RTT/cgroup-aware TCP buffer tuning & rollback, **GitHub acceleration proxy** |
| **Kernel** | BBRv3 Standard / Max kernel installation (latest matching or pinned `x.y.z`), installed kernel inventory, **one-time GRUB boot switch**, safe removal of non-running third-party kernels |
| **SSH** | Port updates with active listener verification (preserving `Match`/`Include`), public key import & password auth lockdown |
| **Docker** | **Dedicated 6-tab Docker TUI workspace** (Containers / Compose / Images / Volumes / Networks / Daemon) + install, registry mirrors, IPv6, cleanup & uninstall |
| **Tools** | Individual installers for `curl`, `wget`, `nano`, `unzip`, `tar`, `tmux`, `iftop`, `btop`, `gdu`, `fzf`, plus install-all |
| **Agent Tools** | Official standalone Linux binary install/update & status checks for **Pi (`pi.dev`)**, **Claude Code**, and **Codex CLI** (zero Node.js/npm) |
| **Prompt** | Starship + Bash/Zsh prompt setup with 7 styles × 9 palettes, live TUI/CLI preview, Starship TOML viewer, and first-baseline restore |
| **Mihomo Proxy** | 37 full-stack actions: core, HTTPS & local YAML subscriptions, 4 Web UIs, GeoData rule sets, hot-reload / mode / delay testing / connections, DNS / TUN / systemd & `clashctl` compatibility |
| **Tests** | Speedtest, **xykt/IPQuality** (privacy mode by default), NextTrace, YABS, connectivity & disk I/O benchmarks |
| **CqShell** | Update checks, verified atomic self-update, and command install/uninstall |

---

### Dedicated Docker TUI Workspace

Select **Docker** in the sidebar or launch directly with `cqs docker [--tab containers|compose|images|volumes|networks|daemon] [--embedded]`:

- **Embedded & Focus Modes**: Toggle fullscreen Focus mode with `z` / `F` (default when launched via `cqs docker`), switch tabs directly with `1–6`, and pull out the sidebar anytime with `[`.
- **End-to-End Operations**:
  - **Containers**: `s` start/stop, `u` restart, `p` pause, `e` / `Shift+E` interactive `docker exec -it`, `Space` multi-select batch actions, `l` live streaming & fullscreen logs, `m` live CPU/memory sparklines & `docker top`, `v` env/mounts with masked secrets (`Shift+S` to reveal).
  - **Compose**: Project/service topology, linked containers, aggregated logs, read-only YAML preview, and `Up / Stop / Restart / Pull / Down`.
  - **Images / Volumes / Networks / Daemon**: Image pull & layer history, one-key `g` registry mirror setup (curated presets, comma-separated multi-mirror failover, or `clear` to restore direct pulls), built-in network and mounted-volume guards, interactive tiered Prune wizard (`Shift+P`), and unified daemon controls alongside all transactional `docker.*` actions.
- **Zero Heavy Dependencies & Tiered Confirmations**: Built strictly on native `docker` / `docker compose` JSON output (compatible with Rootless Docker, `DOCKER_HOST`, and Contexts), sleeping background monitors automatically after 10 seconds idle. Volume removal, bulk deletes (≥3 items), and volume-including Prunes enforce typed confirmation (`REMOVE` / `PRUNE`).

---

### Mihomo Proxy & `clashctl` Compatibility

The **Mihomo Proxy** module implements 37 pure-Rust actions using `${XDG_DATA_HOME:-$HOME/.local/share}/cqs/clash` by default (overridable via `CQS_CLASH_*`):

- **Core & Transactional Subscriptions**: Verified official Mihomo binary installation, HTTPS subscriptions, and local YAML import (`clash.sub.import`), merging Mixin and validating with `mihomo -t` before atomic publication with before-image rollback.
- **4 Verified Web Dashboards**: **Zashboard** (default), **MetaCubeXD**, **Yacd-Meta**, and **Razord-Meta**, bound to loopback by default for safe SSH port-forwarding access.
- **Live Controller & GeoData**: Direct HTTP/1.1 controller integration for validated hot-reload (with rollback), instant `rule`/`global`/`direct` mode switching, concurrent proxy-group latency tests, active connection inspection/closing, and `MetaCubeX/meta-rules-dat` (`standard`/`lite`/`full`) GeoData updates.
- **Core Settings, DNS, TUN & systemd**: Transactional management of `mixed-port`, `allow-lan`, IPv6, domain sniffing, `fake-ip`/`redir-host` DNS presets, masked Mixin inspection/reset, TUN settings, and user/system systemd units.
- **Parent-Shell Proxy Env & `clashctl` Compatibility**:

```bash
# Inject or clear proxy variables in the current shell (bash / zsh / fish)
eval "$(cqs proxy-env on --shell bash)"
eval "$(cqs proxy-env off --shell bash)"

# Familiar nelvko/clashctl ergonomics without modifying shell RC files
eval "$(cqs clashctl shell-init --shell bash)"
clashctl on && clashctl status
clashctl node use 'Proxy' 'Node A'
```

See [docs/MIHOMO.md](docs/MIHOMO.md) (Chinese) for detailed commands, recovery behavior, and compatibility boundaries.

---

### Agent Tools, Common Tools & Prompt Customization

- **Standalone Agent Tools (Zero Node.js / npm)**: Installs official **Linux x86_64 / ARM64 standalone binaries** for **Pi (`pi.dev`)**, **Claude Code**, and **Codex CLI** without Node.js, npm, or remote install scripts. Verifies release metadata, SHA-256 digests, ELF architecture, package layout, and `--version` before atomically switching symlinks in `~/.local/bin` (versioned payloads live under `~/.local/share/cqs/ai/<tool>/`, keeping previous backups and leaving shell configs untouched). Run directly as the target user **without `sudo`** (direct root logins may install for root):

  ```bash
  cqs run ai.pi.install
  cqs run ai.claude.install
  cqs run ai.codex.install
  cqs run ai.status
  ```

- **Common Tools & Prompt**: Individual or batch installation for 10 CLI utilities, plus Starship prompt customization for Bash/Zsh across 7 styles (`full`, `compact`, `box`, `ribbon`, `powerline`, `minimal`, `brackets`) and 9 palettes (`cyan`, `green`, `magenta`, `tokyo-night`, `catppuccin`, `rose-pine`, `nord`, `gruvbox`, `dracula`) with live TUI/CLI previews, Starship TOML inspection, and first-baseline restore.

---

### System Mirrors, Kernel Tuning & Server Tests

- **System Mirrors (LinuxMirrors Lite)**: Benchmark and switch between official, Tsinghua (`tuna`), USTC (`ustc`), Aliyun (`aliyun`), or `auto` measured fastest mirror while preserving official security endpoints and one-step restoration ([docs/MIRRORS.md](docs/MIRRORS.md)).
- **BBR / BBRv3 & TCP Tuning**: Enable BBR and qdiscs on the running kernel, or install BBRv3 Standard/Max kernels with explicit reboot confirmation, one-time GRUB boot switching, and non-running kernel removal.
- **GitHub Acceleration Proxy**: Off by default; configure via TUI hotkey `P`, `cqs run github.proxy`, `--gh-proxy`, or `CQS_GH_PROXY`/`GH_PROXY` (preset `https://gh-proxy.com/`). Rewrites only canonical HTTPS GitHub hosts, falls back cleanly on `api.github.com` proxy rate limits, and never touches non-GitHub URLs or proxy subscriptions.
- **IP Quality Check (xykt/IPQuality)**: Supports dual-stack, IPv4-only, and IPv6-only checks matching the UI language, passes `-p` by default to disable online report uploads and mask full IPs, and verifies pinned script SHA-256 and Bash syntax before execution ([docs/IPQUALITY.md](docs/IPQUALITY.md)).

---

## TUI Controls & Themes

### Keybindings

| Key | Action |
| --- | --- |
| `↑↓` / `j k` / `Home` / `End` | Move through actions in the current module |
| `←→` / `PgUp` / `PgDn` | Switch in-module sections / scroll inspector & logs |
| `Tab` / `1–9` / `H` / `O` | Switch modules / jump by number / Overview dashboard / Optimization |
| `[` | Toggle sidebar (or floating navigation drawer on narrow/mobile terminals) |
| `/` or `Ctrl+K` | Open enhanced fuzzy command palette (actions, recents, UI commands) |
| `Enter` | Inspect, configure parameters, or execute |
| `Space` / `X` / `A` | Optimization checklist: toggle item / review & run plan / apply recommended preset |
| `Alt+↑` / `Alt+↓` | Reorder modules while idle (saved immediately) |
| `T` (`B` / `M` in drawer) | Open live Appearance drawer (toggle transparent background / reduced motion) |
| `P` | Configure GitHub acceleration proxy |
| `R` / `N` | Refresh local metrics / query public IP & ISP on demand |
| `C` / `Y` / `V` | Copy command or logs / open borderless plain-text view for native drag selection |
| `G` / `F2` | Toggle inline task log / open fullscreen log viewer |
| `L` | Switch and persist UI language |
| `?` / `F1` | Context-aware shortcut help |
| `Esc` / `Q` / `Ctrl+C` | Close modal or cancel running task / clean exit |

<details>
<summary>Docker workspace keybindings</summary>

| Key | Action |
| --- | --- |
| `←→` or `1–6` (Focus mode) | Switch Containers / Compose / Images / Volumes / Networks / Daemon |
| `j k` / `Space` / `/` / `a` / `o` | Navigate / multi-select / filter / toggle scope / cycle sort |
| `Enter` / `s` / `u` / `p` / `e` / `d` / `Shift+P` | Action menu / start-stop / restart / pause or pull / `exec` TTY / remove / Prune wizard |
| `i` / `l` / `m` / `v` / `J` / `Shift+S` | Summary / live logs / metrics & `top` / env & mounts / JSON view / reveal secrets |
| `f` / `t` / `w` / `c` / `Shift+M` / `y` | In logs: follow / timestamps / wrap / clear / selection mode / copy |
| `z` / `[` / `Ctrl+K` / `Shift+T` / `Shift+L` | Toggle fullscreen Focus / sidebar drawer / command palette / theme / language |

</details>

### Decoupled Themes & UI Styles

CqShell separates **`theme`** (color palette) from **`style`** (layout & chrome geometry). With `style = "auto"`, each built-in theme loads its signature design language, or you can mix and match freely via the `T` drawer or CLI flags:

| Theme ID | Default Style | Visual Character |
| --- | --- | --- |
| `auto` | `rounded` | Detects terminal background and derives WCAG-compliant surface layers |
| `obsidian` | `bento` | Deep dark borderless Bento cards separated by surface elevation & glow bar |
| `daylight` | `editorial` | Crisp light Swiss editorial typography with clean top rules and whitespace |
| `tokyo-night` | `hud` | Cyber HUD corner brackets, diamond glyphs, segmented meters & accents |
| `catppuccin` | `studio` | Warm dark surfaces with soft tinted header bands |
| `rose-pine` | `atelier` | Sharp right-angle frames, bracketed titles, and dotted rules |
| `high-contrast` | `heavy` | Pure black background, heavy borders, and inverted solid selection |
| `plain` | `ascii` | Default terminal colors & pure ASCII borders (IPMI / serial safe) |

- Precedence: `--theme` > `CQS_THEME` > `config.toml` > `auto`; `--style` > `CQS_STYLE` > `config.toml` > `auto`.
- Serial / IPMI console safe mode: `cqs --theme plain --lang en-US`.
- Custom TOML themes live in `$XDG_CONFIG_HOME/cqs/themes` (`~/.config/cqs/themes`). See [docs/THEMES.md](docs/THEMES.md).

### Configuration & Languages

Default config path: `${XDG_CONFIG_HOME:-$HOME/.config}/cqs/config.toml` (see [config.example.toml](config.example.toml)):

```toml
language = "en-US"
theme = "auto"
style = "auto"
transparent_bg = false
reduced_motion = false
sidebar_collapsed = false
detect_background = true
# locale_dir = "/absolute/path/to/locales"
# github_proxy_enabled = false
# github_proxy = "https://gh-proxy.com/"
```

Language precedence: `--lang` > `CQS_LANG` > `config.toml` > `LC_ALL` / `LC_MESSAGES` / `LANG` > English. External TOML catalogs are documented in [docs/I18N.md](docs/I18N.md).

---

## CLI & Batch Automation

Action IDs are language-independent. Use the CLI directly in non-interactive shells, with `--yes` / `-y` to auto-accept generic prompts:

```bash
# Discovery, appearance & self-updates
cqs list --json
cqs themes && cqs styles
cqs update --check --json
sudo cqs --gh-proxy default update

# Mirrors, BBR & TCP tuning
cqs run system.mirror.test
sudo cqs run system.mirror.apply --set mirror_site=auto
sudo cqs run bbr.enable --set qdisc=fq
sudo cqs run tcp.tune --set bandwidth=1000 --set rtt=80

# Batch execution (single upfront confirmation, ordered execution)
sudo cqs batch system.update tcp.tune bbr.enable --set tcp.tune.rtt=80

# Kernels & server benchmarks
cqs run kernel.list
sudo cqs run kernel.install --set profile=standard
cqs run test.ip --set ip_family=ipv4

# Tools & prompt previews
sudo cqs run tools.tmux
cqs run terminal.preview --set style=all --set color=tokyo-night
cqs run terminal.apply --set shell=bash --set style=box --set color=tokyo-night

# Mihomo proxy & Web dashboard
cqs run clash.status
cqs --yes run clash.sub.add --set name=home --set url=https://example.invalid/sub --set use=on
cqs --yes run clash.panel.install --set panel=zashboard
cqs --yes run clash.start
```

---

## Safety & Scope

- **Platform Boundaries**: System operations target **Debian / Ubuntu Linux**. BBRv3 kernel management requires Debian 12+ / Ubuntu 24.04+ (amd64 / arm64), an independent kernel environment, and GRUB (WSL and containers cannot replace the host kernel). Windows builds support TUI development and CLI metadata checks.
- **Non-Interactive `apt`**: Built-in package operations enforce `DEBIAN_FRONTEND=noninteractive`, `UCF_FORCE_CONFFOLD=1`, and disabled dpkg private PTYs to prevent hidden prompts from stalling upgrades. If a prior upgrade was interrupted externally, inspect `dpkg --audit` instead of removing lock files.
- **Recoverable Transactions**: Network, SSH, Docker, SWAP, mirror, Mihomo, and prompt mutations hold `flock` locks and write before-image / pending recovery journals. Preserve recovery files after unexpected power loss or interruption.
- **No Automatic Reboots**: Even with `--yes`, kernel installations never reboot automatically.

---

## Documentation & Development

| Document | Scope |
| --- | --- |
| [docs/TECHNICAL.md](docs/TECHNICAL.md) | Architecture, locks, transactional recovery & testing acceptance |
| [docs/THEMES.md](docs/THEMES.md) | Decoupled themes & styles, 20 color roles & custom theme authoring |
| [docs/MIHOMO.md](docs/MIHOMO.md) | Mihomo actions, Web panels, controller, TUN/systemd & `clashctl` compatibility |
| [docs/MIRRORS.md](docs/MIRRORS.md) | LinuxMirrors Lite integration scope & recovery |
| [docs/IPQUALITY.md](docs/IPQUALITY.md) | xykt/IPQuality integration, privacy defaults & checksum checks |
| [docs/I18N.md](docs/I18N.md) | TOML localization schema & adding new languages |
| [docs/RELEASING.md](docs/RELEASING.md) | Installer flags, GitHub download proxy, CI & release workflow |
| [CHANGELOG.md](CHANGELOG.md) | Full release changelog & version notes ([`docs/releases/`](docs/releases/)) |

### Local Verification

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Export real TUI render buffers to JSON / HTML for visual QA:

```bash
cargo run --release --example tui_snapshot -- target/previews/status.json 120 36 en-US obsidian '#0b1119' auto
```

---

## Credits & License

Licensed under the [GNU General Public License v3.0](LICENSE).

Upstream inspirations and integrations:

- [yuju520/YujuToolBox](https://github.com/yuju520/YujuToolBox) — foundational upstream inspiration
- [byJoey/Actions-bbr-v3](https://github.com/byJoey/Actions-bbr-v3) — BBRv3 reference and third-party kernel packages
- [kejilion/sh](https://github.com/kejilion/sh) — system management reference
- [nelvko/clashctl](https://github.com/nelvko/clashctl) — Mihomo proxy management and CLI ergonomics reference
- [xykt/IPQuality](https://github.com/xykt/IPQuality) — IP quality benchmark runner

---

## Community & Friend Links

- [**LINUX DO**](https://linux.do/) — A welcoming open-source & tech community built around sincerity, friendliness, unity, and professionalism.

# CqShell

[English](README.en.md)

面向 Debian / Ubuntu 的服务器工具箱，提供 Rust / Ratatui 版本和独立的 `cqs.sh` Shell 版本。集成系统维护、网络调优、内核管理、服务器测试、终端美化与 Docker 管理。

Rust 版的界面、操作说明、确认提示和应用错误使用 TOML 语言文件，内置 **简体中文与英语**。Rust 版不依赖 Shell 版或 Python；Shell 版使用 Bash 和内嵌 Python，独立执行同一套操作。系统工具和明确选择的第三方脚本仍是外部依赖。

## Shell 版本

`cqs.sh` 是可单独复制运行的脚本，要求 **Bash 4.4+、Python 3.11+**（Debian 12+ / Ubuntu 24.04+ 自带相应版本），不需要编译 Rust，也不需要 Node.js。下载需要 curl 和 CA 证书。

通过 curl 下载仓库 `main` 分支的 Shell 版本并启动：

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL \
  https://raw.githubusercontent.com/ZyphrZero/CqShell/main/cqs.sh \
  -o cqs.sh && bash cqs.sh --lang zh-CN
```

下载后也可以执行单项操作，或显式安装全局命令：

```bash
bash cqs.sh list --json
bash cqs.sh run ai.status
sudo bash cqs.sh run bbr.enable --set qdisc=fq
sudo bash cqs.sh batch tcp.tune bbr.enable --set tcp.tune.rtt=80
sudo bash cqs.sh --install     # 显式安装到 /usr/local/bin/cqs
```

Shell 版与 Rust 版提供相同的 55 个操作 ID、参数默认值与枚举，包括 AI 独立二进制安装、IPQuality、可选一键优化和终端还原。支持 `--lang`、`--config`、`--locale-dir`、`--yes`、`languages`、`list / run / batch`；启动不安装快捷命令、不查询公网信息。Shell 使用编号菜单，`o` 打开一键优化、`r` 读取本机状态、`n` 查询公网信息、`l` 切换语言；Ratatui 的鼠标、实时仪表盘与快捷键仅属于 Rust 界面。

配置路径、网络/终端恢复基线及未完成事务记录与 Rust 版兼容。两个版本共用全局 `cqs` 安装目标，`--install` 会替换该目标；不要并发修改同一系统配置。详情和验证命令见 [Shell 说明](docs/SHELL.md)。下方 `install.sh` 仍安装 Rust 二进制。

## 下载与安装

普通用户无需安装 Rust。发布版提供 Debian / Ubuntu 使用的 **x86_64（AMD64）和 ARM64** Linux 可执行文件，安装器自动选择架构、通过 HTTPS 下载并校验 SHA-256，默认安装到 `/root/cqs`，也可用 `--install-dir` 指定目录。

**以下下载入口在首个稳定版 GitHub Release 发布后可用。** 在目标服务器执行：

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL \
  https://github.com/ZyphrZero/CqShell/releases/latest/download/install.sh \
  -o install.sh && sudo bash install.sh
sudo /root/cqs --lang zh-CN
```

服务器需要 Bash、curl、CA 证书、tar、gzip 和 coreutils。缺少下载依赖时先执行 `sudo apt-get update && sudo apt-get install -y curl ca-certificates tar gzip coreutils`。

安装指定版本（包含预发布版本），或指定其他目录：

```bash
sudo bash install.sh --version v0.1.0
sudo bash install.sh --install-dir /opt/cqs
bash install.sh --install-dir "$HOME/.local/bin"
export PATH="$HOME/.local/bin:$PATH"
```

预发布版本的安装器可从对应 Release 手动下载；默认安装仅选择最新稳定版。重复执行安装命令即可升级，也可用 `--version` 安装指定旧版本。安装器只替换 `cqs` 程序，不修改应用配置、不执行系统优化；下载、校验或版本检查失败时保留原有程序。内置中英文翻译随程序分发，无需额外安装语言文件。

默认安装后通过 `/root/cqs` 运行；普通用户需加 `sudo`。卸载时删除实际安装的文件，例如 `sudo rm -- /root/cqs`；指定目录安装则删除该目录下的 `cqs`，保留配置与恢复材料。程序自身的 `--install` / `--uninstall` 管理的是 `/usr/local/bin/cqs` 全局命令，见下方源码构建说明。也可以直接在 GitHub Releases 下载对应架构的 `.tar.gz` 和 `.sha256` 文件：

```bash
sha256sum --check cqs-x86_64-unknown-linux-musl.tar.gz.sha256
tar -xzf cqs-x86_64-unknown-linux-musl.tar.gz
./cqs/cqs --lang zh-CN
```

构建、发布和镜像配置见 [发布说明](docs/RELEASING.md)。

## 从源码构建与启动

在 Debian / Ubuntu 中安装 Rust 1.88+ 及 C 编译工具链：

```bash
cargo build --release --locked
./target/release/cqs --lang zh-CN
./target/release/cqs --lang en-US
```

启动界面不会自动安装命令。需要安装到 `/usr/local/bin/cqs` 时显式执行：

```bash
sudo ./target/release/cqs --install
cqs
sudo cqs --uninstall
```

卸载命令保留软件、配置及恢复材料。普通用户可以浏览菜单、读取状态和配置自己 HOME 下的提示符；系统写操作要求 root。服务器通常通过 `sudo cqs --lang zh-CN` 管理。

Windows PowerShell 中也可以构建并查看界面。`cargo build` 只生成程序，不会自动启动：

```powershell
cargo build --release --locked
.\target\release\cqs.exe --lang zh-CN
```

Windows 构建产物是 `cqs.exe`，不能作为 Linux 二进制部署。系统操作仍仅支持 Debian / Ubuntu；需要执行这些操作时，请在目标 Linux 或 WSL 中编译并运行。Windows 版本可用于界面开发和 CLI 元数据检查。

Windows 会锁住正在运行的可执行文件。重新构建同一输出目录时，如遇到删除 `cqs.exe` 被拒绝，请先在旧界面按 Q 退出。调试版位于 `target\debug`，release 版位于 `target\release`。

## TUI 操作

顶部常驻主机、系统、内核、CPU、内存、系统盘、SWAP、运行时间、时区及 BBR/TCP 状态。本机指标每 3 秒后台刷新，单个指标失败不会阻断其余信息。公网 IPv4/IPv6 和 ISP 按 N 查询，查询结果在切换页面后继续保留；启动时不联网查询。Windows 会显示真实本机资源，Linux 专属字段标为“不适用”，不可执行的操作提前置灰。

| 按键 | 操作 |
| --- | --- |
| Tab / Shift+Tab | 在分类和操作列表之间切换焦点 |
| 上下键 / j / k | 在当前焦点中移动，各分类保留上次位置 |
| 右键 / Enter（分类焦点） | 进入该分类的操作列表 |
| 左键 / Esc | 返回分类；弹窗中返回上一层 |
| Enter（普通操作） | 打开详情、配置参数或单项执行 |
| O | 直接进入“一键优化” |
| 空格（仅一键优化） | 勾选 / 取消勾选，默认全部不选 |
| Enter（一键优化） | 配置项目并保存到清单；无参数项目切换勾选 |
| F5（仅一键优化） | 预览并执行整份清单，普通页面不触发批处理 |
| R / N | 刷新本机指标 / 后台查询公网 IP 与 ISP |
| G / F2 | 显示或收起最近任务日志 / 打开完整日志 |
| L | 切换本次会话语言 |
| PgUp / PgDn | 滚动日志或确认说明 |
| Esc | 执行中请求取消，等待清理和回退后留在界面；弹窗中先返回 |
| Q / Ctrl+C | 运行中请求取消，清理完成后退出；完整日志窗口也可使用 |

普通页面是单项菜单，只有“一键优化”使用复选框。鼠标点击分类/条目定位，点击优化页的复选框切换选择；表单字段、选项和按钮也支持点击，弹窗不会把点击穿透到后台菜单。

表单使用 Tab / 上下键切换字段，左右键选择枚举或移动文本光标，Home/End、Delete、Backspace 和 Ctrl+U 支持编辑。Enter 移到下一项，选择“继续/保存到清单”后提交，也可用 Ctrl+Enter。取消不会执行或保存参数。普通确认默认选择“取消”，可用方向键、鼠标或 Y/N 操作；RESTORE、REMOVE、KEY-TESTED 等特定确认仍要求准确输入文字。交互式第三方工具暂时接管终端，退出后返回 TUI。

日志只在执行任务后展开，取消与失败分别显示；常驻指标不进入执行日志。界面支持 80×24 及以上终端，窄窗口将操作与说明上下排列。构建后可用 `cargo run --release --example tui_snapshot -- target/previews/status.json 120 36 zh-CN` 导出基于实际本机数据的渲染单元，用于布局检查。

`v0.1.1` 修复了系统升级期间 debconf/ucf 隐藏对话框导致的卡住问题。内置 apt 操作使用非交互模式、保留本机修改过的配置并关闭 dpkg 独立伪终端；needrestart 仅列出服务重启需求。后台命令无法读取界面的控制终端，日志中的 ANSI 控制序列会被完整清除。如果旧版本的升级曾被中断，先在独立 SSH 会话确认 apt/dpkg 已退出并执行 `dpkg --audit`，再处理未完成的软件包配置；不要删除锁文件。

## CLI

```bash
cqs --help
cqs list
cqs list --json
cqs run bbr.status
sudo cqs run bbr.enable --set qdisc=fq
sudo cqs run tcp.tune --set bandwidth=1000 --set rtt=80
sudo cqs run kernel.install --set profile=standard
cqs run kernel.list
cqs run terminal.apply --set shell=bash --set color=cyan
sudo cqs batch tcp.tune bbr.enable --set tcp.tune.rtt=80
```

启用已有内核的 BBR / BBRv3：进入 **网络优化 → 启用当前内核的 BBR**，选择 `fq` 等队列并执行；需要调整 TCP 缓冲区时使用同页的 TCP 调优。安装 BBRv3：进入 **内核管理 → 安装 BBRv3 内核**，可选 `standard` / `max`，版本留空使用最新匹配版本，也可填写 `x.y.z`；重启进入新内核后再查看 BBR 状态及配置队列。

Rust 和 Shell 版在内核安装及引导检查成功后，会显示当前内核、目标内核及重连后的验证步骤，并询问是否立即重启。取消或留空会保留安装结果，可稍后执行 `sudo reboot`。`--yes` 不会自动同意重启；非交互运行只输出手动重启指引。重连后用 `uname -r` 核对目标内核，若仍运行旧内核，请检查 GRUB 启动选择。

IP 质量检测使用 [xykt/IPQuality](https://github.com/xykt/IPQuality)，入口为 **服务器测试 → IP 质量体检 · xykt/IPQuality**。支持双栈、仅 IPv4、仅 IPv6，输出语言跟随中英文界面；它检测的是目标机器的 IP，不是状态栏的公网地址查询。

```bash
cqs run test.ip
cqs run test.ip --set ip_family=ipv4
cqs --lang en-US run test.ip --set ip_family=ipv6 --set full_ip=on
```

默认传入上游的 `-p`，禁用在线报告上传，默认不显示完整 IP；检测本身仍会访问外部数据库和服务。依赖缺失时由上游询问安装，CqShell 的 `--yes` 不会转成上游的自动安装参数。脚本完整下载并通过固定版本 SHA-256 和 Bash 语法检查后才执行，退出后返回 TUI。当前接入在 Debian / Ubuntu 的交互终端运行，详见 [接入说明](docs/IPQUALITY.md)。

`list` 列出全部操作、默认值和枚举；操作 ID 不随语言改变。JSON 中 `available` 表示已提供该操作，具体执行仍检查平台、权限及参数。无终端时必须使用 CLI，`--yes` 可自动确认操作提示。

## 系统软件源

系统管理新增 **系统软件源测速 / 更换系统软件源 · LinuxMirrors / 恢复上次系统软件源**。支持官方、清华、中科大和阿里云；`auto` 在服务器上进行真实下载测速，显示推荐后确认换源。固定上游版本并校验摘要，保留官方安全更新源，失败恢复配置，不升级软件。支持范围、自定义配置限制和恢复说明见 [软件源说明](docs/MIRRORS.md)。

```bash
cqs run system.mirror.test
sudo cqs run system.mirror.apply --set mirror_site=auto
sudo cqs run system.mirror.restore
```

## 常用工具与 Agent 工具

**常用工具**恢复旧版的十个独立安装入口：curl、wget、nano、unzip、tar、tmux、iftop、btop、gdu、fzf。选择条目即可安装对应软件；“全部安装 / 自选软件”默认选择 `all`，同时安装 zsh / Starship 组件。提示符配置和还原位于独立的**终端美化**分类。

```bash
sudo cqs run tools.tmux
sudo cqs run tools.btop
sudo cqs run tools.install                         # 全部常用工具及终端组件
sudo cqs run tools.install --set package=fzf       # 保留原来的 CLI 用法
```

新增独立的 **Agent 工具**分类，提供 Pi（pi.dev）、Claude Code、Codex CLI 的安装 / 更新与版本检查：

```bash
cqs run ai.pi.install
cqs run ai.claude.install
cqs run ai.codex.install
cqs run ai.status
```

三款工具统一直接安装官方 **Linux x86_64 / ARM64 独立二进制**，不需要 Node.js/npm，也不执行远程安装脚本。Pi 下载完整独立包并保留主题、WASM 和其他资源；Codex 下载完整 `codex-package`，保留辅助程序、沙箱组件和资源；Claude Code 下载官方平台二进制。

以实际使用这些工具的用户运行命令，不加 `sudo`。直接以 root 登录的服务器可为 root 安装；从普通用户 `sudo cqs` 进入时，AI 安装会要求退出 sudo 后重试。安装只需 curl、CA 证书及网络连接，TUI 内显示下载和校验日志；CLI 可通过 `cqs --yes run ai.pi.install` 在无交互终端中安装。

安装前解析官方稳定版元数据，显示版本、下载地址、SHA-256 和目标命令；校验下载大小、摘要、ELF 架构、包结构以及实际 `--version` 后才切换入口。摘要缺失、不匹配或下载失败会明确报错，不回退到 npm。Pi / Codex 使用 GitHub Release 资产摘要，Claude Code 使用官方 `manifest.json`。归档解压拒绝路径越界、链接和特殊文件。

版本文件保存到 `~/.local/share/cqs/ai/<工具>/` 的独立目录，三个命令默认位于 `~/.local/bin`；Codex 也支持 `CODEX_INSTALL_DIR`。不会修改 Shell 配置；可使用安装日志中的完整路径，或执行 `export PATH="$HOME/.local/bin:$PATH"`。已有入口备份到命令目录下的 `.cqs-ai-backup-*/previous`；切换后检查失败会还原，旧版本及备份不会自动删除。后续更新使用相同安装入口。状态检查优先读取这些入口，也兼容 Pi 的 `PI_CODING_AGENT_DIR` / `~/.pi/agent/bin` 旧入口及 PATH。

登录凭据、会话和项目配置保留，首次使用按工具提示登录。工具本体无需 Node；项目、扩展或 MCP 服务有各自的依赖。

分发方式依据 [Pi 官方 Releases](https://github.com/earendil-works/pi/releases)、[Claude Code 二进制说明](https://code.claude.com/docs/en/setup#binary-integrity-and-code-signing) 和 [OpenAI Codex CLI 文档](https://developers.openai.com/codex/cli)，核对日期为 2026-10-27。

## TOML 多语言

偏好配置默认位于 `${XDG_CONFIG_HOME:-$HOME/.config}/cqs/config.toml`：

```toml
language = "zh-CN"
# locale_dir = "/absolute/path/to/locales"
```

语言优先级：`--lang` → `CQS_LANG` → 配置文件 → `LC_ALL` / `LC_MESSAGES` / `LANG` → 英语。未支持的系统 locale 使用英语；显式指定不存在的语言会报错。

```bash
cqs --config /path/to/config.toml
cqs --locale-dir ./locales languages
```

新增语言时复制完整的 [locales/en-US.toml](locales/en-US.toml)，修改文件名、`locale`、`name` 和译文，再通过 `locale_dir` 加载。语言文件的键和命名占位符必须一致；缺键、额外键、空译文和占位符不一致都会报错。内置译文已编译进二进制。详见 [多语言说明](docs/I18N.md)。

## 功能与使用边界

| 功能 | 内容 |
| --- | --- |
| 系统管理 | 概览、按需公网 IP / ISP 查询、更新、清理、时区、SWAP、fail2ban |
| BBR / TCP | 状态、四种默认队列、带宽与 RTT 调优、cgroup 内存约束、撤销 |
| 内核管理 | BBRv3 标准版 / Max、最新或指定版本安装、内核查看、卸载未运行内核 |
| SSH | 端口修改与监听验证、公钥导入和关闭密码认证 |
| 服务器测试 | Speedtest、IP 质量、NextTrace、YABS、连通性、磁盘 I/O |
| 常用工具 | 十种工具独立安装、全部安装、Bash / zsh 提示符配置与首次基线还原 |
| Agent 工具 | Pi（pi.dev）、Claude Code、Codex CLI 安装 / 更新与版本检查 |
| Docker | 安装、状态、镜像源、IPv6、清理和卸载 |

内核安装支持 Debian 12+ / Ubuntu 24.04+ 的 amd64 / arm64 主机，需要独立内核环境和 GRUB。安装后可确认立即重启或稍后手动重启进入新内核；状态页读取实际运行的 BBR 版本。WSL / 容器不支持更换宿主内核，卸载入口仅处理未运行的指定内核。

旧版本升级前请阅读 [迁移说明](docs/MIGRATION.md)。网络和提示符的旧恢复基线可继续读取；不要同时运行新旧程序，也不要删除未完成操作的恢复记录。

## 开发与验证

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

Linux 测试需要 Bash、curl、openssl、Python 3（本地 TLS / 下载路由夹具）、cc（编译 ELF 测试夹具）、dpkg-deb、sysctl。测试不安装内核、不修改宿主 sysctl、不重载 SSH / Docker。真实 sshd / dockerd 解析测试需显式运行，见 [技术文档](docs/TECHNICAL.md)。

GitHub Actions 在分支推送和 PR 时运行 Rust 检查与安装器测试；推送与 `Cargo.toml` 版本一致的 `v*` 标签后，检查通过才构建 amd64 / arm64 musl 安装包并发布 GitHub Release。手动运行构建工作流只上传 Actions 产物。详见 [发布说明](docs/RELEASING.md)。

## 许可证与参考

沿用 [LICENSE](LICENSE) 中的 GNU GPL v3 文本。Rust 迁移版本：2026-10-26。

- [yuju520/YujuToolBox](https://github.com/yuju520/YujuToolBox)：基础上游项目。
- [byJoey/Actions-bbr-v3](https://github.com/byJoey/Actions-bbr-v3)：BBRv3 功能参考及第三方内核包来源。
- [kejilion/sh](https://github.com/kejilion/sh)：系统管理工具参考。
- [xykt/IPQuality](https://github.com/xykt/IPQuality)：IP 质量体检的实际执行项目。

# 更新日志

本文记录 **CqShell（`cqs`）** 的所有重要变更。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。各版本的独立发布说明见 [`docs/releases/`](docs/releases/)。

## [v0.2.1] - 2026-11-27

> 本版本紧接 `v0.2.0` 发布，除下方修复与文档媒体更新外，完整包含 `v0.2.0` 的全部新特性与加固（详见紧邻的 `v0.2.0` 节与 [`docs/releases/v0.2.1.md`](docs/releases/v0.2.1.md)）。

### 修复
- 修复移动端 `TouchMedium`（如 `59×34` 竖屏）下 2 行高动作列表滚动条计算错误：当列表项数多于可见项数但少于视口行数时，`rows.len() - visible_h` 会触发无符号减法下溢；改为基于 `geom.visual_rows.len()` 计算滑块高度与最大滚动量，并补充回归单元测试。

### 文档与开发工具
- 将 README 静态 PNG 截图全部替换为 4 组由真实 `cqs` TUI 离线逐帧渲染录制的动态 GIF 演示（桌面端全功能交互、Mihomo 6 标签工作台、Docker 6 标签工作台与 `59×34` 移动端触控体验），并在仓库中保留跨平台离线录制管线（`src/tui/gif_recorder.rs` 与 `scripts/render_gifs.mjs`）。
- 在中英文 README 致谢列表中补充 `chen08209/FlClash`。

## [v0.2.0] - 2026-11-27

### 新增
- **原生双传输 MCP 服务器与结构化 JSON 控制面**（零 `tokio`，保持单静态二进制与 MSRV `1.88.0`）：
  - **stdio / SSH stdio 传输（`cqs mcp`）**：实现 JSON-RPC 2.0 换行分隔帧与 `2025-06-18` / `2025-03-26` / `2024-11-05` 协议协商，暴露 13 项工具、5 项静态资源、2 项资源模板与 4 项诊断 Prompt，内置 `--read-only`、`--allow-danger`、`--allow-group`、`--deny-action` 权限护栏、强制密钥脱敏与零自动重启保证。
  - **原生 Streamable HTTP 传输（`cqs mcp serve`）**：基于 `std::net::TcpListener` 与有界工作线程池，默认监听回环 `127.0.0.1:18765/mcp` 与只读模式；内置 `0600` 高熵 Bearer 密钥与运行中零重启热轮换（`cqs mcp token status|init|rotate|set`）、非回环绑定强制鉴权、`Origin` 防 DNS Rebinding 校验、分块传输（`chunked`）与 `MCP-Session-Id` 会话生命周期管理；支持可选 `cqs-mcp.service` systemd 用户/系统常驻服务托管（`UMask=0077`，绝不经命令行传密钥）。
  - **一键安装与脱敏复制入口**：新增 `cqs mcp install / uninstall`（原子增量合并 Claude Code `~/.claude.json`、Codex CLI `~/.codex/config.toml`、Cursor `~/.cursor/mcp.json` 与自定义 `--file` JSON，保留既有配置与修改前备份）、`cqs mcp config [--reveal]` 多格式配置片段生成、`ai` 分组新增「MCP 服务与接入」区块（9 项动作，总动作数增至 115 项），以及 TUI 全局 `Shift+M`「MCP 接入与脱敏密钥卡」（屏幕始终遮罩、OSC 52 一键复制与原地轮换）；禁止通过 MCP 协议本身自我调用 MCP 控制面变更动作以防提权。
  - **结构化 JSON CLI**：新增 `cqs status [--section system|docker|mihomo|all] [--json]`、`cqs describe <id> [--json]`、`cqs choices <id> [--json]`，以及 `run` / `batch` 的 `--json`、`--dry-run` 与 `--params-json`（支持 `-` 标准输入）。
- **全新 Mihomo 6 标签可视化工作台（`cqs mihomo`）**：
  - 覆盖仪表盘（实时流量图谱、峰值/总量、模式切换与按需出站 IP 探测）、代理（可滚动策略组胶囊、多列节点卡片、单节点/整组延迟测速、多维排序、隐超时、选最快、自动组恢复与定位）、配置（订阅流量配额进度条、到期倒计时、批量更新与外部 Providers）、请求（运行中/已关闭/全部连接筛选与单连接关闭）、规则与工具（内嵌脱敏日志检索）。
  - 新增 13 项流媒体与 AI 服务可用性/解锁巡检弹窗与出站 IP 质量画像弹窗（支持隐藏出站 IP 持久化）；支持核心 DNS 查询（`clash.dns.query`）、Fake-IP/DNS 缓存刷新（`clash.cache.flush`）、并发拨号/统一延迟/`find-process-mode` 设置与 `clash.stop` 强制终止选项；运行中修改配置自动热重载或安全重启生效，未开启本机控制接口时提供一键启用入口。
- **独立 SOCKS5 / SOCKS5h 入站与脱敏凭据卡**：新增 `clash.socks.*` 可视化分栏预览配置表单，非回环绑定强制密码认证并生成 `0600` `socks5.auth`；新增全局 `Shift+S`「SOCKS5 / 5h 凭据卡」弹窗（屏幕始终遮罩、禁止明文渲染、仅 OSC 52 一键复制，关闭即清空内存）。
- **仪表盘实时连接数统计**：概览仪表盘与 `cqs status` 新增 Linux 宿主机 TCP / UDP / `TIME_WAIT` 连接数（基于 `/proc/net/sockstat{,6}`），Mihomo 仪表盘与 `cqs_mihomo_snapshot` 新增活跃连接总数与 TCP / UDP 拆分统计并适配窄屏布局。
- **自适应触控密度与小屏底部拇指操作栏**：新增 `ui_density`（`Auto` / `Compact` / `Touch`，可在外观抽屉按 `D` 切换并持久化）与竖屏/窄屏底部拇指快捷操作栏，优化移动端 SSH（含 `59×24`、`82×54`、`52×36`）触控体验。
- **AI 服务与分流体检（`test.ai`）**：集成 `ipforai.cc/sh`（经 `sh -n` 语法预检），默认关闭在线分享短链（`--no-share`），支持 `standard` / `detail` / `summary` / `markdown` 四种模式及指定目标 IP 检测。

### 变更
- 统一交互式二次确认为简洁的 `y/n` 流程，移除表单提交后的冗余重复确认提示。
- 全面优化 TUI 与后台监控性能：`actions::specs_slice()` / `OnceLock` 静态缓存、配置文件指纹缓存消除渲染热路径每帧磁盘解析、`Monitor::snapshot_if_changed()` 跳过空闲重绘与拷贝、静态探测结果按二进制指纹缓存。

### 修复
- 彻底修复 `crossterm 0.29` 在已断开 PTY（`/dev/pts/* (deleted)`）上导致的 100% CPU 死循环：Unix 端统一经 `poll_terminal_event()`（`libc::poll` + `FIONREAD` 预检）与零唤醒 `HangupWatchdog` 守护，并在断连时跳过退出转义序列，自建管道与测试 PTY 均设置 `FD_CLOEXEC`。
- 补充剩余子进程启动路径的瞬态 `ETXTBSY` 自动退避重试，提升 Linux 并发写入后立即执行的稳定性。

## [v0.1.8] - 2026-11-24

### 修复
- 修复退出 TUI 时终端鼠标追踪序列（如 `35;x;yM`）残留到父 Shell 提示符的问题，并合并连续鼠标移动事件以消除高回报率鼠标与 SSH 延迟下的输入积压。
- 补充 Unix `SIGHUP` / `SIGQUIT` 信号捕获，确保终端断开或收到退出信号时仍能正常恢复终端模式。

## [v0.1.7] - 2026-11-24

### 新增
- 新增 `tools.git`（APT 安装）与 `tools.gh`（从 `cli/cli` 官方 Release 下载 `.deb`，校验 SHA-256 与包元数据后安装，支持 `--gh-proxy`）；`tools.install` 全量安装同步包含这两项。

### 修复
- `clash.mode --set persist=on` 改为先确认控制器切换成功，再写入 `mixin.yaml` 与 `runtime.yaml`，避免接口失败时磁盘配置与运行状态不一致。
- `clash.reload` 热重载前检查 TUN 权限与网卡归属，允许复用当前 Mihomo 进程已持有的 TUN 设备。
- `cqs update` 在 GitHub API 限流并回退至 Release 重定向查询时，继续复用已配置的 GitHub 加速代理并响应取消信号。
- SSH 配置解析兼容 `Key=Value` 写法并忽略注释行；`clash.sub.import`、`clash.secret.set` 与 `ssh.key` 拒绝包含 `.` 或 `..` 的路径。
- 公网 IP 查询限制重定向为 HTTPS、设置连接超时与响应体积上限，单项探测失败不再中断其余结果。
- Docker 工作台正确统计数据卷被容器挂载的引用关系，清理失效的 Compose 项目，并在多选启停、删除及卷保护时按全部选中项判定。
- Docker 容器日志流过滤非法参数并在进程退出后完整读取尾部日志；`docker exec -it` 异常退出时自动恢复 TUI 终端状态。
- 修复 TUI 顶层弹窗遮挡顺序、一键优化执行按钮显隐、顶栏运行状态点击区域、面板滚动上限，以及关闭外观抽屉时误触背景控件的问题。

## [v0.1.6] - 2026-11-23

### 新增
- `docker.mirror` 新增 `recommended`（4 源故障转移）及 `1ms`、`xuanyuan`、`daocloud`、`1panel` 预设，支持多源分隔混写与自动去重，并支持通过 `clear` / `direct` 清空镜像源恢复官方直连。
- 支持自定义输入的表单字段可同时展示可点击预设标签；打开 `docker.mirror` 时自动回填 `/etc/docker/daemon.json` 现有配置。
- Docker 工作台的镜像与守护进程页新增 `g` 快捷键入口，并实时显示当前镜像源数量或直连状态。

### 修复
- 子进程启动遇到瞬态 `ETXTBSY`（`ExecutableFileBusy`）时自动做短退避重试，避免 Linux 并发执行偶发报错。

## [v0.1.5] - 2026-11-23

### 新增
- 主题拆分为 8 套配色与 8 套独立 UI 样式，支持通过 `cqs styles [--json]`、`--style`、`CQS_STYLE`、`config.toml` 或外观抽屉（`T`）自由组合，并提供透明背景（`B`）与减弱动效（`M`）开关。
- 重构双栏概览仪表盘与工作台，新增全局命令面板（`/` / `Ctrl+K`）、鼠标划选复制（OSC 52）、纯文本视图（`V`）、分类排序持久化（`Alt+↑↓`）及最低 `40×16` 窄屏适配。
- 新增 `cqs docker` 六标签工作台，覆盖容器、Compose、镜像、数据卷、网络与守护进程管理，支持实时日志、资源监控、`docker exec -it`、多选操作、环境变量脱敏与分级清理向导。
- 集成 Mihomo 代理管理与 `cqs clashctl` / `cqs proxy-env` 兼容命令，支持核心安装、在线订阅与本地 YAML 导入、4 款 Web 面板、控制器测速/热重载、GeoData、DNS、TUN 及 systemd 托管。
- 新增 `cqs update`（`--check` 检测 / 原地校验自更新）与对应的 TUI 更新操作。
- 新增全局 GitHub 加速代理（`github.proxy`、`--gh-proxy`、`CQS_GH_PROXY`、`GH_PROXY`），API 限流时自动回退直连。
- 新增 `kernel.boot` 一次性 GRUB 启动内核切换。

### 变更
- 安装脚本支持选择安装目录与 `--gh-proxy default`，并持久化界面语言设置。

### 移除
- 移除旧版独立 Shell 脚本实现，统一维护纯 Rust 单二进制程序。

### 修复
- 限制发行包下载字节上限，保留弹窗下的终端默认背景色。

## [v0.1.4] - 2026-10-27

### 变更
- 安装器默认安装路径改为 `/usr/local/bin/cqs`，安装后可直接运行 `cqs`（系统写操作使用 `sudo cqs`）。

## [v0.1.3] - 2026-10-27

### 新增
- `install.sh` 新增 `--gh-proxy HTTPS_URL` 参数并兼容 `GH_PROXY` 环境变量，使版本查询、安装包与校验文件下载均经过指定 HTTPS 代理，同时保留完整性与版本校验。

## [v0.1.2] - 2026-10-27

### 新增
- 新增系统软件源测速与换源（`system.mirror.test` / `system.mirror.apply` / `system.mirror.restore`），支持官方、清华、中科大与阿里云源及失败回滚。
- 新增常用时区预设（保留自定义 IANA 时区输入），内核安装完成后增加独立重启确认（`--yes` 不自动重启）。

### 变更
- 大尺寸终端下自动扩展任务日志面板高度；原 AI CLI 分类更名为「Agent 工具」。

## [v0.1.1] - 2026-10-26

### 修复
- 修复系统软件升级遇到 `debconf` / `ucf` 交互提示时卡住且无法用 `Ctrl+C` 或 `Q` 取消退出的问题，默认采用非交互 `apt` 并保留本机配置。
- 完整过滤任务日志中的 ANSI 控制序列，避免残留转义字符。

## [v0.1.0] - 2026-10-26

### 新增
- 发布首个纯 Rust / Ratatui 单二进制版本，提供简中/英文双语 TUI 与 CLI，覆盖系统维护、BBR/TCP 调优、BBRv3 内核管理、SSH 安全、Docker 基础配置、Starship 终端美化与服务器测试。
- 支持 Pi、Claude Code、Codex 官方独立二进制校验安装（无需 Node.js / npm）及常用系统工具独立安装。
- 支持按需查询公网 IP/ISP、一键优化批处理清单与事务化备份恢复。

[Unreleased]: https://github.com/ZyphrZero/CqShell/compare/v0.2.1...HEAD
[v0.2.1]: https://github.com/ZyphrZero/CqShell/compare/v0.2.0...v0.2.1
[v0.2.0]: https://github.com/ZyphrZero/CqShell/compare/v0.1.8...v0.2.0
[v0.1.8]: https://github.com/ZyphrZero/CqShell/compare/v0.1.7...v0.1.8
[v0.1.7]: https://github.com/ZyphrZero/CqShell/compare/v0.1.6...v0.1.7
[v0.1.6]: https://github.com/ZyphrZero/CqShell/compare/v0.1.5...v0.1.6
[v0.1.5]: https://github.com/ZyphrZero/CqShell/compare/v0.1.4...v0.1.5
[v0.1.4]: https://github.com/ZyphrZero/CqShell/compare/v0.1.3...v0.1.4
[v0.1.3]: https://github.com/ZyphrZero/CqShell/compare/v0.1.2...v0.1.3
[v0.1.2]: https://github.com/ZyphrZero/CqShell/compare/v0.1.1...v0.1.2
[v0.1.1]: https://github.com/ZyphrZero/CqShell/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ZyphrZero/CqShell/releases/tag/v0.1.0

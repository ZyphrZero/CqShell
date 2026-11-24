# Changelog

All notable changes to **CqShell (`cqs`)** are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/). Detailed per-version release notes also live under [`docs/releases/`](docs/releases/).

---

## [v0.1.7] - 2026-11-24

### Added
- **常用工具新增 `git` 与官方 GitHub CLI（`gh`）**：新增 `tools.git`（apt 安装）与 `tools.gh`（直连 `cli/cli` 官方 Releases，严格校验稳定版标签、架构 `.deb` 文件名、SHA-256 摘要及 `dpkg-deb` 包名/架构后安装，自动走全局 GitHub 国内加速代理）；`tools.install` 全量模式扩展至 12 款工具，总运维动作增至 **99 项**。
- **README 补充实机截图**：中英文 README 补充桌面端与移动端 TUI 实机预览截图。

### Fixed
- **安全校验与运维操作加固**：
  - **Mihomo 代理与 TUN**：`clash.mode --set persist=on` 改为先调用控制器 `PATCH /configs` 成功后再原子写入 `mixin.yaml` 与 `runtime.yaml`；`clash.reload` 热重载前新增 `tun::validate_reload` 权限与设备归属校验（允许复用当前运行中 Mihomo 进程自身持有的 TUN 设备）；`recover_interrupted_files` 兼容合法 profile 文件名并在写回前校验备份数据完整性。
  - **自更新代理回退**：`self_update` 在 GitHub REST API 限流或被中转服务拒绝、回退到 `/releases/latest` 或 `/releases/tag/<TAG>` 重定向查询时，自动经过已配置的 `github_proxy` 并在解析前剥离代理前缀，同时响应取消信号。
  - **SSH 配置、公网 IP 与路径边界**：`ssh::candidate` 正确处理 `Key=Value` 写法并跳过 `#` 注释行；`system.public_ip` 为 `curl` 强制加上 `--proto-redir =https`、`--connect-timeout` 与 `64 KiB` `--max-filesize` 上限，复用严格 IP 解析并净化 ISP 控制字符，单项失败不影响其余探测；`clash.sub.import`、`clash.secret.set` 与 `ssh.key` 统一拒绝包含 `.` / `..` 的路径。
- **Docker 后台采集、日志流与工作台稳定性**：
  - 容器列表解析保留完整 `mounts` 列表，`cross_link_snapshot` 自动关联卷的 `used_by` 引用关系并清理失效的合成 Compose 项目；Compose `up`/`pull` 强制要求存在且合法的配置文件参数；`read_compose_file` 增加路径检查、有界 `256 KiB` 读取与控制字符净化；`docker exec` 探测容器 Shell 前先校验参数，`docker top` 兼容含空格命令。
  - `LogStreamer` 校验容器 ID 与 Compose 参数防止选项注入，进程退出时完整排空尾行并报告非零退出码；离开 Docker 页后任务完成仅执行单次 `RefreshOnce` 避免唤醒长轮询，并在操作/刷新/外部 exec 后失效旧 inspect 缓存；外部 `docker exec -it` 使用 `ExternalSessionGuard` 确保异常退出也能恢复 TUI 终端状态。
  - 多选容器启停/暂停/删除及卷引用保护改为基于全部选中目标判定；收敛日志与详情滚动上限；支持 Docker 页内 `Alt+↑/↓` 调序、全屏 `[` 呼出侧栏抽屉及 `i` 切换全屏详情，并修复切页/退全屏时的焦点与搜索态残留。
- **核心 TUI 渲染与交互细节**：
  - 统一顶层弹窗渲染优先级与 `events.rs` 事件分发栈完全一致；修复一键优化「执行清单」底部 CTA 按钮显示与空清单隐藏逻辑；精确对齐顶栏运行中状态胶囊命中区；修正说明面板与 Starship TOML 预览最大滚动上限；点击外观抽屉外侧关闭时不再误触背景控件；窗口拉宽时自动收起冗余遮罩侧栏。

---

## [v0.1.6] - 2026-11-23

### Added
- **Docker 多镜像源与预设配置（`docker.mirror`）**：内置 `recommended`（4 源自动故障转移：`docker.1ms.run`、`docker.xuanyuan.me`、`docker.m.daocloud.io`、`docker.1panel.live`）及 `1ms` / `xuanyuan` / `daocloud` / `1panel` 预设，支持逗号/分号/空白分隔的多源混写与自动去重，并支持 `clear` / `off` / `none` / `direct` 一键移除 `registry-mirrors` 恢复官方直连。
- **TUI 表单预设标签与 Docker 快捷入口**：`allow_custom` 表单字段支持「自由输入框 + 可点击预设标签」同步展示；打开 `docker.mirror` 自动回填当前 `/etc/docker/daemon.json` 已有配置；Docker 工作台「镜像」与「守护进程」页新增 `g` 快捷键和 `[g] 镜像源` 按钮，并在守护进程列表实时显示源数量/直连状态。

### Fixed
- **Linux `ETXTBSY` 子进程并发重试**：在 `runtime` 的 `Command::spawn` 与 `Command::status` 中加入对瞬态 `ExecutableFileBusy` 的 5 次短退避重试（`5..80 ms`，响应取消信号），消除 Linux 多线程并发写/执行场景下的偶发启动失败。

### Changed
- 整理部分模块代码结构并同步更新中英文 README 与快照测试。

---

## [v0.1.5] - 2026-11-23

### Added
- **配色与 UI 样式解耦**：内置 8 套配色（`auto` / `obsidian` / `daylight` / `tokyo-night` / `catppuccin` / `rose-pine` / `high-contrast` / `plain`）与 8 套独立 UI 样式（`rounded` / `bento` / `editorial` / `hud` / `studio` / `atelier` / `heavy` / `ascii`），支持自由混搭；新增 `cqs styles [--json]`、`--style`、`CQS_STYLE` 与 `config.toml` 的 `style` 字段。
- **双标签外观抽屉（`T`）与柔和微动效**：支持 `[配色]` / `[样式]` 实时预览、RGB 渐变动效、透明背景（`B`）与减弱动效（`M`）开关。
- **新版概览仪表盘与资源进度条**：统一 CPU / 内存 / 磁盘 / 交换等高半高渐变条与行间留白、CPU 迷你波形、40×16 移动端自适应布局、全局命令面板（`/` / `Ctrl+K`）、鼠标划选复制（OSC 52）、纯文本日志视图（`V`）及分类顺序持久化（`Alt+↑↓`）。
- **Docker 专属 6 标签工作台（`cqs docker`）**：集成容器 / Compose / 镜像 / 卷 / 网络 / 守护进程管理、全屏聚焦模式（`z` / `F`）、实时日志与 CPU/内存监控、原生 `docker exec -it`、多选批量操作、敏感环境变量脱敏及可视化分级 Prune 向导（`Shift+P`）。
- **Mihomo 代理全栈集成与 `clashctl` 兼容**：37 项核心/订阅/本地 YAML 导入/控制器热重载/策略组测速/连接巡检/GeoData/DNS/TUN/systemd 操作、4 款 Web 控制台（Zashboard / MetaCubeXD / Yacd-Meta / Razord-Meta）、`cqs proxy-env` 与 `cqs clashctl` 兼容入口。
- **在线检测更新与命令自更新**：新增 `cqs update --check [--json]`、`sudo cqs update [--version TAG] [--force]`（别名 `self-update`）及 TUI `app.check_update` / `app.update`，直连 `ZyphrZero/CqShell` Releases，校验 SHA-256、ELF 架构与 `--version` 后原子替换。
- **全局 GitHub 国内加速代理**：新增 `github.proxy` 与 `--gh-proxy` / `CQS_GH_PROXY` / `GH_PROXY`（支持 `default` 预设），覆盖所有 GitHub Release/API 拉取且遇到 API 限流自动回退。
- **内核一次性 GRUB 启动切换**：新增 `kernel.boot`，自动校验安全条件并在引导切换后提供验证指引。

### Changed
- 移除旧版独立 Shell 脚本实现，统一维护纯 Rust 单二进制与双语 TOML 词典。
- 交互式安装脚本新增安装目录选择并支持 `--gh-proxy default`；持久化界面语言偏好。

### Fixed
- 独立限定发行包下载字节上限，避免依赖外部 `curl` 行为；保留面板/弹窗终端背景色并修复多处跨平台构建与测试问题。

---

## [v0.1.4] - 2026-10-27

### Changed
- 安装器默认安装路径调整为 `/usr/local/bin/cqs`，安装后直接输入 `cqs` 启动全局命令（系统写操作使用 `sudo cqs`）。
- 目标目录已在 `PATH` 时给出更简洁的启动提示；同步更新中英文 README 安装与卸载说明。

---

## [v0.1.3] - 2026-10-27

### Added
- `install.sh` 新增 `--gh-proxy HTTPS_URL` 参数并兼容 `GH_PROXY` 环境变量，使最新版本查询、安装包与 `.sha256` 下载均经过可信 HTTPS 中转服务，同时完整保留 TLS 校验、SHA-256 完整性校验与版本校验。

---

## [v0.1.2] - 2026-10-27

### Added
- **系统镜像换源（LinuxMirrors Lite）**：新增 `system.mirror.test` / `system.mirror.apply` / `system.mirror.restore`，支持官方、清华、中科大与阿里云源的实测下载测速、备份校验与失败回滚。
- 新增常用时区预设（保留自定义 IANA 时区输入）与内核安装后的显式重启确认（`--yes` 不会自动同意重启）。

### Changed
- 大终端任务日志面板自适应扩展；统一将 AI CLI 分类命名为「Agent 工具」。

---

## [v0.1.1] - 2026-10-26

### Fixed
- 修复 `v0.1.0` 中系统软件包升级遇到隐藏 `debconf` / `ucf` 交互对话框时卡住且无法通过 `Ctrl+C` / `Q` 退出的问题。
- 后台捕获命令脱离控制终端、统一使用非交互 apt 并保留本机已修改配置；完整过滤日志中的 ANSI 控制序列；补充真实 `ucf` 保留集成测试。

---

## [v0.1.0] - 2026-10-26

### Added
- **首个纯 Rust / Ratatui 版本**：将原 `cqs.sh` 迁移为单二进制双语（简中/英文）TUI + CLI 工具箱，覆盖系统维护、BBR/TCP 调优、BBRv3 内核管理、SSH 安全、Docker 基础管理、终端美化（Starship）与服务器测试（Speedtest / IPQuality / NextTrace / YABS / 磁盘 I/O）。
- 新增 Pi、Claude Code、Codex CLI 官方独立 Linux 二进制校验安装（无需 Node.js/npm），并恢复常用系统工具的单独安装入口。
- 新增按需公网 IP/ISP 查询、多选一键优化清单与事务化备份恢复机制。

---

[v0.1.7]: https://github.com/ZyphrZero/CqShell/compare/v0.1.6...v0.1.7
[v0.1.6]: https://github.com/ZyphrZero/CqShell/compare/v0.1.5...v0.1.6
[v0.1.5]: https://github.com/ZyphrZero/CqShell/compare/v0.1.4...v0.1.5
[v0.1.4]: https://github.com/ZyphrZero/CqShell/compare/v0.1.3...v0.1.4
[v0.1.3]: https://github.com/ZyphrZero/CqShell/compare/v0.1.2...v0.1.3
[v0.1.2]: https://github.com/ZyphrZero/CqShell/compare/v0.1.1...v0.1.2
[v0.1.1]: https://github.com/ZyphrZero/CqShell/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ZyphrZero/CqShell/releases/tag/v0.1.0

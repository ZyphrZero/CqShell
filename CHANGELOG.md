# 更新日志

本文记录 **CqShell（`cqs`）** 的所有重要变更。

格式遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。各版本的独立发布说明见 [`docs/releases/`](docs/releases/)。

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

[Unreleased]: https://github.com/ZyphrZero/CqShell/compare/v0.1.8...HEAD
[v0.1.8]: https://github.com/ZyphrZero/CqShell/compare/v0.1.7...v0.1.8
[v0.1.7]: https://github.com/ZyphrZero/CqShell/compare/v0.1.6...v0.1.7
[v0.1.6]: https://github.com/ZyphrZero/CqShell/compare/v0.1.5...v0.1.6
[v0.1.5]: https://github.com/ZyphrZero/CqShell/compare/v0.1.4...v0.1.5
[v0.1.4]: https://github.com/ZyphrZero/CqShell/compare/v0.1.3...v0.1.4
[v0.1.3]: https://github.com/ZyphrZero/CqShell/compare/v0.1.2...v0.1.3
[v0.1.2]: https://github.com/ZyphrZero/CqShell/compare/v0.1.1...v0.1.2
[v0.1.1]: https://github.com/ZyphrZero/CqShell/compare/v0.1.0...v0.1.1
[v0.1.0]: https://github.com/ZyphrZero/CqShell/releases/tag/v0.1.0

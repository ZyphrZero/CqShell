# CqShell Rust 技术文档

对应 `0.1.4` 工作树及 TUI 改版，更新日期 2026-11-07。安装与操作见 [README](../README.md)。历史验收记录保留原时间与范围。

## 架构

```text
main → cli → actions → ops/{system,network,kernel,ssh,docker,terminal,tools,ai,ai_check}
          ↘ tui ↗          ↘ runtime / storage
                i18n ← locales/*.toml
```

- `actions` 定义固定 ID、分类、参数、默认值、权限和交互属性，CLI / TUI 共用解析和执行逻辑。参数可声明来源（`Source`）：订阅、Selector 策略组、可启动内核、可删除的 BBRv3 内核等现有项目由 `actions::choices` 读取，依赖字段（节点）取所选策略组的成员。来源只服务 TUI 选择，CLI 仍接受文本，操作本身照常校验；无法通过参数校验的值（如含控制字符的名称）不进入列表。
- `api` 与 `mcp` 提供面向外部 AI 与自动化脚本的原生控制面（零 `tokio` 依赖）：`src/api/` 统一实现结构化 JSON 信封、访问策略（`ExecutionPolicy`：`--read-only` / `--allow-danger` / `--allow-group` / `--deny-action`）、动作 JSON Schema 生成、两阶段确认与 `--dry-run` 预检，以及本机系统、Docker 与 Mihomo 强制脱敏只读快照；`src/mcp/` 基于纯标准库实现换行分隔 JSON-RPC 2.0 stdio MCP 服务器（支持 `2025-06-18` / `2025-03-26` / `2024-11-05`、`tools`、`resources`、`resourceTemplates` 与 `prompts`）。所有无头执行通过 `Context::captured` 拦截日志与确认，确保 `stdout` 零污染、凭据零泄漏、且 `kernel.install`/`kernel.boot` 永不自动触发重启（详见 [AI_API.md](AI_API.md)）。
- `tui` 采用现代「侧边栏工作台（Sidebar Shell）」架构与分层模块化结构：`tui/state.rs` 维护主界面状态机（独立概览仪表盘 `Overview`、一键优化清单 `Optimization`、11 个功能模块、异步表单与选择器、确认、进度与会话记录）；`tui/events.rs` 按覆盖层优先级栈（确认 → 异步加载 → 选择器/表单 → 提示/帮助/全屏日志 → 增强命令面板 → 窄屏侧栏抽屉 → 外观抽屉 → 主工作区）统一路由键盘与完整鼠标交互（Hover 高亮、单击/双击、顶栏热区、独立滚轮与模态外部关闭保护）；`tui/layout.rs` 实现 Wide（`≥112`）、Medium（`92–111`）、Compact（`80–91`）与 Mobile（`40–79`，最低支持 `40×16`，兼容 iOS Termius `59×24` 等移动端窄屏，自动折叠侧栏为 `[` 浮层抽屉、堆叠指标/工作台/Docker/提示符预览并启用表单与帮助滚动）四档响应式几何，绘制与鼠标命中共用同一份区域定义；`tui/fx.rs` 提供纯 `ratatui` Buffer 级微动效与视觉质感原语（Backdrop Scrim 压暗、1-cell 投影、120ms 弹窗渐显、90ms 切页淡入、450ms 仪表缓动与渐变色阶、运行呼吸灯与进度流光，支持 `reduced_motion` 降级）；`tui/palette.rs` 提供支持最近使用、界面快捷指令与多词模糊匹配的增强命令面板（`/` / `Ctrl+K`）；`tui/render/*` 将顶栏、侧栏、概览 Bento 仪表盘、模块工作台/检查器、右侧外观抽屉、底部/全屏日志与居中悬浮弹层解耦渲染。帧间隔按状态在 33 ms（动画中）、80 ms（转圈/呼吸）与 250 ms（空闲）之间按需切换。`tui/input.rs` 维护 UTF-8 光标和宽字符裁剪。含来源参数的表单先在独立线程读取现有项目，读取超过 150 ms 才显示等待框，Esc 放弃读取；普通操作在工作线程运行；交互工具先退出 raw mode / alternate screen，再继承终端，返回后恢复界面。
- `dashboard` 通过 sysinfo 和 Linux 只读 proc/sysfs 读取常驻指标。独立线程每 3 秒更新本机数据；CPU 首次采样显示“采集中”，达到最小采样间隔才显示占用率。公网查询只有显式 N/查询操作触发，使用另一线程和连接/总超时，结果保存在会话内。数据分别标记采集中、未查询、有效、未知及不适用，不用零值代替读取失败，也不因单个探测失败清空整个面板。
- `runtime` 用参数数组调用系统工具，不拼接 `sh -c`，并发读取 stdout / stderr，检查真实退出状态。解析环境固定 `LC_ALL=C`，外部原始诊断不翻译。
- `github_proxy` 管理 GitHub 国内加速代理：默认关闭，开启时预设前缀为 `https://gh-proxy.com/`，优先级为 `--gh-proxy` > `CQS_GH_PROXY` > `GH_PROXY` > `config.toml`（`github_proxy_enabled` / `github_proxy`），支持 `default` 与 `off`/`none` 覆盖；仅对官方 HTTPS GitHub 域名（`github.com`、`api.github.com`、`raw.githubusercontent.com`、`gist.githubusercontent.com`、`codeload.github.com`、`objects.githubusercontent.com`、`release-assets.githubusercontent.com`）重写下载和 API 地址，不转发非 GitHub 域名及 Clash 订阅链接，且所有资产 URL 规范校验、HTTPS-only 与 SHA-256 完整性校验不变。对 `api.github.com` 元数据请求，若加速代理返回 403 限流或不支持 API，自动记录日志并回退直连 `api.github.com`，而实际资产与脚本下载仍走代理。可通过 CLI `cqs run github.proxy`、全局 `--gh-proxy` 或 TUI 快捷键 `P` / 网络模块「GitHub 国内加速代理」即时配置并持久化。
- `ops/docker/` 与 `tui/docker/` 实现专属 Docker TUI 管理工作台与 `cqs docker [--tab TAB] [--embedded]` 直达入口：后端全部使用原生 `docker` / `docker compose` CLI（`--format json` / NDJSON，双路径兼容 JSON 数组与 NDJSON，无 `tokio` 异步依赖，天然支持 Rootless、`DOCKER_HOST` 与 Context），配合按需唤醒 / 离开 10 秒自动休眠的后台 `DockerMonitor` 线程、单飞防堆积限时查询、120 ms 防抖 Inspect 懒加载、带 `setsid` 进程组回收的有界 `LogStreamer`（上限 5,000 行、单行 4 KiB 截断与控制序列净化），以及基于纯 `test -x` 静态探测（零 `sh -c`）的交互式 `docker exec -it` TTY 交接；前端在侧边栏 `5 Docker` 嵌入与全屏聚焦（`z` / `F`）双态间无缝切换，覆盖容器、Compose、镜像、卷、网络与守护进程 6 大标签页、多选批量操作、敏感环境变量默认脱敏（`Shift+S` 显隐且切容器重置）、L0–L3 分级确认（卷删除/大批量删除/含卷 Prune 强制口令确认）与原有 6 项 `docker.*` 事务动作的完整兼容。
- `storage` 管理锁、路径检查、同目录临时文件、权限与所有者保留、fsync、原子替换和文件事务日志。
- `i18n` 加载 TOML，严格校验键集合与占位符。操作 ID、枚举值和恢复记录不受语言影响。
- `ops` 中的 JSON、cgroup、发布元数据、配置合并、SHA-256 和提示符生成均由 Rust 实现。`ops/ip_quality.rs` 专门接入 xykt/IPQuality，参数使用固定枚举，语言随界面，默认隐私模式；入口脚本固定版本并校验摘要/语法后交还终端执行。其外部数据源、依赖询问及更新流程见 [IPQuality 接入](IPQUALITY.md)。`ops/ai_check.rs` 接入 `https://ipforai.cc/sh` 只读 AI 服务与分流检测，校验枚举参数与可选目标 IP，默认传入 `--no-share` 关闭在线短链上传，确认后经 HTTPS 下载并通过 `sh -n` 语法校验再交还终端运行。Python 仅用于测试中的本地 TLS 服务。
- `ops/ai.rs` 直接安装 Pi、Claude Code 和 Codex 的 Linux x86_64 / ARM64 二进制，不执行 Shell / npm / Node 安装器。固定官方元数据入口 → 稳定版 / 架构筛选 → 展示实际版本与摘要并确认 → HTTPS 下载与 SHA-256 / 大小校验 → 安全解压 → ELF / 资源布局 / 实际版本检查 → 原子切换符号链接。Pi/Codex 要求 GitHub 资产提供 SHA-256，Claude 使用官方 manifest；不允许无校验或 npm 降级。安装在普通工作线程运行，支持非交互 CLI；HOME 与当前用户路径一致，拒绝普通用户通过 sudo 安装，直接 root 登录除外。

默认分发一个二进制，无数据库或后台 daemon。语言文件可选外置；apt/dpkg、systemd、sysctl、OpenSSH 和 Docker 仍负责各自的原生操作。

`ops/clash.rs` 及其子模块用 Rust 实现 Mihomo 管理全流程：
- 核心与订阅（`ops/clash.rs`）：GitHub Release 安装 Mihomo 时验证 HTTPS 资产 URL、SHA-256、大小、gzip 解压上限、ELF 架构和 `-v` 结果，再原子替换当前用户的二进制；启动流程结构化解析主配置和 Mixin，支持 rules/proxies/proxy-groups 的 prepend/append/override/inject，先用临时 YAML 运行 Mihomo `-t`，再原子发布 runtime 配置。PID 记录包括 `/proc/<pid>/stat` 启动时间；停止前还会精确匹配 `/proc/<pid>/cmdline` 的二进制、resources 和 runtime 参数。订阅元数据沿用 `use` / `profiles` 并保留未知字段；支持在线 HTTPS 订阅与本地 YAML 导入（`clash.sub.import`），新增、更新、选择和删除均使用 pending before-image 事务。`proxy-env` 只输出给父 Shell 执行的命令。
- 多面板 Web UI（`ops/clash/panel.rs`）：支持 **Zashboard**（默认，内置纯 Rust 有界 ZIP/Deflate 解压与 IEEE CRC-32 校验）、**MetaCubeXD**、**Yacd-Meta** 与 **Razord-Meta**。Release 面板严格校验官方 Release 元数据、精确大小与 SHA-256，固定 `gh-pages` 面板校验固定 commit 归档大小与 SHA-256；支持跨面板切换与卸载（`clash.panel.remove`），旧标记向后兼容。
- GeoData 规则库（`ops/clash/geodata.rs`）：对接官方 `MetaCubeX/meta-rules-dat`（`latest`），提供 `standard` / `lite` / `full` 三档预设与本地状态巡检；校验规范 URL、大小与 SHA-256 后以 0600 原子替换，并自动复用 GitHub 加速代理。
- 控制器实时控制（`ops/clash/controller.rs`）：零外部进程直连 HTTP/1.1 控制器并严格防密钥泄露，支持策略组切换、运行模式即时切换与可选 Mixin 持久化（`clash.mode`）、校验热重载及失败回滚（`clash.reload`）、策略组并发测速排序（`clash.proxy.delay`）与活跃连接统计/全量断开（`clash.connections`）。
- 核心设置、DNS 与 Mixin（`ops/clash/settings.rs`）：在停止状态下事务化管理 `mixed-port`、`allow-lan`、`ipv6`、HTTP/TLS/QUIC 域名嗅探（`sniffer`）与 `log-level`，支持 `fake-ip` / `redir-host` / `off` 及国内外 DoH 预设与标准 `fake-ip-filter`，并提供密钥脱敏的 Mixin 查看（`clash.mixin.status`）与安全重置（`clash.mixin.reset`）。

默认数据目录为 `$XDG_DATA_HOME/cqs/clash`，未设置 `XDG_DATA_HOME` 时为 `$HOME/.local/share/cqs/clash`；数据路径可通过对应 `CQS_CLASH_*` 变量覆盖。下载只接受无凭据 HTTPS，订阅须含 `proxies` 或 `proxy-providers`。

## 环境与依赖

### TUI 状态与主题

启动默认进入独立「概览仪表盘」（`H` 随时回位），顶部保留 1 行微状态条（品牌、面包屑、紧凑指标胶囊/运行任务胶囊、主机、权限与语言切换），左侧为可折叠导航栏（`[` 切换，置顶概览与一键优化，下方 11 个功能模块按分组呈现并持久化顺序）。在功能页中采用混合直觉导航：`↑↓/j k` 控操作列表，`←→/Tab/1-9` 秒切模块，`/` 或 `Ctrl+K` 唤起增强命令面板，`T` 在右侧打开实时外观抽屉（含透明背景 `B` 与减弱动效 `M` 开关）。

`src/theme` 不依赖 ratatui，定义颜色、20 个表面与语义色角色、精简 `StyleFlags`、自定义主题文件读写、选择优先级和 WCAG 自适应色阶派生。内置 8 套主题：`auto`、`obsidian`、`daylight`、`tokyo-night`、`catppuccin`、`rose-pine`、`high-contrast`、`plain`。`auto` 根据终端背景自动派生满足对比度要求的 5 层表面色阶；不可探测时回退 `obsidian`。旧版配置中的主题 ID 自动平滑迁移；无效主题回退 `auto` 并在提示与外观抽屉中保留诊断信息。

背景查询使用 terminal-colorsaurus 1.0.3，在进入 raw mode/alternate screen、读取键盘事件之前仅执行一次。关闭检测返回 Disabled；非终端返回 Unavailable；TTY 查询超时为 900 ms，库负责恢复终端模式。Unix 查询后 tcflush 清空输入；进入界面后排空 crossterm 队列。

`Step_Event` 包含编号、总数、Action ID、开始/结束状态，以及结束结果与用时。CLI 保持原进度行；TUI 更新右侧步骤进度与底部/全屏语义日志。`BatchReport` 为每项保留 outcome/elapsed，未运行项为 None；同步交还终端的路径返回后回放报告，不伪造跳过项的成功。`Session_Records` 仅存在于内存，最近使用去重保留 10 项（供概览快捷卡与命令面板使用），每个操作覆盖保存本会话最近结果，退出不落盘。

居中悬浮弹层具备背景压暗（Backdrop Scrim）、1-cell 投影与 120 ms 淡入动效，输入无需等动画结束；`reduced_motion = true` 或 `plain` 模式禁用过渡与呼吸流光。`plain` 使用纯 ASCII 字形与终端默认颜色，状态形状不依赖颜色。

### 渲染验证

快照用固定 Snapshot/时钟/权限夹具，不采样真实主机，覆盖宽/中/窄（80×24）/移动端（59×24、40×16）四档断点、深/浅/Plain 主题、概览、工作台、Docker、批量优化进度、外观抽屉、命令面板、表单 Chips、终端提示符预览、高危确认及窄屏侧栏浮层等核心场景。默认逐字节比较，仅显式设置 `CQS_UPDATE_SNAPSHOTS=1` 时更新；并配套全主题 WCAG 对比度属性测试、全断点多主题双语渲染测试、键鼠交互与状态机测试。`examples/tui_snapshot.rs` 支持导出真实渲染缓冲区的 HTML 视觉检视稿。

Linux 验收遵循本地构建 → SFTP 上传 → 文件大小/SHA-256 校验 → 远端只读冒烟及测试；PTY 中的取消/预览/保存验证不等同于系统修改类操作验收。

系统操作仅支持 Debian / Ubuntu Linux。构建要求 Rust 1.88+。内核操作额外要求 Debian 12+ / Ubuntu 24.04+、amd64 / arm64、GRUB、独立内核环境以及完整的当前内核镜像/initramfs。

| 操作 | 主要系统工具 |
| --- | --- |
| 状态 | hostname、uname、lscpu、free、df、uptime、timedatectl、ip、tc、sysctl |
| 软件管理 | apt-get、dpkg、dpkg-query；`tools.gh` 额外使用 curl、dpkg-deb 校验官方 Release `.deb` |
| 网络配置 | sysctl，必要时 modprobe |
| 内核 | curl、dpkg-deb、lsinitramfs、update-grub、systemd-detect-virt；UEFI 需要 mokutil |
| SSH | sshd、ssh-keygen、ss、systemctl |
| Docker 配置 | dockerd（--validate）、systemctl |
| SWAP | fallocate、mkswap、swapon、swapoff |
| 交互式第三方测试 / 安装器 | curl、Bash、交互终端 |
| AI 二进制安装 | curl、CA 证书；校验与解压由 Rust 完成 |
| 提示符 | Starship；配置 zsh 时需要 zsh |

缺少依赖会报错；仅明确选择的软件安装操作调用 apt 或安装器，不在启动时安装依赖。

## 权限、路径和锁

写操作执行前检查权限。提示符只作用于当前 HOME / ZDOTDIR，不自动使用 SUDO_USER。

沿用 `CQS_INSTALL_PATH`、`CQS_STATE_DIR`、`CQS_TERMINAL_CONFIG`、`CQS_BBR_CONFIG`、`CQS_TCP_CONFIG`、`CQS_KERNEL_LOCK`、`CQS_KERNEL_BACKUP`。自定义路径必须可信；修改目标要求绝对路径，拒绝符号链接、父级跳转和控制字符。

- 网络 / SSH / Docker：`<配置文件>.cqs-lock`。
- 内核、软件安装、系统更新与清理：默认 `/run/lock/cqs-bbrv3.lock`。
- 提示符：`$CQS_STATE_DIR/terminal/config.lock`。
- AI 安装：`$CQS_STATE_DIR/ai/install.lock`，不使用需要 root 的系统软件锁。
- SWAP：`/run/lock/cqs-swap.lock`。

锁通过 fs2 / 系统 flock 持有，与旧脚本对应锁兼容，跨越失败恢复生命周期。锁仅协调遵守该锁的进程；不要删除锁文件来绕过锁。外部 apt 仍有自己的锁。

AI 版本文件放在当前 HOME 的 `.local/share/cqs/ai/<工具>/<版本-架构-唯一后缀>/`，三个入口默认是 `.local/bin/<工具>`，Codex 可通过 `CODEX_INSTALL_DIR` 改目录。归档仅提取普通文件和目录，限制条目数量、单文件和解压总大小，剥离特殊权限，不接受链接、设备或父级路径。Pi 保留完整资源；Codex 要求包清单与目标版本/架构一致，并检查 code-mode host、bwrap、rg 等辅助二进制。

入口切换先在同目录建立临时符号链接；旧普通文件以硬链接备份，旧符号链接保留原始链接文本。rename 原子替换入口后再次验证；失败恢复原入口，恢复失败保留 `.cqs-ai-backup-*` 材料并报错。旧版本文件及成功迁移时的入口备份均保留，避免影响仍在运行的旧进程；不自动修改 PATH、清理软件或管理登录凭据。

## 事务与恢复

### 网络

应用流程：校验白名单 → 取得锁 → 读取运行值 → 保存本次恢复记录 → 保存或验证首次基线 → sysctl 应用候选 → 逐项读回 → 原子替换持久配置。

首次基线保持旧布局：`.cqs-backup/runtime.conf`、`original.conf` 或 `no-original`。新版新增 `.cqs-pending.json`，记录本次运行值、文件快照和是否新建基线。失败恢复本次运行值及文件，恢复成功才删除 pending 和本次新建基线；恢复失败保留材料。撤销恢复首次基线，撤销失败尝试回到撤销前状态。参数键集合不同拒绝覆盖基线。

TCP 上限采用两倍带宽时延积，向上取整到 MiB，受有效内存 1/32 和 64 MiB 限制。内存识别遍历 cgroup v1/v2 的可见祖先限制。多个 sysctl 不是内核级原子事务。

### SSH / Docker

候选经 `sshd -t/-T` 或 `dockerd --validate` 校验后，保存 `.cqs-pending.json` 再提交。服务更新或校验失败恢复文件及此前服务活动状态；恢复失败保留记录。

SSH 端口拒绝活动 ssh.socket 和原本未运行的服务，保留 Match / Include，检查展开端口、ListenAddress 和 SSH 主进程的实际监听。密钥登录导入当前 HOME 的公钥，不生成或输出私钥；确认新连接测试后设置全局公钥、密码及键盘交互认证选项。Match 块保留，管理员需核对其条件覆盖。

Docker 镜像源与 IPv6 共用保留无关字段的 JSON 事务。重启成功不等于容器 IPv6 可达；生成 ULA 不提供公网路由。禁用时删除相关 IPv6 地址字段，启用时仅在没有既有子网时生成 ULA。

### 提示符

继续读取旧版 `original.json`、`options.json`，快照使用 Base64 原始字节和权限。新增、重复配置、目标 Shell 切换和还原均使用 `pending.json` 保存本次 before-image。首次基线不覆盖。

失败或可捕获取消时逆序恢复；失败恢复保留 pending。已有 pending、重复路径、损坏基线或 HOME/ZDOTDIR/配置路径不一致时拒绝继续。Shell 初始化只使用固定模板和正确引用的路径；语言文件、恢复 JSON 不作为代码执行。

### SWAP

只管理 `/swapfile` 及其精确 fstab 条目。自动重建先停用并临时保留旧文件，再创建、启用新文件和提交 fstab。失败恢复旧文件、活动状态和 fstab。记录位于 `/etc/fstab.cqs-swap-pending.json`。不使用 `swapoff -a`。

### 内核

`kernel.install` 提供标准版 / Max 安装。版本留空时，从最近 100 个 Release 中选取匹配架构和类型的最高数字版本；指定版本使用对应 tag，不回退到其他版本。参数校验后取得共享内核锁，检查系统和引导环境，下载并核对 SHA-256、包名、架构、内核配置及驱动。

确认后保存 GRUB 备份并调用 apt-get 安装，随后检查包状态、内核镜像、initramfs 和引导生成结果。失败返回原始错误并恢复 GRUB，保留相关备份。程序不自动重启；重启进入新内核后，`bbr.enable` 配置该内核提供的 BBR 实现和默认队列，`tcp.tune` 独立管理 TCP 参数。

卸载先模拟计划，只允许目标非运行第三方内核的 image/headers；失败保留备份，不恢复可能引用已删除内核的旧引导配置。不自动重启。运行故障内核的主机需要先通过控制台切回可启动内核，不能直接卸载正在运行的内核。

## 取消、日志和边界

捕获输出的子进程使用独立会话和进程组，不继承 TUI 的控制终端。取消发送 SIGINT 并等待退出，再执行恢复，不升级到 SIGKILL。忽略 SIGINT 的进程可能需管理员处理。CLI 捕获 SIGINT / SIGTERM，恢复路径忽略取消标记。运行中 Q/Ctrl+C 会记录退出请求，日志弹窗不会拦截；清理完成后退出。Esc 取消任务后留在界面。

所有内置 apt 调用共用 `ops/packages.rs`：设置 `DEBIAN_FRONTEND=noninteractive`、`UCF_FORCE_CONFFOLD=1`，清空冲突的 `UCF_FORCE_CONFFNEW`，通过 `--force-confdef` / `--force-confold` 保留本机配置，关闭 `Dpkg::Use-Pty`、进度动画和颜色。`NEEDRESTART_MODE=l` 仅列出重启需求，`APT_LISTCHANGES_FRONTEND=none` 关闭交互式变更说明。操作失败仍返回原始错误，写操作中断会提示待包管理器退出后检查 `dpkg --audit`。

TUI 为最近执行任务保留最多 2000 行日志，空闲未执行时不预留日志面板；G 收放日志，F2 查看完整日志。取消、失败和成功分别显示，导航及语言切换不会改写上次任务结果。常驻指标更新独立于任务及其取消标记。普通页面不能勾选或通过 F5 执行优化清单；优化页只使用固定 OPTIMIZATION 顺序，编辑参数仅保存清单。Windows 的系统概览使用同一只读常驻数据，Linux 操作在交互前置灰拦截。

日志过滤完整 CSI、OSC、DCS 等 ANSI 控制序列，避免留下 `[K` 等残片。业务错误使用固定错误键和译文，底层诊断保持原文。断电、SIGKILL 和内核崩溃无法执行恢复代码；遇到 pending 时必须先核对实际状态、保留材料并完成恢复或确认提交，不能直接删除记录强行重试。

批量操作不是整体事务。先验证参数及操作 ID，逐项运行时仍保留确认。BBR/TCP 失败记录并继续，其他失败和取消停止剩余项，整轮有失败返回失败。`--yes` 自动接受操作提示；平台、权限和参数检查仍然执行。

## 测试与验收

真实 PTY 集成测试使 TUI 拥有控制终端，启动可取消的后台下载夹具，再从完整日志窗口分别按 Ctrl+C/Q；验证子进程无法打开 `/dev/tty`、任务退出与终端模式还原。真实 ucf 测试使用临时状态目录和独立 debconf 数据库验证保留本机配置，需要已安装 ucf 和 root，因此默认显式忽略，由 Linux CI 单独运行该项；不会操作宿主 SSH 配置或执行系统升级。可通过 `CQS_TEST_UCF` 指定完整的测试 ucf 安装。

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

覆盖语言键/占位符、语言优先级、CLI 无启动副作用、参数拒绝、中文双宽与多种终端大小、真实 PTY 启动/语言切换/退出、事件通道、真实锁、网络/服务部分失败与恢复、cgroup、Release/卸载计划、真实 dpkg-deb 测试包、内核失败后的引导恢复、提示符字节/权限/旧基线兼容，以及真实 curl + 本地 TLS 下载边界。

常用工具与 AI 接入测试覆盖双语 CLI 元数据、TUI 单项入口、软件锁和 apt 参数，以及二进制元数据、架构、摘要、包布局、越界/链接归档拒绝、非交互安装、升级保留与失败恢复。AI 测试的网络边界由本地路由夹具提供，ELF 夹具使用系统 `cc` 编译；解压、ELF 检查、版本命令与入口切换使用真实实现。测试 PATH 中没有 Node 或 npm，不代表上游在线下载或登录已经验收。

可选的官方包验证使用事先下载的官方 Release JSON 和归档，在临时 HOME 中运行安装、校验及真实 `--version`，不登录。设置 `CQS_AI_SMOKE_DIR` 指向包含 `<工具>-release.json` 和 `<工具>.tar.gz` 的目录，设置 `CQS_AI_SMOKE_TOOL=pi` 或 `codex`，运行 `cargo test --locked --lib official_binary_package_runs_without_node -- --ignored`。此测试默认显式跳过，不能把跳过算作通过。

2026-10-27 二进制安装验证：x86_64 Debian WSL 的 84 项默认单元测试、7 项 CLI/PTY 测试与 1 项原生 sysctl 测试通过。另以官方 Pi 1.4.0、Codex 0.164.0 发布包验证摘要、资源解压、安装和真实版本输出，测试 PATH 中没有 Node/npm。Claude 官方下载站在当前网络下无法连接，仅完成 manifest 解析、ELF 夹具安装与失败恢复测试，未完成其真实二进制下载或登录验证；ARM64 未做实机运行验证。

真实 SSH / Docker 解析测试默认显式忽略，需安装或解包原生工具后运行：

```bash
CQS_TEST_SSHD=/usr/sbin/sshd \
CQS_TEST_DOCKERD=/usr/bin/dockerd \
cargo test --locked --test native -- --ignored
```

测试只解析配置，不启动/重载真实服务。命令边界替身只证明编排与文件恢复；测试包只构建和解析，不安装。

生产验收仍需可恢复的 Debian / Ubuntu VM：真实软件更新、SSH 重载及外部登录、SWAP、Docker 服务/连通性、内核安装与重启、amd64 / arm64 启动和实际线路测试。WSL 自动化测试不能替代这些验证。

2026-10-26 初次迁移验证：x86_64 WSL Debian、Rust 1.99.0；47 项单元测试、5 项 CLI/PTY 测试、1 项原生 sysctl 解析测试及另行运行的 2 项 sshd/dockerd 解析测试通过，共 55 项。fmt、Clippy（-D warnings）、release 构建通过。sshd/dockerd 从 Debian 软件包解包到临时目录，只调用配置解析器，没有安装或启动服务。Rust 1.88 工具链下载未完成，最低版本检查和 amd64/arm64 musl 工作流尚未实际运行。本机只读 BBR 检查能读取内核参数，但因缺少 tc 返回依赖错误，未修改系统。

新增操作需同步固定 ID、权限/参数、两份完整 TOML、成功/失败语义及测试。翻译只存文案，不存命令；业务分支不得依赖某种语言的文字。状态格式变更必须说明兼容与恢复策略。

TUI 交互改造验证：Linux 67 项单元测试、6 项 CLI/PTY 测试、1 项原生 sysctl 解析测试通过，共 74 项；2 项原生 SSH/Docker 扩展测试仍显式跳过。Windows 的 TUI/编辑/鼠标/布局专项和 5 项 CLI 测试通过。两平台 Clippy 严格检查及 release 构建通过。另以实际 Windows 和 WSL 本机采样检查了 120×36、80×24 渲染；公网查询测试使用本地进程边界夹具，没有自动联系公网服务，没有运行系统写操作。

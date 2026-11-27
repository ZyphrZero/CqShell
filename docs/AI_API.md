# CqShell 原生 AI 接口（MCP 与结构化 JSON）

CqShell 内置零 `tokio` 依赖的原生 AI 控制面，对外同时提供两套基于同一套动作引擎与脱敏模型的能力：

1. **原生 MCP（Model Context Protocol）双传输服务器**：
   - **stdio / SSH stdio**：`cqs mcp`（JSON-RPC 2.0 换行分隔帧，支持 `2025-06-18` / `2025-03-26` / `2024-11-05` 协议协商），零常驻进程，适合本机或 SSH 公钥远程直连。
   - **Streamable HTTP + Bearer 密钥认证**：`cqs mcp serve` 与可选 `cqs-mcp.service` systemd 常驻托管（默认监听回环 `127.0.0.1:18765/mcp`、默认只读、强制 `0600` 高熵 Bearer 密钥与 `Origin` 防 DNS Rebinding 校验），供 Cursor / Claude Code / Codex / Antigravity 等跨平台客户端通过 URL + Bearer Token 远程接入。
   - **一键安装与脱敏复制入口**：TUI `Agent 工具 -> MCP 服务与接入` 与全局 `Shift+M`「MCP 接入与脱敏密钥卡」（屏幕始终遮罩、OSC 52 一键复制），以及 CLI `cqs mcp install / uninstall / config / token / service`。
2. **结构化 JSON CLI 控制面**：`status`、`describe`、`choices`、`list --json`、`run --json`、`batch --json` 与 `--params-json` / `--dry-run`，供脚本、CI 或具备终端工具的 AI Agent 以确定性 JSON 编排运维。

---

## 1. 快速接入 MCP 客户端

### 1.1 本机一键安装与卸载（自动增量合并，留存备份）

在安装了 Claude Code、Codex CLI、Cursor 或其他标准 `mcpServers` JSON 客户端的主机上，可直接通过 CLI 或 TUI 一键注册：

```bash
# 注册到 Claude Code (~/.claude.json)、Codex CLI (~/.codex/config.toml)、Cursor (~/.cursor/mcp.json)
cqs mcp install --client claude --transport local_stdio --policy read_only
cqs mcp install --client codex --transport local_stdio --policy read_only
cqs mcp install --client cursor --transport local_stdio --policy read_only

# 注册到任意自定义 mcpServers JSON 文件（例如 Antigravity / Claude Desktop / 项目 .mcp.json）
cqs mcp install --client file --file /path/to/mcp.json --transport http --public-url https://mcp.example.com/mcp

# 可逆卸载（仅删除指定名称条目，保留用户其他 MCP 服务器与设置）
cqs mcp uninstall --client claude --name cqs
```

### 1.2 远程连接方式 A：SSH stdio（免开放端口，复用 SSH 密钥）

无需常驻守护进程，也无需独立 MCP 密钥，直接复用本机 SSH 免密认证与加密隧道：

```bash
# 生成各客户端可用的 SSH stdio 配置片段
cqs mcp config --transport ssh_stdio --client json --ssh-target admin@server.example.com
cqs mcp config --transport ssh_stdio --client claude --ssh-target admin@server.example.com
cqs mcp config --transport ssh_stdio --client codex --ssh-target admin@server.example.com
```

```json
{
  "mcpServers": {
    "cqs": {
      "command": "ssh",
      "args": ["-T", "admin@server.example.com", "cqs", "mcp", "--read-only"]
    }
  }
}
```

### 1.3 远程连接方式 B：Streamable HTTP + Bearer 密钥（URL 直连）

适合 Cursor / Antigravity / Claude Code / Codex CLI 等通过 URL + Header 远程接入的场景：

```bash
# 1. 配置远程 HTTP 参数（默认 127.0.0.1:18765/mcp、Bearer 开启、strict read-only）
cqs run ai.mcp.configure --set mcp_listen=127.0.0.1 --set mcp_port=18765 \
  --set mcp_policy=read_only --set mcp_public_url=https://mcp.example.com/mcp

# 2. 启动前台服务，或安装可选 systemd 常驻服务（user / system 双作用域，UMask=0077）
cqs mcp serve
cqs mcp service install --scope user    # 或 sudo cqs mcp service install --scope system
cqs mcp service status --scope user

# 3. 导出客户端配置（默认脱敏为 <MCP_TOKEN>；显式 --reveal 输出真实 Bearer 密钥）
cqs mcp config --transport http --client json
cqs mcp config --transport http --client json --reveal
cqs mcp config --transport http --client claude --reveal
cqs mcp config --transport http --client codex --reveal

# 4. 密钥管理（存于 0600 文件，禁止命令行参数传明文；轮换后运行中 HTTP 服务即时热切换）
cqs mcp token status
cqs mcp token rotate
cqs mcp token set --file /path/to/private-0600.token
```

> **安全建议**：当 `mcp_listen` 绑定非回环地址（如 `0.0.0.0`）时，Bearer 密钥强制开启且不可关闭；裸 HTTP 会明文传输 `Authorization: Bearer` 请求头，生产公网访问请务必配合 **Nginx/Caddy HTTPS 反代**（并填写 `mcp_public_url`），或保持监听 `127.0.0.1` 配合 **Tailscale / WireGuard / `ssh -N -L 18765:127.0.0.1:18765 admin@server.example.com`** 隧道访问。

### 1.4 TUI 一键复制入口（`Shift+M` 脱敏密钥卡）

在 TUI 中进入 **Agent 工具（`ai` 分组）** 的 **MCP 服务与接入** 区块，或直接按全局快捷键 **`Shift+M`**（命令面板 `/` 搜索 `MCP` 也可直达）：
- 屏幕始终以 `************************` 遮罩隐藏 Bearer 密钥，不落入终端回滚缓冲或任务日志；
- 按 **`Tab`** 实时切换 `Streamable HTTP (Bearer)` / `SSH stdio` / `本机 stdio`，按 **`F`** 切换预览格式；
- 按 **`1`（JSON）** / **`2`（Claude）** / **`3`（Codex）** / **`4`（Bearer 密钥或 SSH 隧道命令）** 通过 OSC 52 直接复制到本机剪贴板；
- 按 **`R`** 原地轮换 `0600` Bearer 密钥并立即生效。

---

## 2. MCP 权限策略与安全护栏

`cqs mcp` 在进程边界提供四类硬性安全门禁，且无论客户端如何提示都无法绕过：

| 启动参数 | 行为 |
| --- | --- |
| 默认（无参数） | 允许全部只读观测与只读动作；普通/可逆变更需传 `confirm: true`（未传时返回 `confirmation_required` 预检计划）；**高危/破坏性动作（`risk="danger"`）默认拒绝** |
| `--read-only` | 严格只读模式：隐藏 `cqs_docker_operate` 工具，并拒绝所有 `mutates=true` 动作（即使传 `confirm: true`） |
| `--allow-danger` | 授权高危/破坏性操作（仍强制要求每次调用显式 `confirm: true`），与 `--read-only` 互斥 |
| `--allow-group <CSV>` | 仅允许指定分组（如 `network,docker,clash`）；启动时严格校验分组名 |
| `--deny-action <CSV>` | 禁止指定动作 ID（如 `ssh.port,ssh.key,kernel.remove`）；启动时严格校验 ID |

### 内置不变量（不可关闭）

- **零明文密钥泄漏**：绝不调用 SOCKS5 明文读取接口；Mihomo 控制器密钥、SOCKS5 密码、MCP Bearer 密钥与订阅 URL Token 永不写入日志、Toast 或命令行参数；Docker 容器/镜像 Inspect 强制对敏感环境变量脱敏为 `********` 并剥离未脱敏 `raw_json`。
- **防 MCP 自我提权红线**：所有变更 MCP 自身配置、Bearer 密钥、`systemd` 服务及本机客户端注册的动作（`ai.mcp.configure`、`ai.mcp.token.*`、`ai.mcp.service.*`、`ai.mcp.install`、`ai.mcp.uninstall`）均禁止通过 MCP 协议本身被外部 AI 自我调用（即使开启 `--allow-danger` 亦拒绝，仅允许本机 CLI/TUI 操作）；只读的 `ai.mcp.status` 允许脱敏查询。
- **绝不自动重启主机**：所有 JSON / MCP 执行运行于无头捕获上下文（`Context::captured`），`kernel.install` 与 `kernel.boot` 仅完成安装或写入一次性 GRUB 启动项，永不自动执行 `reboot`。
- **阻断交互式终端与无限流**：需要独占 TTY 的第三方脚本（`docker.install`、`test.ip`、`test.ai`、`test.speed`、`test.route`、`test.performance`）以及 `clash.logs follow=on` 均在预检阶段直接返回 `error.tty`，不会阻塞或污染 JSON-RPC stdout。
- **有界输出与协议纯净**：单条 JSON-RPC 消息上限 4 MiB；动作日志上限 2000 行；Docker 日志上限 1000 行（单行 4 KiB 截断 + ANSI 清洗）；Mihomo 日志上限 500 行（全量脱敏）；stdout 严格只写 JSON-RPC 帧。

---

## 3. MCP 能力清单

### 3.1 Tools（13 项，含 MCP Tool Annotations 与 `structuredContent`）

| 工具名 | 性质 | 说明 |
| --- | --- | --- |
| `cqs_system_status` | 只读 | 本机 OS、内核、CPU、内存/cgroup 限额、SWAP、磁盘、时区、BBR/TCP、宿主机 TCP/UDP 连接数、Mihomo 概览与权限（默认不联网，可选 `include_public_ip` / `sample_cpu`） |
| `cqs_list_actions` | 只读 | 按 `group` / `query` / `mutates` / `root` / `non_interactive_only` 检索动作目录与策略可见性 |
| `cqs_describe_action` | 只读 | 获取单个动作完整参数元数据、默认值、动态来源、风险等级与标准 JSON Schema |
| `cqs_list_action_choices` | 只读 | 实时读取本机动态可选项（订阅名、Mihomo 策略组与节点、可切核/可卸载内核） |
| `cqs_run_action` | 读/写 | 预检（`dry_run: true`）或执行单个非交互动作；变更类未传 `confirm: true` 时返回 `confirmation_required` 预检报告 |
| `cqs_run_batch` | 读/写 | 预检或按序执行一组动作，返回逐项耗时、结果与完整日志；任意一项违反策略则整批拒执行 |
| `cqs_docker_snapshot` | 只读 | Docker 引擎、容器、Compose、镜像、卷、网络、磁盘占用与可选单次 `stats` |
| `cqs_docker_inspect` | 只读 | 容器或镜像结构化详情（敏感环境变量强制脱敏） |
| `cqs_docker_logs` | 只读 | 有界拉取容器最近 1–1000 行 ANSI 清洗日志 |
| `cqs_docker_operate` | 写操作 | 受控 Docker 生命周期操作（容器启停重启暂停删除、Compose、镜像、卷、网络、Prune、服务），复核 `cli_preview` 与 L0–L3 风险分级（`--read-only` 下隐藏） |
| `cqs_mihomo_snapshot` | 只读 | 脱敏 Mihomo 状态、端口、TUN、面板、SOCKS5、订阅配额、策略组延迟、活跃连接总数与 TCP/UDP 拆分及可选规则/Providers |
| `cqs_mihomo_logs` | 只读 | 读取脱敏后的 Mihomo 最近 1–500 行日志 |
| `cqs_check_update` | 只读 | 检查 `ZyphrZero/CqShell` 最新 Release 版本（不修改本地文件） |

### 3.2 Resources & Resource Templates

- `cqs://status` — 本机系统与网络状态快照（不触网）
- `cqs://actions` — 完整动作目录与策略可用状态
- `cqs://policy` — 当前 MCP 服务权限策略、进程权限与安全保证
- `cqs://docker/snapshot` — Docker 全量只读快照
- `cqs://mihomo/snapshot` — Mihomo 脱敏只读快照
- `cqs://actions/{action_id}` — 单个动作的完整规格与 JSON Schema
- `cqs://choices/{action_id}` — 单个动作在当前主机上的实时动态可选项

### 3.3 Prompts

- `server-audit` — 只读服务器健康体检工作流
- `safe-optimize` — 先预检后审批的服务器优化规划（可选 `goal`）
- `mihomo-diagnose` — Mihomo 代理与 DNS 分流排障（可选 `domain`）
- `docker-diagnose` — Docker 引擎与容器故障排查（可选 `container`）

---

## 4. 结构化 JSON CLI 接口

除 MCP 外，所有核心观测、发现与编排能力均可通过 CLI 原生输出结构化 JSON（统一包含 `"schema_version": "1"` 与 `"ok": true|false`）：

```bash
# 1. 读取本机 / Docker / Mihomo / 全量结构化快照
cqs status --json
cqs status --section docker --include-stats --json
cqs status --section mihomo --include-rules --include-providers --json
cqs status --section all --json

# 2. 查看动作目录、单动作 JSON Schema 与本机动态可选项
cqs list --json
cqs describe bbr.enable --json
cqs choices clash.proxy.select --json

# 3. 预检（dry-run）与 JSON 参数传入（支持 --params-json '-' 从 stdin 读取）
cqs run bbr.enable --json --dry-run --params-json '{"qdisc":"fq_codel"}'
echo '{"style":"box","color":"tokyo-night","time":true}' | \
  cqs run terminal.preview --json --params-json -

# 4. 变更动作：不加 --yes 返回 confirmation_required 预检；加 --yes 执行并返回日志与耗时
sudo cqs --yes run bbr.enable --json --params-json '{"qdisc":"fq"}'

# 5. 批量预检与执行（返回逐项 steps 耗时与结果）
sudo cqs batch tcp.tune bbr.enable --json --dry-run \
  --params-json '{"tcp.tune":{"rtt":80},"bbr.enable":{"qdisc":"fq"}}'
sudo cqs --yes batch tcp.tune bbr.enable --json \
  --params-json '{"tcp.tune":{"rtt":80},"bbr.enable":{"qdisc":"fq"}}'
```

### 响应状态与退出码约定

- `status` 字段取值：`"completed"`、`"dry_run"`、`"confirmation_required"`、`"policy_denied"`、`"failed"`、`"cancelled"`。
- 退出码保持与现有 CLI 完全一致：
  - `0`：成功（含 `--dry-run` 成功预检）
  - `2`：参数、未知动作、必填项或命令行语法错误（`error.input` / `error.action` / `error.required` / `error.cli`）
  - `130`：操作被取消或中断（`error.cancelled`）
  - `1`：需确认（`error.confirm`）、策略拦截（`error.policy`）或其他运行时错误

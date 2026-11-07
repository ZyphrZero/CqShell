# Mihomo / clashctl Rust 集成

CqShell 的 **Mihomo 代理** 分类提供 24 个 CLI/TUI 动作，涵盖安装、进程、订阅、控制器、策略组、Web 控制台、密钥、日志、TUN 配置和 systemd 托管。所有逻辑由 Rust 实现，不依赖或执行参考项目的 Shell 实现。

## 安装与配置

```bash
cqs --yes run clash.install
cqs --yes run clash.sub.add --set name=main --set url=https://example.invalid/sub --set use=on
cqs run clash.status
```

示例订阅 URL 是占位地址，请替换为自己的 HTTPS 订阅。订阅及 Mixin 是不同文件：订阅更新会刷新主配置，用户的控制器、面板和 TUN 设置保存在 `mixin.yaml`，不被订阅覆盖。

默认数据根目录为 `${XDG_DATA_HOME:-$HOME/.local/share}/cqs/clash`：

| 位置 | 内容 |
| --- | --- |
| `bin/mihomo` | 校验过的官方二进制 |
| `data/config.yaml` | 当前订阅主配置 |
| `data/mixin.yaml` | 本地设置和 Mixin 操作 |
| `data/runtime.yaml` | 合并后、用于进程的配置 |
| `data/profiles.yaml` | 订阅元数据 |
| `data/mihomo.pid` / `mihomo.start` | PID 与 `/proc` 启动时间 |
| `data/mihomo.log` | Mihomo 输出，Linux 权限 `0600` |
| `data/pending.json` | 多文件事务 before-image；正常完成后删除 |
| `data/service-pending.json` | systemd 事务恢复记录 |
| `resources/ui/` | 经过校验的独立版本面板资源 |

支持 `CQS_CLASH_HOME`、`CQS_CLASH_DATA_DIR`、`CQS_CLASH_RESOURCES_DIR`、`CQS_CLASH_BINARY`、`CQS_CLASH_CONFIG`、`CQS_CLASH_MIXIN`、`CQS_CLASH_RUNTIME`、`CQS_CLASH_PROFILES`、`CQS_CLASH_PID`、`CQS_CLASH_PID_START`、`CQS_CLASH_LOG`、`CQS_CLASH_LOCK` 覆盖相应路径。每个实例必须一直使用相同用户和路径。

持久设置修改要求先停止 Mihomo。修改会先合并候选配置；如已安装 Mihomo，还必须通过真实 `-t` 校验，才事务发布 Mixin 和 runtime。失败保留原配置。启动也会恢复未完成的配置事务；恢复失败保留 journal，不伪造成功。

选择订阅、添加并选择订阅、更新当前订阅时，也会在提交前验证与当前 Mixin 合并后的配置。运行中的内核不会自动重启：保存成功后仍需先 `clash.stop` 再 `clash.start`；重复 `clash.start` 不会重新加载。已有选中订阅但主配置丢失时会报错，不会自动退回直连；仅未选择订阅的实例可以使用默认直连配置。

`proxy-env on` 需要可用的 HTTP 和 SOCKS 端口，或者一个同时支持两种协议的 `mixed-port`。只有一种协议时会明确报错，不会把 HTTP 与 SOCKS 端口互相替代，也不会输出不完整的环境变量脚本。

如果 PID 或启动时间记录不完整、无法读取进程信息，或存活进程与记录不匹配，状态为 `unknown`。启动、停止和要求停机的设置操作会拒绝继续，并保留记录；不要为了消除报错直接删除记录或按 PID 强制结束进程。应先核对该实例的进程参数及 `/proc` 启动时间，恢复可信记录或确认原进程已退出。

## clashctl 兼容命令

兼容层保留 Rust 后端、实例路径和安全检查，不执行原项目的 Shell 脚本，也不导入 `CLASHCTL_HOME` 中的数据。

```bash
# Bash；Zsh 将 bash 改为 zsh
eval "$(cqs clashctl shell-init --shell bash)"
clashctl on
clashctl off --stop
clashctl sub add --name main --use 'https://example.invalid/sub'
clashctl sub update --all
clashctl sub rename main home
clashctl node use 'Proxy' 'Node A'
```

Fish 使用 `cqs clashctl shell-init --shell fish | source`。初始化只在当前 Shell 定义 `clashctl`、`clashon`、`clashoff`、`clashstart`、`clashstop`、`clashstatus`、`clashui`、`clashsub`、`clashnode`、`clashtun`、`clashmixin`、`clashsecret`、`clashlog`、`clashupgrade`、`clashupdate`、`clashversion`、`clashhelp`，不会自动写入 `.bashrc`、`.zshrc` 或 Fish 配置。若自行持久化初始化命令，请只在目标用户和对应 Shell 的配置中添加，并保留其余内容。已安装原版 clashctl 时，加载这些函数会明确选择 CqShell 后端；不会自动替换原版程序或数据。

不需要改变父 Shell 环境的命令，也可直接运行 `cqs clashctl COMMAND`。以 `clashctl` 或上述 `clash*` 名称调用同一二进制也会进入兼容入口。没有加载函数时直接运行二进制的 on/off 会报错，不会假装修改了父 Shell。on 失败时保留原代理变量；off 本身不依赖内核或程序状态，off --stop 会先清理代理变量，再返回停止内核的实际结果。

| 原生命令形式 | 当前行为与边界 |
| --- | --- |
| `on` / `off [--stop]` | 父 Shell 代理启闭，on 自动启动；支持 Bash、Zsh、Fish |
| `start` / `stop` / `status` / `ui` | 调用现有生命周期、状态和面板地址；status 同时显示 Shell 代理状态，不显示认证值 |
| `sub add [-n NAME] [-u] [--raw] URL` | HTTPS 原生配置；下载时以 `User-Agent: clash.meta` 请求，按 UA 选择格式的面板据此返回 Clash 配置；仍收到 Base64 节点列表等非 Clash 格式时明确报错。省略名称时使用主机名并避开重名，不做格式转换 |
| `sub list` / `sub ls` / 裸 `sub` | 输出订阅列表，不弹出交互选择器 |
| `sub use NAME` / `sub del NAME` / `sub delete NAME` | 显式选择或删除；不省略名称猜测目标 |
| `sub update [NAME] [--all] [--raw]` | 默认更新当前订阅；--all 逐项更新，任一失败返回非零，不宣称全成功 |
| `sub rename OLD NEW` | 事务更新名称与当前选择，保留原文件路径及未知元数据；复用旧名称不会覆盖已有订阅文件 |
| `node list` / `node ls [GROUP]` | 列出策略组或指定组的成员 |
| `node use GROUP NODE` / `node GROUP NODE` | 显式切换 Selector 节点；不提供交互选择与测速 |
| `tun [on\|off]` | 无参数查询；on/off 保存配置，保留其他 TUN/DNS 设置，要求先停止；另行 start 才应用 |
| `mixin` / `mixin -r` / `mixin -c` | 查看脱敏后的 Mixin、运行或主配置；-e 交互编辑未实现 |
| `secret` | 仅查看控制器与密钥是否配置，不回显密钥；使用 --file、--generate、--clear 修改，拒绝位置参数明文密钥 |
| `log [-n LINES] [-f]` | 读取脱敏的内核日志；不是订阅操作日志 |
| `upgrade [-r] [-v]` | 已停止内核的官方稳定版校验安装；不通过控制器在线升级，不支持 alpha |
| `version` / `-v` / `--version` | CqShell 兼容层版本及实际 Mihomo 版本 |

这些命令的显式写操作直接调用 Rust 操作，不额外弹出通用动作确认；平台、权限、配置校验、锁和事务恢复仍生效。订阅保存后仍需 `clashctl stop` 再 `clashctl start`，不会自动中断现有代理连接。`update` 不会下载原版脚本覆盖 Rust 程序，应重新运行 CqShell 安装器。订阅转换、订阅操作日志、测速、定时更新等尚未实现的功能会明确报错，不能将此命令兼容层理解为整个上游项目已完整移植。

2026-11-03 兼容层验收：Windows 与 WSL Linux 的全量测试、格式检查和 Clippy 通过。通过 Motty 独立服务在 Debian 12 上使用真实 Mihomo 验证 Bash、Zsh 5.9、Fish 3.6 的代理变量、真实 HTTP/SOCKS 请求、节点切换、on 失败保留环境、off --stop 失败仍清理环境；使用本地可信 HTTPS 验证原生命令的订阅添加、默认命名、选择、重命名、复用旧名称、当前/全部更新、删除和密钥脱敏。Fish 及运行库仅从校验过摘要的 Debian 包提取到临时目录，未安装到系统。测试在独立网络命名空间内运行，结束后停止测试实例。

## Web 控制台

```bash
cqs --yes run clash.stop
cqs --yes run clash.panel.install
cqs run clash.panel.status
cqs --yes run clash.start
```

面板使用 MetaCubeX/metacubexd 的官方稳定版 `compressed-dist.tgz`，校验 Release URL、大小和 SHA-256；解压拒绝链接、特殊文件和越界路径。可指定版本：

```bash
cqs --yes run clash.panel.install --set version=v1.273.1
```

安装到新的资源目录，验证后切换 `external-ui` 引用。旧目录保留，安装失败不替换原面板。如果未配置控制器，安装会启用 `127.0.0.1:9090`。默认访问地址为 `http://127.0.0.1:9090/ui/`；以 `clash.panel.status` 的实际输出为准。更改配置后需要启动 Mihomo，单独安装静态资源不会启动服务器。

远程服务器的回环地址可通过 SSH 本地转发访问：

```bash
ssh -L 9090:127.0.0.1:9090 user@server
```

随后在本机浏览器打开该地址，在面板连接表单中填写控制器和密钥。CqShell 不将密钥拼入 URL。默认回环地址不会将控制器开放给公网。

## 控制器、密钥与节点

```bash
cqs --yes run clash.stop
cqs --yes run clash.controller.configure --set controller_address=127.0.0.1:9090
cqs --yes run clash.secret.generate
cqs run clash.controller.status
cqs --yes run clash.start
cqs run clash.proxies
cqs --yes run clash.proxy.select --set proxy_group='Proxy' --set proxy_node='Node A'
```

生成密钥保存在私有 Mixin 的 `secret` 字段，不输出明文。需要在面板中输入时，可在可信编辑器中读取该字段；也可以先在权限 `0600` 的私有文件中准备密钥，再执行：

```bash
cqs --yes run clash.stop
cqs --yes run clash.secret.set --set secret_file=/absolute/path/to/private-secret
cqs --yes run clash.start
```

`secret_file` 只移除结尾的一次 LF/CRLF，限制密钥为至多 1024 字节、不含空格的可打印 ASCII。密钥内容不应直接成为 CLI 参数。`clash.secret.clear` 仅允许控制器未配置或监听回环地址时清除密钥；非回环地址必须先配置密钥。地址仅支持 IP 或 `localhost` 加端口，不支持任意主机名、URL、Unix socket 或 HTTPS 控制器。通配监听地址的本机 API 请求使用对应回环地址。

控制器监听和密钥策略统一校验最终合并配置，覆盖直接启动、systemd 子进程启动、订阅应用和 Mixin 修改，不能通过订阅或手动编辑绕过非回环监听的密钥要求。

节点切换只支持 `Selector` 策略组：先查询节点归属，再 URL 编码组名并 JSON 序列化请求。切换改变运行时选择，不改订阅或 Mixin。请求有取消、超时和大小限制，不使用环境代理，也不跟随重定向。

## 日志

```bash
cqs run clash.logs --set lines=100
cqs run clash.logs --set lines=200 --set follow=on
```

行数支持 `1–2000`，初始尾部读取限制为 1 MiB。跟随模式处理分段行及 Linux 日志轮换，可用 Ctrl+C 或 TUI 取消操作结束。读取时隐藏当前配置中的密钥、代理认证、订阅 URL，以及历史日志中的认证字段和 URL 查询参数。配置无法安全解析时拒绝输出原始日志。

`clash.status` 还显示当前订阅、运行时长、控制器、面板和 TUN 状态。

## TUN

```bash
sudo cqs run clash.tun.status
sudo cqs --yes run clash.stop
sudo cqs --yes run clash.tun.configure --set enabled=on --set tun_stack=mixed --set tun_device=cqs-tun --set auto_route=on --set dns_hijack=on
sudo cqs --yes run clash.start
```

TUN 实例应始终使用同一个 root 用户及数据目录，包括安装和订阅设置。普通用户与 `sudo` 的默认 HOME 不同，不会自动复用同一份配置。

启用和每次启动都要求原生 Linux、root、`CAP_NET_ADMIN`、正确的 `/dev/net/tun` 字符设备，以及未被占用的设备名。WSL、容器和不受支持的平台明确拒绝；不自动提权或创建 root helper。配置动作本身不改变路由，实际路由/DNS 由随后启动的 Mihomo 应用。

停止后使用 `--set enabled=off` 关闭。关闭仅覆盖 `tun.enable`，保留其他 TUN、DNS 和订阅设置；它不需要额外 root 校验，但仍需要该实例文件的访问权限。状态通过 sysfs 及受管进程的 `/proc/<pid>/fdinfo` 判断接口归属，不向其他进程发送信号。

## systemd

原生 systemd Linux 支持 `user` 和 `system` 两种作用域：

```bash
cqs --yes run clash.stop
cqs --yes run clash.service.install --set service_scope=user
cqs --yes run clash.start
cqs run clash.service.status --set service_scope=user
cqs --yes run clash.service.remove --set service_scope=user
```

系统作用域改为 `service_scope=system`，并用 root 运行同一实例。安装只启用自启动，另行执行 `clash.start` 才启动。用户作用域跟随用户服务管理器的生命周期；如需注销后仍运行，应由管理员按主机策略配置 linger，CqShell 不会自动修改该系统策略。

单元使用当前 CqShell 可执行文件的绝对路径、固定的 HOME/实例路径、`Type=forking` 和现有 PIDFile。安装后需保留该可执行文件位置。`clash.start`、`clash.stop` 和 `proxy-env on` 会自动路由到该实例的受管服务，避免重复实例。

安装拒绝覆盖第三方单元、drop-in 或其他作用域的同一实例，失败恢复原 unit/enabled/active。移除会停止并禁用服务，保留二进制、配置和面板资源。WSL 和非 systemd 平台仍使用原有直接进程模式；OpenRC/runit 原生服务适配尚未实现。

## 验证与边界

自动测试涵盖真实 TCP 控制器请求、鉴权和编码、失败配置保留、压缩包边界、密钥脱敏、日志跟随取消、TUN 权限检查、systemd 单元转义和回滚，以及多文件提交中途真实 SIGKILL 后的恢复。

外部资源 smoke 测试默认忽略，可用本机经过官方摘要校验的资源显式运行：

```bash
CQS_CLASH_SMOKE_BINARY=/absolute/path/to/mihomo \
  cargo test --locked --lib ops::clash::controller -- --include-ignored
CQS_CLASH_SMOKE_BINARY=/absolute/path/to/mihomo \
  cargo test --locked --lib ops::clash::regression::real_mihomo_rejects_merged_subscription_before_commit -- --ignored --exact
CQS_PANEL_SMOKE_DIR=/absolute/path/to/cache \
CQS_PANEL_SMOKE_MIHOMO=/absolute/path/to/mihomo \
  cargo test --locked --lib ops::clash::panel::tests::official_archive_publishes_and_passes_real_mihomo_validation -- --ignored --exact
```

面板缓存目录需有官方 `release.json` 和对应的 `archive.tgz`。本次在 WSL Debian 13 用真实 Mihomo 验证控制器切换与 Web 页面/脚本响应；systemd 单元通过真实 `systemd-analyze verify` 静态解析。WSL 验证不能代替原生 TUN 路由、systemd 开机恢复、长期运行、ARM64 运行和公网大订阅验收，这些仍列在集成计划中。

2026-11-03 修复验收：Windows 全量测试与 WSL Linux 全量测试、格式检查及 Clippy 均通过。Debian 12 amd64 上使用已校验的真实 Mihomo v1.19.33，通过 8 项本轮回归测试、真实控制器与启动取消测试、systemd 静态解析，以及 11 项公开 CLI 隔离验收；后者包含真实 HTTP/SOCKS 请求、本地可信 HTTPS 订阅 add/use/update、失败配置保持不变、进程身份恢复和崩溃重启。控制器通配监听测试在只有回环接口的独立网络命名空间中执行，没有开放宿主机控制器端口。

当时该 Debian 12 环境的完整 Mihomo 专项仍有一项原有失败：curl 7.88.1 对分块响应没有及时执行 `--max-filesize`，`release_asset_download_limit_applies_to_chunked_responses` 检测到超限文件已经写入。程序最终会拒绝大小不匹配的资产，但不能据此认为下载期间的落盘上限已得到保证；该兼容性问题不属于上述四项修复。

2026-11-06 下载限额修复：引擎与面板发布资产改为在应用侧按声明大小有界读取 curl 的 stdout，只将限额内的字节写入临时文件；发现额外字节或 I/O 错误时中断并回收子进程。curl 成功退出且实际大小完全匹配后才发布到目标路径，失败时清理临时文件并保留原目标。在未升级 curl 7.88.1 的日本 Debian 12 主机上，原分块 HTTPS 回归、精确/不足长度、原文件保留、限额读取与写入、取消和进程回收测试均通过；完整 `cargo test --locked` 通过 375 项，0 失败，11 项按原配置忽略。运行测试的是本地编译后上传的隔离产物，未替换正式安装，也不代表已验收原生 TUN 路由、开机恢复、ARM64 或默认忽略的外部资源测试。

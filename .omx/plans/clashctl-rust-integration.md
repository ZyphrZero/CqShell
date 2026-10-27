# CqShell Rust Mihomo 集成开发计划

更新时间：2026-10-27

范围：将 `E:\clashctl` 的核心 Mihomo 管理能力重写并集成到 CqShell Rust 版本。独立 Shell 版本、`cqs.sh`、Shell 版测试和 Shell 版功能扩展不在本计划范围内。

状态标记：

- `[x]` 已实现并完成本地验证
- `[~]` 已实现，但需要 Linux/Mihomo 真实环境验收
- `[ ]` 尚未开始
- `[!]` 存在阻塞或需要重新设计

## 需求摘要

目标是让 CqShell Rust 版具备 `clashctl` 的首期核心能力：

1. 管理 Mihomo 官方二进制的安装和更新。
2. 合并主配置与 Mixin，生成安全的 runtime YAML。
3. 启动、停止、查看 Mihomo 状态，并识别进程归属。
4. 管理订阅文件及当前选择的订阅。
5. 通过 CqShell CLI 输出当前父 Shell 可执行的代理环境命令。
6. 复用 CqShell 现有的 `Context`、路径管理、锁、原子写入、事务恢复、双语目录和动作注册机制。

## 当前进度

### 阶段 0：边界与架构

- `[x]` 确认 Rust 版作为唯一实现入口，独立 Shell 版本不纳入新增工作。
- `[x]` 确认集成链路：`cli/tui → actions → ops::clash → runtime/storage`。
- `[x]` 确认数据目录和可覆盖路径变量，见 [`src/config.rs:29`](../../src/config.rs:29) 和 [`src/config.rs:44`](../../src/config.rs:44)。
- `[x]` 引入 `serde_yaml` 处理 Clash/Mihomo 配置，见 [`Cargo.toml`](../../Cargo.toml)。

### 阶段 1：Rust Mihomo 核心模块

- `[x]` 新增 [`src/ops/clash.rs`](../../src/ops/clash.rs)。
- `[x]` 接入 CqShell 错误模型、运行时命令执行、锁和原子存储。
- `[x]` 增加 `State`、订阅元数据和代理配置内部模型。
- `[x]` 增加双语提示、动作标签和参数标签，见 [`locales/en-US.toml`](../../locales/en-US.toml) 与 [`locales/zh-CN.toml`](../../locales/zh-CN.toml)。

### 阶段 2：Mihomo 二进制安装/更新

- `[x]` 解析 GitHub Release 稳定版本和架构资产，入口为 [`src/ops/clash.rs:252`](../../src/ops/clash.rs:252)。
- `[x]` 校验资产 URL、摘要、大小、gzip 解压上限、ELF 架构和实际 `-v` 版本。
- `[x]` 使用临时目录和原子写入替换 Mihomo 二进制。
- `[~]` 已在 WSL2 Debian 13 通过真实 GitHub Release API 完成一次安装验收；独立 Debian/Ubuntu 主机和升级覆盖仍待完成。

验收标准：

- 无摘要、摘要不匹配、架构不匹配、版本不匹配时，旧二进制保持不变。
- 下载失败或解压超限时，不留下半成品入口。
- `x86_64`、`aarch64` 目标的 Release 资产选择明确且可诊断。

### 阶段 3：配置和 Mixin 合并

- `[x]` 结构化读取主 YAML 和 Mixin YAML。
- `[x]` 支持 `rules`、`proxies`、`proxy-groups` 的 `prepend` 和 `append`。
- `[x]` 支持按 `name` 的 `override`。
- `[x]` 支持 `proxy-groups.inject`，并去除重复节点。
- `[x]` 启动前先写临时 runtime 文件并调用 Mihomo `-t`，通过后再发布 runtime 配置。
- `[x]` 拒绝错误的 YAML 类型、错误的序列结构和缺少代理来源的订阅配置。

实现位置：

- [`src/ops/clash.rs:618`](../../src/ops/clash.rs:618)
- [`src/ops/clash.rs:681`](../../src/ops/clash.rs:681)
- [`src/ops/clash.rs:730`](../../src/ops/clash.rs:730)

### 阶段 4：进程生命周期

- `[x]` `clash.start`：生成 runtime、调用 `-t`、启动 Mihomo、写入 PID 记录。
- `[x]` `clash.stop`：只向 CqShell 自己识别出的进程发送 SIGTERM。
- `[x]` `clash.status`：显示运行状态和 HTTP/SOCKS 代理地址。
- `[x]` 记录 PID 对应的 `/proc/<pid>/stat` 启动时间，降低 PID 复用风险。
- `[x]` 匹配二进制路径、resources 路径和 runtime 参数，避免误操作其他同名进程。
- `[~]` 已在 WSL2 Debian 13 使用真实 Mihomo 验证启动、崩溃后重启、SIGTERM 和重复 start/stop；独立 Linux 主机及服务管理器行为仍待完成。

实现位置：

- [`src/ops/clash.rs:79`](../../src/ops/clash.rs:79)
- [`src/ops/clash.rs:91`](../../src/ops/clash.rs:91)
- [`src/ops/clash.rs:171`](../../src/ops/clash.rs:171)

### 阶段 5：订阅管理

- `[x]` `clash.sub.list`：列出订阅及当前选择。
- `[x]` `clash.sub.add`：HTTPS 下载、YAML 校验、Mihomo `-t` 校验、原子保存。
- `[x]` `clash.sub.use`：选择订阅并更新主配置。
- `[x]` `clash.sub.update`：按原 URL 重新下载并更新当前配置。
- `[x]` `clash.sub.remove`：删除订阅，并在删除当前订阅时清理主/runtime 配置。
- `[x]` 保留 `profiles.yaml` 未知字段，兼容 `clashctl` 的元数据布局。
- `[x]` 所有多文件变更使用 pending before-image 事务。
- `[~]` 已用临时受信 CA 的真实 HTTPS fixture 验证 add/use/update/remove；公网订阅源、长期更新、断网恢复和大配置压力仍待完成。

实现位置：

- [`src/ops/clash.rs:396`](../../src/ops/clash.rs:396)
- [`src/ops/clash.rs:427`](../../src/ops/clash.rs:427)
- [`src/ops/clash.rs:481`](../../src/ops/clash.rs:481)
- [`src/ops/clash.rs:504`](../../src/ops/clash.rs:504)
- [`src/ops/clash.rs:543`](../../src/ops/clash.rs:543)

### 阶段 6：CLI、TUI 和本地化

- `[x]` 动作注册已加入 [`src/actions.rs:435`](../../src/actions.rs:435) 至 [`src/actions.rs:503`](../../src/actions.rs:503)。
- `[x]` TUI 已加入 Mihomo 分类，并保持既有 kernel 等分类索引兼容，见 [`src/tui.rs:31`](../../src/tui.rs:31)。
- `[x]` CLI 增加 `proxy-env`，见 [`src/cli.rs:95`](../../src/cli.rs:95) 和 [`src/cli.rs:283`](../../src/cli.rs:283)。
- `[x]` 中英文动作、字段、错误和提示均已补齐。
- `[x]` CLI 参数验证和无副作用元数据查询已覆盖。

## 待开发阶段

### 阶段 7：Linux 真实环境验收

状态：`[~]` 已完成 WSL Debian 13 隔离验收，公网订阅源、提交中断时序和宿主机原生服务环境仍待补充。

验收环境：Debian/Ubuntu Linux、`curl`、可执行 Mihomo、真实 resources 目录、可用 HTTPS 网络。

验收内容：

- 安装指定版本并读取真实 `mihomo -v`。
- 使用直连配置启动和停止。
- 使用含 Mixin 的配置启动并检查最终 runtime。
- 使用真实订阅执行 add/use/update/remove。
- 进程崩溃后再次 `status/start` 的行为。
- 启动过程中取消、SIGTERM、重复 start/stop。
- runtime、PID、pending 文件权限和恢复行为。

本轮已完成（2026-10-27，WSL2 Debian 13）：

- 复用 Cargo 源缓存完成 Linux release 构建；`cargo test --locked --all-targets` 通过 104 项，2 项外部原生服务测试按既有条件跳过。
- 通过真实 GitHub Release API 安装 `v1.19.33` 的 `amd64-v1` Mihomo 资产，验证 SHA-256、ELF 架构和 `mihomo -v`；安装目录位于临时数据目录，未触碰系统路径。
- 在临时 HOME 和 `CQS_CLASH_HOME` 下启动真实 Mihomo，验证 `-t`、Mixin `rules.append` 合并、runtime 发布、重复 `start` 幂等、HTTP/SOCKS 地址展示和 `0600` 文件权限。
- 发送 `SIGKILL` 模拟崩溃后，`status` 正确识别停止状态，随后 `start` 成功重建 PID 记录；`SIGTERM` 停止后 PID 和启动时间记录被清理。
- 使用临时受信 CA 的 HTTPS 订阅 fixture 完成 `add/use/update/remove`；更新后的规则进入主配置，当前订阅删除后主配置和 runtime 被清理，事务文件未残留。
- 重复 `stop` 返回“already stopped”且不留下 PID 文件；构造合法 pending before-image 后触发订阅操作，恢复旧配置、删除 pending 记录并保留 `0600` 权限。
- 使用仅用于验收的慢速 Mihomo 夹具发送 `SIGINT`，确认 `-t` 被取消、子进程退出、runtime 保持原内容且不写入 PID 记录。
- 所有写操作位于 `/tmp/cqs-e2e` 或 `/tmp/cqs-sub-e2e`，未修改宿主 HOME、系统服务或系统配置。

尚未完成：

- 使用公网 HTTPS 订阅源完成一次 add/update 的长配置和断网恢复验收；当前仅验证了真实 TLS、证书校验和本地 HTTPS fixture。
- 精确模拟多文件提交中途遭 `SIGKILL` 的时序；本轮验证了 SIGKILL 后的进程状态恢复和合法 pending before-image 恢复路径。
- 在独立 Debian/Ubuntu 主机上验证发行版服务环境、重启后的进程恢复和长时间运行行为；WSL 验收不等价于 systemd/TUN 环境。

完成标准：所有写操作在临时 HOME/数据目录中完成，不能修改宿主代理、系统服务或系统配置；失败必须保留可诊断日志和恢复材料。

### 阶段 8：TUN 模式

状态：`[ ]` 未开始。

计划内容：

- 读取和更新 `tun.enable`、设备名、route 和 DNS 配置。
- 设计普通用户/管理员权限边界。
- 处理 root helper 或受支持的服务托管方式。
- 检测 TUN 设备归属、链路状态和进程归属。
- 增加启用失败、关闭失败和恢复测试。

完成标准：TUN 开关不会误杀其他 Mihomo 实例；配置和服务状态失败时可恢复；容器、WSL 和不支持 TUN 的环境明确拒绝。

### 阶段 9：控制器、节点和策略组

状态：`[ ]` 未开始。

计划内容：

- 解析 `external-controller` 和 `secret`。
- 对控制器地址、端口和 secret 做安全校验。
- 实现 `/proxies` 查询。
- 实现策略组节点列表、当前节点和切换。
- 对节点名称进行 URL 编码和 JSON 参数化。
- 增加控制器不可用、返回 400/404、secret 错误和节点不存在测试。

完成标准：所有控制器请求只连接配置声明的地址；不把 secret 写入日志；切换失败不修改本地配置。

### 阶段 10：面板、secret 和日志

状态：`[ ]` 未开始。

计划内容：

- 面板资源下载和版本校验。
- `external-ui` 路径校验和状态展示。
- secret 读取、设置和脱敏显示。
- Mihomo 日志查看、尾部跟随和错误诊断。
- 状态页面增加面板 URL、TUN、当前订阅和运行时长。

完成标准：日志和状态输出不泄露 secret；面板资源不会路径穿越；不存在面板时状态查询仍可用。

### 阶段 11：服务管理器适配

状态：`[ ]` 未开始。

当前实现使用 CqShell 自己管理的单个进程，尚未复刻 `clashctl` 对 systemd、systemd-user、OpenRC、runit 和 nohup 的完整差异。

计划内容：

- 明确 CqShell 的服务托管策略。
- systemd user/service unit 生成和校验。
- 非 systemd 环境的兼容实现。
- 服务状态、日志位置、自启动和停止语义。
- 防止用户态和 root 态同时运行两个实例。

完成标准：每种服务管理方式都能报告明确状态，重复启动不会产生双实例，服务配置变更失败可恢复。

### 阶段 12：发布与长期维护

状态：`[ ]` 未开始。

计划内容：

- Linux x86_64/ARM64 真实构建和运行验收。
- Mihomo Release API 变化监控和资产格式兼容策略。
- 订阅格式兼容性回归样本。
- 文档中的配置变量、动作 ID 和恢复格式版本化。
- 增加 Linux CI 中的 Mihomo 模拟进程夹具。
- 记录升级、回滚和不兼容变更。

## 验证计划

已完成：

- `cargo fmt --all -- --check`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked --test cli`
- `cargo test --locked --lib ops::clash::tests`
- Rust 库测试 69 项通过；2 项 Linux cgroup 模拟测试在 Windows 主机上跳过
- `cargo build --release --locked`
- `git diff --check`
- WSL Debian 13：`cargo test --locked --all-targets --target-dir /tmp/cqs-wsl-target`
- WSL 临时目录：真实 Mihomo 安装、启动/重复启动、崩溃恢复、SIGTERM 停止和 HTTPS 订阅 add/use/update/remove

待完成：

1. 公网 HTTPS 订阅源的长配置、断网恢复和重试验收。
2. 独立 Debian/Ubuntu 主机上的重启恢复和服务环境验收。
3. Linux 真实 TUN/服务管理器验收。
4. 控制器节点切换和 secret 脱敏验收。
5. 多架构 Release 资产和冷启动验收。

## 风险与应对

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| Mihomo Release 资产命名或 API 字段变化 | 安装/更新失败 | 严格拒绝未知资产，保留旧二进制；为 Release JSON 增加固定夹具 |
| 订阅格式复杂或包含不支持代理协议 | 启动失败 | 下载后先 YAML 校验，再执行 Mihomo `-t`；保留原配置和失败文件 |
| PID 被系统复用 | 误停止其他进程 | PID + 启动时间 + 完整 argv 三重匹配 |
| Mixin 合并语义与上游差异 | 节点/规则行为变化 | 固定合并规则，增加规则、节点、策略组和 inject 的快照测试 |
| 事务恢复记录损坏 | 配置无法继续更新 | 限制 pending 路径范围，恢复失败保留记录并报告具体文件 |
| TUN 需要 root/helper | 权限和双实例风险 | 在阶段 8 单独设计权限模型，未完成前不伪造支持 |
| Windows 开发环境无法验证 Linux cgroup/TUN | 测试结果不完整 | 保留 Windows 快速测试，Linux CI/VM 承担系统验收 |

## 设计决策

### 决策：先做单进程 Rust 管理，再扩展服务管理器

当前实现直接由 CqShell 管理 Mihomo 子进程，先完成配置、订阅、进程归属和恢复语义。systemd/OpenRC/runit 适配会在真实 Linux 验收后独立设计，避免在没有平台验证时复制 Shell 分支。

### 决策：代理环境通过 Rust CLI 输出

子进程不能直接修改父 Shell 环境，因此 `proxy-env` 只生成命令文本。它属于 Rust CLI 的边界接口，不意味着新增或维护独立 Shell 版本。

### 决策：订阅文件和元数据统一事务提交

订阅内容、`profiles.yaml`、当前配置和 runtime 之间存在一致性要求；统一使用 CqShell `storage::write_transaction`，避免只更新一部分文件后留下假成功状态。

## 下一步

下一开发切片仍是**阶段 7 收口**：补充公网订阅源、提交中断时序和独立 Debian/Ubuntu 主机验收。收口前不扩展 TUN、节点切换或面板功能；WSL 已确认 Mihomo 二进制、runtime YAML、订阅事务和进程恢复的核心行为。

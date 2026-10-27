# 系统软件源与测速

系统管理中新增三个入口，Rust / Shell 版使用相同的操作 ID：

```bash
cqs run system.mirror.test
sudo cqs run system.mirror.apply --set mirror_site=auto
sudo cqs run system.mirror.apply --set mirror_site=ustc
sudo cqs run system.mirror.restore

bash cqs.sh run system.mirror.test
sudo bash cqs.sh run system.mirror.apply --set mirror_site=auto
```

`mirror_site` 可选 `auto`、`official`、`tuna`（清华）、`ustc`（中科大）、`aliyun`（阿里云）。`auto` 在当前服务器测速，显示排名和推荐地址，再确认是否应用；指定站点不先测速。普通用户可以测速，写操作需要 root。`--yes` 接受展示的换源计划，适合已经明确授权的自动化任务。

## 范围与测量方法

接入范围为 Debian 12 / 13、Ubuntu 22.04 / 24.04 / 26.04，amd64 / arm64。拒绝其他版本及可识别的 Armbian、Proxmox VE、Raspberry Pi OS 环境，避免上游同时修改这些系统的附加仓库。Ubuntu ARM64 使用 `ubuntu-ports`。

每次按顺序访问四个镜像的当前版本 `main/binary-<架构>/Packages.xz`，通过 HTTPS 请求前 1 MiB；curl 使用 3 秒建连、8 秒总超时、1 MiB 最大文件限制且不重试。必须收到完整的 HTTP 206 分段响应、正确的 Content-Range、XZ 文件头和有效计时，才纳入排名。下载速度为传输字节数除以包含 DNS / TLS / 建连在内的总时间，同时显示首字节时间。失败原因保留，不以零速或缓存结果代替失败；全部失败时不选择镜像。

这是短时仓库索引下载采样，不是持续带宽测量，不测 ICMP Ping，不保证后续所有软件包都同样快。不在启动时测速，不根据客户端所在地区选择源。安全更新固定使用发行版官方 HTTPS 仓库，不参与普通源排名；换源时仍由 APT 验证安全仓库可用性与签名。

## LinuxMirrors 接入契约

使用 `SuperManito/LinuxMirrors` 的 `ChangeMirrorsLite.sh`：

```text
revision: 76fbf1738c22108cdbcc73973612bdb07ad6789b
SHA-256: 963c605bf114bdb7123eaa71d42c7034ac38916b96a55d9f03afa40f397b9fb6
source: https://github.com/SuperManito/LinuxMirrors
license: MIT
```

运行时从固定 GitHub raw 地址完整下载，核对原始 LF 字节的 SHA-256 并通过 `bash -n` 后执行。摘要来自 Git 提交对象，不能使用 Windows 自动转换为 CRLF 的检出文件计算。不会调用浮动的 main 分支脚本，也不执行下载流。脚本不随 CqShell 分发。

参数固定使用 HTTPS、明确主源和安全源路径，关闭软件升级、缓存清理、清屏及上游备份。CqShell 自行提供兼容两种实现的持久备份。调用使用参数数组，不接收自定义 Shell 片段或任意镜像地址。Rust 版不增加 Python 依赖；独立 Shell 版沿用 Python 3.11+。

上游会**按自己的默认模板重建系统主源**，包括组件、backports 和默认禁用源码仓库；可能把传统配置迁移到 DEB822。这些行为在确认说明中展示。不是对原配置无损替换 URL：系统主源中的第三方仓库、未知镜像域名、版本别名和自定义选项会阻止操作。独立的 `sources.list.d/*.list` / `*.sources` 第三方文件保持原样；其自身的下载错误也会使完整 APT 刷新失败。含多架构或自定义信任设置的主源应手动管理。

目标只包含 `/etc/apt/sources.list` 与当前发行版的 `sources.list.d/debian.sources` 或 `ubuntu.sources`。生成后的主源需包含选中的镜像和官方安全源；传统格式补上发行版 `Signed-By` 限制。随后使用 `APT::Update::Error-Mode=any` 刷新，不能以旧缓存掩盖下载失败。仅刷新索引，不执行 upgrade / autoremove / clean。

## 备份与恢复

沿用软件安装 / 内核操作的 CqShell 共享锁。该锁不协调其他配置管理工具，不要并发修改 APT 配置；APT 自身的锁也不能被删除或绕过。

恢复材料位于 `/etc/apt/.cqs-mirrors/`，目录模式 0700、记录 0600：

- `pending.json`：操作前的原始字节、权限、文件是否存在，以及之前的撤销记录。覆盖下载后的实际提交和 APT 验证阶段。
- `last.json`：最近一次成功操作前后的文件快照。成功后替换，不无限保留历史。

下载或校验失败不改源。执行失败、APT 验证失败或可捕获取消时恢复此前配置；恢复失败保留 pending，下一次应用会拒绝继续。`system.mirror.restore` 优先处理 pending，否则撤销 last；后者发现后续人工编辑时拒绝覆盖。恢复配置后再刷新 APT；旧源无法联网时仍保留恢复后的配置并报告刷新错误。多文件提交不是内核级原子事务，断电后需要运行恢复入口。索引缓存与安装的软件包不属于文件快照。

Rust 与 Shell 使用同一 version=1 JSON 格式，已通过真实文件的交叉恢复测试。恢复记录仅作为数据解析，目标文件名必须与发行版的两个固定目标完全一致。

## 验证

```bash
cargo test --locked --lib ops::mirrors
CQS_TEST_BINARY=target/debug/cqs python3 -m unittest discover -s tests -p 'test_shell_*.py' -v
bash -n cqs.sh tests/mirrors_native.sh
shellcheck cqs.sh tests/mirrors_native.sh
```

`test_shell_mirrors.py` 包含真实 curl + 本地 TLS 下载、错误响应、分段下载、截断、失败恢复、手工编辑冲突和输入拒绝测试。设置 `CQS_LINUXMIRRORS_SCRIPT` 为上述摘要匹配的上游原始脚本，可额外运行 20 组生成器验证（五个版本 × 两种架构 × 两种格式）；此测试只调用上游生成器并写临时路径，不运行主入口或安装软件。

可选的完整 Linux 验证需 root、mount namespace、Debian 13 和公网访问：

```bash
sudo bash tests/mirrors_native.sh /absolute/path/to/cqs /absolute/path/to/cqs.sh
```

该测试将临时 APT 配置、索引、缓存和日志目录挂载到私有 mount namespace 内，运行真实下载、校验、LinuxMirrors 主入口与 APT，再由 Shell 恢复 Rust 的备份。命令输出的 `/tmp/cqs-native-mirrors.*` 目录保留用于检查，不修改宿主源或升级软件。其他发行版及 ARM64 的生成器验证不能替代完整系统实测。

维护操作元数据时可以运行 `python3 tools/sync_shell_metadata.py --binary /absolute/path/to/cqs`，仅重新生成 Shell 内嵌操作和语言数据。升级上游时需重新审查文件写入范围、参数、退出状态、额外命令和生成模板，再同步两端 revision / digest 并重跑验证。

### 2026-10-27 本地验证记录

- Debian 13 x86_64 WSL：98 项 Rust 单元测试、8 项 CLI/PTY、1 项原生 sysctl 检查通过；4 项已有可选 Rust 检查跳过。
- Shell：117 项通过，1 项已有 Starship 检查因缺少原生工具跳过。包含真实 curl/TLS、20 组固定上游生成器组合，以及 Rust/Shell 真实文件交叉恢复。
- `cargo fmt --all --check`、`git diff --check`、Bash 语法和 ShellCheck 通过。Linux 严格 Clippy 被未修改的 `src/ops/docker.rs` 两处 `nonminimal_bool` 告警阻塞；Windows 早期模块检查通过，后续复验因系统提交内存不足未完成。
- Rust 与 Shell 均实测公网镜像下载；当时清华、中科大、阿里云返回有效样本，官方 CDN 超时被排除。该结果仅代表当时的本机线路。
- 私有 mount namespace 中真实下载并校验 LinuxMirrors、运行主入口和 APT 签名验证成功；Shell 恢复了 Rust 快照的原始字节并删除新增源文件。首个测试目录遗漏旧式源依赖的公钥目录，恢复后的刷新如实报错；补齐测试目录的官方公钥后，真实 APT 刷新通过。测试脚本现已使用明确的 Signed-By 修正此夹具。重新跑完整脚本时遇到 GitHub raw 下载超时，在修改源之前停止，未把这些重试计为通过。
- 没有修改宿主 APT 配置或安装/升级软件。Ubuntu 与 ARM64 目前通过生成器验证，尚未完成完整系统换源验收。

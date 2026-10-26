# CqShell

面向 Debian / Ubuntu 的单文件 Shell 工具箱，快捷命令为 `cqs`。

## 简介
集成系统管理、BBR/TCP 网络调优、第三方内核管理、服务器测试、常用工具安装和 Docker 管理。

## 支持列表
仅支持Debian、Ubuntu系统

## 使用方法
### 准备
安装curl
```
apt update -y  && apt install -y curl
```
或者
```
apt update -y  && apt install -y wget
```

或者手动下载本脚本至服务器

### 下载并执行
用curl下载
```
curl -fSs -O https://raw.githubusercontent.com/ZyphrZero/YujuToolBox/main/cqs.sh && chmod +x cqs.sh && ./cqs.sh
```
用wget下载
```
wget -q https://raw.githubusercontent.com/ZyphrZero/YujuToolBox/main/cqs.sh && chmod +x cqs.sh && ./cqs.sh
```

脚本入口现为 `cqs.sh`；源码仓库地址仍沿用 `ZyphrZero/YujuToolBox`。

首次以 root 运行并确认协议后，会将当前脚本安装到 `/usr/local/bin/cqs`。也可显式安装快捷命令：

```bash
sudo bash cqs.sh --install
cqs
cqs --help
cqs --version
```

安装使用脚本自身路径，从其他工作目录运行也能正确安装；重复安装相同内容不会改写文件。协议确认单独保存在 `${XDG_STATE_HOME:-$HOME/.local/state}/cqs/agreement.accepted`，不再修改脚本源码，也不会在启动时自动安装软件包。

使用 `sudo cqs --uninstall` 或主菜单卸载快捷命令。下载的脚本、已安装的功能与恢复快照会保留。

## 功能介绍

| 菜单 | 功能 |
| --- | --- |
| 系统相关 | 软件更新、系统清理、网络调优、时区、SWAP、SSH |
| 测试脚本 | 带宽、IP 质量、回程路由、性能和 I/O 测试 |
| 常用工具 | 软件包安装、zsh 和 Starship |
| Docker 管理 | 安装、状态、镜像源、IPv6、清理和卸载 |

## 代码结构

保持 `cqs.sh` 单文件分发，无需构建或下载额外的本地模块。文件内按职责组织：

- **入口与状态**：`cqs_main` 处理命令行参数、协议确认和快捷命令安装；`source cqs.sh` 只加载定义。
- **公共操作**：依赖检查、权限检查、文件锁、HTTPS 脚本下载集中维护。
- **业务函数**：网络、内核、系统与 Docker 操作保留各自的校验、失败处理和回滚边界。
- **交互菜单**：子菜单以“编号、标签、命令”声明，统一由 `menu_loop` 显示和分发；返回使用 `return`，不递归重启主菜单。
- **测试**：通过 `tests/shell_support.py` 加载实际脚本，不使用正则截取函数；系统变更在临时目录和明确的命令替身中验证。

为兼容已有部署，`.yuju-backup`、`.yuju-lock` 及内核备份/锁的历史路径继续使用，避免更名后丢失恢复基线或绕过旧进程的锁。界面、命令和新临时文件使用 CqShell / cqs 命名。

### BBR 网络管理

入口：“系统相关 → BBR 网络管理”。修改网络参数、安装或卸载内核需要 root 权限；脚本仍为单文件，可直接下载使用。

| 功能 | 行为 |
| --- | --- |
| 状态检查 | 显示运行内核、BBR 运行版本、可用拥塞算法、默认队列参数和 `tc` 显示的实际网卡队列 |
| 队列选择 | BBR + `fq` / `fq_codel` / `fq_pie` / `cake`，使用各队列的默认参数 |
| TCP 调优 | 均衡配置，或按目标线路带宽、RTT 和有效内存计算缓冲区 |
| 撤销配置 | 分别撤销 BBR/队列和 TCP 配置，恢复首次操作前的运行值及原配置文件 |
| 第三方内核 | 安装/更新标准 BBRv3、指定版本安装、可选 Max、查看内核、卸载未运行的第三方版本 |

BBR 的算法实现来自内核。`tcp_congestion_control=bbr` 本身不能证明正在使用 v3；只有运行内核的 `/sys/module/tcp_bbr/version` 明确为 `3`，才显示运行版本 v3。未暴露版本时显示“未知”。`modinfo` 的磁盘模块版本单独显示，不用于认定当前运行版本。

队列选择只修改默认队列参数，不自动重建现有网卡的 root qdisc，避免覆盖已有的多队列、整形或过滤规则。`cake` 默认参数也不等于已配置了线路带宽整形。需要检查实际队列时使用状态菜单或 `tc qdisc show`。

### TCP 缓冲区计算与恢复

手动调优使用目标业务线路的带宽（Mbit/s）和 RTT（毫秒）：

```text
候选上限 = 2 × 带宽 × RTT / 8（先换算成 bytes）
最终上限 = 向上取整到 MiB，再限制到有效内存的 1/32，且最多 64 MiB
```

有效内存取物理内存与可见 cgroup v1/v2 内存限制中的较小值，通过 `/proc/self/cgroup` 和 `mountinfo` 定位当前组并检查可见祖先；子组的 `max` 不会忽略祖先限制。无法解析或定位时拒绝调优。至少需要 64 MiB；输入带宽范围为 `(0, 100000]` Mbit/s，RTT 为 `(0, 10000]` ms。均衡配置以 `1000 Mbit/s、50 ms` 为起点，同样受内存上限约束。“一键优化”使用此均衡配置，不会安装第三方内核。

缓冲区是每个 socket 的上限，不会预分配该容量，也不能保证高并发时的总内存消耗或吞吐提升。本轮调优只管理 TCP 自动收发缓冲区、MTU 探测、输出队列上限及空闲慢启动；保留内核自动计算的 `tcp_mem`，不修改连接追踪、端口范围、ECN 或文件描述符限制。

运行参数会先保存快照，应用后逐项读回校验，再原子替换配置。失败时尝试恢复本次操作前的运行参数；恢复失败会保留恢复材料并显示路径。重复调优保留首次基线，用 `flock` 阻止同一配置并发修改。

| 配置 | 恢复快照 |
| --- | --- |
| `/etc/sysctl.d/99-bbr.conf` | `/etc/sysctl.d/99-bbr.conf.yuju-backup/` |
| `/etc/sysctl.d/99-tcp-tune.conf` | `/etc/sysctl.d/99-tcp-tune.conf.yuju-backup/` |

撤销恢复的是首次管理时的状态，而不是猜测发行版默认值；没有完整快照时会拒绝撤销。其他 sysctl 文件仍可能覆盖本工具配置。旧版本配置中被移出的额外参数及旧的 `limits.conf` 条目不会自动复原，需根据服务器原配置处理。

### 第三方 BBRv3 内核安装与管理

实现参考 [byJoey/Actions-bbr-v3](https://github.com/byJoey/Actions-bbr-v3)，审阅版本为 `c23a60ab2e6668c158273dbb18a4f86c04e0c05d`（2026-10-26）。安装包从其 GitHub Releases 获取；本工具不联网执行该项目的 `install.sh`，也不附带其 IPv6 开关或模块黑名单操作。

安装要求：Debian 12+ / Ubuntu 24.04+，`amd64` / `arm64`，GRUB、当前内核镜像与 initramfs 完整，能够控制主机内核及引导。Docker/LXC/OpenVZ/WSL 中禁止安装或卸载内核。UEFI 环境需 `mokutil` 确认 Secure Boot 已关闭。缺少命令时会明确报出依赖，不会自动安装依赖。

需要 `curl`、`python3`、`sha256sum`、`dpkg`/`dpkg-deb`、`apt-get`、`update-grub`、`lsinitramfs`、`systemd-detect-virt`、`flock`；常用 Debian/Ubuntu 对应包包括 `curl python3 coreutils dpkg apt grub2-common initramfs-tools-core systemd util-linux`。网络调优需要 `python3 procps util-linux`；实际队列查看需要 `iproute2`。

安装流程：

1. 选择标准版、指定版本或 Max。默认更新从最近 100 条 Release 中选择匹配架构与类型的最高稳定版本；指定版本直接查询对应 tag，不会静默改装其他版本。
2. 下载恰好一份匹配的 image 和 headers，排除 debug、libc-dev、预发布和错误架构的包。校验官方仓库下载 URL、GitHub 提供的 SHA-256、`.deb` 元数据及包内 BBR/FQ 配置。摘要用于完整性检查，不等同于独立签名认证，仍需信任第三方发布者。
3. 显示目标内核，输入 `INSTALL` 后才安装，保存 `/var/backups/yuju-bbrv3/` 下的 GRUB 快照，保留旧内核。已知 `mimic/0.7.1` DKMS 不兼容会被拦截，其他 DKMS/包安装错误不会跳过。
4. 检查软件包配置状态、镜像、initramfs 和 GRUB；Hyper-V/Azure 额外检查必要驱动及模块是否进入 initramfs。失败会尝试恢复 GRUB，并报告可能存在部分安装的包，不能直接重启。
5. 不自动重启。使用者自行重启后，通过状态菜单验证运行内核及 BBR 版本；如启动失败，通过服务器控制台在 GRUB 中选择旧内核。

内核安装、卸载、系统更新与系统清理共用 `/run/lock/yuju-bbrv3.lock`，锁覆盖整个操作及失败回退，避免本工具的并发操作恢复过期引导快照或删除刚安装的新内核。目标内核版本与正在运行的版本相同时拒绝覆盖，即使只是软件包 revision 更新；应先启动另一个完整内核后再操作。该锁不能协调其他软件包管理工具，操作期间不要在其他终端运行内核安装、卸载或系统软件包更新。

Max 使用第三方内核中更激进的拥塞控制策略，仅用于自有链路测试；可能牺牲公平性、延迟和丢包表现。普通 TCP 调优不会将现有 BBR 自动升级为 v3 或 Max。

卸载时输入第三方内核的完整版本，再输入 `REMOVE` 确认。只能移除未运行的 `*-joeyblog-bbrv3[-max]`，必须保留当前完整、可启动的内核；如 apt 模拟计划涉及其他包，会拒绝卸载。检测到本工具的引导备份或正在运行第三方内核后，“系统清理”会跳过旧内核删除和自动孤立包卸载，保留回退路径。

### SSH、Docker 与远程脚本

修改 SSH 端口需要 root、正在运行的 `ssh.service`、`sshd`、`systemctl`、`flock` 和 `ss`（iproute2）。端口必须是 1–65535 的整数，候选配置先经过 `sshd -t/-T` 校验，再原子替换并 reload，并确认服务主进程实际监听新端口；失败时恢复原文件。服务原本关闭、`ssh.socket` 正在管理监听端口、Include 配置引入额外端口，或 `ListenAddress` 固定其他端口时，会明确拒绝操作，需先处理对应配置。一键优化遇到端口修改失败会停止，不再继续修改登录方式。操作后请保留当前会话，确认防火墙放行并测试新连接。

Docker IPv6 开关需要 root、`python3`、`flock`、`systemctl` 和支持 `--validate` 的 `dockerd`（Docker 23+）。仅合并 IPv6 相关字段，保留数据目录、日志、镜像源等配置；验证通过后替换文件并重启 Docker，失败时恢复原文件。首次启用且没有既有 IPv6 子网时生成 ULA 子网，这不等同于配置公网 IPv6 路由。服务恢复失败时会保留临时目录中的备份并显示路径。

测速、IP 检测、路由测试和 Docker 安装等远程脚本入口统一使用 HTTPS，禁止重定向降级到 HTTP，完整下载并通过 Bash 语法检查后执行。下载失败、内容为空或语法错误均不会执行；HTTPS 和语法检查不代替发布者签名认证，仍需信任脚本来源。

### 验证

在 Debian/Ubuntu 或 WSL 的 Linux Python 环境中运行：

```bash
bash -n cqs.sh
python3 -B -m unittest discover -s tests -v
```

测试使用临时目录和明确的系统命令替身验证控制流程，不修改宿主机内核参数、引导或 `/etc` 文件。包含真实 `sysctl --dry-run` 解析、`flock` 锁竞争、`dpkg-deb` 测试包构建与校验、嵌套 cgroup 路径解析、SSH/Docker 失败回滚；本地 TLS 服务配合真实 `curl` 验证 HTTPS 降级、残缺下载和执行退出状态。测试需要 `dpkg-deb`；TLS 测试另需 `curl openssl`。原生配置解析测试使用 PATH 中的 `sshd`/`dockerd`，也可通过 `CQS_TEST_SSHD`、`CQS_TEST_DOCKERD` 指定解压后的可执行文件；不会启动真实服务。缺少可选原生工具时相应测试会明确跳过。这些检查不能代替目标服务器上的真实内核启动、服务切换、重启持久化和吞吐量验证。

## 项目参考
https://github.com/kejilion/sh

https://github.com/xykt/IPQuality

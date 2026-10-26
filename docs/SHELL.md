# 独立 Shell 版本

`cqs.sh` 从迁移前版本恢复网络/内核检查、cgroup 内存计算及终端基线处理，并按当前 Rust 实现补齐 52 个操作。它是单文件 Bash 程序，结构化数据、事务与菜单输入使用内嵌 Python；运行时无需仓库、Rust 二进制或 Node.js。

## 启动和依赖

要求 Debian / Ubuntu、Bash 4.4+、Python 3.11+。缺失时由管理员显式安装，脚本启动不会安装依赖。各操作的 apt-get、curl、flock、sysctl、sshd、dockerd 等依赖与 [技术文档](TECHNICAL.md) 相同。

```bash
bash cqs.sh --lang zh-CN
bash cqs.sh --help
bash cqs.sh list --json
bash cqs.sh run terminal.apply --set shell=bash --set color=cyan
sudo bash cqs.sh run ssh.key --set public_key=/root/client.pub
```

默认启动编号菜单；`o` 进入一键优化，默认不选任何项目。选择编号填写参数后加入清单，`s` 预览并执行，`n` 清空，`0` 返回。操作仍需确认；取消或失败不会显示成功。公网信息仅通过 `n` 或 `system.public_ip` 查询。Rust 的 Ratatui 实时仪表盘、鼠标和按键布局不移植到编号菜单。

语言优先级与 Rust 一致；中英文操作名称和说明内嵌，支持同一份 TOML 偏好与外部语言目录。系统命令的原生输出及部分 Shell 诊断保留原文。`list --json` 的操作 ID、权限、默认值、枚举及说明通过真实 Rust CLI 对比测试，不能只增加菜单而遗漏执行分支。

## 安装和恢复

`sudo bash cqs.sh --install` 显式复制完整脚本到 `${CQS_INSTALL_PATH:-/usr/local/bin/cqs}`。该位置可能已有 Rust 版；安装后可用 `cqs --version`、`cqs list` 检查。`--uninstall` 只删除目标命令。发布工作流同时附带 `cqs.sh` 和 `cqs.sh.sha256`；现有 `install.sh` 继续下载 Rust 程序。

Shell 版沿用所有 `CQS_*`、HOME、XDG 和 ZDOTDIR 路径。网络、SSH、Docker、SWAP、提示符的失败回滚使用与 Rust 兼容的恢复记录；检测到 pending 会停止，不能用旧 0.0.1 脚本绕过。内核操作保留 GRUB 备份，不自动重启。SSH 导入公钥并要求 `KEY-TESTED`，不在服务器生成或显示私钥。清理默认保留内核，Docker 卸载默认保留数据。

AI 安装直接验证官方稳定版元数据、摘要、包结构、ELF 架构与实际版本后替换入口。IPQuality 与 Rust 使用相同固定 revision / SHA-256，保留 `-p` 隐私模式，不向上游转发 `--yes`。

## 验证与维护

```bash
bash -n cqs.sh
shellcheck cqs.sh
cargo build --locked
CQS_TEST_BINARY=target/debug/cqs python3 -m unittest discover -s tests -p 'test_shell_*.py' -v
```

测试使用临时 HOME、真实文件、锁、编译的 ELF 和命令边界夹具，检查元数据一致性、参数拒绝、无启动写入、配置合并、下载校验及失败恢复。命令边界夹具不代表真实服务、官方在线下载、ARM64 或内核重启已验证；生产系统写操作仍需在可恢复的 VM 验收。

维护新操作时同步 `src/actions.rs`、语言文件、`cqs.sh` 的执行分支与 `BEGIN BUNDLED CLI DATA` 数据块。CI 会检查中英文完整元数据与 Rust 的一致性。不要把状态文件或远程元数据当作 Shell 代码执行。

# 从 Shell 版迁移到 Rust

0.1.0 将应用业务实现迁移到 Rust。旧 cqs.sh 和依赖 source 的 Python 测试由 Rust 模块及测试替代，旧版本可从 Git 历史查阅。新版不在内部调用旧业务函数。

## 功能对应

| 旧功能 | 新操作 |
| --- | --- |
| 系统信息 | system.info；公网信息独立为 system.public_ip |
| 更新、清理、时区、fail2ban | system.update / clean / timezone / fail2ban |
| SWAP 添加 / 自动 / 删除 | swap.create / auto / remove |
| BBR 状态 / 四种队列 / 撤销 | bbr.status / enable / restore |
| TCP 均衡 / 自定义 / 撤销 | tcp.tune（默认 1000 Mbit/s、50 ms）/ restore |
| BBRv3 标准 / 指定版本 / Max | kernel.install 的 profile / version 参数，留空版本自动选择最新匹配版本 |
| 内核列表 / 卸载 | kernel.list / remove |
| SSH 端口 / 密钥登录 | ssh.port / key |
| 十种工具 / 全部安装 | tools.curl / wget / nano / unzip / tar / tmux / iftop / btop / gdu / fzf 独立入口；tools.install 的 package 参数保留，默认 all |
| AI 工具（新增） | ai.pi.install / ai.claude.install / ai.codex.install / ai.status |
| 终端组件 / 配置 / 状态 / 还原 | terminal.install / apply / status / restore |
| Docker 全部功能 | docker.install / status / clean / mirror / ipv6 / uninstall |
| 六种服务器测试 | test.speed / ip / route / performance / connectivity / io |
| 一键优化 | TUI 一键优化分类、勾选与 F5；或 cqs batch |
| 安装 / 卸载快捷命令 | --install / --uninstall，或 app.install / uninstall |

## 有意调整的行为

- 启动、help、version、列举操作不自动安装或写协议标记；写操作执行时单独确认。
- 公网 IP / ISP 按需查询，启动不联网；Docker 安装明确选择 official / china，不按 IP 静默切换。
- 清理默认处理 apt 缓存和过期 journal；full 完整清理需明确选择。保留全部内核，不再隐式执行旧的删内核、deborphan 或 autoremove。
- SSH 导入用户公钥，确认新连接测试后再关闭密码认证，不在服务器生成并打印私钥。
- Docker 镜像源与 IPv6 均使用 JSON 事务；卸载默认保留数据，purge_data=on 才删除默认数据和配置目录。
- SWAP 只操作 /swapfile，保留失败恢复材料，不使用 swapoff -a 或 drop_caches。
- Starship 由 Rust 下载、校验并读取官方 Release 归档，不再使用内嵌 Shell 安装器或自动代理镜像。
- Speedtest 明确确认条款，结果显示 Mbit/s；连通性检查不把 gai.conf 文本当作所有应用的实际地址优先级。
- 失败返回错误，不沿用部分旧函数用最后一条 echo 掩盖失败的行为。CLI 返回 0（成功）、1（运行失败）、2（参数/启动错误），取消返回 130。

## 升级步骤

1. 结束旧版进程，保留配置和恢复材料，处理未完成的失败操作。新旧程序不得同时修改系统。
2. 构建/下载匹配架构的二进制，先用 --version、languages、list 检查。
3. 保持 HOME、ZDOTDIR 与 CQS_* 路径一致。旧网络 .cqs-backup 和提示符 original.json / options.json 可继续读取。
4. 显式 --install 原子替换旧快捷命令。旧下载脚本不会被程序删除，但不应再运行。
5. 先做只读检查，在可恢复 VM 完成写操作验收后再用于生产服务器。

网络、SSH 和 Docker 新增 .cqs-pending.json；SWAP 新增 /etc/fstab.cqs-swap-pending.json；终端继续使用 pending.json。存在未完成记录时拒绝覆盖，卸载不删除这些文件。

旧网络/终端基线兼容已纳入 Rust 测试，不代表所有部署环境均已验证。旧程序不理解新增恢复记录，失败后不能直接切回旧程序继续写入。

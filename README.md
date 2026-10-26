# CqShell

面向 Debian / Ubuntu 的单文件 Shell 工具箱，集成系统维护、网络调优、服务器测试和 Docker 管理，快捷命令为 `cqs`。

## 快速开始

以 root 用户运行，需要先安装 `curl`：

```bash
curl -fSsL -o cqs.sh https://raw.githubusercontent.com/ZyphrZero/CqShell/main/cqs.sh
bash cqs.sh
```

首次确认协议后会自动安装快捷命令，后续直接运行：

```bash
cqs               # 打开菜单
cqs --help        # 查看帮助
cqs --uninstall   # 卸载快捷命令
```

卸载快捷命令不会删除已安装的软件和恢复快照。

## 核心功能

| 功能 | 内容 |
| --- | --- |
| 系统管理 | 软件更新、清理、时区、SWAP、SSH 配置 |
| BBR / TCP | 状态检测、队列选择、带宽与 RTT 调优、配置撤销 |
| 内核管理 | BBRv3 标准版 / Max、指定版本安装、卸载未运行的内核 |
| 服务器测试 | 带宽、IP 质量、回程路由、性能和磁盘 I/O |
| 常用工具 | 软件安装、zsh 与 Starship 终端配置 |
| Docker | 安装、状态查看、镜像源、IPv6、清理和卸载 |

## 使用说明

- 修改系统配置需要 root 权限；涉及 SSH 或内核操作前，请备份配置并确保能使用服务器控制台。
- 第三方内核安装支持 Debian 12+ / Ubuntu 24.04+ 的 amd64、arm64 主机，需要 GRUB，不支持在 WSL 或容器中安装。安装后需自行重启验证。
- Max 内核适合实验性吞吐测试；网络调优效果取决于实际线路，不保证提升带宽。

技术实现与开发说明见 [技术文档](docs/TECHNICAL.md)。

## 许可证

见 [LICENSE](LICENSE)。

## 项目参考与上游

- [yuju520/YujuToolBox](https://github.com/yuju520/YujuToolBox)：基础上游项目。
- [byJoey/Actions-bbr-v3](https://github.com/byJoey/Actions-bbr-v3)：BBRv3 功能参考及第三方内核包来源。
- [kejilion/sh](https://github.com/kejilion/sh)：系统管理工具参考。
- [xykt/IPQuality](https://github.com/xykt/IPQuality)：IP 质量检测参考。

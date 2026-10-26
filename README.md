# YujuToolBox
一个Shell脚本工具箱

## 简介
本工具箱集成了一系列有用的系统管理工具和脚本，旨在帮助用户轻松管理和优化Linux服务器。

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
curl -sS -O https://raw.githubusercontent.com/yuju520/YujuToolBox/main/yuju.sh && chmod +x yuju.sh && ./yuju.sh
```
用wget下载
```
wget -q https://raw.githubusercontent.com/yuju520/YujuToolBox/main/yuju.sh && chmod +x yuju.sh && ./yuju.sh
```

## 功能介绍
待补充

### BBR

在“系统相关 → 启用 BBR（内核自带）”中启用，需要 root 权限。脚本检查内核实际提供的拥塞控制算法，必要时尝试加载 `tcp_bbr`，不会安装或替换内核。

BBR 操作只设置 `net.ipv4.tcp_congestion_control=bbr` 和 `net.core.default_qdisc=fq`。运行参数应用并读回验证成功后，才替换 `/etc/sysctl.d/99-bbr.conf`；应用或保存失败会尝试恢复原运行参数，并保留原配置文件。恢复失败时会显示错误及原参数。重复执行也会补齐配置。

TCP 缓冲区等参数由独立的“TCP调优”菜单管理。旧版 `99-bbr.conf` 中的额外调优项会在成功启用后被移出该文件，但已经生效的额外运行参数不会自动复原。

配置文件可能被其他 sysctl 配置覆盖；`default_qdisc` 的值也不代表现有网卡队列已切换。可分别用 `sysctl net.ipv4.tcp_congestion_control net.core.default_qdisc` 和 `tc qdisc show` 检查运行状态，实际吞吐量需要在目标服务器测试。

### 验证

在 Debian/Ubuntu 或 WSL 的 Linux Python 环境中运行：

```bash
bash -n yuju.sh
python3 -B -m unittest discover -s tests -v
```

测试使用临时目录和系统命令替身验证控制流程，不修改宿主机内核参数或 `/etc` 文件。另有真实 `sysctl --dry-run` 的配置解析检查；缺少此选项时会明确标记跳过。这些测试不代替真实 Linux 服务器上的 BBR 启用、重启持久化及吞吐量验证。

## 项目参考
https://github.com/kejilion/sh

https://github.com/xykt/IPQuality

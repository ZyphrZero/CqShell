# 构建、安装与发布

## 用户安装

发布产物面向 Debian / Ubuntu，包含两个 Linux musl 目标：

| 架构 | 文件 |
| --- | --- |
| x86_64 / AMD64 | `cqs-x86_64-unknown-linux-musl.tar.gz` |
| ARM64 / aarch64 | `cqs-aarch64-unknown-linux-musl.tar.gz` |

每个压缩包附带同名 `.sha256` 文件。从 v0.1.2 起，Release 同时提供 `install.sh`、`cqs.sh` 及各自的 `.sha256` 文件。压缩包内的 Rust 可执行文件固定为 `cqs/cqs`，其余内容包括独立 Shell 脚本、许可证、文档、示例配置和翻译文件。旧版 v0.1.0 / v0.1.1 附件保持不变。构建与发布仅面向 Linux。

推荐先下载完整安装器，再运行：

```bash
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fL \
  https://github.com/ZyphrZero/CqShell/releases/latest/download/install.sh \
  -o install.sh && sudo bash install.sh
sudo /root/cqs --lang zh-CN
```

安装器参数：

| 参数 | 行为 |
| --- | --- |
| 无参数 | 下载最新稳定版，安装到 `/root/cqs` |
| `--version v0.1.0` | 下载指定版本；标签必须以 `v` 开头 |
| `--install-dir /absolute/path` | 安装到该目录的 `cqs`；支持空格，要求绝对路径 |
| `--help` | 显示帮助，不联网 |

默认安装需要 root 权限，安装后执行 `/root/cqs`（普通用户使用 `sudo /root/cqs`）。可用 `sudo bash install.sh --install-dir /opt/cqs` 安装到 `/opt/cqs/cqs`，也可以用 `bash install.sh --install-dir "$HOME/.local/bin"` 安装到当前用户目录；将该目录加入 PATH 后即可执行 `cqs`，系统写操作仍需要 root。卸载时删除实际文件，例如 `sudo rm -- /root/cqs`。程序自身的 `--uninstall` 仅移除 `/usr/local/bin/cqs` 全局命令。安装和升级只替换程序，不删除配置或恢复材料。

安装器要求 Linux x86_64 / ARM64、Bash、curl、可信 CA、tar、gzip、coreutils。它先解析一次最新版本标签，然后使用固定标签下载两个文件，避免下载期间新版本发布导致压缩包与校验值混用。文件通过 SHA-256 校验后，仅提取 `cqs/cqs` 的内容，验证 `--version`，最后在安装目录内暂存并原子替换目标。HTTP 降级、缺失文件、校验失败、无法运行或版本不符都会返回非零退出码。

如临时目录挂载为 `noexec`，可通过 `TMPDIR` 指定可执行且可写的临时目录。默认下载地址是 `https://github.com/ZyphrZero/CqShell/releases`。Fork 或受信镜像可用 `CQS_RELEASE_BASE_URL` 指定另一 HTTPS 地址；它必须实现同样的 `/latest` → `/tag/vX.Y.Z` 重定向和 `/download/vX.Y.Z/<文件>` 路径。该地址决定程序和校验值的来源，只应指向自己信任的发布方；脚本不支持关闭 TLS 或跳过校验。

## CI 和构建

- `.github/workflows/ci.yml`：分支推送、PR、手动运行，以及被发布流程复用。仅在 Linux 执行 fmt、Clippy、Rust 测试、release 构建、Rust 1.88 最低版本检查、ShellCheck、Shell 行为与元数据对齐测试，以及安装器 HTTPS 集成测试。
- `.github/workflows/build.yml`：推送 `v*` 标签或手动运行。先核对版本并运行完整 CI，再分别在原生 amd64 / ARM64 Ubuntu runner 上构建 musl 程序、检查版本与语言命令、打包并测试安装器，最后上传 Actions 产物。
- 只有标签推送才进入 Release 发布作业；手动运行（包括选择标签）只构建。发布作业必须等待两个架构都成功，并再次校验压缩包 SHA-256。

工作流默认只有 `contents: read`；仅发布作业使用 `contents: write`，通过 GitHub 自动提供的 `GITHUB_TOKEN` 调用 GitHub CLI，无需另建个人访问令牌。

## 发布一个版本

1. 修改 `Cargo.toml` 的 `package.version`，运行 Cargo 更新并提交对应的 `Cargo.lock`。确保源码、翻译、安装器、文档和 `.github/workflows/` 全部提交并推送；工作流只读取标签指向的提交，不包含本地未提交修改。
2. 对需要发布的提交打标签。标签必须严格等于 `v` 加 `Cargo.toml` 版本，例如当前 `0.1.2` 对应：

   ```bash
   git tag -a v0.1.2 -m "CqShell v0.1.2"
   git push origin v0.1.2
   ```

3. 在 Actions 查看 `Build and release Linux binaries`。检查和两个架构的构建全部通过后，流程创建草稿、上传全部文件，再发布 Release。`0.2.0-rc.1` 这样的版本会标记为预发布，用户必须指定 `--version v0.2.0-rc.1` 安装。
4. 确认 Release 附件中有两个 `.tar.gz`、两个 `.tar.gz.sha256`、`install.sh`、`install.sh.sha256`、`cqs.sh` 和 `cqs.sh.sha256`，共八个文件。

失败的检查或构建不会创建 Release。若上传或发布阶段失败并留下同名未发布草稿，先在 GitHub 核对并删除该草稿，再重跑失败作业。工作流不会覆盖已有的已发布 Release；修复已发布版本应递增版本并使用新标签。

## 本地验证安装器

在 Linux 中运行：

```bash
bash -n install.sh
shellcheck install.sh
cargo build --release --locked
CQS_TEST_BINARY=target/release/cqs \
  python3 -m unittest discover -s tests -p 'test_install.py' -v
```

测试使用实际构建的程序、本地 HTTPS 服务和临时受信证书，实际调用 curl、sha256sum、tar 和安装命令，只写临时测试目录。覆盖安装、指定版本、升级、软链接替换、下载及校验失败、TLS 验证、版本不符和归档异常。构建工作流额外通过 `CQS_TEST_ARCHIVE` 传入本次打包的真实 `.tar.gz`，以检查安装器与发布包格式一致。Python 和 OpenSSL 只用于测试，不是安装器的运行时依赖。

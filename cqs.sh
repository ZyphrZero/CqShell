#!/bin/bash
# Function subshells isolate locals; dispatcher associative keys are literal strings.
# shellcheck disable=SC2030,SC2031
# 项目信息与持久路径集中定义，所有自有配置、恢复材料和锁使用 cqs 命名。
CQS_INSTALL_PATH="${CQS_INSTALL_PATH:-/usr/local/bin/cqs}"
CQS_STATE_DIR="${CQS_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/cqs}"
CQS_BBR_CONFIG="${CQS_BBR_CONFIG:-/etc/sysctl.d/99-cqs-bbr.conf}"
CQS_TCP_CONFIG="${CQS_TCP_CONFIG:-/etc/sysctl.d/99-cqs-tcp.conf}"
CQS_KERNEL_LOCK="${CQS_KERNEL_LOCK:-/run/lock/cqs-bbrv3.lock}"
CQS_KERNEL_BACKUP="${CQS_KERNEL_BACKUP:-/var/backups/cqs-bbrv3}"
CQS_TERMINAL_CONFIG="${CQS_TERMINAL_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/cqs/terminal.toml}"

# ─── 颜色定义 ────────────────────────────────────────────────
white='\033[0m'
red='\033[31m'


# 纯 root 检测：仅打印提示，失败返回 1
require_root() {
    if [[ $EUID -ne 0 ]]; then
        echo -e "${red}提示: ${white}该功能需要root用户才能运行！"
        return 1
    fi
    return 0
}

require_commands() {
    local cmd
    for cmd in "$@"; do
        command -v "$cmd" >/dev/null 2>&1 || { echo "缺少依赖: $cmd" >&2; return 1; }
    done
}

# 调用方在子 shell 中持有局部 FD，操作及 EXIT 回退结束后自动释放。
acquire_lock() {
    local -n descriptor="$1"
    local path="$2" message="${3:-另一个进程正在修改该配置，请稍后重试}"
    require_commands flock || return 1
    cqs_check_path "$path" || return 1
    mkdir -p -- "$(dirname -- "$path")" || return 1
    exec {descriptor}>>"$path" || return 1
    flock -n "$descriptor" || { echo "$message" >&2; return 1; }
}

install_shortcut() (
    local source temporary
    require_root || return 1
    source=$(readlink -f -- "${BASH_SOURCE[0]}") || return 1
    cqs_check_path "$CQS_INSTALL_PATH" || return 1
    mkdir -p -- "$(dirname -- "$CQS_INSTALL_PATH")" || return 1
    [[ -f "$source" && ! -L "$CQS_INSTALL_PATH" ]] || return 1
    if [[ -e "$CQS_INSTALL_PATH" && ! -f "$CQS_INSTALL_PATH" ]]; then
        echo "快捷命令目标不是普通文件: $CQS_INSTALL_PATH" >&2; return 1
    fi
    if [[ "$source" -ef "$CQS_INSTALL_PATH" ]] || cmp -s -- "$source" "$CQS_INSTALL_PATH"; then
        chmod 755 -- "$CQS_INSTALL_PATH"
        return $?
    fi
    temporary=$(mktemp "${CQS_INSTALL_PATH}.XXXXXX") || return 1
    trap 'rm -f -- "$temporary"' EXIT
    trap 'exit 130' INT
    trap 'exit 130' TERM
    install -m 755 -- "$source" "$temporary" && mv -fT -- "$temporary" "$CQS_INSTALL_PATH" || return 1
    echo "已安装快捷命令 cqs: $CQS_INSTALL_PATH"
)

uninstall_shortcut() {
    require_root || return 1
    cqs_check_path "$CQS_INSTALL_PATH" || return 1
    rm -f -- "$CQS_INSTALL_PATH" || return 1
    echo "已卸载 cqs 快捷命令，下载的脚本和恢复快照仍保留"
}

check_bbr_available() {
    local available
    available=$(sysctl -n net.ipv4.tcp_available_congestion_control 2>/dev/null) || return 1
    [[ "$available" =~ (^|[[:space:]])bbr($|[[:space:]]) ]]
}

load_bbr_module() {
    check_bbr_available && return 0
    if ! command -v modprobe >/dev/null 2>&1; then
        echo -e "${red}BBR 尚不可用，且未找到 modprobe；请检查 kmod 软件包或宿主机内核支持${white}" >&2
        return 1
    fi
    if ! modprobe tcp_bbr; then
        echo -e "${red}加载 tcp_bbr 失败，请检查内核模块及容器权限${white}" >&2
        return 1
    fi
    if ! check_bbr_available; then
        echo -e "${red}加载后仍未检测到 BBR 可用算法${white}" >&2
        return 1
    fi
}

valid_bbr_qdisc() {
    case "$1" in fq|fq_codel|fq_pie|cake) return 0 ;; *) return 1 ;; esac
}

apply_bbr_config() {
    local conf_file="$1" qdisc="${2:-fq}"
    valid_bbr_qdisc "$qdisc" || { echo "不支持的队列算法: $qdisc" >&2; return 1; }
    network_apply_config "$conf_file" "net.core.default_qdisc=$qdisc" "net.ipv4.tcp_congestion_control=bbr"
}

# 可选参数用于检查指定的 sysfs 版本文件；正常调用读取运行内核。
# shellcheck disable=SC2120
get_bbr_version() {
    local version_file="${1:-/sys/module/tcp_bbr/version}" value
    if [[ -r "$version_file" ]] && read -r value < "$version_file" && [[ "$value" =~ ^[123]$ ]]; then
        printf 'v%s\n' "$value"
    else
        echo "未知"
    fi
}

# 使用 2 倍带宽时延积，向上取整到 MiB；上限为有效内存的 1/32，最多 64 MiB。
calculate_tcp_buffer() {
    local bandwidth="$1" rtt="$2" memory_kb="$3"
    [[ "$bandwidth" =~ ^[0-9]{1,6}([.][0-9]{1,3})?$ &&
       "$rtt" =~ ^[0-9]{1,6}([.][0-9]{1,3})?$ && "$memory_kb" =~ ^[0-9]{1,12}$ ]] || return 1
    awk -v bw="$bandwidth" -v rtt="$rtt" -v mem="$memory_kb" 'BEGIN {
        if (bw <= 0 || bw > 100000 || rtt <= 0 || rtt > 10000 || mem < 65536) exit 1;
        mib = 1048576;
        cap = int(mem * 1024 / 32 / mib) * mib;
        if (cap > 64 * mib) cap = 64 * mib;
        bytes = 2 * bw * 1000000 / 8 * rtt / 1000;
        rounded = int(bytes / mib) * mib;
        if (rounded < bytes) rounded += mib;
        if (rounded < mib) rounded = mib;
        if (rounded > cap) rounded = cap;
        printf "%.0f\n", rounded;
    }'
}

# 可选 proc 路径仅供离线测试，正常运行使用 /proc。
# shellcheck disable=SC2120
network_memory_kb() {
    command -v python3 >/dev/null 2>&1 || { echo "读取 cgroup 内存限制需要 python3" >&2; return 1; }
    # 可选 proc 路径供离线测试使用；挂载点从 mountinfo 读取，不能假定在挂载根组中。
    python3 - "${1:-/proc}" <<'PY'
import pathlib
import re
import sys

def unescape(value):
    return re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), value)

try:
    proc = pathlib.Path(sys.argv[1])
    memory = int(re.search(r"^MemTotal:\s+(\d+)\s+kB$", (proc / "meminfo").read_text(), re.M)[1])
    memberships = {}
    for line in (proc / "self/cgroup").read_text().splitlines():
        hierarchy, controllers, group = line.split(":", 2)
        if hierarchy == "0" and not controllers:
            memberships["cgroup2"] = group
        elif "memory" in controllers.split(","):
            memberships["cgroup"] = group
    located = set()
    for line in (proc / "self/mountinfo").read_text().splitlines():
        fields, filesystem = line.split(" - ", 1)
        fields, filesystem = fields.split(), filesystem.split()
        kind = filesystem[0]
        if kind not in memberships or (kind == "cgroup" and "memory" not in filesystem[2].split(",")):
            continue
        root, mount = unescape(fields[3]), pathlib.Path(unescape(fields[4]))
        group = memberships[kind]
        if ".." in pathlib.PurePosixPath(group).parts:
            continue
        if root == "/":
            relative = group.lstrip("/")
        elif group == root:
            relative = ""
        elif group.startswith(root.rstrip("/") + "/"):
            relative = group[len(root):].lstrip("/")
        else:
            continue
        directory = mount / relative
        if not directory.is_dir():
            continue
        located.add(kind)
        filename = "memory.max" if kind == "cgroup2" else "memory.limit_in_bytes"
        # 子组的 max 不会取消父组限制；一直检查到当前可见的挂载根。
        while True:
            limit_file = directory / filename
            if limit_file.exists():
                value = limit_file.read_text().strip()
                if value != "max":
                    if not value.isdecimal():
                        raise ValueError("invalid memory limit: " + str(limit_file))
                    memory = min(memory, int(value) // 1024)
            if directory == mount:
                break
            directory = directory.parent
    if memberships.keys() - located:
        raise ValueError("cannot locate the current memory cgroup in visible mounts")
    print(memory)
except (OSError, ValueError, IndexError, TypeError) as error:
    print("无法确定有效内存，未应用调优: " + str(error), file=sys.stderr)
    sys.exit(1)
PY
}

apply_tcp_profile() {
    local conf_file="$1" buffer="$2"
    [[ "$buffer" =~ ^[0-9]{1,8}$ ]] && (( buffer >= 1048576 && buffer <= 67108864 )) || return 1
    network_apply_config "$conf_file" \
        "net.core.rmem_max=$buffer" "net.core.wmem_max=$buffer" \
        "net.ipv4.tcp_rmem=4096 131072 $buffer" "net.ipv4.tcp_wmem=4096 16384 $buffer" \
        "net.ipv4.tcp_moderate_rcvbuf=1" "net.ipv4.tcp_mtu_probing=1" \
        "net.ipv4.tcp_limit_output_bytes=1048576" "net.ipv4.tcp_slow_start_after_idle=0"
}

bbrv3_release_manifest() {
    # GitHub 返回的数据只能生成经过校验的数据清单，不能作为 shell 代码执行。
    python3 - "$@" <<'PY'
import json
import re
import sys

try:
    path, architecture, profile, requested = sys.argv[1:]
    prefix = {"amd64": "x86_64", "arm64": "arm64"}[architecture]
    if profile not in ("standard", "max"):
        raise ValueError("unsupported kernel profile")
    suffix = "-max" if profile == "max" else ""
    pattern = re.compile(re.escape(prefix) + r"-(\d+)\.(\d+)\.(\d+)" + re.escape(suffix))
    with open(path, encoding="utf-8") as source:
        releases = json.load(source)
    if isinstance(releases, dict):
        if "tag_name" not in releases:
            raise ValueError(releases.get("message", "invalid API response"))
        releases = [releases]
    if not isinstance(releases, list):
        raise ValueError("release response must be an array")
    candidates = []
    for release in releases:
        tag = release.get("tag_name", "")
        match = pattern.fullmatch(tag)
        if match and release.get("draft") is False and release.get("prerelease") is False:
            if not requested or requested == tag:
                candidates.append((tuple(map(int, match.groups())), release))
    if not candidates:
        raise ValueError("no matching stable release for this architecture/profile")
    version, release = max(candidates, key=lambda item: item[0])
    tag = release["tag_name"]
    kernel = ".".join(map(str, version)) + "-joeyblog-bbrv3" + suffix
    selected = []
    for kind in ("image", "headers"):
        name_pattern = re.compile(r"linux-" + kind + "-" + re.escape(kernel)
                                  + r"_[A-Za-z0-9.+~\-]+_" + architecture + r"\.deb")
        assets = [asset for asset in release.get("assets", [])
                  if name_pattern.fullmatch(asset.get("name", ""))]
        if len(assets) != 1:
            raise ValueError("expected exactly one matching " + kind + " package")
        asset = assets[0]
        name, url, digest = asset["name"], asset.get("browser_download_url", ""), asset.get("digest", "")
        expected = "https://github.com/byJoey/Actions-bbr-v3/releases/download/" + tag + "/" + name
        if url != expected or not isinstance(digest, str) or not re.fullmatch(r"sha256:[0-9a-fA-F]{64}", digest):
            raise ValueError("untrusted download URL or missing SHA-256 digest: " + name)
        selected.append((name, url, digest[7:].lower()))
    print(tag)
    for asset in selected:
        print("\t".join(asset))
except (ValueError, KeyError, TypeError, AttributeError, OSError) as error:
    print("BBRv3 release validation failed: " + str(error), file=sys.stderr)
    sys.exit(1)
PY
}

bbrv3_supported_system() {
    [[ "$2" =~ ^[0-9]+([.][0-9]+)*$ ]] || return 1
    case "$1" in
        debian) dpkg --compare-versions "$2" ge 12 ;;
        ubuntu) dpkg --compare-versions "$2" ge 24.04 ;;
        *) return 1 ;;
    esac
}

bbrv3_host_preflight() {
    local mode="${1:-install}" virt status audit secure dkms_status
    require_root || return 1
    local -a commands=(dpkg dpkg-query apt-get update-grub systemd-detect-virt)
    case "$mode" in
        install) commands+=(curl python3 sha256sum dpkg-deb lsinitramfs) ;;
        remove) ;;
        *) return 1 ;;
    esac
    require_commands "${commands[@]}" || return 1
    virt=$(systemd-detect-virt --container); status=$?
    if (( status != 1 )) || [[ -e /.dockerenv || -d /proc/vz || "$(uname -r)" == *[Mm]icrosoft* ]]; then
        echo "容器/WSL 或无法确认独立内核环境（$virt），拒绝安装或卸载内核" >&2
        return 1
    fi
    if [[ ! -s /boot/grub/grub.cfg || ! -s "/boot/vmlinuz-$(uname -r)" || ! -s "/boot/initrd.img-$(uname -r)" ]]; then
        echo "需要 GRUB 和当前可启动内核的镜像/initramfs，无法确认时不修改内核" >&2
        return 1
    fi
    [[ "$mode" != remove ]] || return 0
    if [[ -d /sys/firmware/efi ]]; then
        command -v mokutil >/dev/null 2>&1 || { echo "UEFI 环境需要 mokutil 检查 Secure Boot" >&2; return 1; }
        secure=$(LC_ALL=C mokutil --sb-state) || return 1
        [[ "$secure" == *"SecureBoot disabled"* ]] || {
            echo "无法确认 Secure Boot 已关闭；第三方内核未配置可信签名，拒绝继续" >&2
            return 1
        }
    fi
    audit=$(dpkg --audit) || return 1
    [[ -z "$audit" ]] || { printf '存在未完成配置的软件包，请先修复:\n%s\n' "$audit" >&2; return 1; }
    if command -v dkms >/dev/null 2>&1; then
        dkms_status=$(dkms status) || return 1
        if [[ "$dkms_status" =~ mimic[/,][[:space:]]*0\.7\.1([/,[:space:]]|$) ]]; then
            echo "上游报告 mimic/0.7.1 的 DKMS 兼容问题，请先解决后再安装内核" >&2
            return 1
        fi
        [[ -z "$dkms_status" ]] || printf '当前 DKMS 模块（安装时必须构建成功）:\n%s\n' "$dkms_status"
    fi
}

bbrv3_validate_packages() {
    local directory="$1" release="$2" architecture="$3" kind package arch
    local image="$directory/image.deb"
    for kind in image headers; do
        package=$(dpkg-deb -f "$directory/$kind.deb" Package) || return 1
        arch=$(dpkg-deb -f "$directory/$kind.deb" Architecture) || return 1
        if [[ "$package" != "linux-$kind-$release" || "$arch" != "$architecture" ]]; then
            echo "安装包元数据与目标内核不匹配: $package / $arch" >&2
            return 1
        fi
    done
    if ! (set -o pipefail; dpkg-deb --fsys-tarfile "$image" | tar -xOf - "./boot/config-$release") > "$directory/kernel.config"; then
        echo "无法从内核包读取配置" >&2
        return 1
    fi
    if ! grep -Eq '^CONFIG_TCP_CONG_BBR=[ym]$' "$directory/kernel.config" ||
       ! grep -Eq '^CONFIG_NET_SCH_FQ=[ym]$' "$directory/kernel.config"; then
        echo "目标包缺少 BBR 或 FQ 支持" >&2
        return 1
    fi
    bbrv3_check_hyperv "$directory/kernel.config"
}

bbrv3_check_hyperv() {
    local config="$1" initrd="${2:-}" symbol module listing=""
    [[ -d /sys/bus/vmbus ]] || return 0
    if [[ -n "$initrd" ]]; then listing=$(lsinitramfs "$initrd") || return 1; fi
    local -a symbols=(HYPERV HYPERV_STORAGE HYPERV_NET PCI_HYPERV BLK_DEV_SD)
    if grep -q 'CONFIG_HYPERV_VMBUS' "$config"; then symbols+=(HYPERV_VMBUS); fi
    for symbol in "${symbols[@]}"; do
        grep -Eq "^CONFIG_$symbol=[ym]$" "$config" || {
            echo "Hyper-V/Azure 目标内核缺少启动驱动 CONFIG_$symbol" >&2; return 1;
        }
        if [[ -n "$initrd" ]] && grep -qx "CONFIG_$symbol=m" "$config"; then
            case "$symbol" in
                HYPERV)
                    grep -Eq '^CONFIG_HYPERV_VMBUS=[ym]$' "$config" && continue
                    module='hv[-_]vmbus' ;;
                HYPERV_VMBUS) module='hv[-_]vmbus' ;;
                HYPERV_STORAGE) module='hv[-_]storvsc' ;;
                HYPERV_NET) module='hv[-_]netvsc' ;;
                PCI_HYPERV) module='pci[-_]hyperv' ;;
                BLK_DEV_SD) module='sd[-_]mod' ;;
            esac
            if ! grep -Eq "/$module\.ko(\.(xz|gz|zst))?$" <<< "$listing"; then
                echo "initramfs 缺少 Hyper-V/Azure 必需模块: $module" >&2
                return 1
            fi
        fi
    done
}

# The optional boot directory isolates backup tests; CLI keeps /boot/grub.
bbrv3_backup_grub() (
    local backup name boot="${1:-/boot/grub}"
    umask 077
    cqs_check_path "$CQS_KERNEL_BACKUP" directory || return 1
    mkdir -p -- "$CQS_KERNEL_BACKUP" && chmod 700 -- "$CQS_KERNEL_BACKUP" || return 1
    cqs_check_path "$boot" directory || return 1
    for name in grub.cfg grubenv; do cqs_check_path "$boot/$name" || return 1; done
    backup=$(mktemp -d "$CQS_KERNEL_BACKUP/boot.XXXXXX") || return 1
    for name in grub.cfg grubenv; do
        if [[ -f "$boot/$name" ]]; then
            cp -p -- "$boot/$name" "$backup/$name" || return 1
        else
            touch "$backup/no-$name" || return 1
        fi
    done
    uname -r > "$backup/running-kernel" || return 1
    sync -f -- "$backup" || return 1
    printf '%s\n' "$backup"
)

# The optional boot directory permits isolated recovery tests; CLI uses /boot/grub.
bbrv3_restore_grub() (
    local backup="$1" boot="${2:-/boot/grub}" name temporary=""
    cqs_check_path "$backup" directory && cqs_check_path "$boot" directory || return 1
    [[ -s "$backup/grub.cfg" ]] || { echo "Incomplete GRUB backup: $backup" >&2; return 1; }
    # Validate both snapshots before restoring either file.
    for name in grub.cfg grubenv; do
        cqs_check_path "$backup/$name" && cqs_check_path "$backup/no-$name" && cqs_check_path "$boot/$name" || return 1
        [[ -f "$backup/$name" || -f "$backup/no-$name" ]] || { echo "Incomplete GRUB backup: $name" >&2; return 1; }
    done
    trap 'if [[ -n "$temporary" ]]; then rm -f -- "$temporary"; fi' EXIT
    for name in grub.cfg grubenv; do
        if [[ -f "$backup/$name" ]]; then
            temporary=$(mktemp "$boot/.cqs-grub.XXXXXX") || return 1
            cp -p -- "$backup/$name" "$temporary" && mv -fT -- "$temporary" "$boot/$name" || return 1
            temporary=""
        else
            rm -f -- "$boot/$name" || return 1
        fi
        sync -f -- "$boot" || return 1
    done
    echo "GRUB restored; repair package state before rebooting."
)

bbrv3_boot_ready() {
    local release="$1" config="$2" kind status
    for kind in image headers; do
        status=$(dpkg-query -W -f='${Status}' "linux-$kind-$release") || return 1
        [[ "$status" == "install ok installed" || "$status" == "hold ok installed" ]] || return 1
    done
    [[ -s "/boot/vmlinuz-$release" && -s "/boot/initrd.img-$release" ]] || return 1
    lsinitramfs "/boot/initrd.img-$release" >/dev/null || return 1
    bbrv3_check_hyperv "$config" "/boot/initrd.img-$release" || return 1
    update-grub
}

bbrv3_install_kernel() (
    local profile="${1:-standard}" requested="${2:-}" architecture prefix suffix="" tag release
    local work name url digest actual kind backup api lock_fd install_started=0 install_complete=0
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" "另一个内核操作正在进行，请稍后重试" || return 1
    bbrv3_host_preflight || return 1
    local ID="" VERSION_ID=""
    [[ -r /etc/os-release ]] || return 1
    # shellcheck source=/dev/null
    . /etc/os-release
    bbrv3_supported_system "$ID" "$VERSION_ID" || {
        echo "第三方内核安装仅支持 Debian 12+ / Ubuntu 24.04+" >&2; return 1;
    }
    architecture=$(dpkg --print-architecture) || return 1
    case "$architecture" in amd64) prefix=x86_64 ;; arm64) prefix=arm64 ;; *) echo "不支持的架构" >&2; return 1 ;; esac
    case "$profile" in standard) ;; max) suffix=-max ;; *) return 1 ;; esac
    if [[ -n "$requested" && ! "$requested" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "版本号格式应为 x.y.z" >&2; return 1
    fi
    work=$(mktemp -d /var/tmp/cqs-bbrv3.XXXXXX) || return 1
    # Invoked by the EXIT trap while the kernel lock is held.
    # shellcheck disable=SC2317
    bbrv3_finish_install() {
        if (( install_started && ! install_complete )); then
            bbrv3_restore_grub "$backup" || echo "GRUB 恢复失败，请立即通过服务器控制台处理" >&2
            echo "安装未完成，可能存在部分安装的软件包，暂勿重启；引导备份: $backup" >&2
        fi
        rm -rf -- "$work"
    }
    trap bbrv3_finish_install EXIT
    trap 'exit 130' INT
    trap 'exit 130' TERM
    tag=""
    api='https://api.github.com/repos/byJoey/Actions-bbr-v3/releases?per_page=100'
    if [[ -n "$requested" ]]; then
        tag="$prefix-$requested$suffix"
        api="https://api.github.com/repos/byJoey/Actions-bbr-v3/releases/tags/$tag"
    fi
    curl --proto '=https' --proto-redir '=https' -fSL --connect-timeout 15 --max-time 60 \
        "$api" -o "$work/releases.json" || return 1
    bbrv3_release_manifest "$work/releases.json" "$architecture" "$profile" "$tag" > "$work/manifest" || return 1
    read -r tag < "$work/manifest" || return 1
    release="${tag#"$prefix-"}"
    release="${release%-max}-joeyblog-bbrv3$suffix"
    if [[ "$release" == "$(uname -r)" ]]; then
        echo "拒绝覆盖正在运行的内核 $release；请先启动另一个完整内核后再更新此版本" >&2
        return 1
    fi
    echo "第三方来源: byJoey/Actions-bbr-v3；目标: $release ($architecture)"
    [[ "$profile" != max ]] || echo "Max 修改拥塞控制策略，仅适合自有链路吞吐测试，可能增加丢包、排队和延迟"
    echo "下一步下载并校验 image/headers；不会安装 libc-dev，也不会自动重启"
    cqs_confirm "Download verified kernel packages: $release" DOWNLOAD || return $?
    while IFS=$'\t' read -r name url digest; do
        case "$name" in linux-image-*) kind=image ;; linux-headers-*) kind=headers ;; *) return 1 ;; esac
        curl --proto '=https' --proto-redir '=https' -fSL --connect-timeout 15 --max-time 900 --retry 2 \
            "$url" -o "$work/$kind.deb" || return 1
        actual=$(sha256sum "$work/$kind.deb") || return 1
        [[ "${actual%% *}" == "$digest" ]] || { echo "SHA-256 校验失败: $name" >&2; return 1; }
    done < <(tail -n +2 "$work/manifest")
    bbrv3_validate_packages "$work" "$release" "$architecture" || return 1
    echo "两个安装包的 SHA-256、架构、版本和内核配置检查通过"
    echo "即将安装 $release；保留当前内核 $(uname -r)。请确认可以使用服务器控制台或救援模式"
    cqs_confirm "Install $release; retain console/rescue access." INSTALL || return $?
    backup=$(bbrv3_backup_grub /boot/grub) || return 1
    echo "引导备份: $backup"
    # 内核包的安装钩子负责 initramfs；任何钩子/DKMS 失败均停止，绝不跳过。
    install_started=1
    if apt-get install -y --no-remove "$work/image.deb" "$work/headers.deb" &&
        bbrv3_boot_ready "$release" "$work/kernel.config"; then
        install_complete=1
        echo "内核包和引导配置已安装；当前仍运行 $(uname -r)，尚不能宣称新 BBRv3 已生效"
        echo "请自行重启并通过 BBR 状态菜单验证；启动失败时从 GRUB 选择旧内核"
    else
        echo "内核安装/引导验证失败，可能存在部分安装的软件包；请查看 apt/DKMS 错误，暂勿重启" >&2
        return 1
    fi
)

bbrv3_list_kernels() {
    local installed
    echo "正在运行: $(uname -r)；BBR 运行版本: $(get_bbr_version)"
    installed=$(dpkg-query -W -f='${db:Status-Abbrev}\t${Package}\n' 'linux-image-*') || return 1
    printf '%s\n' "$installed" | awk '$1 ~ /^.[iUFHWt]/ {print $1, $2}'
}

bbrv3_removable_kernel() {
    local release="$1" running="$2" installed="$3"
    [[ "$release" =~ ^[0-9]+\.[0-9]+\.[0-9]+-joeyblog-bbrv3(-max)?$ && "$release" != "$running" ]] || return 1
    grep -Fxq "linux-image-$release" <<< "$installed" && grep -Fxq "linux-image-$running" <<< "$installed"
}

bbrv3_remove_kernel() (
    local release="$1" installed package status backup plan lock_fd
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" "另一个内核操作正在进行，请稍后重试" || return 1
    bbrv3_host_preflight remove || return 1
    bbrv3_list_kernels || return 1
    installed=$(dpkg-query -W -f='${db:Status-Abbrev}\t${Package}\n' 'linux-image-*') || return 1
    installed=$(awk '$1 ~ /^.[iUFHWt]/ {print $2}' <<< "$installed")
    bbrv3_removable_kernel "$release" "$(uname -r)" "$installed" || {
        echo "只能卸载已安装且未运行的 joeyblog BBRv3 内核；当前内核必须保留为可启动后备" >&2; return 1;
    }
    status=$(dpkg-query -W -f='${Status}' "linux-image-$(uname -r)") || return 1
    [[ "$status" == "install ok installed" || "$status" == "hold ok installed" ]] || {
        echo "当前运行内核的软件包状态不完整，拒绝卸载后备内核" >&2; return 1;
    }
    local -a packages=("linux-image-$release")
    package="linux-headers-$release"
    status=$(dpkg-query -W -f='${db:Status-Abbrev}' "$package" 2>/dev/null)
    [[ ! "$status" =~ ^.[iUFHWt] ]] || packages+=("$package")
    plan=$(LC_ALL=C apt-get -s purge "${packages[@]}") || return 1
    while read -r status package _; do
        if [[ "$status" == Remv || "$status" == Purg ]]; then
            [[ "$package" == "linux-image-$release" || "$package" == "linux-headers-$release" ]] || {
                echo "卸载计划会影响额外软件包 $package，已停止" >&2; return 1;
            }
        fi
    done <<< "$plan"
    printf '仅卸载:\n%s\n' "${packages[@]}"
    cqs_confirm "Remove $release" REMOVE || return $?
    backup=$(bbrv3_backup_grub /boot/grub) || return 1
    if apt-get purge -y "${packages[@]}" && update-grub; then
        echo "指定的非运行内核已卸载；当前内核及网络参数保持不变"
    else
        echo "卸载或更新引导失败；备份: $backup。部分包可能已移除，请在控制台修复引导后再重启" >&2
        return 1
    fi
)

terminal_config() (
    local action="$1" lock_fd state="$CQS_STATE_DIR/terminal"
    shift
    require_commands python3 flock readlink || return 1
    [[ "$state" == "$(readlink -m -- "$state")" ]] || {
        echo "终端状态目录必须为不含符号链接的绝对路径: $state" >&2; return 1;
    }
    if [[ "$action" != status && "$action" != defaults ]]; then
        (umask 077; mkdir -p -- "$state" && chmod 700 -- "$state") || return 1
    fi
    if [[ -d "$state" ]]; then acquire_lock lock_fd "$state/config.lock" || return 1; fi
    python3 - "$action" "$state" "$HOME" "${ZDOTDIR:-$HOME}" "$CQS_TERMINAL_CONFIG" "$@" <<'PY'
import base64
import json
import os
from pathlib import Path
import shlex
import signal
import stat
import sys
import tempfile

action, state_dir, home, zdotdir, config, *values = sys.argv[1:]
state_dir = Path(state_dir)
paths = {"bash": Path(home) / ".bashrc", "zsh": Path(zdotdir) / ".zshrc", "config": Path(config)}
backup = state_dir / "original.json"
options = state_dir / "options.json"
recovery = state_dir / "pending.json"
lock = state_dir / "config.lock"
choices = {"shell": ("bash", "zsh", "both"), "style": ("compact", "full"),
           "color": ("cyan", "green", "magenta"), "time": ("on", "off"), "git": ("on", "off")}
defaults = dict(zip(choices, ("bash", "full", "cyan", "off", "on")))
begin, end = b"# >>> cqs terminal >>>", b"# <<< cqs terminal <<<"


def interrupted(signum, frame):
    raise KeyboardInterrupt(f"终端操作被信号 {signum} 中断")


signal.signal(signal.SIGTERM, interrupted)


def check_path(path):
    if not path.is_absolute() or any(ord(c) < 32 or ord(c) == 127 for c in str(path)) or ".." in path.parts:
        raise ValueError(f"需要无控制字符的绝对路径: {path}")
    for parent in (path, *path.parents):
        if parent.is_symlink():
            raise ValueError(f"拒绝符号链接路径: {parent}")
    if path.exists() and not path.is_file():
        raise ValueError(f"目标不是普通文件: {path}")


def capture(path):
    check_path(path)
    if not path.exists():
        return None
    return {"data": base64.b64encode(path.read_bytes()).decode("ascii"),
            "mode": stat.S_IMODE(path.stat().st_mode)}


def content(data, mode=0o600):
    return {"data": base64.b64encode(data).decode("ascii"), "mode": mode}


def json_content(data):
    return content((json.dumps(data, ensure_ascii=False, indent=2) + "\n").encode())


def remove_synced(path):
    if path.exists():
        path.unlink()
        fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)


def write(path, saved):
    check_path(path)
    if saved is None:
        remove_synced(path)
        return
    data = base64.b64decode(saved["data"], validate=True)
    mode = saved["mode"]
    if type(mode) is not int or not 0 <= mode <= 0o7777:
        raise ValueError("恢复文件权限无效")
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.cqs-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            if path.exists():
                previous = path.stat()
                os.fchown(stream.fileno(), previous.st_uid, previous.st_gid)
            os.fchmod(stream.fileno(), mode)
            os.fsync(stream.fileno())
        os.replace(temporary, path)
        directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def load_options():
    data = json.loads(options.read_text()) if options.exists() else defaults.copy()
    if not isinstance(data, dict) or set(data) != set(choices) or any(data[k] not in choices[k] for k in choices):
        raise ValueError(f"终端配置项无效: {options}")
    return data


def edit_rc(raw, shell, enabled):
    # 仅接管带标记的区块与旧版本逐字追加的初始化行，保留其余字节。
    result, inside = [], False
    legacy = f'eval "$(starship init {shell})"'.encode()
    for line in raw.splitlines(keepends=True):
        stripped = line.rstrip(b" \t\r\n\v\f")
        if stripped == begin:
            if inside:
                raise ValueError("终端初始化区块嵌套，请先检查启动文件")
            inside = True
        elif stripped == end:
            if not inside:
                raise ValueError("终端初始化区块缺少起始标记")
            inside = False
        elif not inside and stripped != legacy:
            result.append(line)
    if inside:
        raise ValueError("终端初始化区块缺少结束标记")
    raw = b"".join(result)
    if enabled:
        if raw and not raw.endswith(b"\n"):
            raw += b"\n"
        block = (f'{begin.decode()}\n'
                 'if command -v starship >/dev/null 2>&1; then\n'
                 f'    export STARSHIP_CONFIG={shlex.quote(str(paths["config"]))}\n'
                 f'    eval "$(starship init {shell})"\n'
                 f'fi\n{end.decode()}\n')
        raw += block.encode()
    return raw


def render(settings):
    color = settings["color"]
    full = settings["style"] == "full"
    prompt = "$username$hostname$directory$git_branch$git_status$cmd_duration$time$line_break$character"
    return f"""# Managed by CqShell; configure from the terminal menu.
add_newline = {str(full).lower()}
format = '{prompt}'

[username]
show_always = true
style_user = 'bold {color}'
style_root = 'bold red'
format = '[$user]($style)'

[hostname]
ssh_only = false
style = 'bold {color}'
format = '[@$hostname]($style) '

[directory]
style = 'bold {color}'
truncation_length = 3
truncation_symbol = '.../'
read_only = ' ro'
format = '[$path]($style)[$read_only](red) '

[git_branch]
disabled = {str(settings['git'] == 'off').lower()}
symbol = 'git:'
style = 'bold {color}'
format = '[$symbol$branch]($style) '

[git_status]
disabled = {str(settings['git'] == 'off').lower()}
style = 'yellow'
format = '([$all_status$ahead_behind]($style) )'
ahead = 'up${{count}}'
behind = 'down${{count}}'
diverged = 'up${{ahead_count}}/down${{behind_count}}'

[cmd_duration]
disabled = {str(not full).lower()}
min_time = 2000
format = '[took $duration](yellow) '

[time]
disabled = {str(settings['time'] == 'off').lower()}
time_format = '%H:%M:%S'
format = '[$time](dimmed white) '

[line_break]
disabled = {str(not full).lower()}

[character]
success_symbol = '[>](bold {color})'
error_symbol = '[>](bold red)'
vimcmd_symbol = '[<](bold {color})'
""".encode()


try:
    for path in (*paths.values(), backup, options, recovery, lock):
        check_path(path)
    all_paths = [os.path.normpath(str(p)) for p in (*paths.values(), backup, options, recovery, lock)]
    if len(set(all_paths)) != len(all_paths):
        raise ValueError("终端配置、启动文件和状态文件路径不能重合")
    if recovery.exists():
        raise ValueError(f"上次终端操作未完成，请先检查恢复材料: {recovery}")
    mapping = {key: str(path) for key, path in paths.items()}
    original = json.loads(backup.read_text()) if backup.exists() else None
    if backup.exists():
        if (not isinstance(original, dict) or set(original) != {"paths", "files"} or original.get("paths") != mapping
                or not isinstance(original.get("files"), dict) or set(original["files"]) != set(paths)):
            raise ValueError(f"恢复基线与当前 HOME/ZDOTDIR/配置路径不一致: {backup}")
        for saved in original["files"].values():
            if saved is not None:
                if not isinstance(saved, dict) or set(saved) != {"data", "mode"}:
                    raise ValueError("Invalid snapshot fields")
                base64.b64decode(saved["data"], validate=True)
                if type(saved["mode"]) is not int or not 0 <= saved["mode"] <= 0o7777:
                    raise ValueError("恢复基线权限无效")
    settings = load_options() if action != "restore" else defaults.copy()
    if action == "status":
        print("状态: " + ("已启用" if original else "未启用"))
        print("配置: " + ", ".join(f"{k}={v}" for k, v in settings.items()))
        print(f"提示符文件: {paths['config']}\n恢复基线: {backup}")
        sys.exit(0)
    if action == "defaults":
        print(" ".join(settings[k] for k in choices))
        sys.exit(0)
    current = {key: capture(path) for key, path in paths.items()}
    if action == "apply":
        if len(values) != len(choices) or any(v not in choices[k] for k, v in zip(choices, values)):
            raise ValueError("终端配置参数无效")
        settings = dict(zip(choices, values))
        candidate = {}
        for shell in ("bash", "zsh"):
            saved = current[shell]
            raw = base64.b64decode(saved["data"]) if saved else b""
            enabled = settings["shell"] in (shell, "both")
            data = edit_rc(raw, shell, enabled)
            candidate[paths[shell]] = content(data, saved["mode"] if saved else 0o600) if data or saved else None
        candidate[paths["config"]] = content(render(settings))
        if original is None:
            original = {"paths": mapping, "files": current}
        updates = {backup: json_content(original), **candidate, options: json_content(settings)}
    elif action == "restore":
        if original is None:
            raise ValueError("没有终端恢复基线，未修改任何配置")
        updates = {paths[k]: saved for k, saved in original["files"].items()}
        updates.update({options: None, backup: None})
    else:
        raise ValueError("未知终端配置操作")
    before = {path: capture(path) for path in updates}
    # 提交前持久保存本次状态；断电或强制终止后拒绝覆盖恢复线索。
    write(recovery, json_content({str(p): saved for p, saved in before.items()}))
    changed = []
    try:
        for path, saved in updates.items():
            changed.append(path)
            write(path, saved)
    except BaseException as error:
        failures = []
        for path in reversed(changed):
            try:
                write(path, before[path])
            except (OSError, ValueError) as rollback_error:
                failures.append(f"{path}: {rollback_error}")
        if failures:
            raise RuntimeError(f"{error}; 回退失败: {'; '.join(failures)}; 恢复材料: {recovery}")
        remove_synced(recovery)
        raise
    remove_synced(recovery)
    print("已保存终端美化配置" if action == "apply" else "已还原首次美化前的配置")
    print("请关闭当前终端并重新登录后生效；登录 Shell 和已安装软件保持不变")
except (OSError, ValueError, KeyError, TypeError, RuntimeError, KeyboardInterrupt) as error:
    print(f"终端配置失败: {error}", file=sys.stderr)
    sys.exit(130 if isinstance(error, KeyboardInterrupt) else 1)
PY
)

terminal_apply() {
    require_commands starship python3 || return 1
    case "$1" in
        zsh|both) require_commands zsh || return 1 ;;
    esac
    terminal_config apply "$@"
}

cqs_check_path() {
    python3 - "$1" "${2:-file}" <<'PY'
import os
from pathlib import Path
import sys
raw=sys.argv[1]
p=Path(raw)
if (not p.is_absolute() or any(v in ('.','..') for v in raw.split('/'))
        or any(ord(c)<32 or ord(c)==127 for c in raw)
        or any(a.is_symlink() for a in (p,*p.parents))
        or (p.exists() and not (p.is_dir() if sys.argv[2]=='directory' else p.is_file()))):
    print('Unsafe path: '+raw,file=sys.stderr)
    sys.exit(1)
PY
}

# Network snapshots deliberately use the Rust version's on-disk format.
network_apply_config() {
    cqs_network_transaction apply "$@"
}

network_restore_config() {
    cqs_network_transaction restore "$@"
}

cqs_network_transaction() {
    require_commands python3 sysctl || return 1
    python3 - "$@" <<'PY'
import base64
import fcntl
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
import unicodedata

KEYS = {
    "net.core.default_qdisc", "net.ipv4.tcp_congestion_control",
    "net.core.rmem_max", "net.core.wmem_max", "net.ipv4.tcp_rmem",
    "net.ipv4.tcp_wmem", "net.ipv4.tcp_moderate_rcvbuf",
    "net.ipv4.tcp_mtu_probing", "net.ipv4.tcp_limit_output_bytes",
    "net.ipv4.tcp_slow_start_after_idle",
}
cancelled = False

class Cancelled(Exception):
    pass

def cancel(_signal, _frame):
    global cancelled
    cancelled = True

signal.signal(signal.SIGINT, cancel)
signal.signal(signal.SIGTERM, cancel)

def check():
    if cancelled:
        raise Cancelled("操作已取消")

def safe_path(path, file=True):
    path = Path(path)
    if not path.is_absolute() or ".." in path.parts or any(unicodedata.category(c) == "Cc" for c in str(path)):
        raise ValueError(f"不安全的路径: {path}")
    for entry in (path, *path.parents):
        try:
            mode = entry.lstat().st_mode
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(mode) or (entry == path and file and not stat.S_ISREG(mode)):
            raise ValueError(f"拒绝链接或非普通文件: {entry}")
        if entry != path and not stat.S_ISDIR(mode):
            raise ValueError(f"父路径不是目录: {entry}")
    return path

def sync_parent(path):
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)

def atomic_write(path, data, mode):
    safe_path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".cqs-network-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            if path.exists():
                previous = path.stat()
                os.fchown(output.fileno(), previous.st_uid, previous.st_gid)
            os.fchmod(output.fileno(), mode)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        sync_parent(path)
    finally:
        if os.path.lexists(temporary):
            os.unlink(temporary)

def remove(path):
    safe_path(path)
    try:
        path.unlink()
    except FileNotFoundError:
        return
    sync_parent(path)

def capture(path):
    safe_path(path)
    try:
        data = path.read_bytes()
        mode = path.stat().st_mode & 0o7777
    except FileNotFoundError:
        return None
    return {"data": base64.b64encode(data).decode("ascii"), "mode": mode}

def write_saved(path, saved):
    if saved is None:
        remove(path)
    else:
        if set(saved) != {"data", "mode"} or not isinstance(saved["mode"], int) or not 0 <= saved["mode"] <= 0o7777:
            raise ValueError("无效文件快照")
        atomic_write(path, base64.b64decode(saved["data"], validate=True), saved["mode"])

def parse(text):
    values = {}
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise ValueError(f"无效网络参数: {line}")
        key, value = (part.strip() for part in line.split("=", 1))
        if key not in KEYS or not value or key in values or any(unicodedata.category(c) == "Cc" and c != "\t" for c in value):
            raise ValueError(f"不支持或重复的网络参数: {key}")
        values[key] = value
    if not values:
        raise ValueError("恢复快照为空")
    return dict(sorted(values.items()))

def render(values):
    return "".join(f"{key} = {value}\n" for key, value in sorted(values.items())).encode()

def command(args, output=False, recovery=False):
    if not recovery:
        check()
    result = subprocess.run(["sysctl", *args], stdin=subprocess.DEVNULL,
                            text=True, capture_output=True,
                            env=dict(os.environ, LC_ALL="C"))
    if not output and result.stdout:
        print(result.stdout, end="", flush=True)
    if result.stderr:
        print(result.stderr, end="", file=sys.stderr, flush=True)
    if not recovery:
        check()
    if result.returncode:
        raise RuntimeError(f"sysctl {' '.join(args)}: exit {result.returncode}")
    return result.stdout.rstrip()

def read_values(values, recovery=False):
    return {key: command(["-n", key], output=True, recovery=recovery) for key in values}

def normalized(value):
    return " ".join(value.split())

def verify(values, recovery=False):
    for key, expected in values.items():
        actual = command(["-n", key], output=True, recovery=recovery)
        if normalized(actual) != normalized(expected):
            raise RuntimeError(f"参数验证失败: {key}，期望 {expected}，实际 {actual}")

def restore_values(values, recovery=False):
    errors = []
    for key, value in values.items():
        try:
            current = command(["-n", key], output=True, recovery=recovery)
            if normalized(current) != normalized(value):
                command(["-w", f"{key}={value}"], recovery=recovery)
        except Exception as error:
            errors.append(str(error))
    try:
        verify(values, recovery=recovery)
    except Exception as error:
        errors.append(str(error))
    if errors:
        if cancelled and not recovery:
            raise Cancelled("操作已取消")
        raise RuntimeError("恢复原参数失败: " + "; ".join(errors))

def baseline(backup):
    safe_path(backup, file=False)
    if not backup.is_dir():
        raise ValueError(f"没有完整的恢复快照: {backup}")
    for name in ("runtime.conf", "original.conf", "no-original"):
        safe_path(backup / name)
    original = capture(backup / "original.conf")
    if (original is not None) == (backup / "no-original").exists():
        raise ValueError(f"恢复快照不完整: {backup}")
    return parse((backup / "runtime.conf").read_text()), original

def remove_baseline(backup):
    for name in ("runtime.conf", "original.conf", "no-original"):
        remove(backup / name)
    backup.rmdir()
    sync_parent(backup)

def create_baseline(config, backup, before, original):
    staged = Path(tempfile.mkdtemp(prefix=".cqs-baseline-", dir=config.parent))
    try:
        atomic_write(staged / "runtime.conf", render(before), 0o600)
        if original is None:
            atomic_write(staged / "no-original", b"", 0o600)
        else:
            write_saved(staged / "original.conf", original)
        os.rename(staged, backup)
        sync_parent(backup)
    finally:
        if staged.exists():
            remove_baseline(staged)

def apply(config, pending, backup, desired):
    new_baseline = not backup.exists()
    if not new_baseline and baseline(backup)[0].keys() != desired.keys():
        raise ValueError("新配置与恢复快照的参数集合不同，请先撤销原配置")
    before = read_values(desired)
    original = capture(config)
    atomic_write(pending, json.dumps({"runtime": before, "file": original, "new_baseline": new_baseline}, indent=2).encode(), 0o600)
    try:
        if new_baseline:
            create_baseline(config, backup, before, original)
        check()
        fd, candidate = tempfile.mkstemp(prefix=".cqs-network-candidate-", dir=config.parent)
        try:
            with os.fdopen(fd, "wb") as output:
                output.write(render(desired))
            command(["-p", candidate])
            verify(desired)
            check()
            atomic_write(config, render(desired), 0o644)
        finally:
            os.unlink(candidate)
    except Exception as error:
        try:
            restore_values(before, recovery=True)
            write_saved(config, original)
        except Exception as recovery:
            raise RuntimeError(f"{error}; {recovery}; 保留恢复材料: {pending}") from error
        if new_baseline and backup.exists():
            remove_baseline(backup)
        remove(pending)
        raise
    remove(pending)
    print(f"已保存网络配置: {config}")

def restore(config, pending, backup):
    target, original = baseline(backup)
    before = read_values(target)
    current = capture(config)
    atomic_write(pending, json.dumps({"runtime": before, "file": current}, indent=2).encode(), 0o600)
    try:
        restore_values(target)
        check()
        write_saved(config, original)
    except Exception as error:
        try:
            restore_values(before, recovery=True)
            write_saved(config, current)
        except Exception as recovery:
            raise RuntimeError(f"{error}; {recovery}; 保留恢复材料: {pending}") from error
        remove(pending)
        raise
    remove_baseline(backup)
    remove(pending)
    print(f"已恢复首次操作前的运行参数和配置: {config}")

def main():
    if len(sys.argv) < 3 or sys.argv[1] not in ("apply", "restore"):
        raise ValueError("network apply/restore requires an absolute configuration path")
    action = sys.argv[1]
    desired = None
    if action == "apply":
        if any(unicodedata.category(c) == "Cc" and c != "\t" for pair in sys.argv[3:] for c in pair):
            raise ValueError("网络参数不能包含控制字符")
        desired = parse("\n".join(sys.argv[3:]))
    elif len(sys.argv) != 3:
        raise ValueError("restore 不接受额外参数")
    config = safe_path(sys.argv[2])
    pending = safe_path(str(config) + ".cqs-pending.json")
    backup = safe_path(str(config) + ".cqs-backup", file=False)
    lock = safe_path(str(config) + ".cqs-lock")
    config.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError(f"另一个进程正在修改该配置: {lock}") from error
        if pending.exists():
            raise ValueError(f"存在未完成操作，保留恢复材料: {pending}")
        check()
        if action == "apply":
            apply(config, pending, backup, desired)
        else:
            restore(config, pending, backup)
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)

try:
    main()
except Cancelled as error:
    print(error, file=sys.stderr)
    sys.exit(130)
except (OSError, ValueError, RuntimeError, KeyError) as error:
    print(error, file=sys.stderr)
    sys.exit(130 if cancelled else 1)
PY
}


# System/service operations share the Rust before-image format for recovery.
cqs_system_backend() {
    require_commands python3 || return 1
    CQS_KERNEL_LOCK="${CQS_KERNEL_LOCK:-/run/lock/cqs-bbrv3.lock}" \
    CQS_TCP_CONFIG="${CQS_TCP_CONFIG:-/etc/sysctl.d/99-cqs-tcp.conf}" \
    python3 - "$@" <<'CQS_SYSTEM_PY'
import base64
import contextlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlsplit

cancelled = False
recovering = False

def check():
    if cancelled and not recovering:
        raise InterruptedError("Operation cancelled")

def checked_path(value, file=False):
    raw = os.fspath(value)
    path = Path(raw)
    if (not path.is_absolute() or any(part in (".", "..") for part in raw.split("/"))
            or any(ord(c) < 32 or ord(c) == 127 for c in raw)):
        raise ValueError("Unsafe path: " + raw)
    for item in (path, *path.parents):
        if item.is_symlink():
            raise ValueError("Symlink is not allowed: " + str(item))
    if file and path.exists() and not path.is_file():
        raise ValueError("Expected a regular file: " + raw)
    return path

def sync_parent(path):
    if os.name == "posix":
        fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(fd)
        finally:
            os.close(fd)

def atomic_write(target, data, mode=0o644):
    check()
    target = checked_path(target, True)
    target.parent.mkdir(parents=True, exist_ok=True)
    old = target.stat() if target.exists() else None
    fd, temporary = tempfile.mkstemp(prefix=".cqs-write-", dir=target.parent)
    try:
        with os.fdopen(fd, "wb") as out:
            out.write(data)
            if os.name == "posix":
                if old:
                    os.fchown(out.fileno(), old.st_uid, old.st_gid)
                os.fchmod(out.fileno(), mode)
            out.flush()
            os.fsync(out.fileno())
        os.replace(temporary, target)
        sync_parent(target)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)

def remove_file(target):
    target = checked_path(target, True)
    if target.exists():
        target.unlink()
        sync_parent(target)

def saved(target):
    target = checked_path(target, True)
    if not target.exists():
        return None
    return {"data": base64.b64encode(target.read_bytes()).decode("ascii"),
            "mode": stat.S_IMODE(target.stat().st_mode)}

def saved_write(target, value):
    if value is None:
        remove_file(target)
    else:
        if set(value) != {"data", "mode"} or type(value["mode"]) is not int or not 0 <= value["mode"] <= 0o7777:
            raise ValueError("Invalid recovery snapshot")
        atomic_write(target, base64.b64decode(value["data"], validate=True), value["mode"])

def pending_write(path, value):
    path = checked_path(path, True)
    if path.exists():
        raise ValueError("Unfinished transaction; inspect recovery snapshot: " + str(path))
    atomic_write(path, (json.dumps(value, indent=2) + "\n").encode(), 0o600)

@contextlib.contextmanager
def lock(target):
    import fcntl
    target = checked_path(target, True)
    target.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(target, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)

@contextlib.contextmanager
def recovery():
    global recovering
    before = recovering
    recovering = True
    try:
        yield
    finally:
        recovering = before

def run(*args, capture=False, allowed=(0,)):
    check()
    environment = dict(os.environ, LC_ALL="C")
    child = subprocess.Popen([os.fspath(x) for x in args], env=environment,
                             stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE if capture else None,
                             stderr=subprocess.PIPE if capture else None,
                             start_new_session=os.name == "posix")
    interrupted = False
    while True:
        try:
            out, err = child.communicate(timeout=0.1)
            break
        except subprocess.TimeoutExpired:
            if cancelled and not recovering and not interrupted:
                interrupted = True
                if os.name == "posix":
                    os.killpg(child.pid, signal.SIGINT)
                else:
                    child.send_signal(signal.SIGINT)
    check()
    if child.returncode not in allowed:
        detail = err.decode(errors="replace") if err else ""
        raise RuntimeError("Command failed ({}): {}\n{}".format(child.returncode, " ".join(map(str, args)), detail))
    return out.decode().rstrip() if capture else child.returncode

def active(service):
    return run("systemctl", "is-active", "--quiet", service, allowed=(0, 3, 4)) == 0

def package_lock():
    return lock(os.environ.get("CQS_KERNEL_LOCK", "/run/lock/cqs-bbrv3.lock"))

def service_update(target, data, service, was_active, validate, activate):
    target = checked_path(target, True)
    pending = Path(str(target) + ".cqs-pending.json")
    checked_path(pending, True)
    if pending.exists():
        raise ValueError("Unfinished transaction: " + str(pending))
    before = saved(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, candidate_name = tempfile.mkstemp(prefix=".cqs-candidate-", dir=target.parent)
    candidate = Path(candidate_name)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
        validate(candidate)
        pending_write(pending, {"file": before, "service": service, "was_active": was_active})
        try:
            atomic_write(target, data, before["mode"] if before else 0o644)
            activate()
            check()
        except BaseException as error:
            try:
                with recovery():
                    saved_write(target, before)
                    run("systemctl", "restart" if was_active else "stop", service)
                    if active(service) != was_active:
                        raise RuntimeError("Service recovery verification failed: " + service)
            except BaseException as restore_error:
                raise RuntimeError("{}; recovery failed: {}; snapshot: {}".format(error, restore_error, pending)) from error
            remove_file(pending)
            raise
        remove_file(pending)
        print("Saved: " + str(target))
    finally:
        candidate.unlink(missing_ok=True)

def ssh_candidate(original, settings):
    output = [key + " " + value + "\n" for key, value in settings]
    in_match = False
    names = {key.lower() for key, _ in settings}
    for line in original.splitlines(keepends=True):
        words = line.split()
        key = words[0].lower() if words else ""
        if key == "match":
            in_match = True
        if not in_match and key in names:
            continue
        output.append(line)
    return "".join(output)

def ssh_effective(effective, settings):
    rows = [line.split() for line in effective.splitlines()]
    for key, value in settings:
        found = [row[1] for row in rows if len(row) >= 2 and row[0].lower() == key.lower()]
        if found != [value]:
            raise ValueError("Effective SSH setting differs: {}: {} != {}".format(key, found, value))
        if key == "Port" and any(row[0] == "listenaddress" and (len(row) < 2 or not row[1].endswith(":" + value)) for row in rows if row):
            raise ValueError("ListenAddress fixes a different SSH port")

def ssh_configure(settings):
    if active("ssh.socket") or not active("ssh"):
        raise ValueError("Requires active ssh.service without ssh.socket")
    target = Path("/etc/ssh/sshd_config")
    with lock(str(target) + ".cqs-lock"):
        checked_path(target, True)
        data = ssh_candidate(target.read_text(), settings).encode()
        def validate(candidate):
            run("sshd", "-t", "-f", candidate)
            ssh_effective(run("sshd", "-T", "-f", candidate, capture=True), settings)
        def activate():
            run("systemctl", "reload", "ssh")
            if not active("ssh"):
                raise RuntimeError("SSH service is not active after reload")
            ports = [value for key, value in settings if key == "Port"]
            if ports:
                for _ in range(50):
                    check()
                    pid = run("systemctl", "show", "--property=MainPID", "--value", "ssh", capture=True)
                    if pid.isascii() and pid.isdigit() and int(pid) > 0:
                        listeners = run("ss", "-H", "-ltnp", "sport = :" + ports[0], capture=True)
                        if "pid=" + pid + "," in listeners:
                            return
                    time.sleep(0.1)
                raise RuntimeError("SSH is not listening on the requested port")
        service_update(target, data, "ssh", True, validate, activate)

def ssh_port(port):
    if not re.fullmatch(r"[0-9]{1,5}", port) or not 1 <= int(port) <= 65535:
        raise ValueError("SSH port must be 1..65535")
    ssh_configure([("Port", str(int(port)))])

def ssh_key_install(public_key):
    public_key = checked_path(public_key, True)
    lines = [line for line in public_key.read_text().splitlines() if line.strip()]
    prefixes = ("ssh-ed25519 ", "ssh-rsa ", "ecdsa-sha2-", "sk-ssh-ed25519@", "sk-ecdsa-sha2-")
    if len(lines) != 1 or not lines[0].startswith(prefixes):
        raise ValueError("Expected one SSH public key")
    run("ssh-keygen", "-l", "-f", public_key)
    directory = checked_path(Path(os.environ["HOME"]) / ".ssh")
    directory.mkdir(parents=True, exist_ok=True)
    directory.chmod(0o700)
    target = directory / "authorized_keys"
    with lock(directory / "cqs.lock"):
        before = saved(target)
        current = base64.b64decode(before["data"]).decode() if before else ""
        if lines[0].strip() not in [line.strip() for line in current.splitlines()]:
            if current and not current.endswith("\n"):
                current += "\n"
            atomic_write(target, (current + lines[0].strip() + "\n").encode(), 0o600)
    print("Public key installed: " + str(target))

def docker_candidate(text, action, value, subnet=None):
    config = json.loads(text)
    if not isinstance(config, dict):
        raise ValueError("daemon.json must contain an object")
    if action == "mirror":
        parsed = urlsplit(value)
        if (parsed.scheme not in ("http", "https") or not parsed.hostname
                or parsed.username is not None or parsed.password is not None
                or parsed.query or parsed.fragment or "?" in value or "#" in value
                or any(ord(c) < 33 or ord(c) == 127 for c in value)):
            raise ValueError("Mirror requires an HTTP(S) URL without credentials, query or fragment")
        parsed.port  # Reject malformed/out-of-range ports before writing.
        config["registry-mirrors"] = [value]
    elif action == "ipv6":
        if value not in ("on", "off"):
            raise ValueError("IPv6 must be on/off")
        config["ipv6"] = value == "on"
        if value == "on":
            if not any(isinstance(config.get(key), str) and config[key] for key in ("fixed-cidr-v6", "bip6")):
                if subnet is None:
                    prefix = "fd" + os.urandom(5).hex()
                    subnet = ":".join(prefix[i:i+4] for i in (0, 4, 8)) + ":1::/64"
                config["fixed-cidr-v6"] = subnet
        else:
            for key in ("fixed-cidr-v6", "bip6", "default-gateway-v6"):
                config.pop(key, None)
    else:
        raise ValueError("Unknown Docker configuration action")
    return (json.dumps(config, indent=2, ensure_ascii=False) + "\n").encode()

def docker_configure(action, value):
    target = Path("/etc/docker/daemon.json")
    with lock(str(target) + ".cqs-lock"):
        checked_path(target, True)
        original = target.read_text() if target.exists() else "{}"
        data = docker_candidate(original, action, value)
        was_active = active("docker")
        def validate(candidate):
            run("dockerd", "--validate", "--config-file", candidate)
        def activate():
            run("systemctl", "restart", "docker")
            if not active("docker"):
                raise RuntimeError("Docker did not become active")
        service_update(target, data, "docker", was_active, validate, activate)

def docker_status():
    for args in (("version",), ("compose", "version"), ("image", "ls"), ("ps", "-a"), ("volume", "ls"), ("network", "ls")):
        run("docker", *args)
    target = Path("/etc/docker/daemon.json")
    if target.exists():
        data = json.loads(target.read_text())
        if not isinstance(data, dict):
            raise ValueError("daemon.json must contain an object")
        if "registry-mirrors" in data:
            print(json.dumps(data["registry-mirrors"]))

def docker_uninstall(purge_data):
    if purge_data not in ("on", "off"):
        raise ValueError("purge_data must be on/off")
    with package_lock():
        packages = []
        for package in ("docker-ce", "docker-ce-cli", "docker.io", "docker-engine", "docker", "docker-buildx-plugin", "docker-compose-plugin", "containerd.io"):
            status = run("dpkg-query", "-W", "-f=${db:Status-Abbrev}", package, capture=True, allowed=(0, 1))
            if len(status) > 1 and status[1] in "iUFHWt":
                packages.append(package)
        if not packages:
            raise ValueError("No installed Docker packages found")
        run("systemctl", "stop", "docker.service", "docker.socket")
        run("apt-get", "purge", "-y", *packages)
        if purge_data == "on":
            for name in ("/var/lib/docker", "/etc/docker"):
                target = checked_path(name)
                if target.exists():
                    shutil.rmtree(target)

def physical_kb(proc=Path("/proc")):
    for line in (proc / "meminfo").read_text().splitlines():
        match = re.fullmatch(r"MemTotal:\s+([0-9]+)\s+kB", line)
        if match:
            return int(match[1])
    raise ValueError("Cannot read physical memory")

def swap_size(memory_mb):
    gb = memory_mb // 1024
    return min(memory_mb * 2, 4096) if gb <= 4 else memory_mb if gb <= 8 else 8192 if gb <= 64 else 16384

def fstab_without_swap(text):
    return "".join(line for line in text.splitlines(keepends=True) if not line.split() or line.split()[0] != "/swapfile")

def swap_active():
    return any(line.split() and line.split()[0] == "/swapfile" for line in Path("/proc/swaps").read_text().splitlines())

def swap(operation, size):
    if Path("/proc/vz").exists():
        raise ValueError("OpenVZ does not support this swap operation")
    if operation == "remove":
        megabytes = 0
    elif operation == "auto":
        megabytes = swap_size(physical_kb() // 1024)
    elif operation == "create" and re.fullmatch(r"[0-9]+", size):
        megabytes = int(size)
    else:
        raise ValueError("Invalid swap operation or size")
    if operation != "remove" and not 1 <= megabytes <= 1048576:
        raise ValueError("Swap size must be 1..1048576 MiB")
    with lock("/run/lock/cqs-swap.lock"):
        target = checked_path("/swapfile", True)
        fstab = checked_path("/etc/fstab", True)
        before = saved(fstab)
        if before is None:
            raise ValueError("/etc/fstab does not exist")
        text = base64.b64decode(before["data"]).decode()
        existed = target.exists()
        registered = any(line.split() and line.split()[0] == "/swapfile" for line in text.splitlines())
        if operation == "create" and (existed or registered):
            raise ValueError("/swapfile already exists or is registered")
        if registered and not existed:
            raise ValueError("fstab refers to a missing /swapfile")
        if operation == "remove" and not existed:
            raise ValueError("/swapfile does not exist")
        was_active = swap_active()
        pending = checked_path("/etc/fstab.cqs-swap-pending.json", True)
        if pending.exists():
            raise ValueError("Unfinished transaction: " + str(pending))
        backup = None
        if existed:
            fd, name = tempfile.mkstemp(prefix=".cqs-swap-backup-", dir="/")
            os.close(fd)
            backup = Path(name)
        try:
            pending_write(pending, {"fstab": before, "active": was_active, "backup": str(backup) if backup else None})
        except BaseException:
            if backup:
                remove_file(backup)
            raise
        moved = created = False
        try:
            if was_active:
                run("swapoff", "/swapfile")
            if backup:
                os.replace(target, backup)
                moved = True
            if operation != "remove":
                created = True  # fallocate may leave a file even when it reports failure.
                run("fallocate", "-l", str(megabytes) + "M", "/swapfile")
                target.chmod(0o600)
                run("mkswap", "/swapfile")
                run("swapon", "/swapfile")
            new_fstab = fstab_without_swap(text)
            if operation != "remove":
                if not new_fstab.endswith("\n"):
                    new_fstab += "\n"
                new_fstab += "/swapfile none swap defaults 0 0\n"
            atomic_write(fstab, new_fstab.encode(), before["mode"])
        except BaseException as error:
            try:
                with recovery():
                    if created:
                        if swap_active():
                            run("swapoff", "/swapfile")
                        remove_file(target)
                    if moved:
                        os.replace(backup, target)
                    if was_active and not swap_active():
                        run("swapon", "/swapfile")
                    saved_write(fstab, before)
            except BaseException as restore_error:
                raise RuntimeError("{}; recovery failed: {}; snapshot: {}".format(error, restore_error, pending)) from error
            if backup:
                remove_file(backup)
            remove_file(pending)
            raise
        if backup:
            remove_file(backup)
        remove_file(pending)
        print(Path("/proc/swaps").read_text())

def fail2ban():
    with package_lock():
        run("apt-get", "install", "-y", "fail2ban")
        target = Path("/etc/fail2ban/jail.d/cqs.local")
        with lock(str(target) + ".cqs-lock"):
            effective = run("sshd", "-T", capture=True)
            ports = [line[5:] for line in effective.splitlines() if line.startswith("port ")]
            if not ports or any(not re.fullmatch(r"[0-9]{1,5}", port) or not 1 <= int(port) <= 65535 for port in ports):
                raise ValueError("Cannot determine effective SSH ports")
            data = ("[sshd]\nenabled = true\nbackend = systemd\nport = " + ",".join(ports)
                    + "\nbantime = 86400\nfindtime = 86400\nmaxretry = 3\n").encode()
            was_active = active("fail2ban")
            def activate():
                run("fail2ban-client", "-t")
                run("systemctl", "restart", "fail2ban")
                if not active("fail2ban"):
                    raise RuntimeError("Fail2ban did not become active")
            service_update(target, data, "fail2ban", was_active, lambda _: None, activate)
            run("systemctl", "enable", "fail2ban")

def clean(scope):
    if scope not in ("standard", "full"):
        raise ValueError("Cleanup scope must be standard/full")
    with package_lock():
        run("apt-get", "clean")
        run("journalctl", "--vacuum-time=7d")
        print("Keeping all kernels; remove a specific kernel through kernel.remove.")
        if scope == "full":
            for root in ("/var/log", "/root", "/home", "/ql"):
                if Path(root).exists():
                    run("find", root, "-xdev", "-type", "f", "-name", "*.log", "-exec", "truncate", "-s", "0", "{}", ";")
            for root in ("/tmp", "/var/tmp"):
                run("find", root, "-xdev", "-type", "f", "-mtime", "+1", "-delete")
            homes = [Path("/root")]
            if Path("/home").is_dir():
                homes.extend(Path("/home").iterdir())
            for home in homes:
                cache = checked_path(home / ".cache")
                if cache.is_dir():
                    for entry in cache.iterdir():
                        check()
                        if entry.is_dir() and not entry.is_symlink():
                            shutil.rmtree(entry)
                        else:
                            entry.unlink()
            if shutil.which("docker"):
                run("docker", "system", "prune", "-a", "-f", "--volumes")

def cpu_sample(text):
    for line in text.splitlines():
        if line.startswith("cpu "):
            values = list(map(int, line.split()[1:9]))
            if len(values) < 4 or any(value < 0 for value in values):
                raise ValueError("Invalid CPU counters")
            return sum(values), values[3] + (values[4] if len(values) > 4 else 0)
    raise ValueError("No CPU counters")

def system_info():
    for args in (("hostname",), ("uname", "-r"), ("uname", "-m"), ("lscpu",), ("free", "-m"), ("df", "-h", "/"), ("uptime", "-p"), ("timedatectl", "show", "--property=Timezone", "--value")):
        print(args[0] + ":", flush=True)
        run(*args)
    print(Path("/etc/os-release").read_text())
    old_total, old_idle = cpu_sample(Path("/proc/stat").read_text())
    time.sleep(0.3)
    total, idle = cpu_sample(Path("/proc/stat").read_text())
    if total <= old_total or idle < old_idle or idle - old_idle > total - old_total:
        raise ValueError("Invalid CPU counter delta")
    print("CPU: {:.1f}%".format(100 * (1 - (idle - old_idle) / (total - old_total))))
    print("TCP: " + ("enabled" if Path(os.environ.get("CQS_TCP_CONFIG", "/etc/sysctl.d/99-cqs-tcp.conf")).exists() else "disabled"))
    run("ip", "-brief", "address")
    run("sysctl", "net.ipv4.tcp_congestion_control", "net.ipv4.tcp_available_congestion_control", "net.core.default_qdisc")
    version = Path("/sys/module/tcp_bbr/version")
    value = version.read_text().strip() if version.exists() else ""
    print("BBR: " + (value if value in ("1", "2", "3") else "unknown"))
    run("tc", "qdisc", "show")

def public_ip():
    failures = []
    for family, endpoint in (("IPv4", "https://ipv4.ip.sb"), ("IPv6", "https://ipv6.ip.sb"), ("ISP", "https://ipinfo.io/json")):
        try:
            value = run("curl", "--proto", "=https", "-fSs", "--max-time", "5", endpoint, capture=True)
            if family == "ISP":
                value = json.loads(value)["org"]
                if not isinstance(value, str):
                    raise ValueError("Invalid ipinfo.org")
            print(family + ": " + value)
        except (RuntimeError, OSError, ValueError, KeyError, TypeError) as error:
            print(str(error), file=sys.stderr)
            failures.append(family)
    if failures:
        raise RuntimeError("Public IP lookup failed: " + ", ".join(failures))

def timezone(value):
    if (not value or value.startswith("/") or any(part in ("", ".", "..") for part in value.split("/"))
            or any(ord(c) < 32 or ord(c) == 127 for c in value)):
        raise ValueError("Invalid timezone")
    run("timedatectl", "set-timezone", value)

def main(args):
    operation, *values = args
    if operation == "ssh-port":
        ssh_port(*values)
    elif operation == "ssh-key-install":
        ssh_key_install(*values)
    elif operation == "ssh-key-enable":
        ssh_configure([("PubkeyAuthentication", "yes"), ("PasswordAuthentication", "no"), ("KbdInteractiveAuthentication", "no")])
    elif operation in ("docker-mirror", "docker-ipv6"):
        docker_configure(operation.removeprefix("docker-"), *values)
    elif operation == "docker-status":
        docker_status()
    elif operation == "docker-uninstall":
        docker_uninstall(*values)
    elif operation == "swap":
        swap(*values)
    elif operation == "fail2ban":
        fail2ban()
    elif operation == "clean":
        clean(*values)
    elif operation == "info":
        system_info()
    elif operation == "public-ip":
        public_ip()
    elif operation == "timezone":
        timezone(*values)
    else:
        raise ValueError("Unknown system action: " + operation)

if __name__ == "__main__":
    def interrupted(_signum, _frame):
        global cancelled
        cancelled = True
    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    try:
        main(sys.argv[1:])
    except (Exception, KeyboardInterrupt) as error:
        print("cqs: " + str(error), file=sys.stderr)
        sys.exit(130 if isinstance(error, (InterruptedError, KeyboardInterrupt)) else 1)
CQS_SYSTEM_PY
}

cqs_system_clean() {
    require_root || return 1
    case "${1:-standard}" in
        standard) ;;
        full) cqs_confirm '/var/log /root /home /ql; /tmp /var/tmp; caches; Docker volumes' CLEAN || return $? ;;
        *) echo 'scope must be standard/full' >&2; return 1 ;;
    esac
    cqs_system_backend clean "${1:-standard}"
}
cqs_system_info() { cqs_system_backend info; }
cqs_system_public_ip() { cqs_system_backend public-ip; }
cqs_system_timezone() { require_root && cqs_system_backend timezone "$1"; }
cqs_swap() { require_root && cqs_system_backend swap "$1" "${2:-}"; }
cqs_fail2ban() { require_root && cqs_system_backend fail2ban; }
cqs_ssh_port() { require_root && cqs_system_backend ssh-port "$1"; }
cqs_ssh_key() {
    require_root || return 1
    cqs_system_backend ssh-key-install "$1" || return $?
    cqs_confirm 'Test the installed public key in a new SSH connection before disabling passwords' KEY-TESTED || return $?
    cqs_system_backend ssh-key-enable
}
cqs_docker() (
    local action="$1" value="${2:-}" lock_fd url
    case "$action" in
        status) cqs_system_backend docker-status ;;
        mirror|ipv6) require_root && cqs_system_backend "docker-$action" "$value" ;;
        clean) require_root && docker system prune -a -f --volumes ;;
        uninstall) require_root && cqs_system_backend docker-uninstall "${value:-off}" ;;
        install)
            require_root || return 1
            case "${value:-official}" in
                official) url=https://get.docker.com ;;
                china) url=https://linuxmirrors.cn/docker.sh ;;
                *) echo 'source must be official/china' >&2; return 1 ;;
            esac
            if command -v docker >/dev/null 2>&1; then docker version; return $?; fi
            acquire_lock lock_fd "$CQS_KERNEL_LOCK" || return 1
            cqs_remote "$url" || return $?
            docker version
            ;;
        *) echo "Unknown Docker action: $action" >&2; return 1 ;;
    esac
)


cqs_system_update() (
    # acquire_lock assigns this descriptor through a nameref and the subshell owns its lifetime.
    # shellcheck disable=SC2034
    local lock_fd
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" || return 1
    # Match the Rust upgrade policy without leaking it out of this subshell.
    export DEBIAN_FRONTEND=noninteractive UCF_FORCE_CONFFOLD=1 UCF_FORCE_CONFFNEW=''
    export NEEDRESTART_MODE=l APT_LISTCHANGES_FRONTEND=none
    local -a options=(
        -o Dpkg::Use-Pty=0 -o Dpkg::Progress-Fancy=0 -o APT::Color=0
        -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold
    )
    apt-get "${options[@]}" update && apt-get "${options[@]}" upgrade -y
)

# LinuxMirrors integration. The embedded backend shares its journal format with Rust.
cqs_mirrors() (
    export CQS_LANG CQS_YES CQS_KERNEL_LOCK CQS_MESSAGES_FILE
    export -f cqs_confirm cqs_metadata
    python3 /dev/fd/3 "$@" 3<<'CQS_MIRRORS_PY'
import base64
import contextlib
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit

REVISION = "76fbf1738c22108cdbcc73973612bdb07ad6789b"
SCRIPT_URL = f"https://raw.githubusercontent.com/SuperManito/LinuxMirrors/{REVISION}/ChangeMirrorsLite.sh"
SCRIPT_SHA256 = "963c605bf114bdb7123eaa71d42c7034ac38916b96a55d9f03afa40f397b9fb6"
SAMPLE = 1024 * 1024
SITES = ("official", "tuna", "ustc", "aliyun")
STATE = ".cqs-mirrors"
MESSAGES = {}
CANCELLED = False


class Failure(Exception):
    def __init__(self, key, detail=""):
        self.key, self.detail = key, str(detail)
        super().__init__(MESSAGES.get(key, key + ": {detail}").replace("{detail}", self.detail))


def message(key, detail=""):
    print(MESSAGES.get(key, key + ": {detail}").replace("{detail}", str(detail)), flush=True)


def check():
    if CANCELLED:
        raise Failure("error.cancelled")


def run(args, capture=False, env=None):
    check()
    child = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE if capture else None,
                             stderr=subprocess.PIPE if capture else None, start_new_session=True, env=env)
    interrupted = False
    while True:
        if CANCELLED and not interrupted and child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGINT)
            except ProcessLookupError:
                pass
            interrupted = True
        try:
            stdout, stderr = child.communicate(timeout=0.1)
            break
        except subprocess.TimeoutExpired:
            continue
    check()
    if child.returncode:
        raise Failure("error.command", f"{args[0]}: {child.returncode}\n" + (stderr or b"").decode(errors="replace"))
    return (stdout or b"").decode("utf-8").rstrip()


class Host:
    def __init__(self, release, arch):
        values = dict(line.split("=", 1) for line in release.splitlines() if "=" in line)
        self.distro = values.get("ID", "").strip("\"'")
        self.codename = values.get("VERSION_CODENAME", "").strip("\"'")
        self.arch = arch
        supported = {"debian": {"bookworm", "trixie"}, "ubuntu": {"jammy", "noble", "resolute"}}
        if self.codename not in supported.get(self.distro, set()) or arch not in {"amd64", "arm64"}:
            raise Failure("error.mirror_host", f"{self.distro} {self.codename} {arch}")
        self.branch = "debian" if self.distro == "debian" else "ubuntu-ports" if arch == "arm64" else "ubuntu"
        self.official = "deb.debian.org" if self.distro == "debian" else "ports.ubuntu.com" if arch == "arm64" else "archive.ubuntu.com"
        self.security = ("security.debian.org", "debian-security") if self.distro == "debian" else (
            ("ports.ubuntu.com", "ubuntu-ports") if arch == "arm64" else ("security.ubuntu.com", "ubuntu"))
        self.keyring = f"/usr/share/keyrings/{self.distro}-archive-keyring.gpg"
        self.targets = ("sources.list", f"sources.list.d/{self.distro}.sources")

    def domain(self, site):
        sites = dict(official=self.official, tuna="mirrors.tuna.tsinghua.edu.cn",
                     ustc="mirrors.ustc.edu.cn", aliyun="mirrors.aliyun.com")
        if site not in sites:
            raise Failure("error.input", "mirror_site")
        return sites[site]

    def base(self, site):
        return f"https://{self.domain(site)}/{self.branch}/"

    @classmethod
    def detect(cls):
        for marker in ("/etc/armbian-release", "/etc/pve/.version", "/etc/rpi-issue"):
            if Path(marker).exists():
                raise Failure("error.mirror_host", marker)
        return cls(Path("/etc/os-release").read_text(), run(["dpkg", "--print-architecture"], capture=True))


def parse_measurement(output, body, headers):
    fields = output.strip().split("\t")
    try:
        if len(fields) != 4 or fields[0] != "206" or len(body) != SAMPLE or not body.startswith(b"\xfd7zXZ\0"):
            raise ValueError()
        if not any(line.startswith("content-range: bytes 0-1048575/") for line in headers.lower().splitlines()):
            raise ValueError()
        size, elapsed, first = map(float, fields[1:])
        if size != SAMPLE or not math.isfinite(elapsed) or elapsed <= 0 or not math.isfinite(first) or not 0 <= first <= elapsed:
            raise ValueError()
        return 1 / elapsed, first * 1000
    except ValueError as error:
        raise Failure("error.mirror_sample", output) from error


def probe(url):
    with tempfile.TemporaryDirectory(prefix="cqs-mirror-sample-") as work:
        body, headers = Path(work) / "sample", Path(work) / "headers"
        output = run(["curl", "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "-fLsS",
                      "--connect-timeout", "3", "--max-time", "8", "--retry", "0",
                      "--range", "0-1048575", "--max-filesize", "1048576", "--dump-header", str(headers),
                      "--output", str(body), "--write-out", "%{http_code}\t%{size_download}\t%{time_total}\t%{time_starttransfer}", url], capture=True)
        return parse_measurement(output, body.read_bytes(), headers.read_text())


def benchmark(host):
    message("notice.mirror_probe", f"{host.distro} {host.codename} / {host.arch}")
    results = []
    for site in SITES:
        check()
        url = f"{host.base(site)}dists/{host.codename}/main/binary-{host.arch}/Packages.xz"
        print(f"{site}: {url}", flush=True)
        try:
            speed, first = probe(url)
            print(f"{site}: {speed:.2f} MiB/s; TTFB {first:.0f} ms", flush=True)
            results.append((site, speed, first))
        except Failure as error:
            if error.key == "error.cancelled":
                raise
            message("notice.mirror_failed", f"{site}: {error}")
    results.sort(key=lambda row: (-row[1], row[2]))
    for rank, (site, speed, first) in enumerate(results, 1):
        print(f"{rank}. {site}: {speed:.2f} MiB/s; TTFB {first:.0f} ms", flush=True)
    if not results:
        raise Failure("error.mirror_speed")
    message("notice.mirror_best", f"{results[0][0]} ({host.base(results[0][0])})")
    return results


def safe_path(path):
    path = Path(path)
    if not path.is_absolute() or ".." in path.parts or any(ord(c) < 32 or ord(c) == 127 for c in str(path)):
        raise Failure("error.path", path)
    for ancestor in (path, *path.parents):
        if ancestor.is_symlink():
            raise Failure("error.path", ancestor)
    if path.exists() and not path.is_file():
        raise Failure("error.path", path)
    return path


def capture_file(path):
    path = safe_path(path)
    try:
        return dict(data=base64.b64encode(path.read_bytes()).decode(), mode=stat.S_IMODE(path.stat().st_mode))
    except FileNotFoundError:
        return None


def decode(saved):
    if not isinstance(saved, dict) or set(saved) != {"data", "mode"} or type(saved["mode"]) is not int or not 0 <= saved["mode"] <= 0o7777:
        raise Failure("error.snapshot", "invalid saved file")
    return base64.b64decode(saved["data"], validate=True)


def sync_parent(path):
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_saved(path, saved):
    path = safe_path(path)
    if saved is None:
        if path.exists():
            path.unlink()
            sync_parent(path)
        return
    data = decode(saved)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            if path.exists():
                info = path.stat()
                os.fchown(output.fileno(), info.st_uid, info.st_gid)
            os.fchmod(output.fileno(), saved["mode"])
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        sync_parent(path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def save_json(path, value):
    write_saved(path, dict(data=base64.b64encode(json.dumps(value, indent=2).encode()).decode(), mode=0o600))


def capture(apt, host):
    return {name: capture_file(apt / name) for name in host.targets}


def known_uri(host, value):
    try:
        url = urlsplit(value)
        domain = url.hostname or ""
        return (url.scheme in {"http", "https"} and not url.username and not url.password and not url.port
                and not url.query and not url.fragment and
                (domain in {host.domain(site) for site in SITES} | {host.security[0]}
                 or (host.distro == "ubuntu" and domain.endswith(".archive.ubuntu.com")))
                and url.path.strip("/") in {host.branch, host.security[1]})
    except ValueError:
        return False


def check_source(host, name, text):
    suites = {host.codename + suffix for suffix in ("", "-updates", "-security", "-backports")}
    components = {"main", "contrib", "non-free", "non-free-firmware"} if host.distro == "debian" else {"main", "restricted", "universe", "multiverse"}
    def require(condition):
        if not condition:
            raise Failure("error.mirror_config", name)
    if name.endswith(".sources"):
        fields = set()
        for line in text.splitlines() + [""]:
            if line.lstrip().startswith("#"):
                continue
            if not line.strip():
                require(not fields or {"Types", "URIs", "Suites", "Components"} <= fields)
                fields.clear()
                continue
            require(":" in line)
            key, value = line.split(":", 1)
            words = value.split()
            require(words and key not in fields)
            fields.add(key)
            valid = {"Types": lambda: set(words) <= {"deb", "deb-src"},
                     "URIs": lambda: all(known_uri(host, word) for word in words),
                     "Suites": lambda: set(words) <= suites, "Components": lambda: set(words) <= components,
                     "Signed-By": lambda: words == [host.keyring]}
            require(key in valid and valid[key]())
    else:
        for line in text.splitlines():
            words = line.split("#", 1)[0].split()
            if not words:
                continue
            if len(words) > 1 and words[1] == f"[signed-by={host.keyring}]":
                del words[1]
            require(len(words) >= 4 and words[0] in {"deb", "deb-src"} and known_uri(host, words[1])
                    and words[2] in suites and set(words[3:]) <= components)


def preflight(apt, host):
    files = capture(apt, host)
    if all(saved is None for saved in files.values()):
        raise Failure("error.mirror_config", apt)
    for name, saved in files.items():
        if saved is not None:
            check_source(host, name, decode(saved).decode("utf-8"))
    return files


def verify_result(apt, host, site):
    files = preflight(apt, host)
    saved = files[f"sources.list.d/{host.distro}.sources"] or files["sources.list"]
    if saved is None:
        raise Failure("error.mirror_config", apt)
    text = decode(saved).decode()
    words = {word.rstrip("/") for line in text.splitlines() if not line.lstrip().startswith("#") for word in line.split()}
    if (host.base(site).rstrip("/") not in words
            or f"https://{host.security[0]}/{host.security[1]}" not in words):
        raise Failure("error.mirror_config", "LinuxMirrors did not generate the selected repositories")


def read_record(file, host):
    record = json.loads(safe_path(file).read_text())
    if (not isinstance(record, dict) or set(record) != {"version", "distro", "before", "after", "previous"}
            or type(record["version"]) is not int or record["version"] != 1 or record["distro"] != host.distro):
        raise Failure("error.snapshot", file)
    for files in [record["before"]] + ([record["after"]] if record["after"] is not None else []):
        if not isinstance(files, dict) or set(files) != set(host.targets):
            raise Failure("error.snapshot", file)
        for saved in files.values():
            if saved is not None:
                decode(saved)
    if record["previous"] is not None:
        decode(record["previous"])
    return record


def rollback(apt, record):
    for name in record["before"]:
        safe_path(apt / name)
    for name, saved in sorted(record["before"].items()):
        write_saved(apt / name, saved)
    write_saved(apt / STATE / "last.json", record["previous"])
    write_saved(apt / STATE / "pending.json", None)


def transaction(apt, host, before, operation):
    state = apt / STATE
    safe_path(state / "pending.json")
    state.mkdir(mode=0o700, exist_ok=True)
    state.chmod(0o700)
    pending = state / "pending.json"
    if pending.exists():
        raise Failure("error.pending", pending)
    if capture(apt, host) != before:
        raise Failure("error.mirror_changed", apt)
    record = dict(version=1, distro=host.distro, before=before, after=None, previous=capture_file(state / "last.json"))
    save_json(pending, record)
    try:
        check()
        operation()
        check()
        save_json(state / "last.json", dict(record, after=capture(apt, host), previous=None))
        write_saved(pending, None)
    except Exception as error:
        try:
            rollback(apt, record)
        except Exception as recovery:
            raise Failure("error.recovery", f"{error}; {recovery}; {pending}") from recovery
        message("notice.mirror_rolled_back", apt)
        raise


def arguments(host, site):
    return ["--source", host.domain(site), "--branch", host.branch,
            "--source-security", host.security[0], "--branch-security", host.security[1],
            "--protocol", "https", "--use-official-source", "false", "--backup", "false",
            "--upgrade-software", "false", "--clean-cache", "false", "--clean-screen", "false", "--install-epel", "false"]


def strict_update(env=None):
    run(["apt-get", "update", "-o", "APT::Update::Error-Mode=any", "-o", "Acquire::Retries=0",
         "-o", "Acquire::http::Timeout=15", "-o", "Acquire::https::Timeout=15",
         "-o", "Dpkg::Use-Pty=0", "-o", "Dpkg::Progress-Fancy=0", "-o", "APT::Color=0"], env=env)


def apply(apt, host, site):
    pending = apt / STATE / "pending.json"
    if pending.exists():
        raise Failure("error.pending", pending)
    if site != "auto":
        host.domain(site)
    before = preflight(apt, host)
    if site == "auto":
        site = benchmark(host)[0][0]
    detail = (f"{host.distro} {host.codename} ({host.arch})\n{host.base(site)}\n"
              f"security: https://{host.security[0]}/{host.security[1]}/\nLinuxMirrors {REVISION}\n"
              + "\n".join(str(apt / name) for name in before))
    check()
    confirmation = MESSAGES["confirm.mirror"].replace("{detail}", detail)
    answer = subprocess.run(["bash", "-c", 'cqs_confirm "$1" YES', "cqs-mirrors", confirmation], check=False)
    if answer.returncode:
        raise Failure("error.cancelled")
    with tempfile.TemporaryDirectory(prefix="cqs-linuxmirrors-") as work:
        script, config = Path(work) / "LinuxMirrors.sh", Path(work) / "apt.conf"
        run(["curl", "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2", "-fSL",
             "--connect-timeout", "15", "--max-time", "60", "--output", str(script), SCRIPT_URL])
        if hashlib.sha256(script.read_bytes()).hexdigest() != SCRIPT_SHA256:
            raise Failure("error.digest", "LinuxMirrors Lite")
        run(["bash", "-n", "--", str(script)])
        config.write_text('APT::Update::Error-Mode "any";\nAcquire::Retries "0";\nAcquire::http::Timeout "15";\nAcquire::https::Timeout "15";\n')
        env = dict(os.environ, APT_CONFIG=str(config), BASH_ENV="/dev/null", ENV="/dev/null", DEBIAN_FRONTEND="noninteractive")
        def operation():
            run(["bash", "--", str(script), *arguments(host, site)], env=env)
            legacy = apt / "sources.list"
            saved = capture_file(legacy)
            if saved is not None:
                data = decode(saved)
                lines = data.decode().splitlines()
                updated = "".join((f"deb [signed-by={host.keyring}] " + line[4:] if line.startswith("deb https://") else line) + "\n" for line in lines).encode()
                if data != updated:
                    write_saved(legacy, dict(data=base64.b64encode(updated).decode(), mode=saved["mode"]))
            verify_result(apt, host, site)
            strict_update(env)
        transaction(apt, host, before, operation)
    message("notice.mirror_applied", host.base(site))


def restore_files(apt, host):
    pending = apt / STATE / "pending.json"
    if pending.exists():
        rollback(apt, read_record(pending, host))
        return
    record = read_record(apt / STATE / "last.json", host)
    if record["after"] != capture(apt, host):
        raise Failure("error.mirror_changed", apt)
    save_json(pending, record)
    rollback(apt, record)


@contextlib.contextmanager
def package_lock():
    path = safe_path(Path(os.environ.get("CQS_KERNEL_LOCK", "/run/lock/cqs-bbrv3.lock")))
    path.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise Failure("error.lock", path) from error
        yield
    finally:
        os.close(fd)


def main():
    global MESSAGES
    if os.environ.get("CQS_MESSAGES_FILE"):
        MESSAGES = json.loads(Path(os.environ["CQS_MESSAGES_FILE"]).read_text(encoding="utf-8"))
    else:
        metadata = json.loads(subprocess.check_output(["bash", "-c", "cqs_metadata"]))
        MESSAGES = metadata["catalogs"][os.environ.get("CQS_LANG", "en-US")]["messages"]
    def cancelled(_signum, _frame):
        global CANCELLED
        CANCELLED = True
    signal.signal(signal.SIGINT, cancelled)
    signal.signal(signal.SIGTERM, cancelled)
    try:
        host = Host.detect()
        operation = sys.argv[1]
        if operation == "test":
            benchmark(host)
        else:
            if os.geteuid() != 0:
                raise Failure("error.root", "sudo cqs")
            with package_lock():
                if operation == "apply":
                    apply(Path("/etc/apt"), host, sys.argv[2])
                elif operation == "restore":
                    restore_files(Path("/etc/apt"), host)
                    message("notice.mirror_restored", "/etc/apt")
                    strict_update()
                else:
                    raise Failure("error.action", operation)
        return 0
    except (Failure, OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 130 if CANCELLED or (isinstance(error, Failure) and error.key == "error.cancelled") else 1


if __name__ == "__main__":
    sys.exit(main())
CQS_MIRRORS_PY
)

cqs_download() {
    local url="$1" target="$2" seconds="${3:-300}"
    [[ "$url" == https://* && "$url" != *$'\n'* && "$url" != *$'\r'* ]] || return 1
    curl --proto '=https' --proto-redir '=https' -fSL --connect-timeout 15 --max-time "$seconds" --output "$target" "$url" && [[ -s "$target" ]]
}

cqs_remote() (
    local url="$1" script
    [[ -t 0 ]] || { echo 'Interactive tool requires a terminal.' >&2; return 1; }
    cqs_confirm "$url" RUN || return $?
    script=$(mktemp) || return 1
    trap 'rm -f -- "$script"' EXIT
    trap 'exit 130' INT TERM
    cqs_download "$url" "$script" && bash -n -- "$script" && bash -- "$script"
)

cqs_ip_arguments() {
    printf '%s\n' -p -l
    if [[ "${CQS_LANG:-en-US}" == zh-CN ]]; then echo cn; else echo en; fi
    case "$1" in dual) ;; ipv4) echo -4 ;; ipv6) echo -6 ;; *) return 2 ;; esac
    case "$2" in off) ;; on) echo -f ;; *) return 2 ;; esac
}

cqs_ip_test() (
    local script actual arguments
    local -a args
    arguments=$(cqs_ip_arguments "$1" "$2") || return $?
    mapfile -t args <<< "$arguments"
    [[ -t 0 ]] || { echo 'IPQuality requires a terminal.' >&2; return 1; }
    cqs_confirm 'xykt/IPQuality: external databases; online report upload disabled (-p).' RUN || return $?
    script=$(mktemp) || return 1
    trap 'rm -f -- "$script"' EXIT
    trap 'exit 130' INT TERM
    cqs_download 'https://raw.githubusercontent.com/xykt/IPQuality/2384a67c756eb35231f5982b34731e522be3653e/ip.sh' "$script" || return 1
    actual=$(sha256sum "$script") || return 1
    [[ "${actual%% *}" == b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf ]] || {
        echo 'IPQuality SHA-256 mismatch; refusing execution.' >&2; return 1;
    }
    bash -n -- "$script" && bash -- "$script" "${args[@]}"
)

cqs_packages() (
    # shellcheck disable=SC2034 # acquire_lock writes the descriptor by nameref.
    local package="$1" lock_fd
    local -a packages=(curl wget nano unzip tar tmux iftop btop gdu fzf)
    case "$package" in all|curl|wget|nano|unzip|tar|tmux|iftop|btop|gdu|fzf) ;; *) return 2 ;; esac
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" || return 1
    if [[ "$package" == all ]]; then
        apt-get install -y "${packages[@]}" && cqs_terminal_install_unlocked
    else
        apt-get install -y "$package"
    fi
)

cqs_terminal_install() (
    # shellcheck disable=SC2034 # acquire_lock writes the descriptor by nameref.
    local lock_fd
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" || return 1
    cqs_terminal_install_unlocked
)

cqs_terminal_install_unlocked() {
    apt-get install -y zsh curl ca-certificates || return 1
    if ! command -v starship >/dev/null 2>&1; then
        cqs_starship_install || return 1
    fi
    starship --version
}

cqs_starship_install() {
    python3 - <<'PY'
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tarfile
import tempfile

def download(url,path):
    subprocess.run(['curl','--proto','=https','--proto-redir','=https','-fSL','--connect-timeout','15',
                    '--max-time','300','--output',str(path),url],check=True)
    if not path.stat().st_size:
        raise ValueError('empty download')

try:
    arch = platform.machine()
    if arch not in ('x86_64','aarch64'):
        raise ValueError('unsupported architecture: '+arch)
    with tempfile.TemporaryDirectory(prefix='cqs-starship-') as temp:
        work = Path(temp)
        metadata = work/'release.json'
        download('https://api.github.com/repos/starship/starship/releases/latest',metadata)
        release = json.loads(metadata.read_text())
        name = f'starship-{arch}-unknown-linux-musl.tar.gz'
        found = [a for a in release['assets'] if a['name']==name]
        if len(found)!=1:
            raise ValueError('expected one Starship asset')
        asset = found[0]
        url = f"https://github.com/starship/starship/releases/download/{release['tag_name']}/{name}"
        digest = asset.get('digest','')
        if asset['browser_download_url']!=url or not re.fullmatch(r'sha256:[0-9a-fA-F]{64}',digest):
            raise ValueError('untrusted Starship URL/digest')
        archive = work/'starship.tar.gz'
        download(url,archive)
        with archive.open('rb') as stream:
            if hashlib.file_digest(stream,'sha256').hexdigest()!=digest[7:].lower():
                raise ValueError('Starship SHA-256 mismatch')
        with tarfile.open(archive,'r:gz') as tar:
            binaries = [m for m in tar if m.name in ('starship','./starship')]
            if len(binaries)!=1 or not binaries[0].isfile() or binaries[0].size>128*1024*1024:
                raise ValueError('invalid Starship archive')
            binary = tar.extractfile(binaries[0]).read()
        path = Path('/usr/local/bin/starship')
        for ancestor in (path,*path.parents):
            if ancestor.is_symlink():
                raise ValueError('symlink target: '+str(ancestor))
        fd, temporary = tempfile.mkstemp(prefix='.cqs-starship-',dir=path.parent)
        try:
            with os.fdopen(fd,'wb') as stream:
                stream.write(binary)
                os.fchmod(stream.fileno(),0o755)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary,path)
            fd = os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
except (OSError, ValueError, KeyError, TypeError, tarfile.TarError, subprocess.CalledProcessError) as error:
    print('Starship: '+str(error),file=sys.stderr)
    sys.exit(1)
PY
}

cqs_bbr_status() {
    uname -r || return 1
    sysctl net.ipv4.tcp_congestion_control net.ipv4.tcp_available_congestion_control net.core.default_qdisc || return 1
    get_bbr_version
    tc qdisc show
}

cqs_speedtest() (
    # shellcheck disable=SC2034 # acquire_lock writes the descriptor by nameref.
    local lock_fd output
    if ! command -v speedtest >/dev/null 2>&1; then
        require_root || return 1
        acquire_lock lock_fd "$CQS_KERNEL_LOCK" || return 1
        cqs_remote https://packagecloud.io/install/repositories/ookla/speedtest-cli/script.deb.sh || return $?
        apt-get install -y speedtest || return 1
    fi
    cqs_confirm 'Ookla Speedtest: accept the license and GDPR terms.' ACCEPT || return $?
    output=$(speedtest --accept-license --accept-gdpr -f json) || return 1
    printf '%s\n' "$output" | python3 -c '
import json,math,sys
try:
    data=json.load(sys.stdin)
    for key in ("download","upload"):
        bandwidth=data[key]["bandwidth"]
        if type(bandwidth) not in (int,float) or not math.isfinite(bandwidth):
            raise ValueError("invalid bandwidth: "+key)
        print(f"{key}: {bandwidth*8/1000000:.2f} Mbit/s")
    result=data.get("result")
    if isinstance(result,dict) and isinstance(result.get("url"),str):
        print(result["url"])
except (ValueError,KeyError,TypeError) as error:
    print(error,file=sys.stderr); sys.exit(1)
'
)

cqs_connectivity() {
    local family url failed=0
    for family in 4 6; do
        url="https://ipv${family}.icanhazip.com"
        printf 'IPv%s: ' "$family"
        curl "-$family" --proto '=https' -fSs --max-time 5 "$url" || failed=1
    done
    if [[ -f /etc/gai.conf ]]; then awk '/^[[:space:]]*precedence /' /etc/gai.conf || return 1; fi
    echo 'gai.conf does not determine address selection for every application.'
    return "$failed"
}

cqs_io() {
    python3 - "$1" <<'PY'
import os
from pathlib import Path
import sys
import tempfile
import time
try:
    path=Path(sys.argv[1])
    if not path.is_absolute() or '..' in path.parts or any(p.is_symlink() for p in (path,*path.parents)):
        raise ValueError('unsafe directory')
    speeds=[]
    block=b'\0'*(1024*1024)
    for index in range(3):
        with tempfile.TemporaryFile(dir=path) as stream:
            start=time.monotonic()
            for _ in range(1024):
                stream.write(block)
            stream.flush(); os.fsync(stream.fileno())
            speed=1024/(time.monotonic()-start)
            speeds.append(speed)
            print(f'{index+1}/3: {speed:.2f} MiB/s')
    print(f'Average: {sum(speeds)/3:.2f} MiB/s')
except (OSError,ValueError) as error:
    print(error,file=sys.stderr); sys.exit(1)
PY
}


# AI 安装使用官方元数据与 SHA-256；Python 仅处理结构化数据和原子文件操作。
cqs_ai_python() {
    python3 /dev/fd/3 "$@" 3<<'CQS_AI_PY'
import contextlib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import tempfile
import unicodedata

CLAUDE_RELEASES = "https://downloads.claude.ai/claude-code-releases"
MAX_FILE = 1024 * 1024 * 1024
REPOSITORIES = {"pi": "earendil-works/pi", "codex": "openai/codex"}
BINARIES = {"pi": "pi/pi", "claude": "claude", "codex": "bin/codex"}
ARCHITECTURES = {
    "x86_64": ("linux-x64", "x86_64-unknown-linux-musl", 62),
    "aarch64": ("linux-arm64", "aarch64-unknown-linux-musl", 183),
}


def require(condition, detail):
    if not condition:
        raise ValueError(detail)


def check_path(path, file=False):
    path = Path(path)
    require(path.is_absolute() and ".." not in path.parts
            and not any(unicodedata.category(c) == "Cc" for c in str(path)),
            f"需要无控制字符的绝对路径: {path}")
    for parent in (path, *path.parents):
        require(not parent.is_symlink(), f"拒绝符号链接路径: {parent}")
    if file and path.exists():
        require(path.is_file(), f"目标不是普通文件: {path}")
    return path


def launcher(home, tool):
    directory = os.environ.get("CODEX_INSTALL_DIR") if tool == "codex" else None
    return (Path(directory) if directory else home / ".local/bin") / tool


def check_launcher(target):
    check_path(target.parent)
    if os.path.lexists(target):
        mode = target.lstat().st_mode
        require(stat.S_ISREG(mode) or stat.S_ISLNK(mode), f"入口不是文件或符号链接: {target}")


def private_dir(path):
    check_path(path)
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.chmod(0o700)


def sync_parent(path):
    descriptor = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


@contextlib.contextmanager
def install_lock(state):
    import fcntl
    lock = state / "ai/install.lock"
    check_path(lock, file=True)
    private_dir(lock.parent)
    descriptor = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(descriptor)


def run(arguments, capture=False):
    result = subprocess.run(arguments, check=True, text=True,
                            stdout=subprocess.PIPE if capture else None)
    return result.stdout if capture else ""


def download(url, destination, timeout):
    run(["curl", "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2",
         "--fail", "--silent", "--show-error", "--location", "--connect-timeout", "15",
         "--max-time", str(timeout), "--output", str(destination), url])


def read_metadata(path, maximum):
    require(path.stat().st_size <= maximum, f"元数据过大: {path}")
    return path.read_bytes()


def version(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6}", value),
            "发行版本无效")
    return value


def digest(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-fA-F]{64}", value), "SHA-256 无效")
    return value.lower()


def asset_size(value):
    require(type(value) is int and 1 <= value <= MAX_FILE, "发行文件大小无效")
    return value


def github_package(tool, arch, release):
    require(isinstance(release, dict), "发行元数据必须为对象")
    repo = REPOSITORIES[tool]
    tag = release.get("tag_name", "")
    prefix = "rust-v" if tool == "codex" else "v"
    require(isinstance(tag, str) and tag.startswith(prefix), "发行标签无效")
    resolved = version(tag[len(prefix):])
    require(release.get("draft") is False and release.get("prerelease") is False,
            "拒绝草稿或预发行版本")
    name = f"pi-{arch[0]}.tar.gz" if tool == "pi" else f"codex-package-{arch[1]}.tar.gz"
    assets = release.get("assets")
    require(isinstance(assets, list), "发行资源无效")
    found = [asset for asset in assets if isinstance(asset, dict) and asset.get("name") == name]
    require(len(found) == 1, f"发行资源缺失或重复: {name}")
    asset = found[0]
    url = f"https://github.com/{repo}/releases/download/{tag}/{name}"
    require(asset.get("browser_download_url") == url, "发行下载地址无效")
    checksum = asset.get("digest", "")
    require(isinstance(checksum, str) and checksum.startswith("sha256:"), "缺少 SHA-256")
    return {"tool": tool, "arch": arch, "version": resolved, "url": url,
            "digest": digest(checksum[7:]), "size": asset_size(asset.get("size"))}


def claude_package(arch, resolved, manifest):
    resolved = version(resolved)
    require(isinstance(manifest, dict), "Claude 清单必须为对象")
    require("version" not in manifest or manifest["version"] == resolved, "Claude 版本不匹配")
    resource = manifest["platforms"][arch[0]]
    require(isinstance(resource, dict), "Claude 平台资源无效")
    return {"tool": "claude", "arch": arch, "version": resolved,
            "url": f"{CLAUDE_RELEASES}/{resolved}/{arch[0]}/claude",
            "digest": digest(resource.get("checksum")),
            "size": asset_size(resource["size"]) if "size" in resource else None}


def resolve(tool, arch, work):
    metadata = work / "release.json"
    if tool in REPOSITORIES:
        download(f"https://api.github.com/repos/{REPOSITORIES[tool]}/releases/latest", metadata, 60)
        return github_package(tool, arch, json.loads(read_metadata(metadata, 8 * 1024 * 1024)))
    latest = work / "latest"
    download(f"{CLAUDE_RELEASES}/latest", latest, 60)
    resolved = version(read_metadata(latest, 64).decode("utf-8").strip())
    download(f"{CLAUDE_RELEASES}/{resolved}/manifest.json", metadata, 60)
    return claude_package(arch, resolved, json.loads(read_metadata(metadata, 1024 * 1024)))


def validate_download(package, archive):
    length = archive.stat().st_size
    require(0 < length <= MAX_FILE and (package["size"] is None or package["size"] == length),
            "下载大小与发行元数据不符")
    checksum = hashlib.sha256()
    with archive.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            checksum.update(block)
    require(checksum.hexdigest() == package["digest"], "下载 SHA-256 不匹配")


def unpack(archive, destination):
    seen, total = set(), 0
    with tarfile.open(archive, "r:gz") as entries:
        for entry in entries:
            name = PurePosixPath(entry.name)
            require(entry.name and not name.is_absolute() and ".." not in name.parts
                    and not any(unicodedata.category(c) == "Cc" or c in "\\:" for c in entry.name)
                    and name not in seen and len(seen) < 20000,
                    f"归档路径无效或重复: {entry.name!r}")
            seen.add(name)
            target = destination.joinpath(*name.parts)
            if entry.isdir():
                target.mkdir(parents=True, exist_ok=True)
            elif entry.isfile() and not entry.issparse():
                total += entry.size
                require(0 <= entry.size <= MAX_FILE and total <= 2 * MAX_FILE, "归档大小超限")
                target.parent.mkdir(parents=True, exist_ok=True)
                with entries.extractfile(entry) as source, target.open("xb") as output:
                    shutil.copyfileobj(source, output, length=65536)
                    output.flush()
                    os.fchmod(output.fileno(), 0o755 if entry.mode & 0o111 else 0o644)
                    os.fsync(output.fileno())
            else:
                raise ValueError(f"拒绝归档链接、设备或稀疏文件: {entry.name!r}")


def elf(binary, arch):
    with binary.open("rb") as stream:
        header = stream.read(20)
    require(len(header) == 20 and header[:4] == b"\x7fELF" and header[4:7] == b"\x02\x01\x01"
            and int.from_bytes(header[16:18], "little") in (2, 3)
            and int.from_bytes(header[18:20], "little") == arch[2],
            f"ELF 格式或架构不匹配: {binary}")
    binary.chmod(0o755)


def validate_layout(package, root):
    tool, arch, resolved = package["tool"], package["arch"], package["version"]
    binary = root / BINARIES[tool]
    elf(binary, arch)
    if tool == "pi":
        metadata = json.loads(read_metadata(root / "pi/package.json", 1024 * 1024))
        require(isinstance(metadata, dict) and metadata.get("version") == resolved and (root / "pi/photon_rs_bg.wasm").is_file()
                and all((root / "pi" / path).is_dir() for path in ("theme", "assets", "export-html")),
                "Pi 版本或资源不完整")
    elif tool == "codex":
        metadata = json.loads(read_metadata(root / "codex-package.json", 1024 * 1024))
        expected = {"layoutVersion": 1, "version": resolved, "target": arch[1], "variant": "codex",
                    "entrypoint": "bin/codex", "resourcesDir": "codex-resources", "pathDir": "codex-path"}
        require(isinstance(metadata, dict) and all(metadata.get(key) == value for key, value in expected.items()), "Codex 包布局无效")
        for helper in ("bin/codex-code-mode-host", "codex-resources/bwrap", "codex-path/rg"):
            elf(root / helper, arch)
    return binary


def verify_version(binary, expected=None):
    output = run([str(binary), "--version"], capture=True).strip()
    require(output and (expected is None or expected in output.split()),
            f"版本验证失败: {binary}: expected {expected or '--version'}, got {output!r}")
    print(f"{binary}: {output}", flush=True)


def publish(binary, target, resolved):
    check_launcher(target)
    target.parent.mkdir(parents=True, exist_ok=True)
    recovery = Path(tempfile.mkdtemp(prefix=".cqs-ai-backup-", dir=target.parent))
    previous = recovery / "previous"
    had_previous = os.path.lexists(target)
    keep_recovery = False
    try:
        if had_previous:
            if target.is_symlink():
                previous.symlink_to(os.readlink(target))
            else:
                os.link(target, previous)
        next_entry = recovery / "next"
        next_entry.symlink_to(binary)
        sync_parent(next_entry)
        published = False
        try:
            os.replace(next_entry, target)
            published = True
            sync_parent(target)
            verify_version(target, resolved)
        except BaseException as error:
            if not published:
                raise
            try:
                if had_previous:
                    os.replace(previous, target)
                else:
                    target.unlink()
                sync_parent(target)
            except BaseException as rollback_error:
                keep_recovery = True
                raise RuntimeError(f"{error}; 入口回退失败: {rollback_error}; 恢复材料: {recovery}") from error
            raise
        if had_previous:
            keep_recovery = True
            print(f"旧入口备份: {previous}", flush=True)
    finally:
        if not keep_recovery:
            shutil.rmtree(recovery)


def install(tool):
    require(tool in BINARIES, f"未知 AI 工具: {tool}")
    require(platform.system() == "Linux", "AI 安装仅支持 Linux")
    require(not (os.geteuid() == 0 and os.environ.get("SUDO_USER", "") not in ("", "root")),
            "请以当前用户运行 AI 安装，不要使用 sudo")
    arch = ARCHITECTURES.get(platform.machine())
    require(arch is not None, f"不支持的 AI 架构: {platform.machine()}")
    home = check_path(Path(os.environ["HOME"]))
    target = launcher(home, tool)
    check_launcher(target)
    state = Path(os.environ.get("CQS_STATE_DIR") or
                 str(Path(os.environ.get("XDG_STATE_HOME") or home / ".local/state") / "cqs"))
    with install_lock(state), tempfile.TemporaryDirectory(prefix="cqs-ai-") as temporary:
        work = Path(temporary)
        package = resolve(tool, arch, work)
        detail = f"{tool} {package['version']}\n{package['url']}\nSHA-256: {package['digest']}\n{target}"
        run(["bash", "-c", 'cqs_confirm "$1" "$2"', "cqs-ai", detail, "YES"])
        archive = work / "package"
        download(package["url"], archive, 900)
        validate_download(package, archive)
        releases = home / ".local/share/cqs/ai" / tool
        private_dir(releases)
        staged = Path(tempfile.mkdtemp(prefix=f"{package['version']}-{arch[0]}-", dir=releases))
        keep_release = False
        try:
            if tool == "claude":
                shutil.copyfile(archive, staged / "claude")
            else:
                unpack(archive, staged)
            binary = validate_layout(package, staged)
            with binary.open("rb") as stream:
                os.fsync(stream.fileno())
            verify_version(binary, package["version"])
            # 发布后必须保留版本目录，避免回退失败使入口悬空。
            keep_release = True
            publish(binary, target, package["version"])
            print(f"AI 工具已就绪: {target}", flush=True)
        finally:
            if not keep_release:
                shutil.rmtree(staged)


def status():
    home = check_path(Path(os.environ["HOME"]))
    failures = []
    for tool in BINARIES:
        candidates = [launcher(home, tool)]
        if tool == "pi":
            candidates.append(Path(os.environ.get("PI_CODING_AGENT_DIR") or home / ".pi/agent") / "bin/pi")
        binary = next((candidate for candidate in candidates if os.path.lexists(candidate)), None)
        if binary is None:
            binary = shutil.which(tool)
        if binary is None:
            print(f"未安装: {tool}", flush=True)
            continue
        try:
            verify_version(binary)
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            print(str(error), file=sys.stderr)
            failures.append(tool)
    require(not failures, f"AI 版本验证失败: {', '.join(failures)}")


def main():
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        if sys.argv[1:] == ["status"]:
            status()
        elif len(sys.argv) == 3 and sys.argv[1] == "install":
            install(sys.argv[2])
        else:
            raise ValueError("AI 命令参数无效")
    except KeyboardInterrupt:
        print("AI 操作已取消", file=sys.stderr)
        return 130
    except subprocess.CalledProcessError as error:
        print(f"AI 命令失败: {error}", file=sys.stderr)
        return 130 if error.returncode in (130, -signal.SIGINT, -signal.SIGTERM) else 1
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, tarfile.TarError) as error:
        print(f"AI 操作失败: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
CQS_AI_PY
}

cqs_ai_install() (
    if ! command -v python3 >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; then
        printf '%s\n' 'AI 安装需要 python3 和 curl' >&2; return 1;
    fi
    export CQS_YES CQS_LANG CQS_STATE_DIR
    export -f cqs_confirm
    cqs_ai_python install "$1"
)

cqs_ai_status() {
    command -v python3 >/dev/null 2>&1 || { printf '%s\n' '需要 python3' >&2; return 1; }
    cqs_ai_python status
}


# A numbered menu uses the same action IDs, defaults and validation as the CLI.
# Python only handles terminal input here; actions re-enter this standalone script.
cqs_menu() {
    python3 /dev/fd/3 "${BASH_SOURCE[0]}" "$1" 3<<'PY'
import json
import os
import subprocess
import sys

script, metadata = sys.argv[1:]
with open(metadata, encoding='utf-8') as stream:
    data = json.load(stream)
rows, messages = data['rows'], data['messages']
zh = os.environ['CQS_LANG'] == 'zh-CN'
groups = ['system','network','kernel','ssh','docker','tools','ai','terminal','tests','app']
optimization = ['system.update','system.clean','tcp.tune','bbr.enable','system.timezone',
                'swap.auto','system.fail2ban','tools.install','docker.install','ssh.port','ssh.key']
by_id = {r['id']:r for r in rows}
base = ['bash', script, '--lang', os.environ['CQS_LANG']]
if data['config']:
    base += ['--config',data['config']]
if data['locale_dir']:
    base += ['--locale-dir',data['locale_dir']]
if os.environ.get('CQS_YES') == '1':
    base += ['--yes']

def ask(text):
    return input(text).strip()

def parameters(row):
    print(row['description'])
    result = {}
    for field in row['parameters']:
        choices = '/'.join(field['choices'])
        value = ask(f"{field['name']} ({choices}) [{field['default']}] (q=cancel): ")
        if value.lower() == 'q':
            return None
        result[field['name']] = value or field['default']
    return result

def execute(ids, params, batch=False):
    command = base + (['batch'] if batch else ['run']) + ids
    for action in ids:
        for key,value in params[action].items():
            command += ['--set', (action+'.' if batch else '')+key+'='+value]
    result = subprocess.run(command, check=False)
    print(('完成' if zh else 'Completed') if result.returncode == 0 else f"Failed/cancelled: {result.returncode}")

try:
    while True:
        print('\nCqShell · Shell')
        for n,group in enumerate(groups,1):
            print(f"{n}. {messages.get('group.'+group,group)}")
        print('o. '+('一键优化（默认全不选）' if zh else 'Optimization (nothing selected by default)'))
        print('r. '+('本机状态' if zh else 'Local status')+'   n. IP / ISP   l. Language   q. '+('退出' if zh else 'Quit'))
        choice = ask('> ').lower()
        if choice in ('q','0'):
            break
        if choice in ('r','n'):
            execute(['system.info' if choice=='r' else 'system.public_ip'], {'system.info':{},'system.public_ip':{}})
        elif choice == 'l':
            locales = [locale for locale, _name in data['languages']]
            new_locale = locales[(locales.index(os.environ['CQS_LANG']) + 1) % len(locales)]
            result = subprocess.run(base[:2]+['--lang',new_locale]+base[4:], check=False)
            sys.exit(result.returncode)
        elif choice == 'o':
            selected = {}
            while True:
                for n,action in enumerate(optimization,1):
                    print(f"[{'x' if action in selected else ' '}] {n}. {by_id[action]['label']}")
                value = ask('number=configure/toggle, s=run, n=clear, 0=back > ').lower()
                if value == '0':
                    break
                if value == 'n':
                    selected.clear()
                elif value == 's' and selected:
                    ids = [a for a in optimization if a in selected]
                    execute(ids, selected, batch=True)
                    break
                elif value.isdecimal() and 1<=int(value)<=len(optimization):
                    action = optimization[int(value)-1]
                    if action in selected:
                        del selected[action]
                    else:
                        params = parameters(by_id[action])
                        if params is not None:
                            selected[action] = params
        elif choice.isdecimal() and 1<=int(choice)<=len(groups):
            group = groups[int(choice)-1]
            options = [r for r in rows if r['group']==group]
            while True:
                for n,row in enumerate(options,1):
                    print(f"{n}. {row['label']} ({row['id']})")
                value = ask('0=back > ')
                if value == '0':
                    break
                if value.isdecimal() and 1<=int(value)<=len(options):
                    row = options[int(value)-1]
                    params = parameters(row)
                    if params is not None:
                        execute([row['id']],{row['id']:params})
except EOFError:
    pass
except KeyboardInterrupt:
    sys.exit(130)
PY
}


# Public CLI metadata is bundled below; execution never invokes the Rust binary.
cqs_prepare() {
    python3 - "$@" 3< <(cqs_metadata) <<'PY'
import json
import os
from pathlib import Path
import re
import sys
import tomllib
import unicodedata
from urllib.parse import urlsplit

def fail(detail):
    raise ValueError(detail)

def normalize(value):
    value = value.split('.')[0].split('@')[0].replace('_', '-')
    return {'zh':'zh-CN','zh-cn':'zh-CN','zh-hans':'zh-CN','zh-sg':'zh-CN',
            'en':'en-US','en-us':'en-US','c':'en-US','posix':'en-US'}.get(value.lower(), value)

def placeholders(text):
    keys = set(re.findall(r'\{([A-Za-z0-9_]+)\}', text))
    if '{' in re.sub(r'\{[A-Za-z0-9_]+\}', '', text) or '}' in re.sub(r'\{[A-Za-z0-9_]+\}', '', text):
        fail('invalid translation placeholders')
    return keys

try:
    data = json.load(os.fdopen(3))
    catalogs = data['catalogs']
    opts, positional, supplied = {}, [], {}
    args = iter(sys.argv[1:])
    for arg in args:
        option, separator, value = arg.partition('=')
        if option in ('--lang', '--config', '--locale-dir', '--set'):
            if not separator:
                value = next(args, None)
                if value is None or value.startswith('--'):
                    fail('missing value: ' + option)
            if option == '--set':
                key, equal, val = value.partition('=')
                if not equal or not key or key in supplied:
                    fail('invalid/duplicate --set: ' + key)
                supplied[key] = val
            elif option in opts:
                fail('duplicate option: ' + option)
            else:
                opts[option] = value
        elif arg in ('--yes', '-y', '--json', '--install', '--uninstall', '--help', '-h', '--version'):
            opts[arg] = True
        elif arg.startswith('-'):
            fail('unknown option: ' + arg)
        else:
            positional.append(arg)
    config = Path(opts.get('--config', str(Path(os.environ.get('XDG_CONFIG_HOME') or Path.home()/'.config')/'cqs/config.toml')))
    preferences = {}
    if config.exists() or '--config' in opts:
        preferences = tomllib.loads(config.read_text(encoding='utf-8'))
        if set(preferences) - {'language', 'locale_dir'} or any(not isinstance(v, str) for v in preferences.values()):
            fail('invalid preferences: ' + str(config))
    directory = opts.get('--locale-dir', preferences.get('locale_dir'))
    if directory is not None:
        reference = catalogs['en-US']['messages']
        for path in Path(directory).iterdir():
            if path.suffix != '.toml':
                continue
            catalog = tomllib.loads(path.read_text(encoding='utf-8'))
            if (set(catalog) != {'locale','name','messages'} or catalog['locale'] != path.stem
                    or not isinstance(catalog['name'], str) or not catalog['name'].strip()
                    or not isinstance(catalog['messages'], dict)
                    or set(catalog['messages']) != set(reference)):
                fail('invalid catalog: ' + str(path))
            for key, text in catalog['messages'].items():
                if not isinstance(text, str) or not text.strip() or placeholders(text) != placeholders(reference[key]):
                    fail('invalid translation: ' + key)
            catalogs[catalog['locale']] = catalog
    requested = opts.get('--lang', os.environ.get('CQS_LANG', preferences.get('language')))
    if requested is not None:
        locale = normalize(requested)
        if locale not in catalogs:
            fail('unsupported language: ' + requested)
    else:
        locale = normalize(next((os.environ[k] for k in ('LC_ALL','LC_MESSAGES','LANG') if os.environ.get(k)), 'en-US'))
        if locale not in catalogs:
            locale = 'en-US'
    messages = catalogs[locale]['messages']
    if os.environ.get('CQS_MESSAGES_FILE'):
        Path(os.environ['CQS_MESSAGES_FILE']).write_text(json.dumps(messages, ensure_ascii=False), encoding='utf-8')
    rows = data['actions']
    for row in rows:
        row['label'] = messages['action.'+row['id']]
        row['description'] = messages['description.'+row['id']]
    by_id = {row['id']: row for row in rows}
    languages = [[k, catalogs[k]['name']] for k in sorted(catalogs)]
    mode = positional[0] if positional else 'menu'
    yes = '--yes' in opts or '-y' in opts
    output = []
    if '--help' in opts or '-h' in opts:
        mode = 'print'
        output = [f"CqShell {data['version']} (Shell)", messages['app.description'],
                  'Usage: cqs.sh [--lang LOCALE] [--config FILE] [--locale-dir DIR] [--yes] COMMAND',
                  '  list [--json]                 List actions and parameters',
                  '  languages [--json]            List languages',
                  '  run ACTION [--set KEY=VALUE]   Execute one action',
                  '  batch ACTION... [--set ACTION.KEY=VALUE]',
                  '  --install / --uninstall       Manage the global cqs command',
                  '  --version / --help            Show version / help',
                  'No command: interactive menu. Requires Bash 4.4+ and Python 3.11+.']
    elif '--version' in opts:
        mode, output = 'print', ['cqs ' + data['version']]
    else:
        if '--install' in opts or '--uninstall' in opts:
            if positional or ('--install' in opts and '--uninstall' in opts):
                fail('--install/--uninstall conflicts with subcommand')
            mode = 'run'
            positional = ['run', 'app.install' if '--install' in opts else 'app.uninstall']
        if '--json' in opts and mode not in ('list','languages'):
            fail('--json requires list/languages')
        if mode in ('list','languages','menu'):
            if len(positional) > 1 or supplied:
                fail('unexpected action or --set')
            if mode == 'languages':
                output = [json.dumps(languages, ensure_ascii=False)] if '--json' in opts else ['\t'.join(v) for v in languages]
                mode = 'print'
            elif mode == 'list':
                if '--json' in opts:
                    output = [json.dumps(rows, ensure_ascii=False, indent=2)]
                else:
                    for row in rows:
                        output += [row['id']+'  '+row['label'], '  '+row['description']]
                        output += ['  '+f['name']+'='+f['default']+' '+str(f['choices']) for f in row['parameters']]
                mode = 'print'
            else:
                output = [json.dumps({'rows':rows, 'messages':messages, 'languages':languages,
                                      'config':opts.get('--config'), 'locale_dir':directory}, ensure_ascii=False)]
        elif mode in ('run','batch'):
            ids = positional[1:]
            if not ids or (mode == 'run' and len(ids) != 1) or len(set(ids)) != len(ids):
                fail('missing/duplicate action')
            remaining = set(supplied)
            for action in ids:
                if action not in by_id:
                    fail('unknown action: ' + action)
                row = by_id[action]
                fields = {f['name']: f for f in row['parameters']}
                params = {key:f['default'] for key,f in fields.items()}
                for key, value in supplied.items():
                    if mode == 'batch' and not key.startswith(action+'.'):
                        continue
                    name = key[len(action)+1:] if mode == 'batch' else key
                    if name not in fields or any(unicodedata.category(c) == 'Cc' for c in value):
                        fail('invalid parameter: '+key)
                    if fields[name]['choices'] and value not in fields[name]['choices']:
                        fail('invalid choice: '+key)
                    params[name] = value
                    remaining.remove(key)
                for key,f in fields.items():
                    if f['required'] and not params[key]:
                        fail('required parameter: '+key)
                if action == 'ssh.port' and not (re.fullmatch(r'[0-9]{1,5}',params['port']) and 1<=int(params['port'])<=65535):
                    fail('invalid port')
                if action == 'swap.create' and not (re.fullmatch(r'[0-9]+',params['size_mb']) and 1<=int(params['size_mb'])<=1048576):
                    fail('invalid size_mb')
                if action == 'tcp.tune':
                    for name, digits, maximum in [('bandwidth',6,100000),('rtt',6,10000)]:
                        v = params[name]
                        if not re.fullmatch(r'[0-9]{1,'+str(digits)+r'}(\.[0-9]{1,3})?',v) or not 0<float(v)<=maximum:
                            fail('invalid '+name)
                if action == 'kernel.install' and params['version'] and not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+',params['version']):
                    fail('invalid kernel version')
                if action == 'kernel.remove' and not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+-joeyblog-bbrv3(-max)?',params['release']):
                    fail('invalid kernel release')
                if action == 'system.timezone' and (params['timezone'].startswith('/') or any(p in ('','.','..') for p in params['timezone'].split('/'))):
                    fail('invalid timezone')
                if action in ('ssh.key','test.io'):
                    path = Path(params['public_key' if action == 'ssh.key' else 'directory'])
                    if not path.is_absolute() or '..' in path.parts:
                        fail('requires absolute path without ..')
                if action == 'docker.mirror':
                    url = urlsplit(params['mirror'])
                    # Accessing port validates both numeric syntax and the u16 range.
                    port = url.port
                    if (url.scheme not in ('http','https') or not url.hostname or url.username or url.password
                            or '?' in params['mirror'] or '#' in params['mirror']
                            or any(c.isspace() for c in url.hostname)):
                        fail('invalid mirror URL')
                # Translations may contain line breaks; keep the execution plan one row per action.
                summary = [re.sub(r'[\t\r\n]', ' ', row[key]) for key in ('label','description')]
                output.append('\t'.join([action,str(int(row['root'])),str(int(row['mutates']))]+summary+[k+'='+v for k,v in params.items()]))
            if remaining:
                fail('unused --set: '+', '.join(sorted(remaining)))
        else:
            fail('unknown command: '+mode)
    print(locale)
    print(int(yes))
    print(mode)
    print('\n'.join(output))
except (OSError, ValueError, KeyError, TypeError) as error:
    print('CqShell: '+str(error), file=sys.stderr)
    sys.exit(2)
PY
}

cqs_confirm() {
    local detail="$1" token="${2:-YES}" answer
    printf '%s\n' "$detail"
    [[ "${CQS_YES:-0}" != 1 ]] || return 0
    [[ -t 0 ]] || { printf '%s\n' 'Confirmation requires a terminal or --yes.' >&2; return 1; }
    if [[ "${CQS_LANG:-en-US}" == zh-CN ]]; then
        printf '输入 %s 确认（其他输入取消）: ' "$token"
    else
        printf 'Type %s to confirm (anything else cancels): ' "$token"
    fi
    read -r answer && [[ "$answer" == "$token" ]] && return 0
    return 130
}

cqs_platform() {
    local id
    [[ "$(uname -s)" == Linux ]] || { echo 'System actions require Debian / Ubuntu Linux.' >&2; return 1; }
    id=$(awk -F= '$1=="ID" {gsub(/"/,"",$2);print $2}' /etc/os-release) || return 1
    [[ "$id" == debian || "$id" == ubuntu ]] || { echo "Unsupported OS: $id" >&2; return 1; }
}

cqs_execute() {
    local row="$1" id root changes label description token=YES parameter memory buffer
    local -a parts
    local -A p=()
    IFS=$'\t' read -ra parts <<< "$row"
    id=${parts[0]}; root=${parts[1]}; changes=${parts[2]}; label=${parts[3]}; description=${parts[4]}
    for parameter in "${parts[@]:5}"; do p[${parameter%%=*}]=${parameter#*=}; done
    cqs_platform || return 1
    [[ "$root" != 1 ]] || require_root || return 1
    if [[ "$changes" == 1 ]]; then
        case "$id" in
            kernel.install|kernel.remove|ai.*.install|system.mirror.apply) ;;
            *)
                [[ "$id" != terminal.restore ]] || token=RESTORE
                cqs_confirm "$label"$'\n'"$description"$'\n'"${parts[*]:5}" "$token" || return $?
                ;;
        esac
    fi
    case "$id" in
        system.info) cqs_system_info ;;
        system.public_ip) cqs_system_public_ip ;;
        system.update) cqs_system_update ;;
        system.mirror.test) cqs_mirrors test ;;
        system.mirror.apply) cqs_mirrors apply "${p[mirror_site]}" ;;
        system.mirror.restore) cqs_mirrors restore ;;
        system.clean) cqs_system_clean "${p[scope]}" ;;
        system.timezone) cqs_system_timezone "${p[timezone]}" ;;
        swap.create) cqs_swap create "${p[size_mb]}" ;;
        swap.auto) cqs_swap auto ;;
        swap.remove) cqs_swap remove ;;
        system.fail2ban) cqs_fail2ban ;;
        ssh.port) cqs_ssh_port "${p[port]}" ;;
        ssh.key) cqs_ssh_key "${p[public_key]}" ;;
        bbr.status) cqs_bbr_status ;;
        bbr.enable)
            load_bbr_module && apply_bbr_config "$CQS_BBR_CONFIG" "${p[qdisc]}"
            ;;
        bbr.restore) network_restore_config "$CQS_BBR_CONFIG" ;;
        tcp.tune)
            memory=$(network_memory_kb) || return 1
            buffer=$(calculate_tcp_buffer "${p[bandwidth]}" "${p[rtt]}" "$memory") || return 1
            printf 'TCP: %s MiB; memory: %s MiB\n' "$((buffer/1048576))" "$((memory/1024))"
            apply_tcp_profile "$CQS_TCP_CONFIG" "$buffer"
            ;;
        tcp.restore) network_restore_config "$CQS_TCP_CONFIG" ;;
        kernel.list) bbrv3_list_kernels ;;
        kernel.install) bbrv3_install_kernel "${p['profile']}" "${p['version']}" ;;
        kernel.remove) bbrv3_remove_kernel "${p['release']}" ;;
        tools.install) cqs_packages "${p['package']}" ;;
        tools.*) cqs_packages "${id#tools.}" ;;
        ai.*.install) parameter=${id#ai.}; cqs_ai_install "${parameter%.install}" ;;
        ai.status) cqs_ai_status ;;
        terminal.install) cqs_terminal_install ;;
        terminal.apply) terminal_apply "${p[shell]}" "${p[style]}" "${p[color]}" "${p[time]}" "${p[git]}" ;;
        terminal.status) terminal_config status ;;
        terminal.restore) terminal_config restore ;;
        docker.install) cqs_docker install "${p['source']}" ;;
        docker.status) cqs_docker status ;;
        docker.clean) cqs_docker clean ;;
        docker.mirror) cqs_docker mirror "${p[mirror]}" ;;
        docker.ipv6) cqs_docker ipv6 "${p[enabled]}" ;;
        docker.uninstall) cqs_docker uninstall "${p[purge_data]}" ;;
        test.speed) cqs_speedtest ;;
        test.ip) cqs_ip_test "${p[ip_family]}" "${p[full_ip]}" ;;
        test.route) cqs_remote https://nxtrace.org/nt && nexttrace --fast-trace --tcp ;;
        test.performance) cqs_remote https://yabs.sh ;;
        test.connectivity) cqs_connectivity ;;
        test.io) cqs_io "${p[directory]}" ;;
        app.install) install_shortcut ;;
        app.uninstall) uninstall_shortcut ;;
        *) echo "Unknown action: $id" >&2; return 2 ;;
    esac
}

cqs_main() (
    local work mode row status failed=0
    require_commands python3 || return 1
    python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3,11) else 1)' || {
        echo 'CqShell Shell requires Python 3.11+ (TOML support).' >&2; return 1;
    }
    work=$(mktemp -d) || return 1
    export CQS_MESSAGES_FILE="$work/messages.json"
    trap 'rm -rf -- "$work"' EXIT
    trap 'exit 130' INT TERM
    cqs_prepare "$@" > "$work/plan" || return $?
    exec 4< "$work/plan"
    read -r CQS_LANG <&4; read -r CQS_YES <&4; read -r mode <&4
    export CQS_LANG CQS_YES CQS_INSTALL_PATH CQS_STATE_DIR CQS_KERNEL_LOCK CQS_KERNEL_BACKUP CQS_TERMINAL_CONFIG
    export PATH="$PATH:/usr/local/sbin:/usr/sbin:/sbin"
    case "$mode" in
        print) cat <&4 ;;
        menu)
            [[ -t 0 && -t 1 ]] || { echo 'A terminal is required; use list / run / batch.' >&2; return 1; }
            cat <&4 > "$work/menu.json"
            cqs_menu "$work/menu.json"
            ;;
        run|batch)
            if [[ "$mode" == batch ]]; then cqs_confirm "$(tail -n +4 "$work/plan")" YES || return $?; fi
            while IFS= read -r row <&4; do
                [[ -n "$row" ]] || continue
                cqs_execute "$row"; status=$?
                if (( status )); then
                    printf 'Failed (%s): %s\n' "$status" "${row%%$'\t'*}" >&2
                    [[ "$status" != 130 && "$status" != 143 ]] || return 130
                    failed=1
                    case "${row%%$'\t'*}" in tcp.tune|bbr.enable) ;; *) return 1 ;; esac
                    [[ "$mode" == batch ]] || return 1
                fi
            done
            return "$failed"
            ;;
    esac
)


# BEGIN BUNDLED CLI DATA — keep aligned with src/actions.rs and locales/*.toml.
cqs_metadata() {
    cat <<'CQS_METADATA_JSON'
{
  "version": "0.1.1",
  "actions": [
    {
      "available": true,
      "description": "Read host, CPU, memory, disk, IP and BBR status.",
      "group": "system",
      "id": "system.info",
      "interactive": false,
      "label": "System overview",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Query IPv4, IPv6 and ISP over HTTPS. Contacts ip.sb and ipinfo.io only when selected.",
      "group": "system",
      "id": "system.public_ip",
      "interactive": false,
      "label": "Public IP / ISP lookup",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Update apt metadata and upgrade packages under the shared kernel lock. Use noninteractive mode and preserve locally edited configuration. List services needing restart for later maintenance.",
      "group": "system",
      "id": "system.update",
      "interactive": false,
      "label": "Update packages",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Download a 1 MiB repository index sample from each of four HTTPS mirrors on this server, at most 8 seconds per mirror. Rank measured transfer speed, including connection time; report failed samples. Does not change sources. Results are a short sample, not a sustained bandwidth guarantee.",
      "group": "system",
      "id": "system.mirror.test",
      "interactive": false,
      "label": "Benchmark system mirrors",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Use pinned, SHA-256 verified LinuxMirrors Lite. Choose official, Tsinghua, USTC, Aliyun, or auto to benchmark then confirm the fastest successful sample. Rebuild standard APT sources with upstream component/source-package defaults; keep separate third-party files. Use official security repositories. Back up and restore configuration on failure; never upgrade packages.",
      "group": "system",
      "id": "system.mirror.apply",
      "interactive": false,
      "label": "Change system mirror · LinuxMirrors",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "auto",
            "official",
            "tuna",
            "ustc",
            "aliyun"
          ],
          "default": "auto",
          "name": "mirror_site",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Recover an interrupted mirror operation, or undo the last successful change. Refuse to overwrite later manual edits. Restore configuration before refreshing APT; an index refresh failure keeps the restored configuration.",
      "group": "system",
      "id": "system.mirror.restore",
      "interactive": false,
      "label": "Restore previous system sources",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Standard: apt cache and journals older than 7 days. Full also deletes logs, temporary files, user caches and unused Docker data. All kernels are retained.",
      "group": "system",
      "id": "system.clean",
      "interactive": false,
      "label": "Clean system",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "standard",
            "full"
          ],
          "default": "standard",
          "name": "scope",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Set the system timezone using timedatectl.",
      "group": "system",
      "id": "system.timezone",
      "interactive": false,
      "label": "Set timezone",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "Asia/Shanghai",
          "name": "timezone",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Create /swapfile and register it in fstab; refuse an existing file.",
      "group": "system",
      "id": "swap.create",
      "interactive": false,
      "label": "Create swap",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "2048",
          "name": "size_mb",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Compute swap size from physical memory and rebuild /swapfile. Preserve the previous file until the operation succeeds.",
      "group": "system",
      "id": "swap.auto",
      "interactive": false,
      "label": "Rebuild swap automatically",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Disable and delete only /swapfile and its exact fstab entry.",
      "group": "system",
      "id": "swap.remove",
      "interactive": false,
      "label": "Remove swap",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install fail2ban and a managed sshd jail using the actual SSH ports and systemd journal.",
      "group": "system",
      "id": "system.fail2ban",
      "interactive": false,
      "label": "Install fail2ban",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Validate the candidate and actual listener; roll back on failure. External firewall reachability must be checked separately.",
      "group": "ssh",
      "id": "ssh.port",
      "interactive": false,
      "label": "Change SSH port",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "55520",
          "name": "port",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Import a public key for the current HOME, test a new connection, then disable global password and keyboard-interactive authentication. Never print private keys.",
      "group": "ssh",
      "id": "ssh.key",
      "interactive": false,
      "label": "Configure key-only SSH login",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "",
          "name": "public_key",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Inspect the running implementation, congestion control and real interface queues.",
      "group": "network",
      "id": "bbr.status",
      "interactive": false,
      "label": "BBR and queue status",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Enable the BBR implementation supplied by the running kernel, including BBRv3 when provided. Set the default queue and preserve the original settings for restoration.",
      "group": "network",
      "id": "bbr.enable",
      "interactive": false,
      "label": "Enable BBR in the running kernel",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "fq",
            "fq_codel",
            "fq_pie",
            "cake"
          ],
          "default": "fq",
          "name": "qdisc",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Restore the recorded runtime values and original persistent configuration.",
      "group": "network",
      "id": "bbr.restore",
      "interactive": false,
      "label": "Restore BBR settings",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Calculate a buffer ceiling from bandwidth, RTT and cgroup memory limits; apply, verify and persist with recovery.",
      "group": "network",
      "id": "tcp.tune",
      "interactive": false,
      "label": "Tune TCP buffers",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "1000",
          "name": "bandwidth",
          "required": true
        },
        {
          "choices": [],
          "default": "50",
          "name": "rtt",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Restore the first recorded TCP runtime values and configuration.",
      "group": "network",
      "id": "tcp.restore",
      "interactive": false,
      "label": "Restore TCP settings",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Show installed kernel packages and the currently running release.",
      "group": "kernel",
      "id": "kernel.list",
      "interactive": false,
      "label": "List kernels",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install a Standard or Max BBRv3 kernel from byJoey/Actions-bbr-v3, using the latest matching release or a specified version. Verify packages, back up GRUB and update boot configuration. After booting the new kernel, configure BBR and its queue under Network.",
      "group": "kernel",
      "id": "kernel.install",
      "interactive": false,
      "label": "Install BBRv3 kernel",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "standard",
            "max"
          ],
          "default": "standard",
          "name": "profile",
          "required": true
        },
        {
          "choices": [],
          "default": "",
          "name": "version",
          "required": false
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Simulate removal and refuse extra package removals; preserve the running kernel and boot backup.",
      "group": "kernel",
      "id": "kernel.remove",
      "interactive": false,
      "label": "Remove a non-running BBRv3 kernel",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "",
          "name": "release",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install curl through apt for downloads and network requests.",
      "group": "tools",
      "id": "tools.curl",
      "interactive": false,
      "label": "curl downloader",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install wget through apt for file downloads.",
      "group": "tools",
      "id": "tools.wget",
      "interactive": false,
      "label": "wget downloader",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install the nano terminal text editor through apt.",
      "group": "tools",
      "id": "tools.nano",
      "interactive": false,
      "label": "nano text editor",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install unzip through apt to extract ZIP archives.",
      "group": "tools",
      "id": "tools.unzip",
      "interactive": false,
      "label": "unzip archive extractor",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install tar through apt to create and extract archives.",
      "group": "tools",
      "id": "tools.tar",
      "interactive": false,
      "label": "tar archiver",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install tmux through apt to retain sessions and manage multiple terminals.",
      "group": "tools",
      "id": "tools.tmux",
      "interactive": false,
      "label": "tmux terminal multiplexer",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install iftop through apt to inspect live network traffic.",
      "group": "tools",
      "id": "tools.iftop",
      "interactive": false,
      "label": "iftop network monitor",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install btop through apt to inspect CPU, memory and processes.",
      "group": "tools",
      "id": "tools.btop",
      "interactive": false,
      "label": "btop system monitor",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install gdu through apt to analyze directory disk usage.",
      "group": "tools",
      "id": "tools.gdu",
      "interactive": false,
      "label": "gdu disk usage analyzer",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install the fzf interactive fuzzy finder through apt.",
      "group": "tools",
      "id": "tools.fzf",
      "interactive": false,
      "label": "fzf fuzzy finder",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install a selected package; all also installs zsh and Starship without enabling prompt customization.",
      "group": "tools",
      "id": "tools.install",
      "interactive": false,
      "label": "Install all / choose a package",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "all",
            "curl",
            "wget",
            "nano",
            "unzip",
            "tar",
            "tmux",
            "iftop",
            "btop",
            "gdu",
            "fzf"
          ],
          "default": "all",
          "name": "package",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Download the official standalone Pi Linux binary package, verify SHA-256 and retain themes, WASM and other resources. No Node.js/npm required. Verify the version before updating the current user's command.",
      "group": "ai",
      "id": "ai.pi.install",
      "interactive": false,
      "label": "Install / update Pi · pi.dev",
      "mutates": true,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Download the official Claude Code Linux binary and verify SHA-256 against its manifest. No Node.js/npm required. Verify the version before updating ~/.local/bin/claude; retain user configuration and previous versions.",
      "group": "ai",
      "id": "ai.claude.install",
      "interactive": false,
      "label": "Install / update Claude Code",
      "mutates": true,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Download the complete official Codex Linux binary package with its helper programs and resources. Verify SHA-256 and the version. No Node.js/npm required. Default entrypoint: ~/.local/bin/codex; respects CODEX_INSTALL_DIR.",
      "group": "ai",
      "id": "ai.codex.install",
      "interactive": false,
      "label": "Install / update Codex CLI",
      "mutates": true,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Check common user installation directories and PATH for Pi, Claude Code and Codex, then read --version. Distinguish missing commands from execution failures. Does not sign in.",
      "group": "ai",
      "id": "ai.status",
      "interactive": false,
      "label": "AI tool versions",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install zsh and a checksum-verified Starship release. Keep the login shell unchanged.",
      "group": "terminal",
      "id": "terminal.install",
      "interactive": false,
      "label": "Install zsh / Starship",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Update only managed shell initialization blocks and a dedicated Starship TOML file. Preserve a first-use baseline.",
      "group": "terminal",
      "id": "terminal.apply",
      "interactive": false,
      "label": "Configure prompt",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "bash",
            "zsh",
            "both"
          ],
          "default": "bash",
          "name": "shell",
          "required": true
        },
        {
          "choices": [
            "compact",
            "full"
          ],
          "default": "full",
          "name": "style",
          "required": true
        },
        {
          "choices": [
            "cyan",
            "green",
            "magenta"
          ],
          "default": "cyan",
          "name": "color",
          "required": true
        },
        {
          "choices": [
            "on",
            "off"
          ],
          "default": "off",
          "name": "time",
          "required": true
        },
        {
          "choices": [
            "on",
            "off"
          ],
          "default": "on",
          "name": "git",
          "required": true
        }
      ],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Read saved prompt options and recovery baseline status for the current HOME.",
      "group": "terminal",
      "id": "terminal.status",
      "interactive": false,
      "label": "Prompt status",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Restore files byte-for-byte from the first-use baseline. This overwrites later manual edits to managed files.",
      "group": "terminal",
      "id": "terminal.restore",
      "interactive": false,
      "label": "Restore original prompt files",
      "mutates": true,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Run the explicitly selected third-party installer; keep existing daemon configuration.",
      "group": "docker",
      "id": "docker.install",
      "interactive": true,
      "label": "Install Docker",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "official",
            "china"
          ],
          "default": "official",
          "name": "source",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Show versions, images, containers, volumes, networks and registry mirrors.",
      "group": "docker",
      "id": "docker.status",
      "interactive": false,
      "label": "Docker status",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Irreversibly remove unused images, stopped containers, networks and eligible volumes.",
      "group": "docker",
      "id": "docker.clean",
      "interactive": false,
      "label": "Prune Docker data",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Merge the mirror into daemon.json, validate and restart Docker; restore configuration and service state on failure.",
      "group": "docker",
      "id": "docker.mirror",
      "interactive": false,
      "label": "Set registry mirror",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "",
          "name": "mirror",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Merge IPv6 settings and restart Docker. A generated ULA subnet does not provide public IPv6 routing.",
      "group": "docker",
      "id": "docker.ipv6",
      "interactive": false,
      "label": "Configure Docker IPv6",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "on",
            "off"
          ],
          "default": "on",
          "name": "enabled",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Remove installed Docker packages. Data is retained unless purge_data=on explicitly deletes the default data/config directories.",
      "group": "docker",
      "id": "docker.uninstall",
      "interactive": false,
      "label": "Uninstall Docker",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "on",
            "off"
          ],
          "default": "off",
          "name": "purge_data",
          "required": true
        }
      ],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Run Ookla Speedtest; confirm its terms. Installation requires root if the binary is missing.",
      "group": "tests",
      "id": "test.speed",
      "interactive": true,
      "label": "Speedtest",
      "mutates": true,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Use xykt/IPQuality for IP type, risk scores/factors, streaming/AI access and mail checks. Choose dual-stack/IPv4/IPv6; language follows the UI. Online report upload is disabled; full IP display is off by default. The script takes over the terminal and may ask to install missing dependencies.",
      "group": "tests",
      "id": "test.ip",
      "interactive": true,
      "label": "IP quality · xykt/IPQuality",
      "mutates": true,
      "parameters": [
        {
          "choices": [
            "dual",
            "ipv4",
            "ipv6"
          ],
          "default": "dual",
          "name": "ip_family",
          "required": true
        },
        {
          "choices": [
            "off",
            "on"
          ],
          "default": "off",
          "name": "full_ip",
          "required": true
        }
      ],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Install NextTrace with its upstream script, then run a fast TCP trace.",
      "group": "tests",
      "id": "test.route",
      "interactive": true,
      "label": "Return-route test",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Run the third-party YABS script interactively; may consume CPU, disk and network resources.",
      "group": "tests",
      "id": "test.performance",
      "interactive": true,
      "label": "YABS benchmark",
      "mutates": true,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Probe both address families over HTTPS and show explicit gai.conf precedence entries.",
      "group": "tests",
      "id": "test.connectivity",
      "interactive": false,
      "label": "IPv4 / IPv6 connectivity",
      "mutates": false,
      "parameters": [],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Write and sync a 1 GiB temporary file three times in the selected directory; remove it afterward.",
      "group": "tests",
      "id": "test.io",
      "interactive": false,
      "label": "Disk I/O benchmark",
      "mutates": true,
      "parameters": [
        {
          "choices": [],
          "default": "/var/tmp",
          "name": "directory",
          "required": true
        }
      ],
      "root": false,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Atomically install this executable at CQS_INSTALL_PATH (default /usr/local/bin/cqs).",
      "group": "app",
      "id": "app.install",
      "interactive": false,
      "label": "Install cqs command",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    },
    {
      "available": true,
      "description": "Remove only the installed command. Keep software, configuration and recovery material.",
      "group": "app",
      "id": "app.uninstall",
      "interactive": false,
      "label": "Uninstall cqs command",
      "mutates": true,
      "parameters": [],
      "root": true,
      "unavailable_reason": null
    }
  ],
  "catalogs": {
    "en-US": {
      "locale": "en-US",
      "name": "English",
      "messages": {
        "notice.package_audit": "The package operation did not complete. After apt/dpkg exits, check in a separate terminal: {detail}. Finish configuration if needed. Do not delete lock files.",
        "action.tools.curl": "curl downloader",
        "description.tools.curl": "Install curl through apt for downloads and network requests.",
        "action.tools.wget": "wget downloader",
        "description.tools.wget": "Install wget through apt for file downloads.",
        "action.tools.nano": "nano text editor",
        "description.tools.nano": "Install the nano terminal text editor through apt.",
        "action.tools.unzip": "unzip archive extractor",
        "description.tools.unzip": "Install unzip through apt to extract ZIP archives.",
        "action.tools.tar": "tar archiver",
        "description.tools.tar": "Install tar through apt to create and extract archives.",
        "action.tools.tmux": "tmux terminal multiplexer",
        "description.tools.tmux": "Install tmux through apt to retain sessions and manage multiple terminals.",
        "action.tools.iftop": "iftop network monitor",
        "description.tools.iftop": "Install iftop through apt to inspect live network traffic.",
        "action.tools.btop": "btop system monitor",
        "description.tools.btop": "Install btop through apt to inspect CPU, memory and processes.",
        "action.tools.gdu": "gdu disk usage analyzer",
        "description.tools.gdu": "Install gdu through apt to analyze directory disk usage.",
        "action.tools.fzf": "fzf fuzzy finder",
        "description.tools.fzf": "Install the fzf interactive fuzzy finder through apt.",
        "group.ai": "AI tools",
        "action.ai.pi.install": "Install / update Pi · pi.dev",
        "description.ai.pi.install": "Download the official standalone Pi Linux binary package, verify SHA-256 and retain themes, WASM and other resources. No Node.js/npm required. Verify the version before updating the current user's command.",
        "action.ai.claude.install": "Install / update Claude Code",
        "description.ai.claude.install": "Download the official Claude Code Linux binary and verify SHA-256 against its manifest. No Node.js/npm required. Verify the version before updating ~/.local/bin/claude; retain user configuration and previous versions.",
        "action.ai.codex.install": "Install / update Codex CLI",
        "description.ai.codex.install": "Download the complete official Codex Linux binary package with its helper programs and resources. Verify SHA-256 and the version. No Node.js/npm required. Default entrypoint: ~/.local/bin/codex; respects CODEX_INSTALL_DIR.",
        "action.ai.status": "AI tool versions",
        "description.ai.status": "Check common user installation directories and PATH for Pi, Claude Code and Codex, then read --version. Distinguish missing commands from execution failures. Does not sign in.",
        "confirm.ai_install": "Install this official binary release? Verify SHA-256, architecture and version before switching the command. Back up the existing entrypoint and retain previous versions and login configuration. Restart the tool after updating.\n{detail}",
        "notice.ai_version": "Verified command: {detail}",
        "notice.ai_missing": "Command not found: {detail}",
        "notice.ai_ready": "Binary installed and verified: {detail}. Launch using this full path, or add its directory to PATH. Use the same install action for future updates and sign in through the tool on first use.",
        "error.ai_arch": "AI binary installation supports Linux x86_64 / ARM64 only: {detail}",
        "notice.ai_backup": "Previous command entrypoint backed up (previous file): {detail}",
        "error.ai_sudo": "Run cqs without sudo as the intended AI tool user to avoid installing into root's HOME. A direct root login can still install for root: {detail}",
        "field.ip_family": "IP family (dual / ipv4 / ipv6)",
        "field.full_ip": "Show the full IP address in the report",
        "confirm.ip_quality": "Run xykt/IPQuality? It queries external risk databases, streaming/AI services and mail connectivity. Privacy mode disables its online report upload; external providers still receive probe requests. Missing dependencies may trigger an upstream installation prompt.\n{detail}",
        "error.busy": "Another task is still running. {detail}",
        "ui.cancelled": "Cancelled",
        "ui.unavailable": "Unavailable in this environment",
        "ui.preview_banner": "Preview mode · live local metrics; system actions require Debian / Ubuntu",
        "ui.root_banner": "Local server · administrator mode · review changes before confirming",
        "ui.user_banner": "Read-only / user mode · use sudo cqs for system changes",
        "ui.optimization_title": "Optimization · Selected: {count}",
        "ui.optimization_hint": "Space: select  Enter: settings  F5: review/run  Tab: focus  Esc: back",
        "ui.status_hint": "R: refresh  N: public IP  G: log  F2: full log  L: language  O: optimize",
        "ui.optimization_details": "Choose only what you need. Enter configures an item; F5 reviews the entire selection before execution.",
        "ui.action_details": "Enter opens this action; Esc returns to categories.",
        "ui.not_set": "Required — not set",
        "ui.form_batch_title": "Configure this item and add it to the optimization selection.",
        "ui.form_single_title": "Set parameters, then Continue to review and execute.",
        "ui.save_selection": "Save to selection",
        "ui.continue": "Continue",
        "ui.cancel": "Cancel",
        "ui.confirm_execute": "Confirm execution",
        "ui.yes_no_hint": "Default: Cancel. ←→/Tab selects; Enter confirms. Y accepts / N or Esc cancels.",
        "ui.batch_title": "Optimization ({count} actions)",
        "ui.empty_log": "No task has run yet. Esc or F2 returns.",
        "ui.overview_hint": "R: refresh local data  N: public IP / ISP  PgUp/PgDn: scroll  Esc: back",
        "ui.back_hint": "Enter / Esc: back",
        "ui.notice": "Notice",
        "dash.title": "Live system status",
        "dash.age": "local update {age} ago",
        "dash.refresh_hint": "Local metrics refresh every 3s; N queries public IP / ISP.",
        "dash.public_age": "Public lookup age",
        "dash.loading": "Collecting…",
        "dash.not_queried": "N to query",
        "dash.na": "N/A",
        "dash.unknown": "Unknown",
        "dash.available": "available",
        "dash.unavailable": "not provided",
        "dash.configured": "file present",
        "dash.not_configured": "not configured",
        "dash.host": "Host",
        "dash.os": "OS",
        "dash.kernel": "Kernel",
        "dash.arch": "Arch / uptime",
        "dash.cpu": "CPU",
        "dash.cpu_usage": "CPU usage",
        "dash.memory": "Memory",
        "dash.disk": "Disk",
        "dash.swap": "Swap",
        "dash.timezone": "Timezone",
        "dash.ipv4": "IPv4",
        "dash.ipv6": "IPv6",
        "dash.isp": "ISP",
        "dash.network": "Congestion / queue",
        "dash.qdisc": "Queue",
        "dash.tcp": "TCP config",
        "dash.bbr": "BBR / module",
        "dash.diagnostics": "Unavailable metrics / probe errors",
        "action.system.public_ip": "Public IP / ISP lookup",
        "description.system.public_ip": "Query IPv4, IPv6 and ISP over HTTPS. Contacts ip.sb and ipinfo.io only when selected.",
        "notice.cpu_usage": "CPU usage: {detail}",
        "notice.tcp_config": "Managed TCP configuration: {detail}",
        "notice.public_ip": "Public network: {detail}",
        "group.optimization": "Optimization",
        "app.description": "Debian / Ubuntu server toolbox",
        "ui.categories": "Categories",
        "ui.actions": "Actions",
        "ui.details": "Details",
        "ui.logs": "Execution log",
        "ui.ready": "Ready",
        "ui.running": "Running — cancellation waits for cleanup",
        "ui.cancelling": "Cancelling — waiting for child processes and recovery to finish safely",
        "ui.done": "Completed",
        "ui.failed": "Failed",
        "ui.hint": "Tab: focus  ↑↓: move  Enter: open  Esc: back  Q: quit",
        "ui.busy_hint": "Esc: cancel task; Q / Ctrl+C: cancel and exit after cleanup",
        "ui.form_hint": "Tab/↑↓: field  ←→: choice/cursor  Enter: next  Ctrl+Enter: apply  Esc: cancel",
        "ui.confirm_hint": "PgUp/PgDn: review text  Enter: confirm exact token  Esc: cancel",
        "ui.type_confirm": "Type {token} exactly, then press Enter to confirm:",
        "ui.confirm": "Confirm operation",
        "ui.small": "Terminal too small. Resize to at least 80 × 24, or use the CLI.",
        "ui.external": "An interactive program will take over the terminal. Return here after it exits.",
        "ui.return": "Press Enter to return to the TUI.",
        "ui.no_selection": "Select actions with Space first.",
        "ui.readonly": "Read only",
        "ui.mutating": "Changes system or user files",
        "ui.root": "Requires root",
        "ui.user": "Current user",
        "status.enabled": "Enabled",
        "status.disabled": "Disabled",
        "cli.lang": "Language (zh-CN or en-US; external TOML catalogs also supported)",
        "cli.config": "Preferences TOML file",
        "cli.locale_dir": "Directory containing complete locale TOML catalogs",
        "cli.yes": "Accept operation confirmations automatically",
        "cli.json": "Print structured JSON for list/languages",
        "cli.list": "List available actions and parameters",
        "cli.run": "Run one action",
        "cli.batch": "Run the named actions in the given order",
        "cli.languages": "Validate and list all loaded TOML language catalogs",
        "cli.action": "Stable action ID (see list)",
        "cli.set": "Parameter key=value; batch uses action.id.key=value",
        "cli.install": "Install this binary as the cqs command",
        "cli.uninstall": "Uninstall the cqs command, preserving recovery files",
        "cli.help": "Show help",
        "cli.version": "Show version",
        "cli.usage": "Usage",
        "cli.options": "Options",
        "cli.commands": "Commands",
        "confirm.action": "Review this operation before continuing:\n{detail}",
        "confirm.batch": "Run this selection in order? Individual operations may have further confirmations.\n{detail}",
        "confirm.remote": "Download and execute this third-party Bash script with the current user's privileges. HTTPS is not an independent signature:\n{detail}",
        "confirm.kernel_download": "Download and verify the selected BBRv3 kernel?\n{detail}",
        "confirm.kernel_install": "Package checks passed. Install this kernel and update GRUB?\n{detail}",
        "confirm.kernel_remove": "Remove this non-running kernel and update GRUB?\n{detail}",
        "confirm.key_tested": "Public key installed. Open a NEW SSH connection with that key and verify access before confirming. This will disable password and keyboard-interactive authentication in the global configuration. Keep the existing session open.\n{detail}",
        "confirm.clean_full": "Irreversible cleanup: truncate logs, delete old temporary files and user caches, and prune unused Docker data:\n{detail}",
        "confirm.speedtest": "Accept the Ookla Speedtest license/GDPR terms and run a bandwidth test (may consume significant traffic)? {detail}",
        "notice.saved": "Configuration saved: {detail}",
        "notice.restored": "Original state restored: {detail}",
        "notice.backup": "Recovery material: {detail}",
        "notice.qdisc": "Default queue set to {detail}. Existing connections and interface queues may retain their current settings.",
        "notice.buffer": "TCP buffer ceiling / effective memory: {detail}. Concurrent sockets share memory; benchmark the target route.",
        "notice.bbr_version": "Running BBR implementation version: {detail}",
        "notice.bbr_unknown": "Running BBR version is unknown; the kernel does not expose a recognized sysfs version.",
        "notice.ssh": "Keep the current session open. Verify the firewall and a new external SSH connection; local service checks do not prove external reachability.",
        "notice.kernel_installed": "Kernel packages and boot checks completed: {detail}. Reboot manually, then verify the running kernel and networking.",
        "notice.running_kernel": "Running kernel: {detail}",
        "notice.terminal": "Prompt configuration saved. Open a new shell session. The login shell and installed software are unchanged.",
        "notice.download_speed": "Download: {detail}",
        "notice.upload_speed": "Upload: {detail}",
        "notice.connectivity": "Connectivity: {detail}",
        "notice.ip_priority": "The displayed gai.conf precedence entries are policy hints, not proof of application connection order.",
        "notice.io": "Disk write + sync: {detail}",
        "notice.io_average": "Average disk write throughput: {detail}",
        "notice.preserve_kernels": "Cleanup preserves all kernels. Remove a specific third-party kernel from kernel management.",
        "notice.uninstalled": "Command removed; recovery material and installed software are preserved: {detail}",
        "notice.progress": "Running {detail}",
        "notice.success": "Completed: {detail}",
        "info.host": "Host",
        "info.kernel": "Kernel",
        "info.arch": "Architecture",
        "info.cpu": "CPU",
        "info.memory": "Memory / swap (MiB)",
        "info.disk": "Filesystem",
        "info.uptime": "Uptime",
        "info.timezone": "Timezone",
        "error.io": "I/O failed: {detail}",
        "error.data": "Invalid or unreadable data: {detail}",
        "error.config": "Cannot load configuration: {detail}",
        "error.locale": "Invalid language catalog, locale, keys or placeholders: {detail}",
        "error.recovery": "Recovery failed. Preserve the recovery material and resolve the reported errors: {detail}",
        "error.path": "Unsafe or unsupported filesystem path: {detail}",
        "error.lock": "Cannot acquire the operation lock; another process may hold it: {detail}",
        "error.pending": "An earlier operation is incomplete. Inspect its recovery record before proceeding: {detail}",
        "error.cancelled": "Operation cancelled or interrupted. {detail}",
        "error.confirm": "Explicit confirmation is required. Use an interactive terminal or specify {detail} after reviewing the operation.",
        "error.root": "This action requires root privileges: {detail}",
        "error.platform": "System operations support Debian / Ubuntu Linux only: {detail}",
        "error.dependency": "Missing required command or package: {detail}",
        "error.command": "External command failed: {detail}",
        "error.tty": "This action requires exclusive control of an interactive terminal: {detail}",
        "error.url": "Unsupported or invalid URL: {detail}",
        "error.memory": "Cannot determine a safe effective memory limit: {detail}",
        "error.input": "Invalid parameter: {detail}",
        "error.verify": "State verification failed: {detail}",
        "error.snapshot": "Recovery baseline is missing, incomplete or inconsistent: {detail}",
        "error.bbr": "The running kernel does not provide BBR after loading the module: {detail}",
        "error.ssh_service": "SSH must be running as ssh.service without active socket activation: {detail}",
        "error.public_key": "Select a file containing exactly one supported SSH PUBLIC key: {detail}",
        "error.release": "Release metadata or packages failed validation: {detail}",
        "error.kernel_host": "Cannot verify an independent, bootable GRUB host; kernel changes refused: {detail}",
        "error.kernel_system": "Third-party kernels require Debian 12+ or Ubuntu 24.04+: {detail}",
        "error.secure_boot": "Cannot confirm Secure Boot is disabled: {detail}",
        "error.kernel_audit": "Resolve incomplete packages or incompatible DKMS modules first: {detail}",
        "error.kernel_driver": "Required kernel/initramfs support is missing: {detail}",
        "error.running_kernel": "Refusing to replace or remove the running kernel: {detail}",
        "error.digest": "SHA-256 checksum mismatch: {detail}",
        "error.kernel_partial": "Kernel operation did not complete; package or boot changes may be partial. Backup and error: {detail}",
        "error.kernel_plan": "Removal would affect additional packages: {detail}",
        "error.terminal_marker": "Malformed managed prompt block: {detail}",
        "error.swap_exists": "A swap file or fstab entry already exists. Use the explicitly confirmed auto/rebuild action: {detail}",
        "error.required": "Required parameter is empty: {detail}",
        "error.action": "Unknown action ID: {detail}",
        "error.batch": "One or more actions failed: {detail}",
        "error.cli": "Invalid command line: {detail}",
        "group.system": "System",
        "group.network": "Network",
        "group.kernel": "Kernel",
        "group.ssh": "SSH",
        "group.docker": "Docker",
        "group.tools": "Tools",
        "group.terminal": "Prompt",
        "group.tests": "Tests",
        "group.app": "CqShell",
        "field.scope": "Cleanup scope",
        "field.timezone": "Timezone",
        "field.size_mb": "Size (MiB)",
        "field.port": "SSH port",
        "field.public_key": "Public key file (absolute path)",
        "field.qdisc": "Default queue",
        "field.bandwidth": "Bandwidth (Mbit/s)",
        "field.rtt": "RTT (ms)",
        "field.profile": "Kernel variant",
        "field.version": "Version x.y.z (empty: latest matching release)",
        "field.release": "Installed kernel release",
        "field.package": "Package",
        "field.shell": "Shell",
        "field.style": "Prompt style",
        "field.color": "Color",
        "field.time": "Show time",
        "field.git": "Show Git",
        "field.source": "Install source",
        "field.mirror": "Registry mirror URL",
        "field.mirror_site": "Mirror (auto = benchmark; tuna = Tsinghua; ustc = USTC)",
        "action.system.mirror.test": "Benchmark system mirrors",
        "description.system.mirror.test": "Download a 1 MiB repository index sample from each of four HTTPS mirrors on this server, at most 8 seconds per mirror. Rank measured transfer speed, including connection time; report failed samples. Does not change sources. Results are a short sample, not a sustained bandwidth guarantee.",
        "action.system.mirror.apply": "Change system mirror · LinuxMirrors",
        "description.system.mirror.apply": "Use pinned, SHA-256 verified LinuxMirrors Lite. Choose official, Tsinghua, USTC, Aliyun, or auto to benchmark then confirm the fastest successful sample. Rebuild standard APT sources with upstream component/source-package defaults; keep separate third-party files. Use official security repositories. Back up and restore configuration on failure; never upgrade packages.",
        "action.system.mirror.restore": "Restore previous system sources",
        "description.system.mirror.restore": "Recover an interrupted mirror operation, or undo the last successful change. Refuse to overwrite later manual edits. Restore configuration before refreshing APT; an index refresh failure keeps the restored configuration.",
        "confirm.mirror": "Rebuild the listed system sources using LinuxMirrors defaults (including components and disabled source-package entries). Separate third-party files stay in place. Keep security updates on official HTTPS repositories. CqShell will back up and restore on failure; packages will not be upgraded.\n{detail}",
        "notice.mirror_probe": "Mirror download test: {detail}. Four samples, up to 1 MiB and 8 seconds each; connection time is included.",
        "notice.mirror_failed": "Mirror sample failed (excluded from ranking): {detail}",
        "notice.mirror_best": "Fastest successful sample: {detail}",
        "notice.mirror_applied": "System sources changed and APT verification passed: {detail}",
        "notice.mirror_rolled_back": "Operation failed; previous source configuration restored: {detail}",
        "notice.mirror_restored": "Source configuration restored; now refreshing APT indexes: {detail}",
        "error.mirror_host": "Mirror integration supports Debian 12/13 and Ubuntu 22.04/24.04/26.04 on amd64/arm64. Derivatives and other releases require separate validation: {detail}",
        "error.mirror_sample": "Invalid download sample: expected a complete 1 MiB HTTPS range from Packages.xz with valid timing: {detail}",
        "error.mirror_speed": "All mirror samples failed. No mirror was selected and no sources were changed. Check network/TLS access or explicitly select a mirror: {detail}",
        "error.mirror_config": "Unsupported or custom content in a system source file. Keep third-party sources in separate sources.list.d files; custom options, aliases and layouts need manual handling: {detail}",
        "error.mirror_changed": "Source files changed since the snapshot; refusing to overwrite them: {detail}",
        "field.enabled": "IPv6",
        "field.purge_data": "Delete default Docker data/config directories",
        "field.directory": "Benchmark directory (absolute path)",
        "action.system.info": "System overview",
        "description.system.info": "Read host, CPU, memory, disk, IP and BBR status.",
        "action.system.update": "Update packages",
        "description.system.update": "Update apt metadata and upgrade packages under the shared kernel lock. Use noninteractive mode and preserve locally edited configuration. List services needing restart for later maintenance.",
        "action.system.clean": "Clean system",
        "description.system.clean": "Standard: apt cache and journals older than 7 days. Full also deletes logs, temporary files, user caches and unused Docker data. All kernels are retained.",
        "action.system.timezone": "Set timezone",
        "description.system.timezone": "Set the system timezone using timedatectl.",
        "action.swap.create": "Create swap",
        "description.swap.create": "Create /swapfile and register it in fstab; refuse an existing file.",
        "action.swap.auto": "Rebuild swap automatically",
        "description.swap.auto": "Compute swap size from physical memory and rebuild /swapfile. Preserve the previous file until the operation succeeds.",
        "action.swap.remove": "Remove swap",
        "description.swap.remove": "Disable and delete only /swapfile and its exact fstab entry.",
        "action.system.fail2ban": "Install fail2ban",
        "description.system.fail2ban": "Install fail2ban and a managed sshd jail using the actual SSH ports and systemd journal.",
        "action.ssh.port": "Change SSH port",
        "description.ssh.port": "Validate the candidate and actual listener; roll back on failure. External firewall reachability must be checked separately.",
        "action.ssh.key": "Configure key-only SSH login",
        "description.ssh.key": "Import a public key for the current HOME, test a new connection, then disable global password and keyboard-interactive authentication. Never print private keys.",
        "action.bbr.status": "BBR and queue status",
        "description.bbr.status": "Inspect the running implementation, congestion control and real interface queues.",
        "action.bbr.enable": "Enable BBR in the running kernel",
        "description.bbr.enable": "Enable the BBR implementation supplied by the running kernel, including BBRv3 when provided. Set the default queue and preserve the original settings for restoration.",
        "action.bbr.restore": "Restore BBR settings",
        "description.bbr.restore": "Restore the recorded runtime values and original persistent configuration.",
        "action.tcp.tune": "Tune TCP buffers",
        "description.tcp.tune": "Calculate a buffer ceiling from bandwidth, RTT and cgroup memory limits; apply, verify and persist with recovery.",
        "action.tcp.restore": "Restore TCP settings",
        "description.tcp.restore": "Restore the first recorded TCP runtime values and configuration.",
        "action.kernel.list": "List kernels",
        "description.kernel.list": "Show installed kernel packages and the currently running release.",
        "action.kernel.install": "Install BBRv3 kernel",
        "description.kernel.install": "Install a Standard or Max BBRv3 kernel from byJoey/Actions-bbr-v3, using the latest matching release or a specified version. Verify packages, back up GRUB and update boot configuration. After booting the new kernel, configure BBR and its queue under Network.",
        "action.kernel.remove": "Remove a non-running BBRv3 kernel",
        "description.kernel.remove": "Simulate removal and refuse extra package removals; preserve the running kernel and boot backup.",
        "action.tools.install": "Install all / choose a package",
        "description.tools.install": "Install a selected package; all also installs zsh and Starship without enabling prompt customization.",
        "action.terminal.install": "Install zsh / Starship",
        "description.terminal.install": "Install zsh and a checksum-verified Starship release. Keep the login shell unchanged.",
        "action.terminal.apply": "Configure prompt",
        "description.terminal.apply": "Update only managed shell initialization blocks and a dedicated Starship TOML file. Preserve a first-use baseline.",
        "action.terminal.status": "Prompt status",
        "description.terminal.status": "Read saved prompt options and recovery baseline status for the current HOME.",
        "action.terminal.restore": "Restore original prompt files",
        "description.terminal.restore": "Restore files byte-for-byte from the first-use baseline. This overwrites later manual edits to managed files.",
        "action.docker.install": "Install Docker",
        "description.docker.install": "Run the explicitly selected third-party installer; keep existing daemon configuration.",
        "action.docker.status": "Docker status",
        "description.docker.status": "Show versions, images, containers, volumes, networks and registry mirrors.",
        "action.docker.clean": "Prune Docker data",
        "description.docker.clean": "Irreversibly remove unused images, stopped containers, networks and eligible volumes.",
        "action.docker.mirror": "Set registry mirror",
        "description.docker.mirror": "Merge the mirror into daemon.json, validate and restart Docker; restore configuration and service state on failure.",
        "action.docker.ipv6": "Configure Docker IPv6",
        "description.docker.ipv6": "Merge IPv6 settings and restart Docker. A generated ULA subnet does not provide public IPv6 routing.",
        "action.docker.uninstall": "Uninstall Docker",
        "description.docker.uninstall": "Remove installed Docker packages. Data is retained unless purge_data=on explicitly deletes the default data/config directories.",
        "action.test.speed": "Speedtest",
        "description.test.speed": "Run Ookla Speedtest; confirm its terms. Installation requires root if the binary is missing.",
        "action.test.ip": "IP quality · xykt/IPQuality",
        "description.test.ip": "Use xykt/IPQuality for IP type, risk scores/factors, streaming/AI access and mail checks. Choose dual-stack/IPv4/IPv6; language follows the UI. Online report upload is disabled; full IP display is off by default. The script takes over the terminal and may ask to install missing dependencies.",
        "action.test.route": "Return-route test",
        "description.test.route": "Install NextTrace with its upstream script, then run a fast TCP trace.",
        "action.test.performance": "YABS benchmark",
        "description.test.performance": "Run the third-party YABS script interactively; may consume CPU, disk and network resources.",
        "action.test.connectivity": "IPv4 / IPv6 connectivity",
        "description.test.connectivity": "Probe both address families over HTTPS and show explicit gai.conf precedence entries.",
        "action.test.io": "Disk I/O benchmark",
        "description.test.io": "Write and sync a 1 GiB temporary file three times in the selected directory; remove it afterward.",
        "action.app.install": "Install cqs command",
        "description.app.install": "Atomically install this executable at CQS_INSTALL_PATH (default /usr/local/bin/cqs).",
        "action.app.uninstall": "Uninstall cqs command",
        "description.app.uninstall": "Remove only the installed command. Keep software, configuration and recovery material."
      }
    },
    "zh-CN": {
      "locale": "zh-CN",
      "name": "简体中文",
      "messages": {
        "notice.package_audit": "软件包操作未完成。请待 apt/dpkg 退出后，在独立终端检查：{detail}；如有未配置的软件包，再完成配置。不要删除锁文件。",
        "action.tools.curl": "curl 下载工具",
        "description.tools.curl": "通过 apt 安装 curl，用于下载文件与发送网络请求。",
        "action.tools.wget": "wget 下载工具",
        "description.tools.wget": "通过 apt 安装 wget，用于文件下载。",
        "action.tools.nano": "nano 文本编辑器",
        "description.tools.nano": "通过 apt 安装 nano 终端文本编辑器。",
        "action.tools.unzip": "unzip 解压缩工具",
        "description.tools.unzip": "通过 apt 安装 unzip，用于解压 ZIP 文件。",
        "action.tools.tar": "tar 压缩工具",
        "description.tools.tar": "通过 apt 安装 tar，用于打包与解包文件。",
        "action.tools.tmux": "tmux 终端复用",
        "description.tools.tmux": "通过 apt 安装 tmux，用于保留会话与管理多个终端。",
        "action.tools.iftop": "iftop 网络监控",
        "description.tools.iftop": "通过 apt 安装 iftop，用于查看实时网络流量。",
        "action.tools.btop": "btop 系统监控",
        "description.tools.btop": "通过 apt 安装 btop，用于查看 CPU、内存与进程。",
        "action.tools.gdu": "gdu 磁盘分析",
        "description.tools.gdu": "通过 apt 安装 gdu，用于分析目录的磁盘占用。",
        "action.tools.fzf": "fzf 模糊搜索",
        "description.tools.fzf": "通过 apt 安装 fzf 交互式模糊搜索工具。",
        "group.ai": "AI 工具",
        "action.ai.pi.install": "安装 / 更新 Pi · pi.dev",
        "description.ai.pi.install": "直接下载 Pi 官方 Linux 独立二进制包，校验 SHA-256 并保留主题、WASM 等配套资源。无需 Node.js/npm；验证版本后更新当前用户的命令入口。",
        "action.ai.claude.install": "安装 / 更新 Claude Code",
        "description.ai.claude.install": "直接下载 Claude Code 官方 Linux 二进制，按官方 manifest 校验 SHA-256。无需 Node.js/npm；验证版本后更新 ~/.local/bin/claude，保留用户配置与旧版本。",
        "action.ai.codex.install": "安装 / 更新 Codex CLI",
        "description.ai.codex.install": "直接下载 Codex 官方完整 Linux 二进制包，包含运行所需的辅助程序与资源，校验 SHA-256 和版本。无需 Node.js/npm；默认入口为 ~/.local/bin/codex，遵循 CODEX_INSTALL_DIR。",
        "action.ai.status": "查看 AI 工具版本",
        "description.ai.status": "检查 Pi、Claude Code、Codex 的常见用户安装目录及 PATH，读取 --version。缺失与执行失败分别显示；不执行登录。",
        "confirm.ai_install": "安装以下官方二进制版本？校验 SHA-256、架构及版本后切换命令入口；已有入口会备份，旧版本文件与登录配置保留。更新后请重新启动工具。\n{detail}",
        "notice.ai_version": "已验证命令：{detail}",
        "notice.ai_missing": "未找到命令：{detail}",
        "notice.ai_ready": "二进制已安装并验证：{detail}。可用此完整路径启动；也可将其目录加入 PATH。后续更新使用同一安装入口，首次使用请自行登录。",
        "error.ai_arch": "AI 二进制安装仅支持 Linux x86_64 / ARM64：{detail}",
        "notice.ai_backup": "原命令入口已备份（previous 文件）：{detail}",
        "error.ai_sudo": "请退出 sudo，以实际使用 AI 工具的用户运行 cqs 后安装，避免写入 root 的 HOME。直接以 root 登录的服务器仍可为 root 安装：{detail}",
        "field.ip_family": "检测协议（dual 双栈 / ipv4 / ipv6）",
        "field.full_ip": "报告中显示完整 IP 地址",
        "confirm.ip_quality": "运行 xykt/IPQuality？它会访问外部风险数据库、流媒体/AI 服务并检测邮件连通性。隐私模式禁用在线报告上传，外部服务仍会收到检测请求。缺少依赖时，上游可能询问是否安装。\n{detail}",
        "error.busy": "已有任务正在执行。{detail}",
        "ui.cancelled": "已取消",
        "ui.unavailable": "此操作当前不可用",
        "ui.preview_banner": "当前为界面预览 · 本机指标实时采集；Linux 系统操作不可用",
        "ui.root_banner": "本机服务器 · 管理员模式 · 执行前请核对变更",
        "ui.user_banner": "普通用户模式 · 系统修改请使用 sudo cqs",
        "ui.optimization_title": "一键优化 · 已选：{count}",
        "ui.optimization_hint": "空格：勾选  Enter：配置  F5：预览/执行  Tab：焦点  Esc：返回",
        "ui.status_hint": "R：刷新  N：公网查询  G：日志  F2：完整日志  L：语言  O：优化",
        "ui.optimization_details": "仅勾选需要的项目；Enter 配置单项，F5 核对整份清单后执行。",
        "ui.action_details": "Enter 打开当前操作，Esc 返回功能分类。",
        "ui.not_set": "尚未填写",
        "ui.form_batch_title": "配置此项目并加入优化清单，保存后不会立即执行。",
        "ui.form_single_title": "填写参数后选择“继续”，核对后执行。",
        "ui.save_selection": "保存到清单",
        "ui.continue": "继续",
        "ui.cancel": "取消",
        "ui.confirm_execute": "确认执行",
        "ui.yes_no_hint": "默认取消。←→/Tab 切换，Enter 确认；Y 执行，N 或 Esc 取消。",
        "ui.batch_title": "一键优化（{count} 项）",
        "ui.empty_log": "尚未执行任务，按 Esc 或 F2 返回。",
        "ui.overview_hint": "R：刷新本机  N：查询公网 IP / ISP  PgUp/PgDn：滚动  Esc：返回",
        "ui.back_hint": "Enter / Esc：返回",
        "ui.notice": "提示",
        "dash.title": "本机常驻状态",
        "dash.age": "本机 {age} 前更新",
        "dash.refresh_hint": "本机指标每 3 秒刷新；公网 IP / ISP 按 N 查询。",
        "dash.public_age": "公网查询距今",
        "dash.loading": "采集中…",
        "dash.not_queried": "按 N 查询",
        "dash.na": "不适用",
        "dash.unknown": "未知",
        "dash.available": "可用",
        "dash.unavailable": "未提供",
        "dash.configured": "已配置（文件）",
        "dash.not_configured": "未配置",
        "dash.host": "主机",
        "dash.os": "系统",
        "dash.kernel": "内核",
        "dash.arch": "架构 / 运行",
        "dash.cpu": "CPU",
        "dash.cpu_usage": "CPU 占用",
        "dash.memory": "内存",
        "dash.disk": "磁盘",
        "dash.swap": "交换",
        "dash.timezone": "时区",
        "dash.ipv4": "IPv4",
        "dash.ipv6": "IPv6",
        "dash.isp": "ISP",
        "dash.network": "拥塞 / 队列",
        "dash.qdisc": "队列",
        "dash.tcp": "TCP 调优",
        "dash.bbr": "BBR / 模块",
        "dash.diagnostics": "未获取的指标 / 查询错误",
        "action.system.public_ip": "查询公网 IP / ISP",
        "description.system.public_ip": "通过 HTTPS 查询 IPv4、IPv6 和 ISP，仅在选中此操作时连接 ip.sb 和 ipinfo.io。",
        "notice.cpu_usage": "CPU 使用率：{detail}",
        "notice.tcp_config": "受管理的 TCP 配置：{detail}",
        "notice.public_ip": "公网信息：{detail}",
        "group.optimization": "一键优化",
        "app.description": "Debian / Ubuntu 服务器工具箱",
        "ui.categories": "功能分类",
        "ui.actions": "操作",
        "ui.details": "说明",
        "ui.logs": "执行日志",
        "ui.ready": "就绪",
        "ui.running": "执行中 — 取消会等待清理完成",
        "ui.cancelling": "正在取消 — 等待子进程与恢复操作安全结束",
        "ui.done": "已完成",
        "ui.failed": "失败",
        "ui.hint": "Tab：焦点  ↑↓：移动  Enter：打开  Esc：返回  Q：退出",
        "ui.busy_hint": "Esc：取消任务；Q / Ctrl+C：取消并在清理完成后退出",
        "ui.form_hint": "Tab/↑↓：字段  ←→：选项/光标  Enter：下一项  Ctrl+Enter：提交  Esc：取消",
        "ui.confirm_hint": "PgUp/PgDn：翻阅说明  Enter：核对确认文字  Esc：取消",
        "ui.type_confirm": "准确输入 {token} 后按 Enter 确认：",
        "ui.confirm": "确认操作",
        "ui.small": "终端过小。请调整到至少 80 × 24，或使用 CLI。",
        "ui.external": "交互程序将接管终端，结束后返回此界面。",
        "ui.return": "按 Enter 返回 TUI。",
        "ui.no_selection": "请先用空格勾选操作。",
        "ui.readonly": "只读",
        "ui.mutating": "会修改系统或用户文件",
        "ui.root": "需要 root",
        "ui.user": "当前用户",
        "status.enabled": "已启用",
        "status.disabled": "未启用",
        "cli.lang": "语言（zh-CN / en-US，也支持外部 TOML 语言包）",
        "cli.config": "偏好设置 TOML 文件",
        "cli.locale_dir": "完整 TOML 语言文件所在目录",
        "cli.yes": "自动确认操作提示",
        "cli.json": "为 list/languages 输出结构化 JSON",
        "cli.list": "列出操作及参数",
        "cli.run": "执行单个操作",
        "cli.batch": "按指定顺序执行多个操作",
        "cli.languages": "验证并列出所有已加载的 TOML 语言包",
        "cli.action": "固定操作 ID（见 list）",
        "cli.set": "参数 key=value；batch 使用 action.id.key=value",
        "cli.install": "将当前二进制安装为 cqs 命令",
        "cli.uninstall": "卸载 cqs 命令，保留恢复文件",
        "cli.help": "显示帮助",
        "cli.version": "显示版本",
        "cli.usage": "用法",
        "cli.options": "选项",
        "cli.commands": "命令",
        "confirm.action": "执行前请核对本次操作：\n{detail}",
        "confirm.batch": "按顺序执行以下清单？具体操作可能仍需进一步确认。\n{detail}",
        "confirm.remote": "将下载并以当前用户权限执行此第三方 Bash 脚本。HTTPS 不代表独立签名认证：\n{detail}",
        "confirm.kernel_download": "下载并校验所选 BBRv3 内核？\n{detail}",
        "confirm.kernel_install": "软件包检查已通过。安装此内核并更新 GRUB？\n{detail}",
        "confirm.kernel_remove": "卸载此未运行的内核并更新 GRUB？\n{detail}",
        "confirm.key_tested": "公钥已安装。请用该密钥建立新的 SSH 连接并确认可以登录，然后再确认。本操作会关闭全局密码及键盘交互认证。请保留当前会话。\n{detail}",
        "confirm.clean_full": "不可逆清理：清空日志、删除过期临时文件和用户缓存，并清理未使用的 Docker 数据：\n{detail}",
        "confirm.speedtest": "接受 Ookla Speedtest 许可/GDPR 条款并进行带宽测试（可能消耗较多流量）？{detail}",
        "notice.saved": "配置已保存：{detail}",
        "notice.restored": "已恢复原始状态：{detail}",
        "notice.backup": "恢复材料：{detail}",
        "notice.qdisc": "默认队列已设为 {detail}。现有连接和网卡队列可能仍保留原设置。",
        "notice.buffer": "TCP 缓冲区上限 / 有效内存：{detail}。并发连接共同消耗内存，请实测目标线路。",
        "notice.bbr_version": "运行中的 BBR 实现版本：{detail}",
        "notice.bbr_unknown": "运行中的 BBR 版本未知；内核未暴露可识别的 sysfs 版本。",
        "notice.ssh": "请保留当前会话，检查防火墙并验证新的外部 SSH 连接；本地服务检查不代表外部可达。",
        "notice.kernel_installed": "内核软件包安装及引导检查已完成：{detail}。请自行重启，然后验证运行内核和网络。",
        "notice.running_kernel": "运行内核：{detail}",
        "notice.terminal": "提示符配置已保存，请打开新的 Shell 会话。登录 Shell 和已安装的软件保持不变。",
        "notice.download_speed": "下载：{detail}",
        "notice.upload_speed": "上传：{detail}",
        "notice.connectivity": "连通性：{detail}",
        "notice.ip_priority": "所显示的 gai.conf precedence 条目是策略信息，不能证明应用程序实际连接顺序。",
        "notice.io": "磁盘写入并同步：{detail}",
        "notice.io_average": "平均磁盘写入吞吐量：{detail}",
        "notice.preserve_kernels": "清理会保留全部内核。请通过内核管理卸载指定第三方内核。",
        "notice.uninstalled": "命令已卸载；恢复材料和已安装的软件仍保留：{detail}",
        "notice.progress": "正在执行 {detail}",
        "notice.success": "已完成：{detail}",
        "info.host": "主机",
        "info.kernel": "内核",
        "info.arch": "架构",
        "info.cpu": "CPU",
        "info.memory": "内存 / SWAP（MiB）",
        "info.disk": "文件系统",
        "info.uptime": "运行时间",
        "info.timezone": "时区",
        "error.io": "输入输出失败：{detail}",
        "error.data": "数据无效或无法读取：{detail}",
        "error.config": "无法加载配置：{detail}",
        "error.locale": "语言包、语言标识、键或占位符无效：{detail}",
        "error.recovery": "恢复失败。请保留恢复材料并处理以下错误：{detail}",
        "error.path": "文件路径不安全或不受支持：{detail}",
        "error.lock": "无法取得操作锁，其他进程可能正在持有：{detail}",
        "error.pending": "上次操作尚未完成，请先检查恢复记录：{detail}",
        "error.cancelled": "操作已取消或中断。{detail}",
        "error.confirm": "此操作需要明确确认。请使用交互终端，或核对操作后指定 {detail}。",
        "error.root": "此操作需要 root 权限：{detail}",
        "error.platform": "系统操作仅支持 Debian / Ubuntu Linux：{detail}",
        "error.dependency": "缺少必需命令或软件包：{detail}",
        "error.command": "外部命令失败：{detail}",
        "error.tty": "此操作需要独占交互终端：{detail}",
        "error.url": "URL 无效或不受支持：{detail}",
        "error.memory": "无法确定安全的有效内存上限：{detail}",
        "error.input": "参数无效：{detail}",
        "error.verify": "状态验证失败：{detail}",
        "error.snapshot": "恢复基线缺失、不完整或不一致：{detail}",
        "error.bbr": "加载模块后运行内核仍未提供 BBR：{detail}",
        "error.ssh_service": "SSH 必须通过 ssh.service 运行，且不能由活动的 socket 接管：{detail}",
        "error.public_key": "请选择仅包含一条受支持 SSH 公钥的文件：{detail}",
        "error.release": "发布元数据或软件包校验失败：{detail}",
        "error.kernel_host": "无法确认独立且可通过 GRUB 启动的主机环境，拒绝更改内核：{detail}",
        "error.kernel_system": "第三方内核需要 Debian 12+ 或 Ubuntu 24.04+：{detail}",
        "error.secure_boot": "无法确认 Secure Boot 已关闭：{detail}",
        "error.kernel_audit": "请先处理未完整配置的软件包或不兼容的 DKMS 模块：{detail}",
        "error.kernel_driver": "内核或 initramfs 缺少必需支持：{detail}",
        "error.running_kernel": "拒绝替换或卸载当前运行内核：{detail}",
        "error.digest": "SHA-256 摘要不匹配：{detail}",
        "error.kernel_partial": "内核操作未完成，软件包或引导可能只完成部分变更。备份及错误：{detail}",
        "error.kernel_plan": "卸载计划会影响额外软件包：{detail}",
        "error.terminal_marker": "受管理的提示符区块格式错误：{detail}",
        "error.swap_exists": "SWAP 文件或 fstab 条目已存在，请使用明确确认的自动重建操作：{detail}",
        "error.required": "必填参数为空：{detail}",
        "error.action": "未知操作 ID：{detail}",
        "error.batch": "部分操作失败：{detail}",
        "error.cli": "命令行参数无效：{detail}",
        "group.system": "系统管理",
        "group.network": "网络调优",
        "group.kernel": "内核管理",
        "group.ssh": "SSH",
        "group.docker": "Docker",
        "group.tools": "常用工具",
        "group.terminal": "终端美化",
        "group.tests": "服务器测试",
        "group.app": "CqShell",
        "field.scope": "清理范围",
        "field.timezone": "时区",
        "field.size_mb": "容量（MiB）",
        "field.port": "SSH 端口",
        "field.public_key": "公钥文件（绝对路径）",
        "field.qdisc": "默认队列",
        "field.bandwidth": "带宽（Mbit/s）",
        "field.rtt": "RTT（毫秒）",
        "field.profile": "内核类型",
        "field.version": "版本 x.y.z（留空：匹配的最新版本）",
        "field.release": "已安装内核的完整版本",
        "field.package": "软件包",
        "field.shell": "Shell",
        "field.style": "提示符样式",
        "field.color": "颜色",
        "field.time": "显示时间",
        "field.git": "显示 Git",
        "field.source": "安装来源",
        "field.mirror": "镜像源 URL",
        "field.mirror_site": "镜像站（auto=测速推荐；tuna=清华；ustc=中科大）",
        "action.system.mirror.test": "系统软件源测速",
        "description.system.mirror.test": "在本机从四个 HTTPS 镜像站各下载 1 MiB 仓库索引样本，每站最多 8 秒。按包含建连时间的实际下载速度排名，并显示失败原因。不修改软件源；短时采样结果不保证持续带宽。",
        "action.system.mirror.apply": "更换系统软件源 · LinuxMirrors",
        "description.system.mirror.apply": "使用固定版本并校验 SHA-256 的 LinuxMirrors Lite。可选官方、清华、中科大、阿里云；auto 测速后确认最快的有效样本。按上游组件与源码仓库默认设置重建标准 APT 主源，保留独立第三方源文件。安全更新使用官方源。修改前备份，失败恢复配置，不升级软件。",
        "action.system.mirror.restore": "恢复上次系统软件源",
        "description.system.mirror.restore": "恢复未完成的换源操作，或撤销最近一次成功换源。检测到后续人工修改时拒绝覆盖。先恢复配置再刷新 APT；索引刷新失败时仍保留已恢复的配置。",
        "confirm.mirror": "将按 LinuxMirrors 默认模板重建以下系统主源（包括组件设置和默认禁用源码仓库）。保留独立第三方源文件，安全更新使用官方 HTTPS 仓库。CqShell 会备份并在失败时恢复，不升级软件。\n{detail}",
        "notice.mirror_probe": "镜像下载测速：{detail}。共四个样本，每站最多 1 MiB、8 秒，速度包含建连时间。",
        "notice.mirror_failed": "镜像测速失败（不参与排名）：{detail}",
        "notice.mirror_best": "最快的有效样本：{detail}",
        "notice.mirror_applied": "系统源已更换且通过 APT 验证：{detail}",
        "notice.mirror_rolled_back": "操作失败，已恢复此前的软件源配置：{detail}",
        "notice.mirror_restored": "软件源配置已恢复，正在刷新 APT 索引：{detail}",
        "error.mirror_host": "换源接入支持 Debian 12/13、Ubuntu 22.04/24.04/26.04 的 amd64/arm64；衍生系统及其他版本需单独验证：{detail}",
        "error.mirror_sample": "下载样本无效：需要完整的 Packages.xz 1 MiB HTTPS 分段下载及有效计时：{detail}",
        "error.mirror_speed": "全部镜像测速失败，未选择镜像也未修改软件源。请检查网络/TLS，或明确指定镜像站：{detail}",
        "error.mirror_config": "系统主源文件包含不支持或自定义配置。第三方源应放在 sources.list.d 的独立文件；自定义选项、版本别名和布局需要手动处理：{detail}",
        "error.mirror_changed": "源文件已在快照之后发生变化，拒绝覆盖：{detail}",
        "field.enabled": "IPv6",
        "field.purge_data": "删除 Docker 默认数据及配置目录",
        "field.directory": "测试目录（绝对路径）",
        "action.system.info": "系统概览",
        "description.system.info": "读取主机、CPU、内存、磁盘、IP 及 BBR 状态。",
        "action.system.update": "更新系统软件包",
        "description.system.update": "在共享内核锁下更新 apt 索引并升级软件包。使用非交互模式，保留本机修改过的配置；列出需要重启的服务，供之后安排维护。",
        "action.system.clean": "清理系统",
        "description.system.clean": "标准清理：apt 缓存及超过 7 天的 journal。完整清理还会删除日志、临时文件、用户缓存和未使用的 Docker 数据。保留全部内核。",
        "action.system.timezone": "设置时区",
        "description.system.timezone": "使用 timedatectl 设置系统时区。",
        "action.swap.create": "创建 SWAP",
        "description.swap.create": "创建 /swapfile 并注册到 fstab；已有文件时拒绝覆盖。",
        "action.swap.auto": "自动重建 SWAP",
        "description.swap.auto": "根据物理内存计算容量并重建 /swapfile，操作成功前保留原文件。",
        "action.swap.remove": "删除 SWAP",
        "description.swap.remove": "仅停用并删除 /swapfile 及其对应 fstab 条目。",
        "action.system.fail2ban": "安装 fail2ban",
        "description.system.fail2ban": "安装 fail2ban，使用实际 SSH 端口和 systemd journal 配置受管理的 sshd jail。",
        "action.ssh.port": "修改 SSH 端口",
        "description.ssh.port": "校验候选配置和实际监听，失败回退；外部防火墙可达性需另行验证。",
        "action.ssh.key": "配置 SSH 密钥登录",
        "description.ssh.key": "向当前 HOME 导入公钥，测试新连接后关闭全局密码和键盘交互认证。不会打印私钥。",
        "action.bbr.status": "查看 BBR 与队列状态",
        "description.bbr.status": "检查运行中的实现版本、拥塞算法和网卡实际队列。",
        "action.bbr.enable": "启用当前内核的 BBR",
        "description.bbr.enable": "启用当前运行内核提供的 BBR；运行内核提供 BBRv3 时即启用 BBRv3。设置默认队列，并保留原始设置供还原。",
        "action.bbr.restore": "撤销 BBR 设置",
        "description.bbr.restore": "恢复记录的运行值和原始持久配置。",
        "action.tcp.tune": "TCP 缓冲区调优",
        "description.tcp.tune": "根据带宽、RTT 和 cgroup 内存限制计算上限；应用、验证并保存，支持恢复。",
        "action.tcp.restore": "撤销 TCP 调优",
        "description.tcp.restore": "恢复首次记录的 TCP 运行值和配置。",
        "action.kernel.list": "查看内核",
        "description.kernel.list": "显示已安装内核软件包和当前运行版本。",
        "action.kernel.install": "安装 BBRv3 内核",
        "description.kernel.install": "从 byJoey/Actions-bbr-v3 安装标准版或 Max 版 BBRv3 内核，支持最新匹配版本和指定版本。校验软件包、备份 GRUB 并更新引导；重启进入新内核后，在网络优化中设置 BBR 和队列。",
        "action.kernel.remove": "卸载未运行的 BBRv3 内核",
        "description.kernel.remove": "模拟卸载并拒绝移除额外软件包，保留运行内核和引导备份。",
        "action.tools.install": "全部安装 / 自选软件",
        "description.tools.install": "安装指定软件包；all 还会安装 zsh 和 Starship，但不启用提示符美化。",
        "action.terminal.install": "安装 zsh / Starship",
        "description.terminal.install": "安装 zsh 和经过摘要校验的 Starship，保持登录 Shell 不变。",
        "action.terminal.apply": "配置终端提示符",
        "description.terminal.apply": "仅更新受管理的 Shell 初始化区块及独立 Starship TOML 文件，保留首次启用前的基线。",
        "action.terminal.status": "查看提示符配置",
        "description.terminal.status": "读取当前 HOME 的提示符选项和恢复基线状态。",
        "action.terminal.restore": "还原原始提示符文件",
        "description.terminal.restore": "从首次基线逐字节恢复文件；会覆盖后来对受管理文件的手工修改。",
        "action.docker.install": "安装 Docker",
        "description.docker.install": "运行明确选择的第三方安装器，保留现有 daemon 配置。",
        "action.docker.status": "查看 Docker 状态",
        "description.docker.status": "显示版本、镜像、容器、卷、网络和镜像源。",
        "action.docker.clean": "清理 Docker 数据",
        "description.docker.clean": "不可逆地删除未使用的镜像、已停止容器、网络和符合清理条件的卷。",
        "action.docker.mirror": "设置 Docker 镜像源",
        "description.docker.mirror": "合并镜像源到 daemon.json，验证并重启 Docker，失败时恢复配置和服务状态。",
        "action.docker.ipv6": "配置 Docker IPv6",
        "description.docker.ipv6": "合并 IPv6 设置并重启 Docker；生成 ULA 子网不代表具备公网 IPv6 路由。",
        "action.docker.uninstall": "卸载 Docker",
        "description.docker.uninstall": "卸载已安装的 Docker 软件包。默认保留数据；purge_data=on 会删除默认数据和配置目录。",
        "action.test.speed": "带宽测速",
        "description.test.speed": "运行 Ookla Speedtest 并确认其条款；缺少程序时安装需要 root。",
        "action.test.ip": "IP 质量体检 · xykt/IPQuality",
        "description.test.ip": "使用 xykt/IPQuality 检测 IP 类型、风险评分/因子、流媒体/AI 解锁和邮件连通性。支持双栈/IPv4/IPv6，语言跟随界面。禁用在线报告上传，默认隐藏部分 IP。脚本会接管终端，缺少依赖时可能询问安装。",
        "action.test.route": "回程路由测试",
        "description.test.route": "使用上游脚本安装 NextTrace，然后执行 TCP 快速回程测试。",
        "action.test.performance": "YABS 性能测试",
        "description.test.performance": "交互运行第三方 YABS 脚本，可能消耗 CPU、磁盘和网络资源。",
        "action.test.connectivity": "IPv4 / IPv6 连通性",
        "description.test.connectivity": "通过 HTTPS 检查两类地址，并显示 gai.conf 中明确配置的 precedence 条目。",
        "action.test.io": "磁盘 I/O 测试",
        "description.test.io": "在指定目录中三次写入并同步 1 GiB 临时文件，结束后删除。",
        "action.app.install": "安装 cqs 命令",
        "description.app.install": "将当前程序原子安装到 CQS_INSTALL_PATH（默认 /usr/local/bin/cqs）。",
        "action.app.uninstall": "卸载 cqs 命令",
        "description.app.uninstall": "仅删除已安装命令，保留软件、配置和恢复材料。"
      }
    }
  }
}
CQS_METADATA_JSON
}
# END BUNDLED CLI DATA

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    cqs_main "$@"
fi

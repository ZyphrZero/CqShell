#!/bin/bash
# 项目信息与持久路径集中定义，所有自有配置、恢复材料和锁使用 cqs 命名。
CQS_NAME="CqShell"
CQS_VERSION="0.0.1"
CQS_INSTALL_PATH="${CQS_INSTALL_PATH:-/usr/local/bin/cqs}"
CQS_STATE_DIR="${CQS_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/cqs}"
CQS_BBR_CONFIG="${CQS_BBR_CONFIG:-/etc/sysctl.d/99-cqs-bbr.conf}"
CQS_TCP_CONFIG="${CQS_TCP_CONFIG:-/etc/sysctl.d/99-cqs-tcp.conf}"
CQS_KERNEL_LOCK="${CQS_KERNEL_LOCK:-/run/lock/cqs-bbrv3.lock}"
CQS_KERNEL_BACKUP="${CQS_KERNEL_BACKUP:-/var/backups/cqs-bbrv3}"

# ─── 颜色定义 ────────────────────────────────────────────────
white='\033[0m'
green='\033[0;32m'
red='\033[31m'
yellow='\033[33m'
pink='\033[38;5;218m'
cyan='\033[96m'

# ─── 公用工具函数 ─────────────────────────────────────────────

# 操作完成后按任意键返回
break_end() {
    echo -e "${green}按任意键返回菜单...${white}"
    read -n 1 -s -r -p "" || return 0
    echo ""
    clear
}

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
    [[ ! -L "$path" ]] || { echo "锁文件不能是符号链接: $path" >&2; return 1; }
    exec {descriptor}>"$path" || return 1
    flock -n "$descriptor" || { echo "$message" >&2; return 1; }
}

# 菜单项格式：编号|标签|固定命令及参数。输入仅匹配编号，不执行 eval。
menu_loop() {
    local title="$1" item key label action choice matched
    local -a command
    shift
    while true; do
        clear
        printf '%s\n' "$title"
        for item in "$@"; do
            IFS='|' read -r key label action <<< "$item"
            printf '%s. %s\n' "$key" "$label"
        done
        echo "0. 返回"
        read -rp "请选择: " choice || return 0
        [[ "$choice" != 0 ]] || return 0
        matched=0
        for item in "$@"; do
            IFS='|' read -r key label action <<< "$item"
            [[ "$choice" == "$key" ]] || continue
            matched=1
            read -ra command <<< "$action"
            "${command[@]}" || echo "操作未完成，请查看上方错误" >&2
            break
        done
        (( matched )) || echo "无效的选择" >&2
        break_end
    done
}

# 通用软件包安装（apt），替代大量重复的 download_xxx 函数
pkg_install() {
    local pkg="$1"
    clear
    require_root || return 1
    apt install "$pkg" -y || return 1
    echo "${pkg} 安装完成"
}

# 下载完成且 Bash 语法检查通过后才执行；HTTPS 不等同于发布者签名验证。
run_https_script() (
    local url="$1" privilege="${2:-user}" script
    [[ "$url" == https://* ]] || { echo "远程脚本必须使用 HTTPS" >&2; return 1; }
    case "$privilege" in user|root) ;; *) return 1 ;; esac
    command -v curl >/dev/null 2>&1 || { echo "下载远程脚本需要 curl" >&2; return 1; }
    script=$(mktemp) || return 1
    trap 'rm -f -- "$script"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    curl --proto '=https' --proto-redir '=https' -fSL --connect-timeout 15 --max-time 300 \
        "$url" -o "$script" || return 1
    [[ -s "$script" ]] || { echo "下载的脚本为空，拒绝执行" >&2; return 1; }
    bash -n "$script" || return 1
    if [[ "$privilege" == root && $EUID -ne 0 ]]; then
        sudo bash "$script"
    else
        bash "$script"
    fi
)

# ─── 启动与安装 ───────────────────────────────────────────────

install_shortcut() (
    local source temporary
    require_root || return 1
    source=$(readlink -f -- "${BASH_SOURCE[0]}") || return 1
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
    trap 'exit 143' TERM
    install -m 755 -- "$source" "$temporary" && mv -fT -- "$temporary" "$CQS_INSTALL_PATH" || return 1
    echo "已安装快捷命令 cqs: $CQS_INSTALL_PATH"
)

uninstall_shortcut() {
    require_root || return 1
    rm -f -- "$CQS_INSTALL_PATH" || return 1
    echo "已卸载 cqs 快捷命令，下载的脚本和恢复快照仍保留"
}

user_agreement() {
    local answer
    [[ ! -f "$CQS_STATE_DIR/agreement.accepted" ]] || return 0
    clear
    echo -e "${pink}欢迎使用 ${CQS_NAME}${white}"
    echo "此脚本基于自用开发"
    echo -e "${red}请尽量通过选择脚本选项退出${white}"
    echo "如有问题，后果自负"
    echo -e "${pink}============================${white}"
    read -rp "是否同意？(y/n): " answer || return 1
    [[ "$answer" =~ ^[Yy]$ ]] || { echo "已拒绝"; return 1; }
    (umask 077; mkdir -p -- "$CQS_STATE_DIR" && printf 'accepted\n' > "$CQS_STATE_DIR/agreement.accepted") || return 1
}

# ═══════════════════════════════════════════════════════════════
# 1. 系统管理
# ═══════════════════════════════════════════════════════════════

# ─── BBR 工具函数 ──────────────────────────────────────────────

check_bbr_status() {
    local congestion_control
    congestion_control=$(sysctl -n net.ipv4.tcp_congestion_control 2>/dev/null) || return 1
    [[ "$congestion_control" == "bbr" ]]
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

sysctl_apply_file() {
    local conf_file="$1"
    if [[ ! -r "$conf_file" ]]; then
        echo -e "${red}配置文件不存在或不可读: ${conf_file}${white}" >&2
        return 1
    fi
    if ! command -v sysctl >/dev/null 2>&1; then
        echo -e "${red}未找到 sysctl，无法应用内核参数${white}" >&2
        return 1
    fi
    sysctl -p "$conf_file"
}

network_key_allowed() {
    case "$1" in
        net.core.default_qdisc|net.ipv4.tcp_congestion_control|net.core.rmem_max|net.core.wmem_max|\
        net.ipv4.tcp_rmem|net.ipv4.tcp_wmem|net.ipv4.tcp_moderate_rcvbuf|net.ipv4.tcp_mtu_probing|\
        net.ipv4.tcp_limit_output_bytes|net.ipv4.tcp_slow_start_after_idle) return 0 ;;
        *) return 1 ;;
    esac
}

# 仅解析本工具生成的 key = value 行；写入仍交由 sysctl 原生解析。
network_verify_config() {
    local file="$1" key separator expected actual count=0
    local -a fields
    while read -r key separator expected || [[ -n "$key" ]]; do
        [[ -z "$key" || "$key" == \#* ]] && continue
        [[ "$separator" == "=" && -n "$expected" ]] && network_key_allowed "$key" || return 1
        count=$((count + 1))
        actual=$(sysctl -n "$key") || return 1
        read -ra fields <<< "$actual"; actual="${fields[*]}"
        read -ra fields <<< "$expected"; expected="${fields[*]}"
        if [[ "$actual" != "$expected" ]]; then
            printf '参数验证失败: %s，期望 %s，实际 %s\n' "$key" "$expected" "$actual" >&2
            return 1
        fi
    done < "$file"
    (( count > 0 ))
}

network_restore_values() {
    local file="$1" key separator value current failed=0
    while read -r key separator value || [[ -n "$key" ]]; do
        [[ -z "$key" || "$key" == \#* ]] && continue
        [[ "$separator" == "=" && -n "$value" ]] && network_key_allowed "$key" || return 1
        current=$(sysctl -n "$key") || { failed=1; continue; }
        if [[ "$current" != "$value" ]]; then
            sysctl -w "$key=$value" || failed=1
        fi
    done < "$file"
    network_verify_config "$file" || failed=1
    if (( failed )); then
        echo "恢复原参数失败；原参数快照: $file" >&2
        return 1
    fi
}

# BBR 与 TCP 分别保存首次操作前的状态，重复调优不覆盖恢复基线。
network_apply_config() (
    local conf_file="$1"; shift
    local backup="${conf_file}.cqs-backup" work key value pair lock_fd
    local changed=0 committed=0 new_backup=0 recovered=1
    local -A seen=()
    [[ $# -gt 0 ]] || return 1
    if [[ -L "$conf_file" || ( -e "$conf_file" && ! -f "$conf_file" ) || -L "$backup" || -L "${conf_file}.cqs-lock" ]]; then
        echo "配置目标不是普通文件，拒绝覆盖: $conf_file" >&2
        return 1
    fi
    acquire_lock lock_fd "${conf_file}.cqs-lock" || return 1
    work=$(mktemp -d "${conf_file}.work.XXXXXX") || return 1
    network_finish_update() {
        if (( changed && ! committed )); then
            if network_restore_values "$work/runtime.conf"; then
                echo "运行参数已恢复为操作前的值"
            else
                recovered=0
                echo "保留恢复材料: $work；请处理错误后恢复参数" >&2
            fi
        fi
        if (( new_backup && ! committed && recovered )); then
            rm -f -- "$backup/runtime.conf" "$backup/original.conf" "$backup/no-original"
            rmdir -- "$backup"
        fi
        if (( recovered )); then rm -rf -- "$work"; fi
    }
    trap network_finish_update EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    printf '# Managed by CqShell\n' > "$work/desired.conf" || return 1
    for pair in "$@"; do
        key=${pair%%=*}; value=${pair#*=}
        if [[ "$pair" != *=* || -z "$value" || "$value" == *$'\n'* || "$value" == *$'\r'* ]] || ! network_key_allowed "$key"; then
            echo "不支持或重复的网络参数: $key" >&2
            return 1
        fi
        [[ -z "${seen[$key]:-}" ]] || { echo "重复的网络参数: $key" >&2; return 1; }
        seen[$key]=1
        printf '%s = %s\n' "$key" "$value" >> "$work/desired.conf" || return 1
        value=$(sysctl -n "$key") || return 1
        printf '%s = %s\n' "$key" "$value" >> "$work/runtime.conf" || return 1
    done
    if [[ -e "$backup" ]]; then
        if [[ ! -s "$backup/runtime.conf" ]] ||
            [[ ! -f "$backup/original.conf" && ! -f "$backup/no-original" ]]; then
            echo "恢复快照不完整，拒绝覆盖: $backup" >&2
            return 1
        fi
        if [[ "$(awk '{print $1}' "$backup/runtime.conf" | sort)" != "$(awk '{print $1}' "$work/runtime.conf" | sort)" ]]; then
            echo "新配置与恢复快照的参数集合不同，请先撤销原配置" >&2
            return 1
        fi
    else
        mkdir -m 700 "$work/baseline" || return 1
        cp -- "$work/runtime.conf" "$work/baseline/runtime.conf" || return 1
        if [[ -f "$conf_file" ]]; then
            cp -p -- "$conf_file" "$work/baseline/original.conf" || return 1
        else
            touch "$work/baseline/no-original" || return 1
        fi
        mv -T -- "$work/baseline" "$backup" || return 1
        new_backup=1
    fi
    chmod 644 "$work/desired.conf" || return 1
    changed=1
    if ! sysctl_apply_file "$work/desired.conf" || ! network_verify_config "$work/desired.conf"; then
        echo "网络参数应用失败，配置未保存" >&2
        return 1
    fi
    if ! mv -fT -- "$work/desired.conf" "$conf_file"; then
        echo "配置保存失败: $conf_file" >&2
        return 1
    fi
    committed=1
)

network_restore_config() (
    local conf_file="$1" backup="${1}.cqs-backup" work key separator value lock_fd recovered=1
    local changed=0 committed=0
    acquire_lock lock_fd "${conf_file}.cqs-lock" || return 1
    if [[ -L "$conf_file" || -L "$backup" || ! -s "$backup/runtime.conf" ]] ||
        [[ ! -f "$backup/original.conf" && ! -f "$backup/no-original" ]]; then
        echo "没有完整的恢复快照，不能推断系统原值: $backup" >&2
        return 1
    fi
    work=$(mktemp -d "${conf_file}.restore.XXXXXX") || return 1
    network_finish_restore() {
        if (( changed && ! committed )); then
            network_restore_values "$work/current.conf" || recovered=0
        fi
        if (( recovered )); then rm -rf -- "$work"; else echo "保留恢复材料: $work" >&2; fi
    }
    trap network_finish_restore EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    while read -r key separator value || [[ -n "$key" ]]; do
        [[ -z "$key" || "$key" == \#* ]] && continue
        network_key_allowed "$key" || return 1
        value=$(sysctl -n "$key") || return 1
        printf '%s = %s\n' "$key" "$value" >> "$work/current.conf" || return 1
    done < "$backup/runtime.conf"
    changed=1
    if ! network_restore_values "$backup/runtime.conf"; then
        echo "撤销失败，保留快照供重试: $backup" >&2
        return 1
    fi
    if [[ -f "$backup/original.conf" ]]; then
        if ! cp -p -- "$backup/original.conf" "$work/original.conf" ||
            ! mv -fT -- "$work/original.conf" "$conf_file"; then
            return 1
        fi
    elif ! rm -f -- "$conf_file"; then
        return 1
    fi
    committed=1
    rm -f -- "$backup/runtime.conf" "$backup/original.conf" "$backup/no-original" || return 1
    rmdir -- "$backup" || return 1
    echo "已恢复首次操作前的运行参数和配置: $conf_file"
)

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

show_bbr_details() {
    local algorithm version disk_version
    algorithm=$(sysctl -n net.ipv4.tcp_congestion_control) || return 1
    version=$(get_bbr_version)
    echo "运行内核: $(uname -r)"
    echo "当前拥塞算法: $algorithm；BBR 实现版本: $version"
    [[ "$version" != 未知 ]] || echo "运行内核未暴露 BBR 版本，不能仅凭 bbr 名称或内核版本号判定 v3"
    if [[ "$version" == v3 && "$algorithm" == bbr ]]; then echo "BBRv3 已启用"; fi
    if command -v modinfo >/dev/null 2>&1; then
        disk_version=$(modinfo -F version tcp_bbr 2>/dev/null)
        [[ -z "$disk_version" ]] || echo "磁盘模块版本: $disk_version（不作为运行版本的证明）"
    fi
    echo "可用拥塞算法: $(sysctl -n net.ipv4.tcp_available_congestion_control)"
    echo "默认队列参数: $(sysctl -n net.core.default_qdisc)"
    if command -v tc >/dev/null 2>&1; then
        echo "当前网卡实际队列:"
        tc qdisc show || return 1
    else
        echo "未安装 iproute2，无法显示当前网卡队列"
    fi
    echo "其他 sysctl 文件可能覆盖持久配置；切换默认队列不会重建现有网卡的队列树"
}

# 使用 2 倍带宽时延积，向上取整到 MiB；上限为有效内存的 1/32，最多 64 MiB。
calculate_tcp_buffer() {
    local bandwidth="$1" rtt="$2" memory_kb="$3"
    [[ "$bandwidth" =~ ^[0-9]{1,6}([.][0-9]{1,3})?$ &&
       "$rtt" =~ ^[0-9]{1,5}([.][0-9]{1,3})?$ && "$memory_kb" =~ ^[0-9]{1,12}$ ]] || return 1
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

# 主菜单系统信息面板（紧凑双栏布局，直接嵌入主界面）
show_sysinfo() {
    local hostname os_info kernel cpu_arch cpu_info cpu_cores cpu_usage
    local mem_used mem_total mem_pct disk_used disk_total disk_pct
    local swap_used swap_total ipv4 ipv6 isp congestion queue runtime timezone
    local bbr_st bbr_mod_st tcp_tune_st

    hostname=$(hostname)
    os_info=$(grep PRETTY_NAME /etc/os-release 2>/dev/null | cut -d= -f2 | tr -d '"')
    kernel=$(uname -r)
    cpu_arch=$(uname -m)
    cpu_info=$(lscpu 2>/dev/null | awk -F': +' '/Model name/{print $2; exit}')
    cpu_cores=$(nproc)
    cpu_usage=$(awk '{u=$2+$4; t=$2+$4+$5; if(NR==1){u1=u;t1=t} \
        else printf "%.0f", (($2+$4-u1)*100/(t-t1))}' \
        <(grep 'cpu ' /proc/stat) <(sleep 0.3; grep 'cpu ' /proc/stat))

    read -r mem_total mem_used  < <(free -m  | awk '/^Mem/{print $2,$3}')
    read -r swap_total swap_used < <(free -m | awk '/^Swap/{print $2,$3}')
    mem_pct=$(( (mem_total > 0) ? mem_used * 100 / mem_total : 0 ))
    read -r disk_used disk_total disk_pct < <(df -h / | awk 'NR==2{print $3,$2,$5}')

    ipv4=$(curl -s --max-time 2 ipv4.ip.sb 2>/dev/null); [ -z "$ipv4" ] && ipv4="-"
    ipv6=$(curl -s --max-time 2 ipv6.ip.sb 2>/dev/null); [ -z "$ipv6" ] && ipv6="-"
    local ipinfo
    ipinfo=$(curl -s --max-time 3 ipinfo.io 2>/dev/null)
    isp=$(echo "$ipinfo" | grep '"org"' | cut -d'"' -f4); [ -z "$isp" ] && isp="-"
    congestion=$(sysctl -n net.ipv4.tcp_congestion_control 2>/dev/null)
    queue=$(sysctl -n net.core.default_qdisc 2>/dev/null)
    runtime=$(awk -F. '{d=int($1/86400);h=int(($1%86400)/3600);m=int(($1%3600)/60); \
        s=""; if(d)s=s d"天 "; if(h)s=s h"时 "; printf s"%d分",m}' /proc/uptime)
    timezone=$(timedatectl 2>/dev/null | awk '/Time zone/{print $3}')

    if check_bbr_status; then
        bbr_st="${green}✅ 已启用${white}"
    else
        bbr_st="${red}❌ 未启用${white}"
    fi
    if check_bbr_available; then
        bbr_mod_st="${green}✅ 可用 / $(get_bbr_version)${white}"
    else
        bbr_mod_st="${red}❌ 不可用${white}"
    fi
    if [[ -f "$CQS_TCP_CONFIG" ]]; then
        tcp_tune_st="${green}✅ 已配置${white}"
    else
        tcp_tune_st="${yellow}⬜ 未配置${white}"
    fi

    local L="${pink}─────────────────────────────────────────────────${white}"
    echo -e "$L"
    printf " ${cyan}主机:${white} %-22s  ${cyan}系统:${white} %s\n"  "$hostname"  "$os_info"
    printf " ${cyan}内核:${white} %-22s  ${cyan}架构:${white} %s\n"  "$kernel"    "$cpu_arch"
    echo -e "$L"
    printf " ${cyan}CPU :${white} %s × %s核   ${cyan}占用:${white} %s%%\n" \
        "$cpu_info" "$cpu_cores" "$cpu_usage"
    printf " ${cyan}内存:${white} %s/%s MB (%s%%)   ${cyan}硬盘:${white} %s/%s (%s)   ${cyan}交换:${white} %s/%s MB\n" \
        "$mem_used" "$mem_total" "$mem_pct" \
        "$disk_used" "$disk_total" "$disk_pct" \
        "$swap_used" "$swap_total"
    echo -e "$L"
    printf " ${cyan}IPv4:${white} %-22s  ${cyan}IPv6:${white} %s\n"  "$ipv4" "$ipv6"
    printf " ${cyan}ISP :${white} %-22s  ${cyan}拥塞控制:${white} %s   ${cyan}队列算法:${white} %s\n" "$isp" "$congestion" "$queue"
    echo -e " ${cyan}TCP调优:${white} ${tcp_tune_st}   ${cyan}BBR状态:${white} ${bbr_st}   ${cyan}BBR模块:${white} ${bbr_mod_st}"
    echo -e "$L"
    printf " ${cyan}运行:${white} %-22s  ${cyan}时区:${white} %s\n"  "$runtime" "$timezone"
    echo -e "$L"
}

# 1.1 更新系统软件包
system_update() (
    clear
    require_root || return 1
    local lock_fd
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" "另一个内核操作正在进行，暂不更新系统" || return 1
    apt update -y && apt upgrade -y || return 1
    clear
    echo "系统软件包更新完毕"
)

# 1.2 系统清理
system_clean() (
    clear
    require_root || return 1
    local lock_fd
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" "另一个内核操作正在进行，暂不清理系统" || return 1
    local start_space end_space cleared_space PKG_MANAGER CLEAN_CMD PKG_UPDATE_CMD INSTALL_CMD PURGE_CMD
    local preserve_kernels=0
    if [[ -d "$CQS_KERNEL_BACKUP" || "$(uname -r)" == *-joeyblog-bbrv3* ]]; then preserve_kernels=1; fi

    start_space=$(df / | tail -n 1 | awk '{print $3}')
    echo -e "${yellow}正在进行系统清理...${white}"

    if command -v apt-get > /dev/null; then
        PKG_MANAGER="apt"
        CLEAN_CMD="apt-get autoremove -y && apt-get clean"
        if (( preserve_kernels )); then CLEAN_CMD="apt-get clean"; fi
        PKG_UPDATE_CMD="apt-get update"
        INSTALL_CMD="apt-get install -y"
        PURGE_CMD="apt-get purge -y"
    elif command -v dnf > /dev/null; then
        PKG_MANAGER="dnf"
        CLEAN_CMD="dnf autoremove -y && dnf clean all"
        PKG_UPDATE_CMD="dnf update"
        INSTALL_CMD="dnf install -y"
        PURGE_CMD="dnf remove -y"
    elif command -v apk > /dev/null; then
        PKG_MANAGER="apk"
        CLEAN_CMD="apk cache clean"
        PKG_UPDATE_CMD="apk update"
        INSTALL_CMD="apk add"
        PURGE_CMD="apk del"
    else
        echo "不支持的包管理器"
        exit 1
    fi

    echo "正在更新依赖..."
    if [ "$PKG_MANAGER" = "apt" ] && [ ! -x /usr/bin/deborphan ]; then
        $PKG_UPDATE_CMD > /dev/null 2>&1
        $INSTALL_CMD deborphan > /dev/null 2>&1
    fi

    echo "正在删除未使用的内核..."
    if (( preserve_kernels )); then
        echo "检测到第三方内核或引导备份，保留所有内核；请使用 BBR 内核管理菜单卸载指定版本"
    elif [[ "$PKG_MANAGER" == "apt" || "$PKG_MANAGER" == "dnf" ]]; then
        local current_kernel kernel_packages
        current_kernel=$(uname -r)
        if [[ "$PKG_MANAGER" == "apt" ]]; then
            kernel_packages=$(dpkg --list | grep -E '^ii  linux-(image|headers)-[0-9]+' | awk '{print $2}' | grep -v "$current_kernel")
        else
            kernel_packages=$(rpm -qa | grep -E '^kernel-(core|modules|devel)-[0-9]+' | grep -v "$current_kernel")
        fi
        if [ -n "$kernel_packages" ]; then
            echo "找到旧内核，正在删除：$kernel_packages"
            $PURGE_CMD $kernel_packages > /dev/null 2>&1
            [[ "$PKG_MANAGER" == "apt" ]] && update-grub > /dev/null 2>&1
        else
            echo "没有旧内核需要删除。"
        fi
    fi

    echo "正在清理系统日志文件..."
    find /var/log -type f -name "*.log" -exec truncate -s 0 {} \; > /dev/null 2>&1
    find /root    -type f -name "*.log" -exec truncate -s 0 {} \; > /dev/null 2>&1
    find /home    -type f -name "*.log" -exec truncate -s 0 {} \; > /dev/null 2>&1
    find /ql      -type f -name "*.log" -exec truncate -s 0 {} \; > /dev/null 2>&1

    echo "正在清理缓存目录..."
    find /tmp     -type f -mtime +1 -exec rm -f {} \;
    find /var/tmp -type f -mtime +1 -exec rm -f {} \;
    for user in /home/* /root; do
        local cache_dir="$user/.cache"
        if [ -d "$cache_dir" ]; then
            rm -rf "${cache_dir:?}"/* > /dev/null 2>&1
        fi
    done

    if command -v docker &> /dev/null; then
        echo "正在清理Docker镜像、容器和卷..."
        docker system prune -a -f --volumes > /dev/null 2>&1
    fi

    if [[ "$PKG_MANAGER" == "apt" ]] && (( ! preserve_kernels )); then
        echo "正在清理孤立包..."
        deborphan --guess-all | xargs -r apt-get -y remove --purge > /dev/null 2>&1
    fi

    echo "正在清理包管理器缓存..."
    eval "$CLEAN_CMD" > /dev/null 2>&1

    end_space=$(df / | tail -n 1 | awk '{print $3}')
    cleared_space=$((start_space - end_space))
    echo "系统清理完成，清理了 $((cleared_space / 1024))M 空间！"
)

# 1.3 TCP 调优
tcp_tune() {
    local auto="${1:-}" choice=1 bandwidth rtt memory buffer confirm_tune
    clear
    require_root || return 1
    if [[ "$auto" != "--auto" ]]; then
        echo "1. 均衡 TCP 配置（随内存限制缓冲区）"
        echo "2. 按目标线路带宽与 RTT 计算缓冲区"
        echo "3. 撤销本工具最近一轮 TCP 调优"
        echo "0. 返回"
        read -rp "请选择 [0-3]: " choice || return 1
    fi
    case "$choice" in
        0) return 0 ;;
        3)
            read -rp "恢复首次调优前的运行值和配置？[y/N]: " confirm_tune || return 1
            [[ "$confirm_tune" =~ ^[Yy]$ ]] || return 0
            network_restore_config "$CQS_TCP_CONFIG"
            return $?
            ;;
        1) bandwidth=1000; rtt=50 ;;
        2)
            read -rp "目标线路带宽（Mbit/s，最大 100000）: " bandwidth || return 1
            read -rp "目标线路 RTT（毫秒；应使用业务对端实测值）: " rtt || return 1
            ;;
        *) echo "无效的选择" >&2; return 1 ;;
    esac
    memory=$(network_memory_kb) || return 1
    buffer=$(calculate_tcp_buffer "$bandwidth" "$rtt" "$memory") || {
        echo "请输入有效的正数带宽与 RTT；可用内存至少需要 64 MiB" >&2
        return 1
    }
    echo "TCP 收发缓冲区上限: $((buffer / 1048576)) MiB；有效内存: $((memory / 1024)) MiB"
    echo "这是每个 socket 的上限，并发连接仍会共同消耗内存；实际收益需在目标线路测试"
    if [[ "$auto" != "--auto" ]]; then
        read -rp "应用并保存 TCP 配置？[y/N]: " confirm_tune || return 1
        [[ "$confirm_tune" =~ ^[Yy]$ ]] || return 0
    fi
    mkdir -p -- "$(dirname -- "$CQS_TCP_CONFIG")" || return 1
    apply_tcp_profile "$CQS_TCP_CONFIG" "$buffer" || return 1
    echo "TCP 配置已应用并保存至 $CQS_TCP_CONFIG；首次操作前的快照已保留，可通过本菜单撤销"
}

# 1.4 启用内核提供的 BBR

enable_bbr() {
    local qdisc="${1:-fq}"
    clear
    valid_bbr_qdisc "$qdisc" || return 1
    if ! require_root; then
        return 1
    fi

    if ! load_bbr_module; then
        echo -e "${red}当前内核未提供 BBR 拥塞控制算法，无法启用${white}"
        echo "可用算法: $(sysctl -n net.ipv4.tcp_available_congestion_control 2>/dev/null || echo 未知)"
        return 1
    fi

    mkdir -p -- "$(dirname -- "$CQS_BBR_CONFIG")" || return 1
    echo -e "${cyan}正在应用并保存 BBR/$qdisc 配置...${white}"
    if ! apply_bbr_config "$CQS_BBR_CONFIG" "$qdisc"; then
        echo -e "${red}BBR 配置未保存，请处理上方错误后重试${white}" >&2
        return 1
    fi
    echo -e "${green}✅ BBR 已启用，运行版本: $(get_bbr_version)，默认队列参数为 $qdisc${white}"
    echo "配置文件: $CQS_BBR_CONFIG（重启时仍可能被其他 sysctl 配置覆盖）"
    echo "现有连接和网卡队列可能保持原设置，实际吞吐量请通过测速验证"
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
        if match and not release.get("draft") and not release.get("prerelease"):
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
    grep -Eq '^CONFIG_TCP_CONG_BBR=[ym]$' "$directory/kernel.config" &&
        grep -Eq '^CONFIG_NET_SCH_FQ=[ym]$' "$directory/kernel.config" || {
        echo "目标包缺少 BBR 或 FQ 支持" >&2
        return 1
    }
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

bbrv3_backup_grub() {
    local backup name
    mkdir -p -- "$CQS_KERNEL_BACKUP" || return 1
    backup=$(mktemp -d "$CQS_KERNEL_BACKUP/boot.XXXXXX") || return 1
    for name in grub.cfg grubenv; do
        if [[ -f "/boot/grub/$name" ]]; then
            cp -p -- "/boot/grub/$name" "$backup/$name" || return 1
        else
            touch "$backup/no-$name" || return 1
        fi
    done
    uname -r > "$backup/running-kernel" || return 1
    printf '%s\n' "$backup"
}

bbrv3_restore_grub() {
    local backup="$1" name
    [[ -s "$backup/grub.cfg" ]] || { echo "GRUB 备份不完整: $backup" >&2; return 1; }
    for name in grub.cfg grubenv; do
        if [[ -f "$backup/$name" ]]; then
            cp -p -- "$backup/$name" "/boot/grub/$name" || return 1
        elif [[ -f "$backup/no-$name" ]]; then
            rm -f -- "/boot/grub/$name" || return 1
        fi
    done
    echo "已恢复操作前的 GRUB 配置；软件包状态仍需修复，暂勿重启"
}

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
    local work name url digest actual kind backup confirm api lock_fd install_started=0 install_complete=0
    bbrv3_host_preflight || return 1
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" "另一个内核操作正在进行，请稍后重试" || return 1
    local ID="" VERSION_ID=""
    [[ -r /etc/os-release ]] || return 1
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
    bbrv3_finish_install() {
        if (( install_started && ! install_complete )); then
            bbrv3_restore_grub "$backup" || echo "GRUB 恢复失败，请立即通过服务器控制台处理" >&2
            echo "安装未完成，可能存在部分安装的软件包，暂勿重启；引导备份: $backup" >&2
        fi
        rm -rf -- "$work"
    }
    trap bbrv3_finish_install EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
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
    read -rp "继续下载？[y/N]: " confirm || return 1
    [[ "$confirm" =~ ^[Yy]$ ]] || return 0
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
    read -rp "输入 INSTALL 确认安装: " confirm || return 1
    [[ "$confirm" == INSTALL ]] || return 0
    backup=$(bbrv3_backup_grub) || return 1
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
    printf '%s\n' "$installed" | awk '$1 ~ /^[ih][iUFHWt]/ {print $1, $2}'
}

bbrv3_removable_kernel() {
    local release="$1" running="$2" installed="$3"
    [[ "$release" =~ ^[0-9]+\.[0-9]+\.[0-9]+-joeyblog-bbrv3(-max)?$ && "$release" != "$running" ]] || return 1
    grep -Fxq "linux-image-$release" <<< "$installed" && grep -Fxq "linux-image-$running" <<< "$installed"
}

bbrv3_remove_kernel() (
    local release installed package status confirm backup plan lock_fd
    bbrv3_host_preflight remove || return 1
    acquire_lock lock_fd "$CQS_KERNEL_LOCK" "另一个内核操作正在进行，请稍后重试" || return 1
    bbrv3_list_kernels || return 1
    read -rp "要卸载的第三方内核完整版本（不含 linux-image-）: " release || return 1
    installed=$(dpkg-query -W -f='${db:Status-Abbrev}\t${Package}\n' 'linux-image-*') || return 1
    installed=$(awk '$1 ~ /^[ih][iUFHWt]/ {print $2}' <<< "$installed")
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
    [[ ! "$status" =~ ^[ih][iUFHWt] ]] || packages+=("$package")
    plan=$(LC_ALL=C apt-get -s purge "${packages[@]}") || return 1
    while read -r status package _; do
        if [[ "$status" == Remv || "$status" == Purg ]]; then
            [[ "$package" == "linux-image-$release" || "$package" == "linux-headers-$release" ]] || {
                echo "卸载计划会影响额外软件包 $package，已停止" >&2; return 1;
            }
        fi
    done <<< "$plan"
    printf '仅卸载:\n%s\n' "${packages[@]}"
    read -rp "输入 REMOVE 确认: " confirm || return 1
    [[ "$confirm" == REMOVE ]] || return 0
    backup=$(bbrv3_backup_grub) || return 1
    if apt-get purge -y "${packages[@]}" && update-grub; then
        echo "指定的非运行内核已卸载；当前内核及网络参数保持不变"
    else
        echo "卸载或更新引导失败；备份: $backup。部分包可能已移除，请在控制台修复引导后再重启" >&2
        return 1
    fi
)

bbrv3_install_version() {
    local version
    read -rp "版本号（x.y.z）: " version || return 1
    bbrv3_install_kernel standard "$version"
}

restore_bbr() {
    local confirm
    require_root || return 1
    read -rp "恢复首次设置 BBR 前的运行值与配置？[y/N]: " confirm || return 1
    [[ "$confirm" =~ ^[Yy]$ ]] || return 0
    network_restore_config "$CQS_BBR_CONFIG"
}

bbrv3_kernel_menu() {
    menu_loop "第三方 BBRv3 内核管理（byJoey/Actions-bbr-v3）" \
        "1|查看已安装与正在运行的内核|bbrv3_list_kernels" \
        "2|安装/更新标准 BBRv3|bbrv3_install_kernel standard" \
        "3|安装指定版本的标准 BBRv3|bbrv3_install_version" \
        "4|安装/更新 Max 内核（实验性吞吐策略）|bbrv3_install_kernel max" \
        "5|卸载未运行的第三方内核|bbrv3_remove_kernel"
}

bbr_manage() {
    menu_loop "BBR 网络管理" \
        "1|查看运行版本、拥塞控制与实际网卡队列|show_bbr_details" \
        "2|启用 BBR + FQ|enable_bbr fq" \
        "3|启用 BBR + FQ_CODEL|enable_bbr fq_codel" \
        "4|启用 BBR + FQ_PIE|enable_bbr fq_pie" \
        "5|启用 BBR + CAKE（默认参数）|enable_bbr cake" \
        "6|TCP 均衡配置 / 带宽与 RTT 调优|tcp_tune" \
        "7|撤销本工具的 BBR/队列配置|restore_bbr" \
        "8|第三方 BBRv3 内核安装与管理|bbrv3_kernel_menu"
}

# 1.5 将时区改为上海
system_time() {
    sudo timedatectl set-timezone Asia/Shanghai
    echo "已成功将时区改为上海"
}

# ── SWAP 操作（三个独立函数，供子菜单和一键优化复用）──────────

# 交互式添加 SWAP
swap_create() {
    echo -e "${green}请输入需要添加的swap，建议为内存的2倍！【单位为MB】${white}"
    read -p "请输入swap数值:" swapsize
    if grep -q "swapfile" /etc/fstab 2>/dev/null; then
        echo -e "${red}swapfile已存在，swap设置失败，请先删除swap后重新设置！${white}"
        return 1
    fi
    echo -e "${green}swapfile未发现，正在为其创建swapfile${white}"
    fallocate -l "${swapsize}M" /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    echo '/swapfile none swap defaults 0 0' >> /etc/fstab
    echo -e "${green}swap创建成功，并查看信息：${white}"
    cat /proc/swaps
    grep Swap /proc/meminfo
}

# 删除 SWAP
swap_delete() {
    if grep -q "swapfile" /etc/fstab 2>/dev/null; then
        echo -e "${green}swapfile已发现，正在将其移除...${white}"
        sed -i '/swapfile/d' /etc/fstab
        echo "3" > /proc/sys/vm/drop_caches
        swapoff -a
        rm -f /swapfile
        echo -e "${green}swap已删除！${white}"
    else
        echo -e "${red}swapfile未发现，swap删除失败！${white}"
    fi
}

# 自动创建 SWAP，供一键优化调用
swap_create_auto() {
    if [[ -d "/proc/vz" ]]; then
        echo -e "${red}Your VPS is based on OpenVZ, not supported!${white}"
        return 1
    fi
    if grep -q "swapfile" /etc/fstab 2>/dev/null; then
        echo -e "${green}发现现有的swapfile，正在删除...${white}"
        sed -i '/swapfile/d' /etc/fstab
        swapoff /swapfile 2>/dev/null
        rm -f /swapfile
        echo -e "${green}现有的swapfile已删除。${white}"
    else
        echo -e "${yellow}未发现现有的swapfile。${white}"
    fi
    local mem_size swap_size mem_gb
    mem_size=$(free -m | awk '/^Mem:/{print $2}')
    mem_gb=$(( mem_size / 1024 ))

    if [[ $mem_gb -le 4 ]]; then
        local double=$(( mem_size * 2 ))
        swap_size=$(( double < 4096 ? double : 4096 ))
    elif [[ $mem_gb -le 8 ]]; then
        swap_size=$mem_size
    elif [[ $mem_gb -le 64 ]]; then
        swap_size=8192
    else
        swap_size=16384
    fi

    echo -e "${green}物理内存: ${mem_size}MB，将创建 ${swap_size}MB 的 swap 文件。${white}"
    fallocate -l "${swap_size}M" /swapfile
    chmod 600 /swapfile
    mkswap /swapfile
    swapon /swapfile
    echo '/swapfile none swap defaults 0 0' >> /etc/fstab
    echo -e "${green}swap创建成功，当前swap信息如下：${white}"
    cat /proc/swaps
    grep Swap /proc/meminfo
}

# 1.6 SWAP 子菜单
system_swap() {
    require_root || return 1
    if [[ -d /proc/vz ]]; then
        echo "OpenVZ 不支持修改 SWAP" >&2
        return 1
    fi
    menu_loop "SWAP 管理" "1|添加 SWAP|swap_create" "2|删除 SWAP|swap_delete"
}

# 1.7 修改SSH端口
ssh_port_is_listening() {
    local port="$1" pid listeners
    pid=$(systemctl show --property=MainPID --value ssh) || return 1
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
    listeners=$(ss -H -ltnp "sport = :$port") || return 1
    [[ "$listeners" == *"pid=$pid,"* ]]
}

# 可传入端口号参数（供一键优化使用），无参数则交互输入
system_ssh() (
    local port="${1:-}" conf_file="${2:-/etc/ssh/sshd_config}" work lock_fd
    local changed=0 committed=0 recovered=1
    require_root || return 1
    if [[ -z "$port" ]]; then
        read -rp "请输入要设置的SSH端口号（按Enter使用默认端口55520）: " port || return 1
        port=${port:-55520}
    fi
    [[ "$port" =~ ^[0-9]{1,5}$ ]] && (( 10#$port >= 1 && 10#$port <= 65535 )) || {
        echo "SSH 端口必须是 1 到 65535 的整数" >&2; return 1;
    }
    port=$((10#$port))
    require_commands sshd systemctl flock ss || return 1
    if systemctl is-active --quiet ssh.socket; then
        echo "SSH 由 ssh.socket 管理，请先配置其监听端口；本次未修改 sshd 配置" >&2
        return 1
    fi
    if ! systemctl is-active --quiet ssh; then
        echo "SSH 服务未运行，拒绝修改端口或启动服务" >&2
        return 1
    fi
    [[ -f "$conf_file" && ! -L "$conf_file" && ! -L "${conf_file}.cqs-lock" ]] || return 1
    acquire_lock lock_fd "${conf_file}.cqs-lock" "另一个进程正在修改 SSH 配置" || return 1
    work=$(mktemp -d "${conf_file}.work.XXXXXX") || return 1
    ssh_finish_update() {
        if (( changed && ! committed )); then
            if cp -p -- "$work/original.conf" "$work/restore.conf" &&
                mv -fT -- "$work/restore.conf" "$conf_file"; then
                systemctl reload-or-restart ssh || recovered=0
            else
                recovered=0
            fi
            echo "SSH 修改失败，已尝试恢复原配置" >&2
        fi
        if (( recovered )); then rm -rf -- "$work"; else echo "SSH 恢复失败，保留备份: $work" >&2; fi
    }
    trap ssh_finish_update EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    cp -p -- "$conf_file" "$work/original.conf" || return 1
    cp -p -- "$conf_file" "$work/desired.conf" || return 1
    # 只替换全局 Port；Match 块和 Include 保留，随后检查实际展开的端口集合。
    awk -v port="$port" 'BEGIN { print "Port " port }
        tolower($1) == "match" { in_match=1 }
        !in_match && tolower($1) == "port" { next }
        { print }' "$conf_file" > "$work/desired.conf" || return 1
    sshd -t -f "$work/desired.conf" || { echo "SSH 配置校验失败，未修改原文件" >&2; return 1; }
    local effective
    effective=$(sshd -T -f "$work/desired.conf") || return 1
    if [[ "$(awk '$1 == "port" {print $2}' <<< "$effective")" != "$port" ]]; then
        echo "Include 等配置提供了额外 SSH 端口，请先处理冲突；未修改原文件" >&2
        return 1
    fi
    if ! awk -v port="$port" '$1 == "listenaddress" && $2 !~ (":" port "$") { bad=1 }
        END { exit bad }' <<< "$effective"; then
        echo "ListenAddress 固定了其他监听端口，请先处理冲突；未修改原文件" >&2
        return 1
    fi
    changed=1
    mv -fT -- "$work/desired.conf" "$conf_file" || return 1
    systemctl reload ssh && systemctl is-active --quiet ssh || return 1
    # reload 可能只投递 SIGHUP；确认本服务的主进程已实际监听新端口。
    local attempt ready=0
    for ((attempt=0; attempt<50; attempt++)); do
        if ssh_port_is_listening "$port"; then ready=1; break; fi
        systemctl is-active --quiet ssh || break
        sleep 0.1
    done
    (( ready )) || { echo "SSH 未监听目标端口，撤销修改" >&2; return 1; }
    committed=1
    echo "SSH端口已修改为 $port"
    echo "请保持当前会话，并确认防火墙放行新端口后测试新连接"
)

# 1.8 安装 fail2ban
system_fail2ban() {
    apt install fail2ban -y
    sudo bash -c 'cat <<EOF > /etc/fail2ban/jail.local
[DEFAULT]
#忽略的IP列表,不受设置限制（白名单）
ignoreip = 127.0.0.1

#允许ipv6
allowipv6 = auto

#日志修改检测机制（gamin、polling和auto这三种）
backend = systemd

[sshd]

#是否激活此项（true/false）
enabled = true

#过滤规则filter的名字，对应filter.d目录下的sshd.conf
filter = sshd

#ssh端口
port = ssh

#动作的相关参数
action = iptables[name=SSH, port=ssh, protocol=tcp]

#检测的系统的登陆日志文件
logpath = /var/log/secure

#屏蔽时间，单位：秒
bantime = 86400

#这个时间段内超过规定次数会被ban掉
findtime = 86400

#最大尝试次数
maxretry = 3
EOF'
    sudo systemctl enable fail2ban
    sudo systemctl restart fail2ban
    clear
    echo "已成功安装fail2ban"
}

# 1.9 密钥登录
system_keygen() {
    if [ -f ~/.ssh/id_rsa ]; then
        echo "SSH Key已经存在"
    else
        echo "创建SSH Key..."
        ssh-keygen -t rsa -f ~/.ssh/id_rsa -q -N ""
    fi
    cat ~/.ssh/id_rsa.pub >> ~/.ssh/authorized_keys
    chmod 600 ~/.ssh/authorized_keys
    chmod 700 ~/.ssh
    sudo sed -i 's/^#\?PubkeyAuthentication.*/PubkeyAuthentication yes/g' /etc/ssh/sshd_config
    sudo sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/g' /etc/ssh/sshd_config
    sudo systemctl restart sshd
    echo "密码登录已关闭！"
    echo "密钥登录已开启！"
    echo "您的SSH Key为，请牢记！"
    cat ~/.ssh/id_rsa
}

# ═══════════════════════════════════════════════════════════════
# 2. 测试脚本
# ═══════════════════════════════════════════════════════════════

# 2.1 SpeedTest 带宽测速
bandwidth_test() {
    clear
    if ! command -v jq &> /dev/null || ! command -v bc &> /dev/null; then
        echo "jq和bc未安装，现在进行安装..."
        sudo apt-get update
        sudo apt-get install -y jq bc
    else
        echo "jq和bc已安装，即将跳过..."
    fi

    local country
    country=$(curl -s ipinfo.io/country)

    if [ "$country" == "CN" ]; then
        echo "国内测速脚本的项目因滥用已被屏蔽，暂无可使用的国内测速API。"
    else
        if ! command -v speedtest &> /dev/null; then
            echo "speedtest-cli 未安装，正在安装..."
            sudo apt-get install curl -y > /dev/null 2>&1
            run_https_script https://packagecloud.io/install/repositories/ookla/speedtest-cli/script.deb.sh root || return 1
            sudo apt-get install speedtest -y
        else
            echo "speedtest-cli 已安装，跳过安装步骤。"
        fi
        clear
        echo "本机器地理位置不在中国，使用speedtest-cli测速..."
        echo "测速中，请等待..."
        local json_output timestamp location
        local download_bandwidth download_speed upload_bandwidth upload_speed result_url
        json_output=$(speedtest --accept-license --accept-gdpr -f json-pretty)
        timestamp=$(echo "$json_output"          | jq -r '.timestamp')
        location=$(echo "$json_output"           | jq -r '.server.location')
        download_bandwidth=$(echo "$json_output" | jq -r '.download.bandwidth')
        download_speed=$(echo "scale=2; $download_bandwidth / 1000 / 1000" | bc)
        upload_bandwidth=$(echo "$json_output"   | jq -r '.upload.bandwidth')
        upload_speed=$(echo "scale=2; $upload_bandwidth / 1000 / 1000" | bc)
        result_url=$(echo "$json_output"         | jq -r '.result.url')
        echo "测试时间: $timestamp"
        echo "区域信息: $location"
        echo "下载速度: $download_speed MB/s"
        echo "上传速度: $upload_speed MB/s"
        echo "测试结果链接: $result_url"
    fi
}

# 2.2 IP 质量检测
ip_test() {
    clear
    echo "IP质量检测中..."
    run_https_script https://IP.Check.Place
}

# 2.3 nxtrace 快速回程测试
router_test() {
    clear
    run_https_script https://nxtrace.org/nt || return 1
    nexttrace --fast-trace --tcp
}

# 2.4 yabs 性能测试
performance_test() {
    clear
    run_https_script https://yabs.sh
}

# 2.5 IPv4/IPv6 优先级测试
ip_priority_test() {
    clear
    local ipv4_test="ipv4.test-ipv6.com"
    local ipv6_test="ipv6.test-ipv6.com"
    local ipv4_output ipv6_output ipv6_priority

    ipv4_output=$(curl -4 -s --max-time 5 "$ipv4_test" > /dev/null && echo "IPv4正常工作" || echo "IPv4连接失败")
    ipv6_output=$(curl -6 -s --max-time 5 "$ipv6_test" > /dev/null && echo "IPv6正常工作" || echo "IPv6连接失败")

    echo "测试连通性:"
    echo "$ipv4_output"
    echo "$ipv6_output"
    echo ""
    echo "优先级测试:"
    ipv6_priority=$(grep "precedence ::ffff:0:0/96" /etc/gai.conf 2>/dev/null | grep -v "#" | wc -l)

    if [[ "$ipv6_output" == "IPv6正常工作" && "$ipv4_output" == "IPv4正常工作" ]]; then
        if [ "$ipv6_priority" -gt 0 ]; then
            echo "IPv4的优先级更高"
        else
            echo "IPv6的优先级更高"
        fi
    elif [[ "$ipv6_output" == "IPv6正常工作" ]]; then
        echo "只有IPv6正常工作, 因此IPv6优先级更高"
    elif [[ "$ipv4_output" == "IPv4正常工作" ]]; then
        echo "只有IPv4正常工作, 因此IPv4优先级更高"
    else
        echo "IPv4 和 IPv6 似乎都无法正常工作。"
    fi
}

# 2.6 硬盘 I/O 测试
io_test() {
    (LANG=C dd if=/dev/zero of=test_$$ bs=64k count=16k conv=fdatasync && rm -f test_$$) \
        2>&1 | awk -F, '{io=$NF} END { print io}' | sed 's/^[ \t]*//;s/[ \t]*$//'
}

io_info() {
    echo "开始测试IO性能..."
    local io1 io2 io3 ioraw1 ioraw2 ioraw3 ioall ioavg
    io1=$(io_test); echo "硬盘I/O (第一次测试) : $io1"
    io2=$(io_test); echo "硬盘I/O (第二次测试) : $io2"
    io3=$(io_test); echo "硬盘I/O (第三次测试) : $io3"
    ioraw1=$(echo "$io1" | awk 'NR==1 {print $1}')
    [ "$(echo "$io1" | awk 'NR==1 {print $2}')" == "GB/s" ] && ioraw1=$(awk "BEGIN{print $ioraw1 * 1024}")
    ioraw2=$(echo "$io2" | awk 'NR==1 {print $1}')
    [ "$(echo "$io2" | awk 'NR==1 {print $2}')" == "GB/s" ] && ioraw2=$(awk "BEGIN{print $ioraw2 * 1024}")
    ioraw3=$(echo "$io3" | awk 'NR==1 {print $1}')
    [ "$(echo "$io3" | awk 'NR==1 {print $2}')" == "GB/s" ] && ioraw3=$(awk "BEGIN{print $ioraw3 * 1024}")
    ioall=$(awk "BEGIN{print $ioraw1 + $ioraw2 + $ioraw3}")
    ioavg=$(awk "BEGIN{print $ioall / 3}")
    echo "硬盘I/O (平均值) : $ioavg MB/s"
}

# ═══════════════════════════════════════════════════════════════
# 3. 常用工具安装
# ═══════════════════════════════════════════════════════════════

# 3.11 Starship 终端美化安装器（自包含，检测国内自动切镜像）
starship_install() {
    set -eu
    printf '\n'

    BOLD="$(tput bold 2>/dev/null || printf '')"
    GREY="$(tput setaf 0 2>/dev/null || printf '')"
    UNDERLINE="$(tput smul 2>/dev/null || printf '')"
    RED="$(tput setaf 1 2>/dev/null || printf '')"
    GREEN="$(tput setaf 2 2>/dev/null || printf '')"
    YELLOW="$(tput setaf 3 2>/dev/null || printf '')"
    BLUE="$(tput setaf 4 2>/dev/null || printf '')"
    MAGENTA="$(tput setaf 5 2>/dev/null || printf '')"
    NO_COLOR="$(tput sgr0 2>/dev/null || printf '')"

    detect_in_china() {
        local response
        if command -v curl &>/dev/null; then
            response=$(curl -s --connect-timeout 2 -m 4 cip.cc 2>/dev/null)
        elif command -v wget &>/dev/null; then
            response=$(wget -qO- --timeout=4 cip.cc 2>/dev/null)
        else
            return 1
        fi
        echo "$response" | grep -q "中国"
    }

    SUPPORTED_TARGETS="x86_64-unknown-linux-gnu x86_64-unknown-linux-musl \
                      i686-unknown-linux-musl aarch64-unknown-linux-musl \
                      arm-unknown-linux-musleabihf x86_64-apple-darwin \
                      aarch64-apple-darwin x86_64-pc-windows-msvc \
                      i686-pc-windows-msvc aarch64-pc-windows-msvc \
                      x86_64-unknown-freebsd"

    info()      { printf '%s\n' "${BOLD}${GREY}>${NO_COLOR} $*"; }
    warn()      { printf '%s\n' "${YELLOW}! $*${NO_COLOR}"; }
    error()     { printf '%s\n' "${RED}x $*${NO_COLOR}" >&2; }
    completed() { printf '%s\n' "${GREEN}✓${NO_COLOR} $*"; }
    has()       { command -v "$1" 1>/dev/null 2>&1; }

    curl_is_snap() {
        local curl_path
        curl_path="$(command -v curl)"
        case "$curl_path" in /snap/*) return 0 ;; *) return 1 ;; esac
    }

    get_tmpfile() {
        local suffix="$1"
        if has mktemp; then
            printf "%s.%s" "$(mktemp)" "${suffix}"
        else
            printf "/tmp/starship.%s" "${suffix}"
        fi
    }

    test_writeable() {
        local path="${1:-}/test.txt"
        if touch "${path}" 2>/dev/null; then rm "${path}"; return 0; else return 1; fi
    }

    download() {
        local file="$1" url="$2"
        if has curl && curl_is_snap; then
            warn "curl installed through snap cannot download starship."
            warn "See https://github.com/starship/starship/issues/5403 for details."
            warn "Searching for other HTTP download programs..."
        fi
        local cmd rc
        if has curl && ! curl_is_snap; then
            cmd="curl --fail --silent --location --output $file $url"
        elif has wget; then
            cmd="wget --quiet --output-document=$file $url"
        elif has fetch; then
            cmd="fetch --quiet --output=$file $url"
        else
            error "No HTTP download program (curl, wget, fetch) found, exiting…"
            return 1
        fi
        $cmd && return 0 || rc=$?
        error "Command failed (exit code $rc): ${BLUE}${cmd}${NO_COLOR}"
        printf "\n" >&2
        info "This is likely due to Starship not yet supporting your configuration."
        info "If you would like to see a build for your configuration,"
        info "please create an issue requesting a build for ${MAGENTA}${TARGET}${NO_COLOR}:"
        info "${BOLD}${UNDERLINE}https://github.com/starship/starship/issues/new/${NO_COLOR}"
        return $rc
    }

    unpack() {
        local archive=$1 bin_dir=$2 sudo=${3-}
        case "$archive" in
            *.tar.gz)
                local flags
                flags=$(test -n "${VERBOSE-}" && echo "-xzvof" || echo "-xzof")
                ${sudo} tar "${flags}" "${archive}" -C "${bin_dir}"
                return 0
                ;;
            *.zip)
                local flags
                flags=$(test -z "${VERBOSE-}" && echo "-qqo" || echo "-o")
                UNZIP="${flags}" ${sudo} unzip "${archive}" -d "${bin_dir}"
                return 0
                ;;
        esac
        error "Unknown package extension."
        printf "\n"
        info "This almost certainly results from a bug in this script--please file a"
        info "bug report at https://github.com/starship/starship/issues"
        return 1
    }

    usage() {
        printf "%s\n" \
            "install.sh [option]" \
            "" \
            "Fetch and install the latest version of starship, if starship is already" \
            "installed it will be updated to the latest version."
        printf "\n%s\n" "Options"
        printf "\t%s\n\t\t%s\n\n" \
            "-V, --verbose" "Enable verbose output for the installer" \
            "-f, -y, --force, --yes" "Skip the confirmation prompt during installation" \
            "-p, --platform" "Override the platform identified by the installer [default: ${PLATFORM}]" \
            "-b, --bin-dir" "Override the bin installation directory [default: ${BIN_DIR}]" \
            "-a, --arch" "Override the architecture identified by the installer [default: ${ARCH}]" \
            "-B, --base-url" "Override the base URL used for downloading releases [default: ${BASE_URL}]" \
            "-v, --version" "Install a specific version of starship [default: ${VERSION}]" \
            "-h, --help" "Display this help message"
    }

    elevate_priv() {
        if ! has sudo; then
            error 'Could not find the command "sudo", needed to get permissions for install.'
            info "If you are on Windows, please run your shell as an administrator, then"
            info "rerun this script. Otherwise, please run this script as root, or install"
            info "sudo."
            exit 1
        fi
        if ! sudo -v; then
            error "Superuser not granted, aborting installation"
            exit 1
        fi
    }

    install() {
        local ext="$1"
        local sudo msg archive
        if test_writeable "${BIN_DIR}"; then
            sudo=""
            msg="Installing Starship, please wait…"
        else
            warn "Escalated permissions are required to install to ${BIN_DIR}"
            elevate_priv
            sudo="sudo"
            msg="Installing Starship as root, please wait…"
        fi
        info "$msg"
        archive=$(get_tmpfile "$ext")
        download "${archive}" "${URL}"
        unpack "${archive}" "${BIN_DIR}" "${sudo}"
    }

    detect_platform() {
        local platform
        platform="$(uname -s | tr '[:upper:]' '[:lower:]')"
        case "${platform}" in
            msys_nt*)   platform="pc-windows-msvc" ;;
            cygwin_nt*) platform="pc-windows-msvc" ;;
            mingw*)     platform="pc-windows-msvc" ;;
            linux)      platform="unknown-linux-musl" ;;
            darwin)     platform="apple-darwin" ;;
            freebsd)    platform="unknown-freebsd" ;;
        esac
        printf '%s' "${platform}"
    }

    detect_arch() {
        local arch
        arch="$(uname -m | tr '[:upper:]' '[:lower:]')"
        case "${arch}" in
            amd64) arch="x86_64" ;;
            armv*) arch="arm" ;;
            arm64) arch="aarch64" ;;
        esac
        if [ "${arch}" = "x86_64" ] && [ "$(getconf LONG_BIT)" -eq 32 ]; then
            arch=i686
        elif [ "${arch}" = "aarch64" ] && [ "$(getconf LONG_BIT)" -eq 32 ]; then
            arch=arm
        fi
        printf '%s' "${arch}"
    }

    detect_target() {
        local arch="$1" platform="$2"
        local target="${arch}-${platform}"
        [ "${target}" = "arm-unknown-linux-musl" ] && target="${target}eabihf"
        printf '%s' "${target}"
    }

    confirm() {
        if [ -z "${FORCE-}" ]; then
            printf "%s " "${MAGENTA}?${NO_COLOR} $* ${BOLD}[y/N]${NO_COLOR}"
            set +e
            read -r yn </dev/tty
            local rc=$?
            set -e
            if [ $rc -ne 0 ]; then
                error "Error reading from prompt (please re-run with the '--yes' option)"
                exit 1
            fi
            if [ "$yn" != "y" ] && [ "$yn" != "yes" ]; then
                error 'Aborting (please answer "yes" to continue)'
                exit 1
            fi
        fi
    }

    check_bin_dir() {
        local bin_dir="${1%/}"
        if [ ! -d "$BIN_DIR" ]; then
            error "Installation location $BIN_DIR does not appear to be a directory"
            info "Make sure the location exists and is a directory, then try again."
            usage
            exit 1
        fi
        local good
        good=$(IFS=:; for path in $PATH; do
            if [ "${path%/}" = "${bin_dir}" ]; then printf 1; break; fi
        done)
        [ "${good}" != "1" ] && warn "Bin directory ${bin_dir} is not in your \$PATH"
    }

    print_install() {
        for s in "bash" "zsh" "ion" "tcsh" "xonsh" "fish"; do
            local config_file="~/.${s}rc"
            local config_cmd="eval \"\$(starship init ${s})\""
            case ${s} in
                ion)   config_file="~/.config/ion/initrc"; config_cmd="eval \$(starship init ${s})" ;;
                fish)  config_file="~/.config/fish/config.fish"; config_cmd="starship init fish | source" ;;
                tcsh)  config_cmd="eval \`starship init ${s}\`" ;;
                xonsh) config_cmd="execx(\$(starship init xonsh))" ;;
            esac
            printf "  %s\n  Add the following to the end of %s:\n\n\t%s\n\n" \
                "${BOLD}${UNDERLINE}${s}${NO_COLOR}" \
                "${BOLD}${config_file}${NO_COLOR}" \
                "${config_cmd}"
        done
        for s in "elvish" "nushell"; do
            local warning="${BOLD}Warning${NO_COLOR}"
            local config_file config_cmd
            case ${s} in
                elvish)
                    config_file="~/.elvish/rc.elv"
                    config_cmd="eval (starship init elvish)"
                    warning="${warning} Only elvish v0.17 or higher is supported."
                    ;;
                nushell)
                    config_file="${BOLD}your nu config file${NO_COLOR} (find it by running ${BOLD}\$nu.config-path${NO_COLOR} in Nushell)"
                    config_cmd="mkdir (\$nu.data-dir | path join \"vendor/autoload\")
            starship init nu | save -f (\$nu.data-dir | path join \"vendor/autoload/starship.nu\")"
                    warning="${warning} This will change in the future.
      Only Nushell v0.96 or higher is supported."
                    ;;
            esac
            printf "  %s\n  %s\n  And add the following to the end of %s:\n\n\t%s\n\n" \
                "${BOLD}${UNDERLINE}${s}${NO_COLOR}" \
                "${warning}" \
                "${config_file}" \
                "${config_cmd}"
        done
        printf "  %s\n  Add the following to the end of %s:\n  %s\n\n\t%s\n\n" \
            "${BOLD}${UNDERLINE}PowerShell${NO_COLOR}" \
            "${BOLD}Microsoft.PowerShell_profile.ps1${NO_COLOR}" \
            "You can check the location of this file by querying the \$PROFILE variable in PowerShell." \
            "Invoke-Expression (&starship init powershell)"
        printf "  %s\n  You need to use Clink (v1.2.30+) with Cmd. Add the following to a file %s and place this file in Clink scripts directory:\n\n\t%s\n\n" \
            "${BOLD}${UNDERLINE}Cmd${NO_COLOR}" \
            "${BOLD}starship.lua${NO_COLOR}" \
            "load(io.popen('starship init cmd'):read(\"*a\"))()"
        printf "\n"
    }

    is_build_available() {
        local arch="$1" platform="$2" target="$3"
        local good
        good=$(IFS=" "; for t in $SUPPORTED_TARGETS; do
            if [ "${t}" = "${target}" ]; then printf 1; break; fi
        done)
        if [ "${good}" != "1" ]; then
            error "${arch} builds for ${platform} are not yet available for Starship"
            printf "\n" >&2
            info "If you would like to see a build for your configuration,"
            info "please create an issue requesting a build for ${MAGENTA}${target}${NO_COLOR}:"
            info "${BOLD}${UNDERLINE}https://github.com/starship/starship/issues/new/${NO_COLOR}"
            printf "\n"
            exit 1
        fi
    }

    # 默认值
    [ -z "${PLATFORM-}" ] && PLATFORM="$(detect_platform)"
    [ -z "${BIN_DIR-}" ]  && BIN_DIR=/usr/local/bin
    [ -z "${ARCH-}" ]     && ARCH="$(detect_arch)"
    if [ -z "${BASE_URL-}" ]; then
        if detect_in_china; then
            BASE_URL="https://ghfast.top/https://github.com/starship/starship/releases"
            info "检测到国内机器，已自动切换镜像源"
        else
            BASE_URL="https://github.com/starship/starship/releases"
        fi
    fi
    [ -z "${VERSION-}" ] && VERSION="latest"

    # 解析参数
    while [ "$#" -gt 0 ]; do
        case "$1" in
            -p | --platform)   PLATFORM="$2"; shift 2 ;;
            -b | --bin-dir)    BIN_DIR="$2";  shift 2 ;;
            -a | --arch)       ARCH="$2";     shift 2 ;;
            -B | --base-url)   BASE_URL="$2"; shift 2 ;;
            -v | --version)    VERSION="$2";  shift 2 ;;
            -V | --verbose)    VERBOSE=1;     shift 1 ;;
            -f | -y | --force | --yes) FORCE=1; shift 1 ;;
            -h | --help)       usage; exit ;;
            -p=*) PLATFORM="${1#*=}"; shift 1 ;;
            -b=*) BIN_DIR="${1#*=}";  shift 1 ;;
            -a=*) ARCH="${1#*=}";     shift 1 ;;
            -B=*) BASE_URL="${1#*=}"; shift 1 ;;
            -v=*) VERSION="${1#*=}";  shift 1 ;;
            -V=*) VERBOSE="${1#*=}";  shift 1 ;;
            -f=* | -y=* | --force=* | --yes=*) FORCE="${1#*=}"; shift 1 ;;
            *) error "Unknown option: $1"; usage; exit 1 ;;
        esac
    done

    TARGET="$(detect_target "${ARCH}" "${PLATFORM}")"
    is_build_available "${ARCH}" "${PLATFORM}" "${TARGET}"

    printf "  %s\n" "${UNDERLINE}Configuration${NO_COLOR}"
    info "${BOLD}Bin directory${NO_COLOR}: ${GREEN}${BIN_DIR}${NO_COLOR}"
    info "${BOLD}Platform${NO_COLOR}:      ${GREEN}${PLATFORM}${NO_COLOR}"
    info "${BOLD}Arch${NO_COLOR}:          ${GREEN}${ARCH}${NO_COLOR}"

    if [ -n "${VERBOSE-}" ]; then VERBOSE=v; info "${BOLD}Verbose${NO_COLOR}: yes"; else VERBOSE=; fi

    printf '\n'

    local EXT=tar.gz
    [ "${PLATFORM}" = "pc-windows-msvc" ] && EXT=zip

    local URL
    if [ "${VERSION}" != "latest" ]; then
        URL="${BASE_URL}/download/${VERSION}/starship-${TARGET}.${EXT}"
    else
        URL="${BASE_URL}/latest/download/starship-${TARGET}.${EXT}"
    fi

    info "Tarball URL: ${UNDERLINE}${BLUE}${URL}${NO_COLOR}"
    confirm "Install Starship ${GREEN}${VERSION}${NO_COLOR} to ${BOLD}${GREEN}${BIN_DIR}${NO_COLOR}?"
    check_bin_dir "${BIN_DIR}"
    install "${EXT}"
    completed "Starship ${VERSION} installed"
    printf '\n'
    info "Please follow the steps for your shell to complete the installation:"
    print_install
    set +eu
}

# 3.11 zsh + Starship 终端美化
download_zsh() {
    clear
    apt install zsh -y && apt install curl -y
    chsh -s "$(which zsh)"
    ( starship_install -y ) || echo -e "${yellow}Starship 安装失败，已跳过${white}"
    echo 'eval "$(starship init zsh)"'  >> ~/.zshrc
    echo 'eval "$(starship init bash)"' >> ~/.bashrc
    echo "zsh+Starship安装完成,重新登录终端即可启用"
}

# 3.12 一键安装所有常用工具
download_all() {
    clear
    apt install curl wget nano unzip tar tmux iftop btop gdu fzf -y
    download_zsh
    echo "已全部安装"
}

# ═══════════════════════════════════════════════════════════════
# 4. Docker 管理
# ═══════════════════════════════════════════════════════════════

# 4.1 安装 Docker
docker_install() {
    clear
    if command -v docker &> /dev/null; then
        echo "Docker已经安装。"
        return 0
    fi
    local country
    country=$(curl -s ipinfo.io/country)
    if [ "$country" == "CN" ]; then
        echo "本机器地理位置为中国，正在使用国内安装脚本..."
        run_https_script https://linuxmirrors.cn/docker.sh root || return 1
        rm -f ./docker-install
        echo "Docker安装完成，正在切换镜像源（由1panel提供）..."
        touch /etc/docker/daemon.json
        cat > /etc/docker/daemon.json << 'EOF'
{
    "registry-mirrors": ["https://docker.1panel.live"]
}
EOF
    else
        echo "本机器地理位置不在中国，正在使用官方Docker安装脚本..."
        run_https_script https://get.docker.com root || return 1
        touch /etc/docker/daemon.json
    fi
    echo "Docker安装过程完成。"
}

# 4.2 查看 Docker 全局状态
docker_status() {
    if ! command -v docker &> /dev/null; then
        echo "Docker 环境不存在，请确保 Docker 已安装并配置。"
        return 1
    fi
    echo "Docker版本"
    docker -v
    docker compose version
    echo ""
    echo "Docker镜像列表";  docker image ls;   echo ""
    echo "Docker容器列表";  docker ps -a;       echo ""
    echo "Docker卷列表";    docker volume ls;   echo ""
    echo "Docker网络列表";  docker network ls;  echo ""
    if [ -f /etc/docker/daemon.json ]; then
        local mirrors
        mirrors=$(jq -r '.["registry-mirrors"][]' /etc/docker/daemon.json 2>/dev/null)
        if [ -n "$mirrors" ]; then
            echo "镜像源地址"
            echo "$mirrors"
        else
            echo "未配置镜像源"
        fi
    fi
}

# 4.3 清理无用的镜像容器网络
docker_clean() {
    clear
    read -p "$(echo -e "${yellow}提示: ${white}将清理无用的镜像容器网络，包括停止的容器，确定清理吗？(Y/N): ")" choice
    case "$choice" in
        [Yy]) docker system prune -af --volumes ;;
        [Nn]) ;;
        *)    echo "无效的选择，请输入 Y 或 N。" ;;
    esac
}

# 4.4 更换 Docker 源
docker_mirrors() {
    local docker_daemon="${1:-/etc/docker/daemon.json}" new_mirror
    if [ ! -f "$docker_daemon" ]; then
        echo "文件 $docker_daemon 不存在，请确保 Docker 已安装并配置。"
        return 1
    fi
    while true; do
        read -rp "请输入镜像源地址（含http://或https://），输入'q'退出: " new_mirror || return 0
        [[ "$new_mirror" != q ]] || { echo "操作已取消。"; return 0; }
        [[ "$new_mirror" =~ ^https?:// ]] && break
        echo "错误：镜像源地址必须以 http:// 或 https:// 开头，请重新输入。"
    done

    if grep -q '"registry-mirrors"' "$docker_daemon"; then
        echo "检测到已有镜像源地址。"
        sudo sed -i 's|"registry-mirrors":\s*\[[^]]*\]|"registry-mirrors": ["'"$new_mirror"'"]|' "$docker_daemon"
        echo "镜像源地址已更新为: $new_mirror"
    else
        echo "未检测到镜像源地址。"
        sudo cat > "$docker_daemon" << EOF
{
    "registry-mirrors": ["$new_mirror"]
}
EOF
        echo "registry-mirrors 已添加为: $new_mirror"
    fi
    echo "正在重启 Docker 服务以应用更改..."
    sudo systemctl restart docker
    echo "修改完成！"
}

docker_set_ipv6() (
    # lock_fd 通过 acquire_lock 的 nameref 写入，并由本子 shell 持有。
    # shellcheck disable=SC2034
    local enabled="$1" conf_file="${2:-/etc/docker/daemon.json}" work lock_fd
    local changed=0 committed=0 had_original=0 recovered=1 was_active=0
    require_root || return 1
    case "$enabled" in on|off) ;; *) return 1 ;; esac
    require_commands python3 dockerd systemctl flock || return 1
    [[ ! -L "$conf_file" && ! -L "${conf_file}.cqs-lock" ]] || return 1
    [[ ! -e "$conf_file" || -f "$conf_file" ]] || return 1
    mkdir -p -- "$(dirname -- "$conf_file")" || return 1
    acquire_lock lock_fd "${conf_file}.cqs-lock" "另一个进程正在修改 Docker 配置" || return 1
    if systemctl is-active --quiet docker; then was_active=1; fi
    work=$(mktemp -d "${conf_file}.work.XXXXXX") || return 1
    docker_finish_ipv6() {
        if (( changed && ! committed )); then
            if (( had_original )); then
                cp -p -- "$work/original.json" "$work/restore.json" &&
                    mv -fT -- "$work/restore.json" "$conf_file" || recovered=0
            else
                rm -f -- "$conf_file" || recovered=0
            fi
            if (( recovered )); then
                if (( was_active )); then
                    systemctl restart docker || recovered=0
                else
                    systemctl stop docker || recovered=0
                fi
            fi
            echo "Docker 修改失败，已尝试恢复原配置" >&2
        fi
        if (( recovered )); then rm -rf -- "$work"; else echo "Docker 恢复失败，保留备份: $work" >&2; fi
    }
    trap docker_finish_ipv6 EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    if [[ -f "$conf_file" ]]; then
        had_original=1
        cp -p -- "$conf_file" "$work/original.json" || return 1
        cp -p -- "$conf_file" "$work/desired.json" || return 1
    else
        printf '{}\n' > "$work/desired.json" || return 1
        chmod 644 "$work/desired.json" || return 1
    fi
    python3 - "$work/desired.json" "$enabled" <<'PY'
import json
import pathlib
import secrets
import sys

try:
    path = pathlib.Path(sys.argv[1])
    config = json.loads(path.read_text())
    if not isinstance(config, dict):
        raise ValueError("daemon.json must contain an object")
    config["ipv6"] = sys.argv[2] == "on"
    if config["ipv6"]:
        if not config.get("fixed-cidr-v6") and not config.get("bip6"):
            prefix = "fd" + secrets.token_hex(5)
            config["fixed-cidr-v6"] = ":".join(prefix[i:i+4] for i in (0, 4, 8)) + ":1::/64"
    else:
        for key in ("fixed-cidr-v6", "bip6", "default-gateway-v6"):
            config.pop(key, None)
    path.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n")
except (OSError, ValueError) as error:
    print("Docker 配置处理失败: " + str(error), file=sys.stderr)
    sys.exit(1)
PY
    [[ $? -eq 0 ]] || return 1
    dockerd --validate --config-file "$work/desired.json" || {
        echo "Docker 配置校验失败，未修改原文件" >&2; return 1;
    }
    changed=1
    mv -fT -- "$work/desired.json" "$conf_file" || return 1
    systemctl restart docker && systemctl is-active --quiet docker || return 1
    committed=1
    if [[ "$enabled" == on ]]; then echo "Docker已开启v6访问"; else echo "Docker已关闭v6访问"; fi
)

# 4.9 卸载 Docker
docker_uninstall() {
    clear
    read -p "$(echo -e "${red}注意: ${white}确定卸载docker环境吗？(Y/N): ")" choice
    case "$choice" in
        [Yy])
            docker rm $(docker ps -a -q) && docker rmi $(docker images -q) && docker network prune
            sudo apt-get purge -y docker-engine docker docker.io docker-ce docker-ce-cli
            sudo rm -rf /var/lib/docker
            sudo rm -rf /etc/docker
            sudo rm -rf /var/run/docker.sock
            ;;
        [Nn]) ;;
        *)    echo "无效的选择，请输入 Y 或 N。" ;;
    esac
}


# ═══════════════════════════════════════════════════════════════
# 子菜单
# ═══════════════════════════════════════════════════════════════

# 1. 系统相关子菜单
system_related() {
    menu_loop "系统相关" \
        "1|更新系统软件包|system_update" \
        "2|系统清理|system_clean" \
        "3|TCP 调优|tcp_tune" \
        "4|BBR 网络管理|bbr_manage" \
        "5|将时区改为上海|system_time" \
        "6|修改 SWAP|system_swap" \
        "7|修改 SSH 端口|system_ssh" \
        "8|安装 fail2ban|system_fail2ban" \
        "9|密钥登录|system_keygen"
}

# 2. 测试脚本子菜单
test_script() {
    menu_loop "测试脚本" \
        "1|SpeedTest 带宽测速|bandwidth_test" \
        "2|xykt IP 质量体检|ip_test" \
        "3|nxtrace 快速回程测试|router_test" \
        "4|yabs 性能测试|performance_test" \
        "5|IPv4/IPv6 优先级测试|ip_priority_test" \
        "6|硬盘 I/O 测试|io_info"
}

# 3. 常用工具下载子菜单
# 单个包安装通过 pkg_install 完成，消除了 10 个重复的 download_xxx 函数
useful_tools() {
    menu_loop "常用工具" \
        "1|curl 下载工具|pkg_install curl" \
        "2|wget 下载工具|pkg_install wget" \
        "3|nano 文本编辑器|pkg_install nano" \
        "4|unzip 解压缩工具|pkg_install unzip" \
        "5|tar 压缩工具|pkg_install tar" \
        "6|tmux 终端复用|pkg_install tmux" \
        "7|iftop 网络监控|pkg_install iftop" \
        "8|btop 系统监控|pkg_install btop" \
        "9|gdu 磁盘分析|pkg_install gdu" \
        "10|fzf 模糊搜索|pkg_install fzf" \
        "11|zsh + Starship 终端美化|download_zsh" \
        "12|一键安装所有|download_all"
}

# 4. Docker 管理子菜单
docker_manage() {
    menu_loop "Docker 管理" \
        "1|安装 Docker|docker_install" \
        "2|查看 Docker 全局状态|docker_status" \
        "3|清理无用镜像、容器和网络|docker_clean" \
        "4|更换 Docker 镜像源|docker_mirrors" \
        "5|开启 Docker IPv6|docker_set_ipv6 on" \
        "6|关闭 Docker IPv6|docker_set_ipv6 off" \
        "9|卸载 Docker|docker_uninstall"
}

# ═══════════════════════════════════════════════════════════════
# 9. 一键优化
# ═══════════════════════════════════════════════════════════════

onekey_optimization() {
    local network_failed=0 choice
    require_root || return 1
    echo "一键优化"
    echo -e "${pink}============================${white}"
    echo "优化内容如下："
    echo "- 更新系统软件包"
    echo "- 系统清理"
    echo -e "- TCP调优"
    echo -e "- 启用 BBR（内核自带）"
    echo -e "- 设置时区到${yellow}上海${white}"
    echo -e "- 自动设置${yellow}虚拟内存${white}"
    echo -e "- 安装fail2ban"
    echo -e "- 安装${yellow}所有常用工具${white}"
    echo -e "- 设置SSH端口号为${yellow}55520${white}"
    echo -e "- 修改为密钥登录"
    echo -e "${pink}============================${white}"
    echo -e "${red}注意：请牢记端口号和密钥，否则重启后无法登录${white}"
    read -p "确定一键优化吗？(Y/N): " choice

    case "$choice" in
        [Yy])
            clear
            echo -e "${pink}============================${white}"
            if ! system_update; then
                echo -e "[${red}FAIL${white}] 系统更新未完成，已停止后续优化"
                return 1
            fi
            echo -e "[${green}OK${white}] 1/10. 更新系统到最新"

            echo -e "${pink}============================${white}"
            if ! system_clean; then
                echo -e "[${red}FAIL${white}] 系统清理未完成，已停止后续优化"
                return 1
            fi
            echo -e "[${green}OK${white}] 2/10. 清理系统垃圾文件"

            echo -e "${pink}============================${white}"
            if tcp_tune --auto; then
                echo -e "[${green}OK${white}] 3/10. TCP调优"
            else
                network_failed=1
                echo -e "[${red}FAIL${white}] 3/10. TCP调优未全部完成"
            fi

            echo -e "${pink}============================${white}"
            if enable_bbr fq; then
                echo -e "[${green}OK${white}] 4/10. 启用 BBR"
            else
                network_failed=1
                echo -e "[${red}FAIL${white}] 4/10. BBR 启用失败，已继续后续优化"
            fi


            echo -e "${pink}============================${white}"
            system_time
            echo -e "[${green}OK${white}] 5/10. 设置时区到${yellow}上海${white}"

            echo -e "${pink}============================${white}"
            swap_create_auto
            echo -e "[${green}OK${white}] 6/10. 自动设置${yellow}虚拟内存${white}"

            echo -e "${pink}============================${white}"
            system_fail2ban
            echo -e "[${green}OK${white}] 7/10. 安装fail2ban"

            echo -e "${pink}============================${white}"
            download_all
            docker_install
            echo -e "[${green}OK${white}] 8/10. 安装${yellow}Docker等常用工具${white}"

            echo -e "${pink}============================${white}"
            if ! system_ssh 55520; then
                echo -e "[${red}FAIL${white}] SSH 端口修改失败，停止后续登录方式修改"
                return 1
            fi
            echo -e "[${green}OK${white}] 9/10. 设置SSH端口号为${yellow}55520${white}"

            echo -e "${pink}============================${white}"
            system_keygen
            echo -e "[${green}OK${white}] 10/10. 修改为密钥登录"
            echo -e "${pink}============================${white}"

            clear
            if (( network_failed )); then
                echo -e "${yellow}一键优化流程已结束，但 TCP/BBR 存在失败项，请单独重试${white}"
            else
                echo -e "${green}一键优化已完成${white}"
            fi
            echo "您现在的SSH端口为55520，您的SSH Key如下，请牢记："
            cat ~/.ssh/id_rsa
            ;;
        [Nn])
            echo "已取消"
            ;;
        *)
            echo "无效的选择，请输入 Y 或 N。"
            ;;
    esac
}

# ═══════════════════════════════════════════════════════════════
# 主界面
# ═══════════════════════════════════════════════════════════════

cqs_menu() {
    local choice confirm
    while true; do
        clear
        echo -e "${pink}============================${white}"
        echo -e "${pink}${CQS_NAME} 【v${CQS_VERSION}】        输入 ${red}cqs${pink} 可快速启动${white}"
        show_sysinfo
        echo -e "${pink}============================${white}"
        echo "1. 系统相关->"
        echo "2. 测试脚本->"
        echo "3. 常用工具下载->"
        echo "4. Docker管理->"
        echo -e "${pink}============================${white}"
        echo "9. 一键优化"
        echo -e "${pink}============================${white}"
        echo "555. 卸载脚本"
        echo -e "${pink}============================${white}"
        echo "0. 退出脚本"
        echo -e "${pink}============================${white}"
        read -rp "请输入你的选择: " choice || return 0

        case $choice in
            1)   clear; system_related ;;
            2)   clear; test_script ;;
            3)   clear; useful_tools ;;
            4)   clear; docker_manage ;;
            9)   clear; onekey_optimization ;;
            555)
                clear
                echo "卸载 ${CQS_NAME}"
                echo -e "${pink}============================${white}"
                echo "将卸载 cqs 快捷命令，不影响已安装的功能"
                read -rp "确定继续吗？(Y/N): " confirm || return 0
                if [[ "$confirm" == "Y" || "$confirm" == "y" ]]; then
                    clear
                    uninstall_shortcut && return 0
                else
                    echo "操作已取消。"
                fi
                ;;
            0)   return 0 ;;
            *)   echo -e "无效的输入!" ;;
        esac
        break_end
    done
}

# ─── 入口：source 只加载定义，不安装、不写配置、不启动菜单 ──────
cqs_main() {
    case "${1:-}" in
        -h|--help)
            printf '%s\n' "CqShell - Debian/Ubuntu 系统工具箱" \
                "用法: cqs [--help | --version | --install | --uninstall]" \
                "不带参数进入菜单；--install 安装 /usr/local/bin/cqs。"
            return 0 ;;
        --version) printf '%s %s\n' "$CQS_NAME" "$CQS_VERSION"; return 0 ;;
        --install) install_shortcut; return $? ;;
        --uninstall) uninstall_shortcut; return $? ;;
        "") ;;
        *) echo "无效参数，请使用 cqs --help" >&2; return 2 ;;
    esac
    user_agreement || return 1
    if [[ $EUID -eq 0 ]]; then install_shortcut || return 1; fi
    cqs_menu
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    cqs_main "$@"
fi

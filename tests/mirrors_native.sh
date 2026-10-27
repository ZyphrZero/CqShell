#!/usr/bin/env bash
# Explicit Linux integration test: real LinuxMirrors and APT in private mounts.
# Keeps the temporary directory for inspecting logs; never upgrades packages.
set -euo pipefail
[[ $EUID == 0 && $# == 2 ]] || { echo "Usage: sudo bash tests/mirrors_native.sh /absolute/cqs /absolute/cqs.sh" >&2; exit 2; }
binary=$(realpath "$1")
shell_script=$(realpath "$2")
work=$(mktemp -d /tmp/cqs-native-mirrors.XXXXXXXX)
printf '%s\n' "Native test artifacts: $work"
mkdir -p "$work/apt/sources.list.d" "$work/apt/apt.conf.d" "$work/apt/trusted.gpg.d" "$work/lib/lists/partial" "$work/cache/archives/partial" "$work/log"
chmod 755 "$work/apt" "$work/lib" "$work/cache" "$work/log"
# This test intentionally targets only the local Debian 13 validation host.
grep -qx 'ID=debian' /etc/os-release
grep -Eq '^VERSION_CODENAME="?trixie"?$' /etc/os-release
printf '%s\n' 'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] https://mirrors.ustc.edu.cn/debian trixie main' > "$work/apt/sources.list"
cp "$work/apt/sources.list" "$work/original.list"
unshare --mount --propagation private bash -s -- "$work" "$binary" "$shell_script" <<'NATIVE_TEST'
set -euo pipefail
work=$1
binary=$2
shell_script=$3
mount --bind "$work/apt" /etc/apt
mount --bind "$work/lib" /var/lib/apt
mount --bind "$work/cache" /var/cache/apt
mount --bind "$work/log" /var/log/apt
export CQS_KERNEL_LOCK="$work/package.lock"
"$binary" --lang en-US --yes run system.mirror.apply --set mirror_site=aliyun > "$work/apply.log" 2>&1
# The pinned generator migrates Debian 13's legacy input to DEB822.
grep -q 'https://mirrors.aliyun.com/debian' /etc/apt/sources.list.d/debian.sources
test -f /etc/apt/.cqs-mirrors/last.json
test ! -e /etc/apt/.cqs-mirrors/pending.json
bash "$shell_script" --lang en-US --yes run system.mirror.restore > "$work/restore.log" 2>&1
cmp /etc/apt/sources.list "$work/original.list"
test ! -e /etc/apt/.cqs-mirrors/last.json
test ! -e /etc/apt/.cqs-mirrors/pending.json
NATIVE_TEST
printf '%s\n' 'Native mirror apply and cross-implementation restore passed.'

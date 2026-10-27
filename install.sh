#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Install a prebuilt CqShell binary for Linux x86_64 or ARM64.

Usage: bash install.sh [--version vX.Y.Z] [--install-dir /absolute/path] [--gh-proxy HTTPS_URL]

  --version TAG       Install an exact release (default: latest stable release).
  --install-dir DIR   Install as DIR/cqs and skip the location prompt.
  --gh-proxy URL      Use a trusted HTTPS GitHub download prefix (default: GH_PROXY).
  -h, --help          Show this help.

Interactive installs offer /usr/local/bin, ~/.local/bin, and a custom path; Enter keeps /usr/local/bin.
Non-interactive installs keep /usr/local/bin. Use sudo for system installation.
CQS_RELEASE_BASE_URL may select a trusted HTTPS mirror with GitHub's release layout.
--gh-proxy overrides GH_PROXY and covers version lookup, archives and checksums.
Pass --gh-proxy explicitly through sudo; sudo may discard GH_PROXY.
EOF
}

die() {
    printf 'cqs installer: %s\n' "$*" >&2
    exit 1
}

choose_install_dir() {
    # Keep scripted installs non-interactive while making a terminal install
    # explicit about the system-wide and per-user locations.
    local input_fd=0 output_fd=1 user_dir="${HOME:-}/.local/bin" choice custom passwd_entry
    local user_install=''
    if [[ ! -t 0 || ! -t 1 ]]; then
        if ! { exec 3<>/dev/tty; } 2>/dev/null; then
            return 0
        fi
        input_fd=3
        output_fd=3
    fi
    if [[ -n "${SUDO_USER:-}" && "${EUID:-$(id -u)}" == 0 ]] && command -v getent >/dev/null 2>&1; then
        passwd_entry=$(getent passwd "$SUDO_USER" || true)
        if [[ -n "$passwd_entry" ]]; then
            IFS=: read -r _ _ _ _ _ user_dir _ <<< "$passwd_entry"
            user_dir="$user_dir/.local/bin"
            user_install="$SUDO_USER"
        fi
    fi
    printf '\nChoose an installation directory:\n' >&"$output_fd"
    printf '  1) /usr/local/bin (system-wide, default)\n' >&"$output_fd"
    printf '  2) %s (current user)\n' "$user_dir" >&"$output_fd"
    printf '  3) Enter a custom absolute path\n' >&"$output_fd"
    while :; do
        printf 'Selection [1]: ' >&"$output_fd"
        if ((input_fd == 3)); then
            IFS= read -r choice <&3 || { exec 3>&-; return 0; }
        else
            IFS= read -r choice || return 0
        fi
        case "${choice:-1}" in
            1)
                install_dir='/usr/local/bin'
                if ((input_fd == 3)); then exec 3>&-; fi
                return 0
                ;;
            2)
                install_dir="$user_dir"
                install_user="$user_install"
                if ((input_fd == 3)); then exec 3>&-; fi
                return 0
                ;;
            3)
                printf 'Custom installation directory: ' >&"$output_fd"
                if ((input_fd == 3)); then
                    IFS= read -r custom <&3 || die 'Cannot read the installation directory.'
                else
                    IFS= read -r custom || die 'Cannot read the installation directory.'
                fi
                [[ -n "$custom" ]] || { printf 'The installation directory cannot be empty.\n' >&2; continue; }
                install_dir="$custom"
                if ((input_fd == 3)); then exec 3>&-; fi
                return 0
                ;;
            *) printf 'Please choose 1, 2 or 3.\n' >&"$output_fd" ;;
        esac
    done
}

valid_version() {
    [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]
}

release_tag_from_metadata() {
    local metadata="$1" tag remainder
    local object='^[[:space:]]*\{.*\}[[:space:]]*$'
    local key='(^|[,{[:space:]])"tag_name"[[:space:]]*:'
    local value='^[[:space:]]*"([^"\\]*)"'
    local separator='^[[:space:]]*[,}]'
    # GitHub emits an unescaped tag_name. Extract only that bounded field, never
    # interpret remote data as shell code or add a Python/jq runtime dependency.
    [[ "$metadata" =~ $object && "$metadata" =~ $key ]] || return 1
    remainder="${metadata#*"${BASH_REMATCH[0]}"}"
    [[ "$remainder" =~ $value ]] || return 1
    tag="${BASH_REMATCH[1]}"
    remainder="${remainder#"${BASH_REMATCH[0]}"}"
    [[ "$remainder" =~ $separator && ! "$remainder" =~ $key ]] && valid_version "$tag" || return 1
    printf '%s\n' "$tag"
}

download() {
    curl --disable --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --connect-timeout 15 --max-time 300 --retry 3 "$@"
}

work_dir=''
staged_binary=''
cleanup() {
    local status=$?
    if [[ -n "$staged_binary" ]]; then
        rm -f -- "$staged_binary"
    fi
    if [[ -n "$work_dir" ]]; then
        rm -f -- "$work_dir/archive.tar.gz" "$work_dir/archive.sha256" "$work_dir/cqs"
        rmdir -- "$work_dir"
    fi
    return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

main() {
    local version='' install_dir='/usr/local/bin' install_dir_explicit=0 install_user='' gh_proxy="${GH_PROXY:-}"
    local release_base="${CQS_RELEASE_BASE_URL:-https://github.com/ZyphrZero/CqShell/releases}"
    local command_name target asset resolved checksum_line expected checksum_name extra actual_version
    local metadata github_api='' download_base proxy_port
    local proxy_pattern='^https://([[:alnum:]][[:alnum:].-]*|\[[[:xdigit:]:.]+\])(:[0-9]{1,5})?(/[[:alnum:]_.~%/-]*)?$'
    local github_pattern='^https://github\.com/([[:alnum:]][[:alnum:]_.-]*)/([[:alnum:]][[:alnum:]_.-]*)/releases$'

    while (($#)); do
        case "$1" in
            --version)
                if (($# < 2)) || [[ -z "$2" ]]; then
                    die '--version requires a release tag.'
                fi
                version="$2"
                valid_version "$version" || die 'Expected a release tag such as v0.1.0 or v0.2.0-rc.1.'
                shift 2
                ;;
            --install-dir)
                if (($# < 2)) || [[ -z "$2" ]]; then
                    die '--install-dir requires an absolute path.'
                fi
                install_dir="$2"
                install_dir_explicit=1
                shift 2
                ;;
            --gh-proxy)
                if (($# < 2)) || [[ -z "$2" ]]; then
                    die '--gh-proxy requires a trusted HTTPS download prefix.'
                fi
                gh_proxy="$2"
                shift 2
                ;;
            -h|--help) usage; return ;;
            *) die "Unknown argument: $1 (see --help)." ;;
        esac
    done

    if ((install_dir_explicit == 0)); then
        choose_install_dir
    fi

    [[ "$install_dir" == /* && "$install_dir" != *$'\n'* ]] || die '--install-dir must be an absolute path.'
    release_base="${release_base%/}"
    [[ "$release_base" == https://?* && ! "$release_base" =~ [[:space:]\?\#@] ]] \
        || die 'CQS_RELEASE_BASE_URL must be an HTTPS URL without credentials, query or fragment.'
    if [[ -n "$gh_proxy" ]]; then
        [[ "$gh_proxy" =~ $proxy_pattern ]] \
            || die '--gh-proxy/GH_PROXY must be an HTTPS URL without credentials, query, fragment or whitespace.'
        if [[ -n "${BASH_REMATCH[2]:-}" ]]; then
            proxy_port="${BASH_REMATCH[2]#:}"
            ((10#$proxy_port > 0 && 10#$proxy_port <= 65535)) || die '--gh-proxy/GH_PROXY contains an invalid port.'
        fi
        while [[ "$gh_proxy" == */ ]]; do gh_proxy="${gh_proxy%/}"; done
        gh_proxy="$gh_proxy/"
        [[ "$release_base" =~ $github_pattern ]] \
            || die '--gh-proxy requires a GitHub releases URL; use CQS_RELEASE_BASE_URL alone for other mirrors.'
        github_api="https://api.github.com/repos/${BASH_REMATCH[1]}/${BASH_REMATCH[2]}/releases/latest"
        printf 'Using the configured GitHub download proxy.\n'
    fi
    download_base="$gh_proxy$release_base"
    [[ "$(uname -s)" == Linux ]] || die 'Only Linux is supported; use Debian / Ubuntu or WSL.'
    case "$(uname -m)" in
        x86_64|amd64) target='x86_64-unknown-linux-musl' ;;
        aarch64|arm64) target='aarch64-unknown-linux-musl' ;;
        *) die 'Unsupported architecture. Published binaries support x86_64 and ARM64.' ;;
    esac
    for command_name in curl tar gzip sha256sum mktemp chmod mkdir install mv rm rmdir; do
        command -v "$command_name" >/dev/null 2>&1 || die "Missing dependency: $command_name"
    done

    if [[ -z "$version" ]]; then
        # Resolve latest once so both downloads use the same version tag.
        if [[ -n "$gh_proxy" ]]; then
            # Download proxies commonly reject GitHub HTML pages, including
            # releases/latest. Use the proxied API instead of that redirect.
            metadata=$(download --max-time 60 --max-filesize 1048576 \
                --header 'Accept: application/vnd.github+json' "$gh_proxy$github_api") \
                || die 'Cannot resolve the latest release through the GitHub proxy. Check its API support or supply --version vX.Y.Z.'
            version=$(release_tag_from_metadata "$metadata") \
                || die 'Invalid release metadata from the GitHub proxy. Supply --version vX.Y.Z or use another trusted proxy.'
        else
            resolved=$(download --output /dev/null --write-out '%{url_effective}' "$release_base/latest") \
                || die 'Cannot resolve the latest release. Check connectivity, use --gh-proxy HTTPS_URL, or supply --version vX.Y.Z.'
            [[ "$resolved" == "$release_base/tag/"* ]] || die "Unexpected release redirect: $resolved"
            version="${resolved#"$release_base/tag/"}"
            valid_version "$version" || die "Invalid release tag: $version"
        fi
    fi
    asset="cqs-${target}.tar.gz"
    printf 'Downloading CqShell %s (%s)...\n' "$version" "$target"
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/cqs-install.XXXXXXXX")
    download --output "$work_dir/archive.tar.gz" "$download_base/download/$version/$asset" \
        || die "Cannot download $asset for $version. Check that this release has Linux binary assets."
    download --output "$work_dir/archive.sha256" "$download_base/download/$version/$asset.sha256" \
        || die "Cannot download the checksum for $asset."

    checksum_line=$(<"$work_dir/archive.sha256")
    [[ "$checksum_line" != *$'\n'* ]] || die 'Expected exactly one checksum entry.'
    read -r expected checksum_name extra <<< "$checksum_line"
    [[ "$expected" =~ ^[[:xdigit:]]{64}$ && -z "$extra" ]] || die 'Invalid SHA-256 checksum file.'
    [[ "$checksum_name" == "$asset" || "$checksum_name" == "*$asset" ]] || die 'Checksum filename does not match the requested asset.'
    # Use our own filename; never let a downloaded checksum choose a local path.
    printf '%s  archive.tar.gz\n' "$expected" | (cd "$work_dir" && sha256sum --check --status -) \
        || die 'SHA-256 verification failed; the existing installation was not changed.'

    # Extract only to stdout so archive paths cannot write outside the temporary directory.
    tar --extract --gzip --to-stdout --file "$work_dir/archive.tar.gz" -- cqs/cqs > "$work_dir/cqs" \
        || die 'Cannot extract cqs/cqs from the release archive.'
    [[ -s "$work_dir/cqs" ]] || die 'The release archive does not contain a nonempty executable.'
    chmod 755 "$work_dir/cqs"
    actual_version=$("$work_dir/cqs" --version) || die 'The downloaded program cannot run on this system.'
    [[ "$actual_version" == "cqs ${version#v}" ]] || die "Binary version mismatch: expected cqs ${version#v}, got $actual_version"

    if [[ -n "$install_user" && ! -d "$install_dir" ]]; then
        local user_uid user_gid parent_dir
        user_uid=$(id -u "$install_user") || die 'Cannot resolve the selected user account.'
        user_gid=$(id -g "$install_user") || die 'Cannot resolve the selected user group.'
        parent_dir="${install_dir%/*}"
        mkdir -p -- "$parent_dir" || die 'Cannot create the user installation directory.'
        install -d -m 755 -o "$user_uid" -g "$user_gid" -- "$install_dir" \
            || die 'Cannot create the user installation directory.'
    else
        mkdir -p -- "$install_dir" || die 'Cannot create the installation directory. Use sudo or a writable --install-dir.'
    fi
    [[ -w "$install_dir" ]] || die 'Installation directory is not writable. Use sudo or a writable --install-dir (see --help).'
    [[ ! -d "$install_dir/cqs" ]] || die "Destination is a directory: $install_dir/cqs"
    # Stage on the destination filesystem, then atomically replace the old binary (including symlinks).
    staged_binary=$(mktemp "$install_dir/.cqs.XXXXXXXX")
    install -m 755 -- "$work_dir/cqs" "$staged_binary"
    mv -fT -- "$staged_binary" "$install_dir/cqs"
    staged_binary=''
    printf 'Installed %s to %s/cqs\n' "$actual_version" "$install_dir"
    if [[ "$(command -v cqs)" == "$install_dir/cqs" ]]; then
        printf 'Run: cqs --lang zh-CN\n'
    else
        printf 'Run: %q --lang zh-CN\n' "$install_dir/cqs"
    fi
    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *) printf 'Add this directory to PATH: %s\n' "$install_dir" ;;
    esac
}

main "$@"

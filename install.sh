#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Install a prebuilt CqShell binary for Linux x86_64 or ARM64.

Usage: bash install.sh [--version vX.Y.Z] [--install-dir /absolute/path]

  --version TAG       Install an exact release (default: latest stable release).
  --install-dir DIR   Install as DIR/cqs (default: /root).
  -h, --help          Show this help.

Use sudo for system installation, or --install-dir "$HOME/.local/bin".
CQS_RELEASE_BASE_URL may select a trusted HTTPS mirror with GitHub's release layout.
EOF
}

die() {
    printf 'cqs installer: %s\n' "$*" >&2
    exit 1
}

valid_version() {
    [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$ ]]
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
    local version='' install_dir='/root'
    local release_base="${CQS_RELEASE_BASE_URL:-https://github.com/ZyphrZero/CqShell/releases}"
    local command_name target asset resolved checksum_line expected checksum_name extra actual_version

    while (($#)); do
        case "$1" in
            --version)
                (($# >= 2)) && [[ -n "$2" ]] || die '--version requires a release tag.'
                version="$2"
                valid_version "$version" || die 'Expected a release tag such as v0.1.0 or v0.2.0-rc.1.'
                shift 2
                ;;
            --install-dir)
                (($# >= 2)) && [[ -n "$2" ]] || die '--install-dir requires an absolute path.'
                install_dir="$2"
                shift 2
                ;;
            -h|--help) usage; return ;;
            *) die "Unknown argument: $1 (see --help)." ;;
        esac
    done

    [[ "$install_dir" == /* && "$install_dir" != *$'\n'* ]] || die '--install-dir must be an absolute path.'
    release_base="${release_base%/}"
    [[ "$release_base" == https://?* && ! "$release_base" =~ [[:space:]\?\#@] ]] \
        || die 'CQS_RELEASE_BASE_URL must be an HTTPS URL without credentials, query or fragment.'
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
        resolved=$(download --output /dev/null --write-out '%{url_effective}' "$release_base/latest") \
            || die 'Cannot resolve the latest release. Check connectivity and whether a stable release is published.'
        [[ "$resolved" == "$release_base/tag/"* ]] || die "Unexpected release redirect: $resolved"
        version="${resolved#"$release_base/tag/"}"
        valid_version "$version" || die "Invalid release tag: $version"
    fi
    asset="cqs-${target}.tar.gz"
    printf 'Downloading CqShell %s (%s)...\n' "$version" "$target"
    work_dir=$(mktemp -d "${TMPDIR:-/tmp}/cqs-install.XXXXXXXX")
    download --output "$work_dir/archive.tar.gz" "$release_base/download/$version/$asset" \
        || die "Cannot download $asset for $version. Check that this release has Linux binary assets."
    download --output "$work_dir/archive.sha256" "$release_base/download/$version/$asset.sha256" \
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

    mkdir -p -- "$install_dir" || die 'Cannot create the installation directory. Use sudo or a writable --install-dir.'
    [[ -w "$install_dir" ]] || die 'Installation directory is not writable. Use sudo or a writable --install-dir (see --help).'
    [[ ! -d "$install_dir/cqs" ]] || die "Destination is a directory: $install_dir/cqs"
    # Stage on the destination filesystem, then atomically replace the old binary (including symlinks).
    staged_binary=$(mktemp "$install_dir/.cqs.XXXXXXXX")
    install -m 755 -- "$work_dir/cqs" "$staged_binary"
    mv -fT -- "$staged_binary" "$install_dir/cqs"
    staged_binary=''
    printf 'Installed %s to %s/cqs\n' "$actual_version" "$install_dir"
    printf 'Run: %q --lang zh-CN\n' "$install_dir/cqs"
    case ":$PATH:" in
        *":$install_dir:"*) ;;
        *) printf 'Add this directory to PATH: %s\n' "$install_dir" ;;
    esac
}

main "$@"

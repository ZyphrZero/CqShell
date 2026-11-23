//! Check for new CqShell releases on GitHub (`ZyphrZero/CqShell`) and perform verified,
//! atomic self-updates of the installed `cqs` binary.

use crate::{
    error::{Error, Result},
    runtime::{Context, path, sanitize, sha256},
    storage::{self, Lock},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    cmp::Ordering,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

/// Canonical GitHub repository hosting CqShell releases.
pub const REPOSITORY: &str = "ZyphrZero/CqShell";
/// Canonical GitHub repository homepage.
pub const REPO_URL: &str = "https://github.com/ZyphrZero/CqShell";
/// Default GitHub releases base URL (also overridable via `CQS_RELEASE_BASE_URL`).
pub const DEFAULT_RELEASE_BASE: &str = "https://github.com/ZyphrZero/CqShell/releases";
/// Version compiled into this binary.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_API_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 4 * 1024;
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Architecture {
    X64,
    Arm64,
}

impl Architecture {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "x86_64" | "amd64" => Ok(Self::X64),
            "aarch64" | "arm64" => Ok(Self::Arm64),
            _ => Err(Error::new("error.self_update_arch", value)),
        }
    }

    pub(crate) fn current() -> Result<Self> {
        Self::parse(std::env::consts::ARCH)
    }

    pub(crate) fn target(self) -> &'static str {
        match self {
            Self::X64 => "x86_64-unknown-linux-musl",
            Self::Arm64 => "aarch64-unknown-linux-musl",
        }
    }

    pub(crate) fn asset_name(self) -> String {
        format!("cqs-{}.tar.gz", self.target())
    }

    pub(crate) fn checksum_asset_name(self) -> String {
        format!("cqs-{}.tar.gz.sha256", self.target())
    }

    pub(crate) fn elf_machine(self) -> u16 {
        match self {
            Self::X64 => 62,
            Self::Arm64 => 183,
        }
    }
}

/// SemVer 2.0.0 version for accurate release ordering (including pre-releases).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemVer {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<String>,
    pub build: Option<String>,
}

impl SemVer {
    pub fn parse(raw: &str) -> Result<Self> {
        let stripped = raw.strip_prefix('v').unwrap_or(raw);
        if stripped.is_empty()
            || stripped
                .chars()
                .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '/' | '\\'))
        {
            return Err(Error::new("error.release", raw));
        }
        let (main_and_pre, build) = match stripped.split_once('+') {
            Some((left, right)) => {
                if right.is_empty() || !valid_dotted_ident(right) {
                    return Err(Error::new("error.release", raw));
                }
                (left, Some(right.to_owned()))
            }
            None => (stripped, None),
        };
        let (core, pre) = match main_and_pre.split_once('-') {
            Some((left, right)) => {
                if right.is_empty() || !valid_dotted_ident(right) {
                    return Err(Error::new("error.release", raw));
                }
                (left, right.split('.').map(str::to_owned).collect())
            }
            None => (main_and_pre, Vec::new()),
        };
        let parts: Vec<_> = core.split('.').collect();
        if parts.len() != 3 {
            return Err(Error::new("error.release", raw));
        }
        let parse_num = |segment: &str| -> Result<u64> {
            if segment.is_empty()
                || segment.len() > 12
                || !segment.bytes().all(|b| b.is_ascii_digit())
                || (segment.len() > 1 && segment.starts_with('0'))
            {
                return Err(Error::new("error.release", raw));
            }
            segment
                .parse::<u64>()
                .map_err(|_| Error::new("error.release", raw))
        };
        Ok(Self {
            major: parse_num(parts[0])?,
            minor: parse_num(parts[1])?,
            patch: parse_num(parts[2])?,
            pre,
            build,
        })
    }

    pub fn canonical_version(&self) -> String {
        let mut out = format!("{}.{}.{}", self.major, self.minor, self.patch);
        if !self.pre.is_empty() {
            out.push('-');
            out.push_str(&self.pre.join("."));
        }
        if let Some(build) = &self.build {
            out.push('+');
            out.push_str(build);
        }
        out
    }

    pub fn canonical_tag(&self) -> String {
        format!("v{}", self.canonical_version())
    }
}

fn valid_dotted_ident(value: &str) -> bool {
    value.split('.').all(|part| {
        !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(other.pre.iter()) {
                        let a_num = a.bytes().all(|c| c.is_ascii_digit());
                        let b_num = b.bytes().all(|c| c.is_ascii_digit());
                        let ord = match (a_num, b_num) {
                            (true, true) => a
                                .parse::<u64>()
                                .unwrap_or(u64::MAX)
                                .cmp(&b.parse::<u64>().unwrap_or(u64::MAX)),
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            (false, false) => a.cmp(b),
                        };
                        if ord != Ordering::Equal {
                            return ord;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Validates and normalizes a release tag such as `v0.1.5` or `0.1.5` to `v0.1.5`.
pub fn parse_version_tag(raw: &str) -> Result<String> {
    Ok(SemVer::parse(raw)?.canonical_tag())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    UpdateAvailable,
    UpToDate,
    Ahead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub target: String,
    pub url: String,
    pub checksum_url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseInfo {
    pub repository: String,
    pub current_version: String,
    pub latest_version: String,
    pub tag: String,
    pub status: UpdateStatus,
    pub update_available: bool,
    pub release_url: String,
    pub published_at: Option<String>,
    pub notes: Option<String>,
    pub asset: Option<ReleaseAsset>,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    pub version: Option<String>,
    pub force: bool,
    pub target_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum UpdateOutcome {
    UpToDate(ReleaseInfo),
    Updated { info: ReleaseInfo, target: PathBuf },
}

struct ReleaseSource {
    repository: String,
    release_base: String,
    api_base: Option<String>,
}

fn resolve_release_source(ctx: &Context) -> Result<ReleaseSource> {
    let raw_base = ctx
        .environment
        .get("CQS_RELEASE_BASE_URL")
        .cloned()
        .or_else(|| std::env::var("CQS_RELEASE_BASE_URL").ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_BASE.to_owned());
    let base = validate_release_base(&raw_base)?;
    if let Some((owner, repo)) = parse_github_releases_base(&base) {
        Ok(ReleaseSource {
            repository: format!("{owner}/{repo}"),
            api_base: Some(format!(
                "https://api.github.com/repos/{owner}/{repo}/releases"
            )),
            release_base: base,
        })
    } else {
        Ok(ReleaseSource {
            repository: REPOSITORY.to_owned(),
            api_base: None,
            release_base: base,
        })
    }
}

fn validate_release_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty()
        || !trimmed.starts_with("https://")
        || trimmed
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '?' | '#' | '@' | '\\'))
    {
        return Err(Error::new("error.url", raw));
    }
    let parsed = url::Url::parse(trimmed).map_err(|e| Error::new("error.url", e))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(Error::new("error.url", raw));
    }
    Ok(trimmed.to_owned())
}

fn parse_github_releases_base(base: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(base).ok()?;
    if !parsed.host_str()?.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let segments: Vec<_> = parsed.path_segments()?.collect();
    if segments.len() != 3 || segments[2] != "releases" {
        return None;
    }
    let valid_part = |s: &str| {
        !s.is_empty()
            && !matches!(s, "." | "..")
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if valid_part(segments[0]) && valid_part(segments[1]) {
        Some((segments[0].to_owned(), segments[1].to_owned()))
    } else {
        None
    }
}

fn compare_versions(current: &str, latest: &str) -> Result<UpdateStatus> {
    let cur = SemVer::parse(current)?;
    let lat = SemVer::parse(latest)?;
    Ok(match lat.cmp(&cur) {
        Ordering::Greater => UpdateStatus::UpdateAvailable,
        Ordering::Equal => UpdateStatus::UpToDate,
        Ordering::Less => UpdateStatus::Ahead,
    })
}

fn validate_digest(value: &str) -> Result<String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::new("error.release", "SHA-256"));
    }
    Ok(value.to_ascii_lowercase())
}

fn sanitize_notes(raw: Option<&str>) -> Option<String> {
    let text = sanitize(raw?);
    let mut lines = Vec::new();
    let mut total = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if lines.len() >= 6 || total >= 400 {
            lines.push("...".to_owned());
            break;
        }
        total += trimmed.len();
        lines.push(trimmed.to_owned());
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn canonical_asset(release_base: &str, tag: &str, arch: Architecture) -> ReleaseAsset {
    let name = arch.asset_name();
    let checksum_name = arch.checksum_asset_name();
    ReleaseAsset {
        url: format!("{release_base}/download/{tag}/{name}"),
        checksum_url: format!("{release_base}/download/{tag}/{checksum_name}"),
        name,
        target: arch.target().to_owned(),
        size: None,
        sha256: None,
    }
}

fn parse_github_release(
    release: &Value,
    repository: &str,
    release_base: &str,
    current_version: &str,
    requested_tag: Option<&str>,
    arch: Option<Architecture>,
) -> Result<ReleaseInfo> {
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| Error::new("error.release", "tag_name"))?;
    let parsed_tag = SemVer::parse(tag)?;
    let canonical_tag = parsed_tag.canonical_tag();
    if tag != canonical_tag {
        return Err(Error::new("error.release", tag));
    }
    if release["draft"] != false {
        return Err(Error::new("error.release", tag));
    }
    if requested_tag.is_none() && release["prerelease"] != false {
        return Err(Error::new("error.release", tag));
    }
    if let Some(expected) = requested_tag
        && tag != expected
    {
        return Err(Error::new(
            "error.release",
            format!("expected {expected}, got {tag}"),
        ));
    }

    let latest_version = parsed_tag.canonical_version();
    let status = compare_versions(current_version, &latest_version)?;
    let expected_release_url = format!("{release_base}/tag/{tag}");
    let release_url = release["html_url"]
        .as_str()
        .filter(|url| *url == expected_release_url)
        .map(str::to_owned)
        .unwrap_or(expected_release_url);
    let published_at = release["published_at"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(sanitize);
    let notes = sanitize_notes(release["body"].as_str());

    let asset = if let Some(arch) = arch {
        let mut info = canonical_asset(release_base, tag, arch);
        if let Some(assets) = release["assets"].as_array() {
            let matching: Vec<_> = assets.iter().filter(|a| a["name"] == info.name).collect();
            if matching.len() > 1 {
                return Err(Error::new("error.release", &info.name));
            }
            if let Some(item) = matching.first() {
                if let Some(download_url) = item["browser_download_url"].as_str()
                    && download_url != info.url
                {
                    return Err(Error::new("error.release", &info.name));
                }
                if let Some(size) = item["size"].as_u64() {
                    if !(1..=MAX_ARCHIVE_BYTES).contains(&size) {
                        return Err(Error::new("error.release", "asset size"));
                    }
                    info.size = Some(size);
                }
                if let Some(digest_raw) = item["digest"]
                    .as_str()
                    .and_then(|value| value.strip_prefix("sha256:"))
                {
                    info.sha256 = Some(validate_digest(digest_raw)?);
                }
            }
        }
        Some(info)
    } else {
        None
    };

    Ok(ReleaseInfo {
        repository: repository.to_owned(),
        current_version: current_version.to_owned(),
        latest_version,
        tag: tag.to_owned(),
        status,
        update_available: status == UpdateStatus::UpdateAvailable,
        release_url,
        published_at,
        notes,
        asset,
        source: "github_api".to_owned(),
    })
}

fn parse_redirect_tag(effective_url: &str, release_base: &str) -> Result<String> {
    let prefix = format!("{release_base}/tag/");
    let raw_tag = effective_url
        .trim()
        .strip_prefix(&prefix)
        .ok_or_else(|| Error::new("error.release", effective_url))?;
    let raw_tag = raw_tag.split(['?', '#']).next().unwrap_or(raw_tag);
    let parsed = SemVer::parse(raw_tag)?;
    let canonical = parsed.canonical_tag();
    if raw_tag != canonical {
        return Err(Error::new("error.release", raw_tag));
    }
    Ok(canonical)
}

/// Parses a standard `sha256sum` file containing exactly one entry matching `asset_name`.
pub(crate) fn parse_checksum_file(text: &str, asset_name: &str) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return Err(Error::new("error.release", "checksum format"));
    }
    let mut parts = trimmed.split_whitespace();
    let hash = parts
        .next()
        .ok_or_else(|| Error::new("error.release", "checksum hash"))?;
    let name = parts
        .next()
        .ok_or_else(|| Error::new("error.release", "checksum filename"))?;
    if parts.next().is_some() {
        return Err(Error::new("error.release", "checksum extra tokens"));
    }
    let normalized_name = name.strip_prefix('*').unwrap_or(name);
    if normalized_name != asset_name {
        return Err(Error::new("error.release", name));
    }
    validate_digest(hash)
}

fn curl_fetch_bytes(
    ctx: &Context,
    url: &str,
    accept: Option<&str>,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let user_agent = format!("CqShell/{CURRENT_VERSION}");
    let max_size = max_bytes.to_string();
    let mut args = vec![
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
        "--max-time",
        "30",
        "--max-filesize",
        &max_size,
        "--user-agent",
        &user_agent,
    ];
    if let Some(header) = accept {
        args.extend(["--header", header]);
    }
    args.push(url);
    let out = ctx.command("curl", &args, false)?;
    if !out.status.success() {
        return Err(Error::new(
            "error.command",
            format!(
                "curl {}: {}\n{}",
                url,
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    if out.stdout.is_empty() || out.stdout.len() as u64 > max_bytes {
        return Err(Error::new("error.release", url));
    }
    Ok(out.stdout)
}

fn curl_resolve_redirect(ctx: &Context, url: &str) -> Result<String> {
    let user_agent = format!("CqShell/{CURRENT_VERSION}");
    let null_sink = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let args = [
        "--disable",
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--head",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--connect-timeout",
        "15",
        "--max-time",
        "30",
        "--user-agent",
        &user_agent,
        "--output",
        null_sink,
        "--write-out",
        "%{url_effective}",
        url,
    ];
    let out = ctx.command("curl", &args, false)?;
    if !out.status.success() {
        return Err(Error::new(
            "error.command",
            format!(
                "curl {}: {}\n{}",
                url,
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    String::from_utf8(out.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|e| Error::new("error.data", e))
}

fn fetch_with_optional_proxy<T>(
    ctx: &Context,
    url: &str,
    quiet: bool,
    mut fetch: impl FnMut(&str) -> Result<T>,
) -> Result<T> {
    if quiet {
        let effective = crate::github_proxy::rewrite(url, ctx.github_proxy.as_deref());
        match fetch(&effective) {
            Ok(value) => Ok(value),
            Err(error)
                if error.key != "error.cancelled"
                    && effective != url
                    && crate::github_proxy::is_github_api_url(url) =>
            {
                fetch(url)
            }
            Err(error) => Err(error),
        }
    } else {
        ctx.with_github_url(url, fetch)
    }
}

/// Checks GitHub Releases for `ZyphrZero/CqShell` (or `CQS_RELEASE_BASE_URL`) and compares
/// against the running version. Works on Linux, Windows, and macOS without root privileges.
pub fn check(ctx: &Context, requested_version: Option<&str>, quiet: bool) -> Result<ReleaseInfo> {
    ctx.check()?;
    ctx.require(&["curl"])?;
    let source = resolve_release_source(ctx)?;
    let requested_tag = requested_version.map(parse_version_tag).transpose()?;
    let arch = Architecture::current().ok();

    if let Some(api_base) = &source.api_base {
        let api_url = match &requested_tag {
            Some(tag) => format!("{api_base}/tags/{tag}"),
            None => format!("{api_base}/latest"),
        };
        match fetch_with_optional_proxy(ctx, &api_url, quiet, |effective| {
            curl_fetch_bytes(
                ctx,
                effective,
                Some("Accept: application/vnd.github+json"),
                MAX_API_BYTES,
            )
        }) {
            Ok(bytes) => {
                let json: Value = serde_json::from_slice(&bytes)?;
                return parse_github_release(
                    &json,
                    &source.repository,
                    &source.release_base,
                    CURRENT_VERSION,
                    requested_tag.as_deref(),
                    arch,
                );
            }
            Err(err) if err.key == "error.cancelled" => return Err(err),
            Err(api_err) => {
                // Fall back when the GitHub REST API is rate-limited or rejected by a proxy.
                if let Some(tag) = &requested_tag {
                    let tag_url = format!("{}/tag/{tag}", source.release_base);
                    let effective = curl_resolve_redirect(ctx, &tag_url).map_err(|_| api_err)?;
                    let verified_tag = parse_redirect_tag(&effective, &source.release_base)?;
                    if verified_tag != *tag {
                        return Err(Error::new(
                            "error.release",
                            format!("expected {tag}, got {verified_tag}"),
                        ));
                    }
                    let latest_version = SemVer::parse(&verified_tag)?.canonical_version();
                    let status = compare_versions(CURRENT_VERSION, &latest_version)?;
                    return Ok(ReleaseInfo {
                        repository: source.repository,
                        current_version: CURRENT_VERSION.to_owned(),
                        latest_version,
                        tag: verified_tag.clone(),
                        status,
                        update_available: status == UpdateStatus::UpdateAvailable,
                        release_url: format!("{}/tag/{verified_tag}", source.release_base),
                        published_at: None,
                        notes: None,
                        asset: arch
                            .map(|a| canonical_asset(&source.release_base, &verified_tag, a)),
                        source: "github_redirect".to_owned(),
                    });
                }
                let latest_url = format!("{}/latest", source.release_base);
                let effective = curl_resolve_redirect(ctx, &latest_url).map_err(|_| api_err)?;
                let tag = parse_redirect_tag(&effective, &source.release_base)?;
                let latest_version = SemVer::parse(&tag)?.canonical_version();
                let status = compare_versions(CURRENT_VERSION, &latest_version)?;
                return Ok(ReleaseInfo {
                    repository: source.repository,
                    current_version: CURRENT_VERSION.to_owned(),
                    latest_version,
                    tag: tag.clone(),
                    status,
                    update_available: status == UpdateStatus::UpdateAvailable,
                    release_url: format!("{}/tag/{tag}", source.release_base),
                    published_at: None,
                    notes: None,
                    asset: arch.map(|a| canonical_asset(&source.release_base, &tag, a)),
                    source: "github_redirect".to_owned(),
                });
            }
        }
    }

    // Non-GitHub mirror layout (`CQS_RELEASE_BASE_URL` pointing to a custom HTTPS mirror).
    let tag = match requested_tag {
        Some(tag) => tag,
        None => {
            let effective = curl_resolve_redirect(ctx, &format!("{}/latest", source.release_base))?;
            parse_redirect_tag(&effective, &source.release_base)?
        }
    };
    let latest_version = SemVer::parse(&tag)?.canonical_version();
    let status = compare_versions(CURRENT_VERSION, &latest_version)?;
    Ok(ReleaseInfo {
        repository: source.repository,
        current_version: CURRENT_VERSION.to_owned(),
        latest_version,
        tag: tag.clone(),
        status,
        update_available: status == UpdateStatus::UpdateAvailable,
        release_url: format!("{}/tag/{tag}", source.release_base),
        published_at: None,
        notes: None,
        asset: arch.map(|a| canonical_asset(&source.release_base, &tag, a)),
        source: "mirror_redirect".to_owned(),
    })
}

/// Checks for updates and prints a bilingual summary (or structured JSON when `json` is true).
pub fn check_and_report(
    ctx: &Context,
    requested_version: Option<&str>,
    json: bool,
) -> Result<ReleaseInfo> {
    let info = check(ctx, requested_version, json)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(info);
    }
    report_info(ctx, &info);
    Ok(info)
}

fn report_info(ctx: &Context, info: &ReleaseInfo) {
    let i = &ctx.i18n;
    ctx.log(i.format(
        "notice.self_update_repo",
        &[(
            "detail",
            &format!(
                "{} (https://github.com/{})",
                info.repository, info.repository
            ),
        )],
    ));
    let mut ver_detail = format!("v{} -> {}", info.current_version, info.tag);
    if let Some(date) = &info.published_at {
        ver_detail.push_str(&format!(" ({date})"));
    }
    match info.status {
        UpdateStatus::UpdateAvailable => {
            ctx.log(i.format("notice.self_update_available", &[("detail", &ver_detail)]));
            ctx.log(i.format("notice.self_update_url", &[("detail", &info.release_url)]));
            ctx.log(i.format("notice.self_update_hint", &[("detail", "cqs update")]));
        }
        UpdateStatus::UpToDate => {
            ctx.log(i.format(
                "notice.self_update_current",
                &[("detail", &format!("v{}", info.current_version))],
            ));
        }
        UpdateStatus::Ahead => {
            ctx.log(i.format("notice.self_update_ahead", &[("detail", &ver_detail)]));
        }
    }
    if let Some(notes) = &info.notes {
        ctx.log(i.format("notice.self_update_notes", &[("detail", notes)]));
    }
}

fn resolve_target_binary(ctx: &Context, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    let explicit_env = ctx
        .environment
        .get("CQS_INSTALL_PATH")
        .filter(|v| !v.is_empty())
        .is_some()
        || std::env::var_os("CQS_INSTALL_PATH").is_some_and(|v| !v.is_empty());
    if explicit_env {
        return Ok(ctx.paths.install.clone());
    }
    let exe = std::env::current_exe()?;
    // If current_exe is a symlink (e.g. via a wrapper symlink), resolve the real target file
    // so `storage::check_path` validates the actual binary path cleanly.
    let resolved = fs::canonicalize(&exe).unwrap_or(exe);
    Ok(resolved)
}

fn ensure_writable_target(target: &Path) -> Result<()> {
    storage::check_path(target, true)?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::new("error.path", target.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let euid = unsafe { libc::geteuid() };
        if let Ok(meta) = fs::metadata(target)
            && euid != 0
            && meta.uid() != euid
        {
            return Err(Error::new("error.root", "sudo cqs update"));
        }
    }
    if parent.exists() {
        match tempfile::NamedTempFile::new_in(parent) {
            Ok(probe) => {
                drop(probe);
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(Error::new("error.root", "sudo cqs update"))
            }
            Err(err) => Err(err.into()),
        }
    } else {
        Ok(())
    }
}

/// Safely extracts only `cqs/cqs` from a release `.tar.gz`, rejecting traversal, links,
/// special files, duplicates, and oversized entries.
pub(crate) fn extract_binary(ctx: &Context, archive_path: &Path, destination: &Path) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(File::open(archive_path)?);
    let mut archive = tar::Archive::new(decoder);
    let mut found = false;
    let mut entries_seen = 0usize;
    for entry in archive.entries()? {
        ctx.check()?;
        entries_seen += 1;
        if entries_seen > 5_000 {
            return Err(Error::new("error.release", "too many archive entries"));
        }
        let mut entry = entry?;
        let raw_path = entry.path()?.into_owned();
        if raw_path.as_os_str().is_empty()
            || raw_path
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
            || raw_path
                .to_string_lossy()
                .chars()
                .any(|c| c.is_control() || matches!(c, '\\' | ':'))
        {
            return Err(Error::new("error.release", raw_path.display()));
        }
        let normalized: PathBuf = raw_path
            .components()
            .filter(|part| !matches!(part, Component::CurDir))
            .collect();
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(Error::new("error.release", raw_path.display()));
        }
        if normalized != Path::new("cqs/cqs") {
            continue;
        }
        if !kind.is_file() || found {
            return Err(Error::new("error.release", "cqs/cqs"));
        }
        let declared_size = entry.size();
        if declared_size == 0 || declared_size > MAX_BINARY_BYTES {
            return Err(Error::new("error.release", "binary size"));
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let mut buf = [0u8; 65_536];
        let mut written = 0u64;
        loop {
            ctx.check()?;
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            written = written
                .checked_add(n as u64)
                .ok_or_else(|| Error::new("error.release", "binary size"))?;
            if written > MAX_BINARY_BYTES {
                return Err(Error::new("error.release", "binary size"));
            }
            file.write_all(&buf[..n])?;
        }
        if written == 0 {
            return Err(Error::new("error.release", "empty binary"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        file.sync_all()?;
        found = true;
    }
    if !found {
        return Err(Error::new("error.release", "missing cqs/cqs"));
    }
    Ok(())
}

pub(crate) fn validate_elf(binary: &Path, arch: Architecture) -> Result<()> {
    let mut header = [0u8; 20];
    File::open(binary)?.read_exact(&mut header)?;
    if &header[..4] != b"\x7fELF"
        || header[4..7] != [2, 1, 1]
        || !matches!(u16::from_le_bytes([header[16], header[17]]), 2 | 3)
        || u16::from_le_bytes([header[18], header[19]]) != arch.elf_machine()
    {
        return Err(Error::new("error.release", binary.display()));
    }
    Ok(())
}

fn verify_binary_version(ctx: &Context, binary: &Path, expected_version: &str) -> Result<()> {
    let output = ctx.output(path(binary)?, &["--version"])?;
    let expected = format!("cqs {expected_version}");
    if output.trim() != expected {
        return Err(Error::new(
            "error.verify",
            format!("expected `{expected}`, got `{}`", output.trim()),
        ));
    }
    Ok(())
}

/// Checks for updates and, when needed (or forced), downloads, verifies (SHA-256, ELF
/// architecture, and `--version`), and atomically replaces the installed `cqs` binary.
pub fn update(ctx: &Context, options: &UpdateOptions) -> Result<UpdateOutcome> {
    ctx.check()?;
    if !cfg!(target_os = "linux") {
        return Err(Error::new(
            "error.self_update_platform",
            std::env::consts::OS,
        ));
    }
    let arch = Architecture::current()?;
    ctx.require(&["curl"])?;

    let target = resolve_target_binary(ctx, options.target_path.as_deref())?;
    ensure_writable_target(&target)?;

    let info = check(ctx, options.version.as_deref(), false)?;
    if !options.force {
        let already_satisfied = match &options.version {
            None => info.status != UpdateStatus::UpdateAvailable,
            Some(_) => info.latest_version == info.current_version,
        };
        if already_satisfied {
            report_info(ctx, &info);
            return Ok(UpdateOutcome::UpToDate(info));
        }
    }

    let asset = info
        .asset
        .as_ref()
        .ok_or_else(|| Error::new("error.self_update_arch", std::env::consts::ARCH))?;

    let _lock = Lock::acquire(&ctx.paths.state.join("self-update.lock"))?;

    let summary = format!(
        "repository: {}\nversion: v{} -> {}\nasset: {}\ntarget: {}",
        info.repository,
        info.current_version,
        info.tag,
        asset.name,
        target.display()
    );
    ctx.confirm("confirm.self_update", &summary, "YES")?;

    let work = tempfile::tempdir()?;
    let checksum_path = work.path().join("archive.sha256");
    let archive_path = work.path().join("archive.tar.gz");
    let extracted_path = work.path().join("cqs");

    ctx.message(
        "notice.self_update_downloading",
        format!("{} ({})", info.tag, asset.target),
    );
    ctx.download(&asset.checksum_url, &checksum_path, 60)?;
    if fs::metadata(&checksum_path)?.len() > MAX_CHECKSUM_BYTES {
        return Err(Error::new("error.release", "checksum file too large"));
    }
    let checksum_text = fs::read_to_string(&checksum_path)?;
    let expected_digest = parse_checksum_file(&checksum_text, &asset.name)?;
    if let Some(api_digest) = &asset.sha256
        && api_digest != &expected_digest
    {
        return Err(Error::new("error.digest", &asset.checksum_url));
    }

    if let Some(size) = asset.size {
        ctx.download_exact(&asset.url, &archive_path, 300, size)?;
    } else {
        ctx.download(&asset.url, &archive_path, 300)?;
        let actual_size = fs::metadata(&archive_path)?.len();
        if actual_size == 0 || actual_size > MAX_ARCHIVE_BYTES {
            return Err(Error::new("error.release", "asset size"));
        }
    }

    let actual_digest = sha256(&archive_path)?;
    if actual_digest != expected_digest {
        return Err(Error::new("error.digest", &asset.url));
    }

    extract_binary(ctx, &archive_path, &extracted_path)?;
    validate_elf(&extracted_path, arch)?;
    verify_binary_version(ctx, &extracted_path, &info.latest_version)?;

    let bytes = fs::read(&extracted_path)?;
    storage::atomic_write(&target, &bytes, 0o755)?;
    ctx.message(
        "notice.self_updated",
        format!(
            "v{} -> {} ({})",
            info.current_version,
            info.tag,
            target.display()
        ),
    );
    Ok(UpdateOutcome::Updated { info, target })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::context;
    use serde_json::json;

    #[test]
    fn semver_parsing_and_ordering_follow_semver_rules() {
        assert!(SemVer::parse("v0.1.5").unwrap() > SemVer::parse("0.1.4").unwrap());
        assert!(SemVer::parse("v0.2.0").unwrap() > SemVer::parse("v0.2.0-rc.1").unwrap());
        assert!(SemVer::parse("v0.2.0-rc.10").unwrap() > SemVer::parse("v0.2.0-rc.2").unwrap());
        assert_eq!(
            SemVer::parse("0.1.4+build.1")
                .unwrap()
                .cmp(&SemVer::parse("v0.1.4+build.2").unwrap()),
            Ordering::Equal
        );
        assert_eq!(parse_version_tag("0.1.5").unwrap(), "v0.1.5");
        assert_eq!(parse_version_tag("v0.1.5").unwrap(), "v0.1.5");
        for bad in [
            "",
            "v",
            "0.1",
            "01.2.3",
            "v1.2.3/../x",
            "v1.2.3-",
            "v1.2.3+",
            "v1.2.3 rc1",
        ] {
            assert!(SemVer::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn compare_versions_reports_update_available_up_to_date_and_ahead() {
        assert_eq!(
            compare_versions("0.1.4", "0.1.5").unwrap(),
            UpdateStatus::UpdateAvailable
        );
        assert_eq!(
            compare_versions("0.1.4", "0.1.4").unwrap(),
            UpdateStatus::UpToDate
        );
        assert_eq!(
            compare_versions("0.2.0", "0.1.4").unwrap(),
            UpdateStatus::Ahead
        );
    }

    #[test]
    fn parse_github_release_validates_tag_flags_asset_url_and_digest() {
        let digest = "a".repeat(64);
        let good = json!({
            "tag_name": "v0.2.0",
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/ZyphrZero/CqShell/releases/tag/v0.2.0",
            "published_at": "2026-11-23T00:00:00Z",
            "body": "## Changes\n- Better progress bars",
            "assets": [{
                "name": "cqs-x86_64-unknown-linux-musl.tar.gz",
                "size": 4096,
                "digest": format!("sha256:{digest}"),
                "browser_download_url": "https://github.com/ZyphrZero/CqShell/releases/download/v0.2.0/cqs-x86_64-unknown-linux-musl.tar.gz"
            }]
        });
        let info = parse_github_release(
            &good,
            REPOSITORY,
            DEFAULT_RELEASE_BASE,
            "0.1.4",
            None,
            Some(Architecture::X64),
        )
        .unwrap();
        assert_eq!(info.status, UpdateStatus::UpdateAvailable);
        assert!(info.update_available);
        assert_eq!(info.tag, "v0.2.0");
        assert_eq!(info.latest_version, "0.2.0");
        let asset = info.asset.unwrap();
        assert_eq!(asset.size, Some(4096));
        assert_eq!(asset.sha256.as_deref(), Some(digest.as_str()));

        // Reject draft and prerelease when checking latest
        let mut draft = good.clone();
        draft["draft"] = true.into();
        assert!(
            parse_github_release(
                &draft,
                REPOSITORY,
                DEFAULT_RELEASE_BASE,
                "0.1.4",
                None,
                Some(Architecture::X64)
            )
            .is_err()
        );

        let mut pre = good.clone();
        pre["tag_name"] = "v0.2.0-rc.1".into();
        pre["prerelease"] = true.into();
        pre["assets"][0]["browser_download_url"] =
            "https://github.com/ZyphrZero/CqShell/releases/download/v0.2.0-rc.1/cqs-x86_64-unknown-linux-musl.tar.gz"
                .into();
        assert!(
            parse_github_release(
                &pre,
                REPOSITORY,
                DEFAULT_RELEASE_BASE,
                "0.1.4",
                None,
                Some(Architecture::X64)
            )
            .is_err()
        );
        // Explicit prerelease request succeeds
        assert!(
            parse_github_release(
                &pre,
                REPOSITORY,
                DEFAULT_RELEASE_BASE,
                "0.1.4",
                Some("v0.2.0-rc.1"),
                Some(Architecture::X64)
            )
            .is_ok()
        );

        // Reject mismatched download host
        let mut hijacked = good.clone();
        hijacked["assets"][0]["browser_download_url"] = "https://evil.example/cqs.tar.gz".into();
        assert!(
            parse_github_release(
                &hijacked,
                REPOSITORY,
                DEFAULT_RELEASE_BASE,
                "0.1.4",
                None,
                Some(Architecture::X64)
            )
            .is_err()
        );
    }

    #[test]
    fn checksum_and_redirect_parsers_enforce_strict_formats() {
        let hash = "0123456789abcdef".repeat(4);
        assert_eq!(
            parse_checksum_file(
                &format!("{hash}  cqs-x86_64-unknown-linux-musl.tar.gz\n"),
                "cqs-x86_64-unknown-linux-musl.tar.gz"
            )
            .unwrap(),
            hash
        );
        assert_eq!(
            parse_checksum_file(
                &format!("{hash} *cqs-aarch64-unknown-linux-musl.tar.gz"),
                "cqs-aarch64-unknown-linux-musl.tar.gz"
            )
            .unwrap(),
            hash
        );
        assert!(
            parse_checksum_file(
                &format!("{hash}  wrong.tar.gz"),
                "cqs-x86_64-unknown-linux-musl.tar.gz"
            )
            .is_err()
        );
        assert!(
            parse_checksum_file(
                &format!("{hash}  cqs-x86_64-unknown-linux-musl.tar.gz\nextra line"),
                "cqs-x86_64-unknown-linux-musl.tar.gz"
            )
            .is_err()
        );
        assert_eq!(
            parse_redirect_tag(
                "https://github.com/ZyphrZero/CqShell/releases/tag/v0.1.5",
                DEFAULT_RELEASE_BASE
            )
            .unwrap(),
            "v0.1.5"
        );
        assert!(
            parse_redirect_tag(
                "https://evil.example/releases/tag/v0.1.5",
                DEFAULT_RELEASE_BASE
            )
            .is_err()
        );
    }

    fn build_tarball(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, kind, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
            header.set_entry_type(*kind);
            header.set_mode(0o755);
            header.set_size(data.len() as u64);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("outside").unwrap();
            }
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extract_binary_extracts_only_cqs_and_rejects_malicious_archives() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());

        let good_archive = temp.path().join("good.tar.gz");
        fs::write(
            &good_archive,
            build_tarball(&[
                ("cqs/README.md", tar::EntryType::Regular, b"readme"),
                ("cqs/cqs", tar::EntryType::Regular, b"binary-bytes"),
            ]),
        )
        .unwrap();
        let out = temp.path().join("extracted-cqs");
        extract_binary(&ctx, &good_archive, &out).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"binary-bytes");

        for (label, entries) in [
            (
                "traversal",
                vec![
                    ("../escape", tar::EntryType::Regular, &b"x"[..]),
                    ("cqs/cqs", tar::EntryType::Regular, &b"bin"[..]),
                ],
            ),
            (
                "symlink",
                vec![
                    ("cqs/link", tar::EntryType::Symlink, &b""[..]),
                    ("cqs/cqs", tar::EntryType::Regular, &b"bin"[..]),
                ],
            ),
            (
                "missing",
                vec![("cqs/README.md", tar::EntryType::Regular, &b"hi"[..])],
            ),
            (
                "duplicate",
                vec![
                    ("cqs/cqs", tar::EntryType::Regular, &b"a"[..]),
                    ("cqs/cqs", tar::EntryType::Regular, &b"b"[..]),
                ],
            ),
        ] {
            let bad = temp.path().join(format!("{label}.tar.gz"));
            fs::write(&bad, build_tarball(&entries)).unwrap();
            let dst = temp.path().join(format!("out-{label}"));
            assert!(extract_binary(&ctx, &bad, &dst).is_err(), "{label}");
        }
    }

    #[test]
    fn validate_elf_checks_magic_class_endianness_and_machine() {
        let temp = tempfile::tempdir().unwrap();
        let mut elf_x64 = [0u8; 20];
        elf_x64[..4].copy_from_slice(b"\x7fELF");
        elf_x64[4..7].copy_from_slice(&[2, 1, 1]);
        elf_x64[16..18].copy_from_slice(&3u16.to_le_bytes());
        elf_x64[18..20].copy_from_slice(&62u16.to_le_bytes());
        let p = temp.path().join("cqs");
        fs::write(&p, elf_x64).unwrap();
        assert!(validate_elf(&p, Architecture::X64).is_ok());
        assert!(validate_elf(&p, Architecture::Arm64).is_err());

        fs::write(&p, b"not an elf binary at all").unwrap();
        assert!(validate_elf(&p, Architecture::X64).is_err());
    }
}

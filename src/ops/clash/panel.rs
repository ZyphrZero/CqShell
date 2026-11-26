//! Verified Clash/Mihomo Web UI panels served by Mihomo's controller.
//!
//! Supports Zashboard (default), MetaCubeXD, Yacd-Meta, and Razord-Meta.
//! Release panels verify official GitHub Release metadata, SHA-256 digests, and
//! exact byte sizes; pinned `gh-pages` panels verify exact commit archive size
//! and SHA-256 before bounded extraction.

use super::*;
use std::{collections::BTreeSet, path::Component};

const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_PANEL_ARCHIVE: u64 = 32 * 1024 * 1024;
const MAX_PANEL_FILE: u64 = 32 * 1024 * 1024;
const MAX_PANEL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PANEL_ENTRIES: usize = 8192;
const VERSION_MARKER: &str = ".cqs-panel-version";

const YACD_COMMIT: &str = "1798b8222b78c4cf7395ccea032d664d53f43c91";
const YACD_VERSION: &str = "gh-pages@1798b82";
const YACD_SIZE: u64 = 1_807_736;
const YACD_SHA256: &str = "d2d1fdc35887f247f43a66985b847392f03384eb42e897476120f40a9e8f7d4f";
const YACD_ROOT: &str = "Yacd-meta-1798b8222b78c4cf7395ccea032d664d53f43c91";

const RAZORD_COMMIT: &str = "197d540ef7c0a4842b812e2af86d1c2d76233be3";
const RAZORD_VERSION: &str = "gh-pages@197d540";
const RAZORD_SIZE: u64 = 317_922;
const RAZORD_SHA256: &str = "b3855dd5a178926c9d0012478f0965cd696127587b8db034322b8f658a27ff34";
const RAZORD_ROOT: &str = "Razord-meta-197d540ef7c0a4842b812e2af86d1c2d76233be3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Zashboard,
    MetaCubeXd,
    Yacd,
    Razord,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StripRoot {
    None,
    Optional(&'static str),
    Required(&'static str),
}

enum Source {
    GitHubRelease {
        repository: &'static str,
        asset: &'static str,
        format: ArchiveFormat,
        strip_root: StripRoot,
    },
    PinnedArchive {
        repository: &'static str,
        commit: &'static str,
        version: &'static str,
        size: u64,
        digest: &'static str,
        strip_root: &'static str,
    },
}

impl Panel {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "" | "zashboard" => Ok(Self::Zashboard),
            "metacubexd" => Ok(Self::MetaCubeXd),
            "yacd" => Ok(Self::Yacd),
            "razord" => Ok(Self::Razord),
            _ => Err(Error::new(
                "error.input",
                "panel must be zashboard, metacubexd, yacd or razord",
            )),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Zashboard => "zashboard",
            Self::MetaCubeXd => "metacubexd",
            Self::Yacd => "yacd",
            Self::Razord => "razord",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Zashboard => "Zashboard",
            Self::MetaCubeXd => "MetaCubeXD",
            Self::Yacd => "Yacd-Meta",
            Self::Razord => "Razord-Meta",
        }
    }

    fn source(self) -> Source {
        match self {
            Self::Zashboard => Source::GitHubRelease {
                repository: "Zephyruso/zashboard",
                asset: "dist-cdn-fonts.zip",
                format: ArchiveFormat::Zip,
                strip_root: StripRoot::Optional("dist"),
            },
            Self::MetaCubeXd => Source::GitHubRelease {
                repository: "MetaCubeX/metacubexd",
                asset: "compressed-dist.tgz",
                format: ArchiveFormat::TarGz,
                strip_root: StripRoot::None,
            },
            Self::Yacd => Source::PinnedArchive {
                repository: "MetaCubeX/Yacd-meta",
                commit: YACD_COMMIT,
                version: YACD_VERSION,
                size: YACD_SIZE,
                digest: YACD_SHA256,
                strip_root: YACD_ROOT,
            },
            Self::Razord => Source::PinnedArchive {
                repository: "MetaCubeX/Razord-meta",
                commit: RAZORD_COMMIT,
                version: RAZORD_VERSION,
                size: RAZORD_SIZE,
                digest: RAZORD_SHA256,
                strip_root: RAZORD_ROOT,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Installed {
    panel: Panel,
    version: String,
}

#[derive(Debug)]
struct Release {
    panel: Panel,
    version: String,
    url: String,
    digest: String,
    size: u64,
    format: ArchiveFormat,
    strip_root: StripRoot,
}

fn version(value: &str) -> Result<String> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if value.len() > 24
        || parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty() || part.len() > 6 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(Error::new("error.input", "panel version must be vX.Y.Z"));
    }
    Ok(format!("v{value}"))
}

fn valid_marker_version(panel: Panel, value: &str) -> Option<String> {
    match panel.source() {
        Source::GitHubRelease { .. } => version(value).ok(),
        Source::PinnedArchive { .. } => {
            let hash = value.strip_prefix("gh-pages@")?;
            ((7..=40).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| value.to_owned())
        }
    }
}

fn parse_marker(raw: &str) -> Option<Installed> {
    let trimmed = raw.trim();
    if let Some((panel_id, ver)) = trimmed.split_once(':') {
        let panel = Panel::parse(panel_id).ok()?;
        let version = valid_marker_version(panel, ver)?;
        return Some(Installed { panel, version });
    }
    // Backward compatibility with legacy markers written before multi-panel support.
    let version = version(trimmed).ok()?;
    Some(Installed {
        panel: Panel::MetaCubeXd,
        version,
    })
}

fn release(panel: Panel, value: &serde_json::Value, requested: Option<&str>) -> Result<Release> {
    let Source::GitHubRelease {
        repository,
        asset: asset_name,
        format,
        strip_root,
    } = panel.source()
    else {
        return Err(Error::new(
            "error.release",
            "pinned panel does not use GitHub Releases metadata",
        ));
    };
    if value["draft"] != false || value["prerelease"] != false {
        return Err(Error::new("error.release", "panel release must be stable"));
    }
    let tag = value["tag_name"].as_str().unwrap_or("");
    let tag = version(tag)?;
    if requested.is_some_and(|requested| requested != tag) {
        return Err(Error::new(
            "error.release",
            "panel release version mismatch",
        ));
    }
    let assets = value["assets"]
        .as_array()
        .ok_or_else(|| Error::new("error.release", "panel release assets"))?;
    let mut matches = assets.iter().filter(|asset| asset["name"] == asset_name);
    let asset = matches
        .next()
        .ok_or_else(|| Error::new("error.release", "panel archive is missing"))?;
    let url = format!("https://github.com/{repository}/releases/download/{tag}/{asset_name}");
    if matches.next().is_some() || asset["browser_download_url"] != url {
        return Err(Error::new("error.release", "panel archive source mismatch"));
    }
    let digest = asset["digest"]
        .as_str()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| Error::new("error.release", "panel archive SHA-256 is missing"))?;
    let size = asset["size"]
        .as_u64()
        .filter(|size| (1..=MAX_PANEL_ARCHIVE).contains(size))
        .ok_or_else(|| Error::new("error.release", "panel archive size"))?;
    Ok(Release {
        panel,
        version: tag,
        url,
        digest: digest.to_ascii_lowercase(),
        size,
        format,
        strip_root,
    })
}

fn metadata(ctx: &Context, panel: Panel, requested: Option<&str>) -> Result<Release> {
    let Source::GitHubRelease { repository, .. } = panel.source() else {
        return Err(Error::new(
            "error.release",
            "pinned panel does not fetch release metadata",
        ));
    };
    let endpoint = match requested {
        Some(tag) => format!("tags/{tag}"),
        None => "latest".to_owned(),
    };
    let url = format!("https://api.github.com/repos/{repository}/releases/{endpoint}");
    let file = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    let target = path_string(file.path())?;
    let max_metadata = MAX_METADATA.to_string();
    let value: serde_json::Value = ctx.with_github_url(&url, |effective| {
        ctx.run(
            "curl",
            &[
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--connect-timeout",
                "15",
                "--max-time",
                "30",
                "--max-filesize",
                &max_metadata,
                "--header",
                "Accept: application/vnd.github+json",
                "--output",
                &target,
                effective,
            ],
        )?;
        if fs::metadata(file.path())?.len() > MAX_METADATA {
            return Err(Error::new("error.release", "panel metadata size"));
        }
        Ok(serde_json::from_slice(&fs::read(file.path())?)?)
    })?;
    release(panel, &value, requested)
}

fn archive_path(value: &[u8]) -> Result<PathBuf> {
    let value = std::str::from_utf8(value)
        .map_err(|_| Error::new("error.release", "panel archive path encoding"))?;
    if value.is_empty() || value.starts_with('/') {
        return Err(Error::new("error.release", "unsafe panel archive path"));
    }
    let value = value
        .strip_prefix("./")
        .unwrap_or(value)
        .trim_end_matches('/');
    if value.is_empty() || value == "." {
        return Ok(PathBuf::new());
    }
    let path = Path::new(value);
    if value.len() > 512
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | ':'))
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::new("error.release", "unsafe panel archive path"));
    }
    Ok(path.into())
}

fn strip_entry_root(path: PathBuf, strip_root: StripRoot) -> Result<PathBuf> {
    match strip_root {
        StripRoot::None => Ok(path),
        StripRoot::Optional(prefix) => {
            if path.as_os_str().is_empty() {
                return Ok(path);
            }
            if let Ok(stripped) = path.strip_prefix(prefix) {
                Ok(stripped.to_path_buf())
            } else {
                Ok(path)
            }
        }
        StripRoot::Required(prefix) => {
            if path.as_os_str().is_empty() {
                return Ok(path);
            }
            path.strip_prefix(prefix)
                .map(Path::to_path_buf)
                .map_err(|_| Error::new("error.release", "unexpected panel archive root"))
        }
    }
}

fn verify_root_index(destination: &Path) -> Result<()> {
    let index = destination.join("index.html");
    storage::check_path(&index, true)?;
    if !index.is_file() || fs::metadata(index)?.len() == 0 {
        return Err(Error::new(
            "error.release",
            "panel archive has no root index.html",
        ));
    }
    Ok(())
}

fn unpack(
    bytes: &[u8],
    format: ArchiveFormat,
    strip_root: StripRoot,
    destination: &Path,
    check: impl Fn() -> Result<()>,
) -> Result<()> {
    match format {
        ArchiveFormat::TarGz => unpack_tar(bytes, strip_root, destination, check),
        ArchiveFormat::Zip => unpack_zip(bytes, strip_root, destination, check),
    }
}

fn unpack_tar(
    bytes: &[u8],
    strip_root: StripRoot,
    destination: &Path,
    check: impl Fn() -> Result<()>,
) -> Result<()> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    // Bound every decompressed byte, including tar headers and extension records.
    let mut archive = tar::Archive::new(decoder.take(MAX_PANEL_BYTES + 16 * 1024 * 1024));
    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    for (count, entry) in archive.entries()?.enumerate() {
        check()?;
        if count >= MAX_PANEL_ENTRIES {
            return Err(Error::new(
                "error.release",
                "too many panel archive entries",
            ));
        }
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind == tar::EntryType::XGlobalHeader {
            continue;
        }
        if kind != tar::EntryType::Regular && kind != tar::EntryType::Directory {
            return Err(Error::new(
                "error.release",
                "panel archive contains a link or special file",
            ));
        }
        let path = strip_entry_root(archive_path(&entry.path_bytes())?, strip_root)?;
        if path.as_os_str().is_empty() && kind == tar::EntryType::Directory {
            continue;
        }
        if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
            return Err(Error::new("error.release", "duplicate panel archive path"));
        }
        let target = destination.join(path);
        if kind == tar::EntryType::Directory {
            storage::private_dir(&target)?;
            continue;
        }
        let size = entry.header().size()?;
        total = total
            .checked_add(size)
            .filter(|total| *total <= MAX_PANEL_BYTES)
            .ok_or_else(|| Error::new("error.release", "panel extracted size limit"))?;
        if size > MAX_PANEL_FILE {
            return Err(Error::new("error.release", "panel file size limit"));
        }
        let mut content = Vec::with_capacity(size as usize);
        entry
            .by_ref()
            .take(MAX_PANEL_FILE + 1)
            .read_to_end(&mut content)?;
        if content.len() as u64 != size {
            return Err(Error::new("error.release", "panel file size mismatch"));
        }
        storage::atomic_write(&target, &content, 0o600)?;
    }
    verify_root_index(destination)
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| Error::new("error.release", "truncated ZIP archive"))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| Error::new("error.release", "truncated ZIP archive"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Pure-Rust bounded ZIP extractor supporting Stored (0) and Deflate (8) entries
/// via `flate2`, rejecting encryption, Zip64, multi-disk archives, symlinks/special
/// files, header/central-directory mismatches, CRC errors, and oversized output.
fn unpack_zip(
    bytes: &[u8],
    strip_root: StripRoot,
    destination: &Path,
    check: impl Fn() -> Result<()>,
) -> Result<()> {
    const EOCD_SIG: u32 = 0x0605_4b50;
    const CD_SIG: u32 = 0x0201_4b50;
    const LFH_SIG: u32 = 0x0403_4b50;
    const EOCD_MIN: usize = 22;

    if bytes.len() < EOCD_MIN {
        return Err(Error::new("error.release", "invalid ZIP archive"));
    }
    let min_offset = bytes.len().saturating_sub(EOCD_MIN + 65535);
    let mut eocd_offset = None;
    for offset in (min_offset..=bytes.len() - EOCD_MIN).rev() {
        if read_u32_le(bytes, offset)? == EOCD_SIG {
            let comment_len = read_u16_le(bytes, offset + 20)? as usize;
            if offset + EOCD_MIN + comment_len == bytes.len() {
                eocd_offset = Some(offset);
                break;
            }
        }
    }
    let eocd = eocd_offset.ok_or_else(|| Error::new("error.release", "invalid ZIP archive"))?;
    let disk_num = read_u16_le(bytes, eocd + 4)?;
    let cd_disk = read_u16_le(bytes, eocd + 6)?;
    let entries_disk = read_u16_le(bytes, eocd + 8)?;
    let entries_total = read_u16_le(bytes, eocd + 10)?;
    let cd_size = read_u32_le(bytes, eocd + 12)?;
    let cd_offset = read_u32_le(bytes, eocd + 16)?;
    if disk_num != 0 || cd_disk != 0 || entries_disk != entries_total {
        return Err(Error::new(
            "error.release",
            "multi-disk ZIP archives are not supported",
        ));
    }
    if entries_total == u16::MAX || cd_size == u32::MAX || cd_offset == u32::MAX {
        return Err(Error::new(
            "error.release",
            "Zip64 archives are not supported",
        ));
    }
    if entries_total as usize > MAX_PANEL_ENTRIES {
        return Err(Error::new(
            "error.release",
            "too many panel archive entries",
        ));
    }
    let cd_start = cd_offset as usize;
    let cd_end = cd_start
        .checked_add(cd_size as usize)
        .filter(|end| *end <= eocd)
        .ok_or_else(|| Error::new("error.release", "invalid ZIP central directory"))?;

    let mut seen = BTreeSet::new();
    let mut total = 0u64;
    let mut cursor = cd_start;
    for _ in 0..entries_total {
        check()?;
        if cursor
            .checked_add(46)
            .is_none_or(|header_end| header_end > cd_end)
            || read_u32_le(bytes, cursor)? != CD_SIG
        {
            return Err(Error::new(
                "error.release",
                "invalid ZIP central directory entry",
            ));
        }
        let version_made_by = read_u16_le(bytes, cursor + 4)?;
        let flags = read_u16_le(bytes, cursor + 8)?;
        let method = read_u16_le(bytes, cursor + 10)?;
        let expected_crc = read_u32_le(bytes, cursor + 16)?;
        let compressed_size = read_u32_le(bytes, cursor + 20)? as usize;
        let uncompressed_size = read_u32_le(bytes, cursor + 24)? as u64;
        let name_len = read_u16_le(bytes, cursor + 28)? as usize;
        let extra_len = read_u16_le(bytes, cursor + 30)? as usize;
        let comment_len = read_u16_le(bytes, cursor + 32)? as usize;
        let external_attrs = read_u32_le(bytes, cursor + 38)?;
        let local_offset = read_u32_le(bytes, cursor + 42)? as usize;

        let name_start = cursor + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or_else(|| Error::new("error.release", "truncated ZIP entry name"))?;
        let next_cursor = name_end
            .checked_add(extra_len)
            .and_then(|v| v.checked_add(comment_len))
            .filter(|end| *end <= cd_end)
            .ok_or_else(|| Error::new("error.release", "truncated ZIP entry metadata"))?;
        let raw_name = &bytes[name_start..name_end];
        cursor = next_cursor;

        if flags & 0x0041 != 0 {
            return Err(Error::new(
                "error.release",
                "encrypted ZIP entries are not supported",
            ));
        }
        if !matches!(method, 0 | 8) {
            return Err(Error::new(
                "error.release",
                "unsupported ZIP compression method",
            ));
        }

        let mut is_unix_dir = false;
        if (version_made_by >> 8) == 3 {
            let file_type = (external_attrs >> 16) & 0o170_000;
            match file_type {
                0 | 0o100_000 => (),
                0o040_000 => is_unix_dir = true,
                _ => {
                    return Err(Error::new(
                        "error.release",
                        "panel archive contains a link or special file",
                    ));
                }
            }
        }
        let is_dir = raw_name.ends_with(b"/") || is_unix_dir;

        let path = strip_entry_root(archive_path(raw_name)?, strip_root)?;
        if path.as_os_str().is_empty() && is_dir {
            continue;
        }
        if path.as_os_str().is_empty() || !seen.insert(path.clone()) {
            return Err(Error::new("error.release", "duplicate panel archive path"));
        }

        let lfh_min_end = local_offset
            .checked_add(30)
            .filter(|end| *end <= cd_start)
            .ok_or_else(|| Error::new("error.release", "invalid ZIP local file header"))?;
        if read_u32_le(bytes, local_offset)? != LFH_SIG {
            return Err(Error::new("error.release", "invalid ZIP local file header"));
        }
        let lfh_method = read_u16_le(bytes, local_offset + 8)?;
        let lfh_name_len = read_u16_le(bytes, local_offset + 26)? as usize;
        let lfh_extra_len = read_u16_le(bytes, local_offset + 28)? as usize;
        let lfh_name_end = lfh_min_end
            .checked_add(lfh_name_len)
            .filter(|end| *end <= cd_start)
            .ok_or_else(|| Error::new("error.release", "truncated ZIP local header name"))?;
        if lfh_method != method || &bytes[lfh_min_end..lfh_name_end] != raw_name {
            return Err(Error::new(
                "error.release",
                "ZIP local header does not match central directory",
            ));
        }
        let data_start = lfh_name_end
            .checked_add(lfh_extra_len)
            .filter(|end| *end <= cd_start)
            .ok_or_else(|| Error::new("error.release", "truncated ZIP local header extra"))?;
        let data_end = data_start
            .checked_add(compressed_size)
            .filter(|end| *end <= cd_start)
            .ok_or_else(|| Error::new("error.release", "truncated ZIP entry data"))?;

        let target = destination.join(path);
        if is_dir {
            if uncompressed_size != 0 || compressed_size != 0 {
                return Err(Error::new(
                    "error.release",
                    "ZIP directory entry contains unexpected payload",
                ));
            }
            storage::private_dir(&target)?;
            continue;
        }

        if uncompressed_size > MAX_PANEL_FILE {
            return Err(Error::new("error.release", "panel file size limit"));
        }
        total = total
            .checked_add(uncompressed_size)
            .filter(|total| *total <= MAX_PANEL_BYTES)
            .ok_or_else(|| Error::new("error.release", "panel extracted size limit"))?;

        let payload = &bytes[data_start..data_end];
        let content = match method {
            0 => {
                if payload.len() as u64 != uncompressed_size {
                    return Err(Error::new("error.release", "panel file size mismatch"));
                }
                payload.to_vec()
            }
            8 => {
                let mut decoder = flate2::read::DeflateDecoder::new(payload);
                let mut content = Vec::with_capacity(uncompressed_size as usize);
                decoder
                    .by_ref()
                    .take(uncompressed_size + 1)
                    .read_to_end(&mut content)
                    .map_err(|_| Error::new("error.release", "corrupt ZIP deflate stream"))?;
                if content.len() as u64 != uncompressed_size {
                    return Err(Error::new("error.release", "panel file size mismatch"));
                }
                content
            }
            _ => unreachable!(),
        };
        if crc32_ieee(&content) != expected_crc {
            return Err(Error::new("error.release", "ZIP entry CRC-32 mismatch"));
        }
        storage::atomic_write(&target, &content, 0o600)?;
    }
    if cursor != cd_end {
        return Err(Error::new(
            "error.release",
            "trailing ZIP central directory",
        ));
    }
    verify_root_index(destination)
}

/// Each install publishes a fresh immutable directory; the Mixin transaction
/// switches the reference only after the complete archive passes validation.
pub fn install(ctx: &Context, panel: &str, requested: &str) -> Result<()> {
    ctx.check()?;
    let panel = Panel::parse(panel)?;
    ensure_layout(ctx)?;
    let installed = installed_panel(ctx)?;

    let release = match panel.source() {
        Source::PinnedArchive {
            repository,
            commit,
            version: pinned_version,
            size,
            digest,
            strip_root,
        } => {
            if !requested.is_empty() && requested != "latest" && requested != pinned_version {
                return Err(Error::new(
                    "error.input",
                    format!(
                        "{} uses a verified pinned gh-pages build ({pinned_version}); leave version empty",
                        panel.display_name()
                    ),
                ));
            }
            if installed
                .as_ref()
                .is_some_and(|current| current.panel == panel && current.version == pinned_version)
            {
                ctx.message(
                    "notice.clash_panel",
                    format!(
                        "{} {pinned_version}; already installed, download skipped",
                        panel.display_name()
                    ),
                );
                return Ok(());
            }
            ctx.require(&["curl"])?;
            Release {
                panel,
                version: pinned_version.into(),
                url: format!("https://github.com/{repository}/archive/{commit}.tar.gz"),
                digest: digest.into(),
                size,
                format: ArchiveFormat::TarGz,
                strip_root: StripRoot::Required(strip_root),
            }
        }
        Source::GitHubRelease { .. } => {
            let requested = if requested.is_empty() || requested == "latest" {
                None
            } else {
                Some(version(requested)?)
            };
            if let Some(requested) = requested.as_deref()
                && installed
                    .as_ref()
                    .is_some_and(|current| current.panel == panel && current.version == requested)
            {
                ctx.message(
                    "notice.clash_panel",
                    format!(
                        "{} {requested}; already installed, download skipped",
                        panel.display_name()
                    ),
                );
                return Ok(());
            }
            ctx.require(&["curl"])?;
            let release = metadata(ctx, panel, requested.as_deref())?;
            if installed
                .as_ref()
                .is_some_and(|current| current.panel == panel && current.version == release.version)
            {
                ctx.message(
                    "notice.clash_panel",
                    format!(
                        "{} {}; already installed, download skipped",
                        panel.display_name(),
                        release.version
                    ),
                );
                return Ok(());
            }
            release
        }
    };

    if matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new(
            "error.busy",
            "stop Mihomo before installing its panel",
        ));
    }
    let archive = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    download_release_asset(ctx, &release.url, archive.path(), release.size)?;
    let bytes = fs::read(archive.path())?;
    publish(ctx, &release, &bytes)?;
    ctx.message(
        "notice.clash_panel",
        format!(
            "{} {}; start Mihomo to serve /ui/",
            release.panel.display_name(),
            release.version
        ),
    );
    Ok(())
}

fn publish(ctx: &Context, release: &Release, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != release.size || sha256_hex(bytes) != release.digest {
        return Err(Error::new("error.digest", "panel archive"));
    }
    let panels = ctx.paths.clash_resources.join("ui");
    storage::private_dir(&panels)?;
    let sanitized_version = release.version.replace('@', "-");
    let destination = tempfile::Builder::new()
        .prefix(&format!("{}-{sanitized_version}-", release.panel.id()))
        .tempdir_in(&panels)?;
    unpack(
        bytes,
        release.format,
        release.strip_root,
        destination.path(),
        || ctx.check(),
    )?;
    let marker = format!("{}:{}", release.panel.id(), release.version);
    storage::atomic_write(
        &destination.path().join(VERSION_MARKER),
        marker.as_bytes(),
        0o600,
    )?;
    let external_ui = path_string(destination.path())?;
    let configured_controller = controller::address(ctx)?.is_some();
    let result = edit_mixin(ctx, |map| {
        map.insert(
            Value::String("external-ui".into()),
            Value::String(external_ui),
        );
        if !configured_controller {
            let key = Value::String("external-controller".into());
            if map.get(&key).is_none_or(|value| value.as_str() == Some("")) {
                map.insert(key, Value::String("127.0.0.1:9090".into()));
            }
        }
        Ok(())
    });
    if result.is_ok() || ctx.paths.clash_data.join("pending.json").exists() {
        // Retain resources if an interrupted transaction still needs recovery.
        let _ = destination.keep();
    }
    result
}

pub fn remove(ctx: &Context) -> Result<()> {
    ctx.check()?;
    ensure_layout(ctx)?;
    if matches!(state(ctx)?, State::Running(_) | State::Unknown) {
        return Err(Error::new(
            "error.busy",
            "stop Mihomo before removing its panel",
        ));
    }
    edit_mixin(ctx, |map| {
        let key = Value::String("external-ui".into());
        map.remove(&key);
        let runtime = runtime_with_mixin(ctx, Value::Mapping(map.clone()))?;
        let parsed: Value = serde_yaml::from_slice(&runtime)
            .map_err(|_| Error::new("error.data", "cannot parse panel configuration"))?;
        if parsed
            .get("external-ui")
            .and_then(Value::as_str)
            .is_some_and(|path| !path.is_empty())
        {
            map.insert(key, Value::String(String::new()));
        }
        Ok(())
    })?;
    let panels = ctx.paths.clash_resources.join("ui");
    if panels.exists() {
        storage::check_path(&panels, false)?;
        for entry in fs::read_dir(&panels)? {
            let entry = entry?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                return Err(Error::new("error.path", path.display()));
            }
            if meta.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        }
        let _ = fs::remove_dir(&panels);
    }
    ctx.message("notice.clash_panel", "panel removed");
    Ok(())
}

fn configured_directory(ctx: &Context) -> Result<Option<PathBuf>> {
    let bytes = if matches!(state(ctx)?, State::Running(_)) {
        storage::check_path(&ctx.paths.clash_runtime, true)?;
        fs::read(&ctx.paths.clash_runtime)?
    } else {
        build_runtime_config(ctx).map_err(|error| {
            if error.key == "error.data" {
                Error::new("error.data", "cannot parse panel configuration")
            } else {
                error
            }
        })?
    };
    let value: Value = serde_yaml::from_slice(&bytes)
        .map_err(|_| Error::new("error.data", "cannot parse panel configuration"))?;
    match value.get("external-ui") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path)) if path.is_empty() => Ok(None),
        Some(Value::String(path)) => {
            let path = Path::new(path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                ctx.paths.clash_resources.join(path)
            };
            storage::check_path(&path, false)?;
            if !path.starts_with(&ctx.paths.clash_resources) {
                return Err(Error::new(
                    "error.path",
                    "panel path must be inside Mihomo resources",
                ));
            }
            Ok(Some(path))
        }
        _ => Err(Error::new(
            "error.data",
            "external-ui must be a path string",
        )),
    }
}

fn installed_panel(ctx: &Context) -> Result<Option<Installed>> {
    let Some(directory) = configured_directory(ctx)? else {
        return Ok(None);
    };
    let index = directory.join("index.html");
    storage::check_path(&index, true)?;
    if !index.is_file() {
        return Ok(None);
    }
    let marker = directory.join(VERSION_MARKER);
    storage::check_path(&marker, true)?;
    if let Ok(bytes) = fs::read(&marker)
        && let Ok(value) = std::str::from_utf8(&bytes)
        && let Some(installed) = parse_marker(value)
    {
        return Ok(Some(installed));
    }
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    for panel in [
        Panel::Zashboard,
        Panel::MetaCubeXd,
        Panel::Yacd,
        Panel::Razord,
    ] {
        let prefix = format!("{}-", panel.id());
        if let Some(rest) = name.strip_prefix(&prefix)
            && let Some((installed, _suffix)) = rest.split_once('-')
            && let Ok(ver) = version(installed)
        {
            return Ok(Some(Installed {
                panel,
                version: ver,
            }));
        }
    }
    Ok(None)
}

/// Structured read-only WebUI runtime overview (never exposes controller secrets).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct PanelOverview {
    pub installed: bool,
    pub name: Option<String>,
    pub version: Option<String>,
    pub url: Option<String>,
    pub bind_address: Option<String>,
    pub loopback_only: bool,
}

pub(crate) fn inspect(ctx: &Context) -> PanelOverview {
    let mut out = PanelOverview::default();
    let Ok(Some(directory)) = configured_directory(ctx) else {
        return out;
    };
    let index = directory.join("index.html");
    if storage::check_path(&index, true).is_err() || !index.is_file() {
        return out;
    }
    out.installed = true;
    match installed_panel(ctx) {
        Ok(Some(item)) => {
            out.name = Some(item.panel.display_name().to_owned());
            out.version = Some(item.version);
        }
        _ => {
            out.name = Some("Custom".to_owned());
        }
    }
    if let Ok(Some(ctrl)) = controller::effective_settings(ctx) {
        out.url = Some(format!("http://{}/ui/", ctrl.bind_address));
        out.bind_address = Some(ctrl.bind_address);
        out.loopback_only = ctrl.bind_loopback;
    }
    out
}

pub fn url(ctx: &Context) -> Result<Option<String>> {
    let Some(directory) = configured_directory(ctx)? else {
        return Ok(None);
    };
    let index = directory.join("index.html");
    storage::check_path(&index, true)?;
    if !index.is_file() {
        return Ok(None);
    }
    Ok(controller::address(ctx)?.map(|base| format!("{base}/ui/")))
}

pub fn status(ctx: &Context) -> Result<()> {
    let directory = configured_directory(ctx)?;
    let address = url(ctx)?;
    let installed = installed_panel(ctx)?;
    let label = installed
        .as_ref()
        .map(|item| format!("{} {}; ", item.panel.display_name(), item.version))
        .unwrap_or_default();
    let detail = match (directory, address) {
        (Some(path), Some(address)) => format!(
            "{label}{}; {address}; {}; loopback URLs require a browser on the server or an SSH tunnel",
            path.display(),
            state(ctx)?.label(),
        ),
        (Some(path), None) => format!(
            "{label}{}; index.html or external-controller is not configured",
            path.display()
        ),
        (None, _) => "panel is not installed".into(),
    };
    ctx.message("notice.clash_panel", detail);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_for(panel: Panel) -> serde_json::Value {
        let Source::GitHubRelease {
            repository, asset, ..
        } = panel.source()
        else {
            unreachable!()
        };
        serde_json::json!({
            "tag_name":"v1.2.3", "draft":false, "prerelease":false,
            "assets":[{"name":asset,"size":1234,"digest":format!("sha256:{}", "a".repeat(64)),
                "browser_download_url":format!("https://github.com/{repository}/releases/download/v1.2.3/{asset}")}]
        })
    }

    fn package_tar(panel: Panel, strip_root: StripRoot, bytes: &[u8]) -> Release {
        Release {
            panel,
            version: "v1.2.3".into(),
            url: String::new(),
            digest: sha256_hex(bytes),
            size: bytes.len() as u64,
            format: ArchiveFormat::TarGz,
            strip_root,
        }
    }

    fn package_zip(panel: Panel, strip_root: StripRoot, bytes: &[u8]) -> Release {
        Release {
            panel,
            version: "v3.30.0".into(),
            url: String::new(),
            digest: sha256_hex(bytes),
            size: bytes.len() as u64,
            format: ArchiveFormat::Zip,
            strip_root,
        }
    }

    fn zip_archive(entries: &[(&str, u16, u32, &[u8])]) -> Vec<u8> {
        // Each tuple: (name, method 0|8, unix_mode e.g. 0o100644 or 0o040755, content)
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for &(name, method, unix_mode, content) in entries {
            let local_offset = out.len() as u32;
            let crc = crc32_ieee(content);
            let compressed = match method {
                0 => content.to_vec(),
                8 => {
                    let mut encoder = flate2::write::DeflateEncoder::new(
                        Vec::new(),
                        flate2::Compression::default(),
                    );
                    encoder.write_all(content).unwrap();
                    encoder.finish().unwrap()
                }
                _ => content.to_vec(),
            };
            // Local file header
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0u16.to_le_bytes()); // mod date
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(content.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&compressed);

            // Central directory header
            cd.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            cd.extend_from_slice(&((3u16 << 8) | 20u16).to_le_bytes()); // Unix + 2.0
            cd.extend_from_slice(&20u16.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes());
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(content.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&0u16.to_le_bytes()); // extra len
            cd.extend_from_slice(&0u16.to_le_bytes()); // comment len
            cd.extend_from_slice(&0u16.to_le_bytes()); // disk start
            cd.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            cd.extend_from_slice(&(unix_mode << 16).to_le_bytes());
            cd.extend_from_slice(&local_offset.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        let cd_size = cd.len() as u32;
        out.extend_from_slice(&cd);
        // EOCD
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn crc32_matches_ieee_standard_vector() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0);
    }

    #[test]
    fn zip_extraction_supports_stored_deflate_root_stripping_and_rejects_malformed_entries() {
        let zip = zip_archive(&[
            ("dist/", 0, 0o040_755, b""),
            ("dist/index.html", 8, 0o100_644, b"<html>zashboard</html>"),
            ("dist/assets/app.js", 0, 0o100_644, b"console.log('ok')"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack_zip(&zip, StripRoot::Optional("dist"), dir.path(), || Ok(())).unwrap();
        assert_eq!(
            fs::read(dir.path().join("index.html")).unwrap(),
            b"<html>zashboard</html>"
        );
        assert_eq!(
            fs::read(dir.path().join("assets/app.js")).unwrap(),
            b"console.log('ok')"
        );

        // Symlink inside ZIP must be rejected.
        let symlink_zip = zip_archive(&[
            ("dist/index.html", 0, 0o100_644, b"<html>ok</html>"),
            ("dist/evil", 0, 0o120_777, b"/etc/passwd"),
        ]);
        assert!(
            unpack_zip(
                &symlink_zip,
                StripRoot::Optional("dist"),
                tempfile::tempdir().unwrap().path(),
                || Ok(())
            )
            .is_err()
        );

        // Path traversal inside ZIP must be rejected.
        let traversal_zip = zip_archive(&[("../index.html", 0, 0o100_644, b"evil")]);
        assert!(
            unpack_zip(
                &traversal_zip,
                StripRoot::None,
                tempfile::tempdir().unwrap().path(),
                || Ok(())
            )
            .is_err()
        );

        // Unsupported compression method (bzip2 = 12) must be rejected.
        let bad_method = zip_archive(&[("index.html", 12, 0o100_644, b"<html></html>")]);
        assert!(
            unpack_zip(
                &bad_method,
                StripRoot::None,
                tempfile::tempdir().unwrap().path(),
                || Ok(())
            )
            .is_err()
        );

        // Corrupt CRC-32 must be rejected.
        let mut corrupt_crc = zip_archive(&[("index.html", 0, 0o100_644, b"<html>ok</html>")]);
        // Flip a byte inside the stored file payload right after 30 + "index.html".len() = 40.
        corrupt_crc[40] ^= 0xFF;
        let err = unpack_zip(
            &corrupt_crc,
            StripRoot::None,
            tempfile::tempdir().unwrap().path(),
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(err.detail, "ZIP entry CRC-32 mismatch");
    }

    #[test]
    fn publication_changes_only_the_validated_reference_and_retains_old_resources() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        let old = ctx.paths.clash_resources.join("previous");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("index.html"), b"previous panel").unwrap();
        let mut mixin = serde_yaml::Mapping::new();
        mixin.insert("external-ui".into(), path_string(&old).unwrap().into());
        mixin.insert("external-controller".into(), "127.0.0.1:9191".into());
        mixin.insert("secret".into(), "private-test-secret".into());
        let original = serde_yaml::to_string(&mixin).unwrap();
        fs::write(&ctx.paths.clash_mixin, &original).unwrap();
        let bytes = tarball(&[("index.html", tar::EntryType::Regular, b"new panel")]);
        let mut invalid = package_tar(Panel::MetaCubeXd, StripRoot::None, &bytes);
        invalid.digest = "0".repeat(64);
        assert_eq!(
            publish(&ctx, &invalid, &bytes).unwrap_err().key,
            "error.digest"
        );
        assert_eq!(
            fs::read_to_string(&ctx.paths.clash_mixin).unwrap(),
            original
        );
        publish(
            &ctx,
            &package_tar(Panel::MetaCubeXd, StripRoot::None, &bytes),
            &bytes,
        )
        .unwrap();
        assert_eq!(fs::read(old.join("index.html")).unwrap(), b"previous panel");
        let current = configured_directory(&ctx).unwrap().unwrap();
        assert_eq!(fs::read(current.join("index.html")).unwrap(), b"new panel");
        assert_eq!(
            installed_panel(&ctx).unwrap(),
            Some(Installed {
                panel: Panel::MetaCubeXd,
                version: "v1.2.3".into()
            })
        );
        // Switching to Zashboard ZIP publishes a distinct directory and updates marker.
        let zash_zip = zip_archive(&[
            ("dist/", 0, 0o040_755, b""),
            ("dist/index.html", 8, 0o100_644, b"<html>zashboard</html>"),
        ]);
        publish(
            &ctx,
            &package_zip(Panel::Zashboard, StripRoot::Optional("dist"), &zash_zip),
            &zash_zip,
        )
        .unwrap();
        assert_eq!(
            installed_panel(&ctx).unwrap(),
            Some(Installed {
                panel: Panel::Zashboard,
                version: "v3.30.0".into()
            })
        );
        let address = url(&ctx).unwrap().unwrap();
        assert_eq!(address, "http://127.0.0.1:9191/ui/");
        assert!(!address.contains("private-test-secret"));
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
    }

    #[test]
    fn remove_clears_mixin_and_managed_ui_directories() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        let zash_zip = zip_archive(&[
            ("dist/", 0, 0o040_755, b""),
            ("dist/index.html", 8, 0o100_644, b"<html>zashboard</html>"),
        ]);
        publish(
            &ctx,
            &package_zip(Panel::Zashboard, StripRoot::Optional("dist"), &zash_zip),
            &zash_zip,
        )
        .unwrap();
        assert!(configured_directory(&ctx).unwrap().is_some());
        remove(&ctx).unwrap();
        assert!(configured_directory(&ctx).unwrap().is_none());
        assert!(!ctx.paths.clash_resources.join("ui").exists());
    }

    #[test]
    fn installed_panel_version_reads_new_and_legacy_markers_and_directory_name() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        let directory = ctx
            .paths
            .clash_resources
            .join("ui")
            .join("metacubexd-v1.2.3-test");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("index.html"), b"panel").unwrap();
        fs::write(directory.join(VERSION_MARKER), b"v1.2.3\n").unwrap();
        let mut mixin = serde_yaml::Mapping::new();
        mixin.insert(
            "external-ui".into(),
            path_string(&directory).unwrap().into(),
        );
        fs::write(
            &ctx.paths.clash_mixin,
            serde_yaml::to_string(&mixin).unwrap(),
        )
        .unwrap();
        assert_eq!(
            installed_panel(&ctx).unwrap(),
            Some(Installed {
                panel: Panel::MetaCubeXd,
                version: "v1.2.3".into()
            })
        );
        fs::remove_file(directory.join(VERSION_MARKER)).unwrap();
        assert_eq!(
            installed_panel(&ctx).unwrap(),
            Some(Installed {
                panel: Panel::MetaCubeXd,
                version: "v1.2.3".into()
            })
        );
        fs::write(directory.join(VERSION_MARKER), b"zashboard:v3.30.0\n").unwrap();
        assert_eq!(
            installed_panel(&ctx).unwrap(),
            Some(Installed {
                panel: Panel::Zashboard,
                version: "v3.30.0".into()
            })
        );
        fs::write(
            directory.join(VERSION_MARKER),
            format!("yacd:{YACD_VERSION}\n"),
        )
        .unwrap();
        assert_eq!(
            installed_panel(&ctx).unwrap(),
            Some(Installed {
                panel: Panel::Yacd,
                version: YACD_VERSION.into()
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn same_panel_version_skips_archive_download_for_latest_and_explicit_version() {
        use std::os::unix::fs::PermissionsExt;

        for (requested, expected_log) in [("", "metadata\n"), ("v1.2.3", "")] {
            let temp = tempfile::tempdir().unwrap();
            let mut ctx = crate::tests::context(temp.path());
            let bin = temp.path().join("bin");
            fs::create_dir_all(&bin).unwrap();
            let curl = bin.join("curl");
            fs::write(
                &curl,
                r#"#!/bin/sh
out=
api=0
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then shift; out=$1
  elif echo "$1" | grep -q "api.github.com"; then api=1
  fi
  shift
done
if [ "$api" = 1 ]; then
  printf '%s' "$CQS_TEST_RELEASE_JSON" > "$out"
  printf 'metadata\n' >> "$CQS_TEST_PANEL_LOG"
  exit 0
fi
printf 'archive\n' >> "$CQS_TEST_PANEL_LOG"
exit 97
"#,
            )
            .unwrap();
            fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
            let previous_path = std::env::var("PATH").unwrap_or_default();
            ctx.environment
                .insert("PATH".into(), format!("{}:{previous_path}", bin.display()));
            ctx.environment.insert(
                "CQS_TEST_PANEL_LOG".into(),
                temp.path().join("curl.log").display().to_string(),
            );
            ctx.environment.insert(
                "CQS_TEST_RELEASE_JSON".into(),
                fixture_for(Panel::MetaCubeXd).to_string(),
            );
            ensure_layout(&ctx).unwrap();
            let directory = ctx
                .paths
                .clash_resources
                .join("ui/metacubexd-v1.2.3-existing");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("index.html"), b"panel").unwrap();
            fs::write(directory.join(VERSION_MARKER), b"v1.2.3").unwrap();
            let mut mixin = serde_yaml::Mapping::new();
            mixin.insert(
                "external-ui".into(),
                path_string(&directory).unwrap().into(),
            );
            fs::write(
                &ctx.paths.clash_mixin,
                serde_yaml::to_string(&mixin).unwrap(),
            )
            .unwrap();

            install(&ctx, "metacubexd", requested).unwrap();

            let log = fs::read_to_string(temp.path().join("curl.log")).unwrap_or_default();
            assert_eq!(log, expected_log);
            assert_eq!(
                installed_panel(&ctx).unwrap(),
                Some(Installed {
                    panel: Panel::MetaCubeXd,
                    version: "v1.2.3".into()
                })
            );
            assert_eq!(fs::read(directory.join("index.html")).unwrap(), b"panel");
            assert_eq!(
                fs::read_dir(ctx.paths.clash_resources.join("ui"))
                    .unwrap()
                    .count(),
                1
            );
        }
    }

    #[test]
    fn failed_configuration_validation_cleans_new_directory_and_keeps_mixin() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        let original = "external-ui: previous\n";
        fs::write(&ctx.paths.clash_mixin, original).unwrap();
        fs::write(&ctx.paths.clash_config, "{invalid yaml").unwrap();
        let bytes = tarball(&[("index.html", tar::EntryType::Regular, b"new panel")]);
        assert!(
            publish(
                &ctx,
                &package_tar(Panel::MetaCubeXd, StripRoot::None, &bytes),
                &bytes
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(&ctx.paths.clash_mixin).unwrap(),
            original
        );
        assert_eq!(
            fs::read_dir(ctx.paths.clash_resources.join("ui"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn publication_enables_a_disabled_controller_on_loopback() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "external-controller: ''\n").unwrap();
        let bytes = tarball(&[("index.html", tar::EntryType::Regular, b"new panel")]);
        publish(
            &ctx,
            &package_tar(Panel::MetaCubeXd, StripRoot::None, &bytes),
            &bytes,
        )
        .unwrap();
        assert_eq!(url(&ctx).unwrap().unwrap(), "http://127.0.0.1:9090/ui/");
    }

    fn tarball(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, kind, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(*kind);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("outside").unwrap();
            }
            header.set_cksum();
            builder.append_data(&mut header, path, *bytes).unwrap();
        }
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&builder.into_inner().unwrap()).unwrap();
        gzip.finish().unwrap()
    }

    #[test]
    fn release_requires_stable_exact_url_size_digest_and_unique_asset() {
        for panel in [Panel::Zashboard, Panel::MetaCubeXd] {
            let fixture = fixture_for(panel);
            assert_eq!(release(panel, &fixture, None).unwrap().version, "v1.2.3");
            assert!(release(panel, &fixture, Some("v2.0.0")).is_err());
            for index in 0..6 {
                let mut value = fixture.clone();
                match index {
                    0 => value["draft"] = true.into(),
                    1 => {
                        value["assets"][0]["browser_download_url"] =
                            "https://other.invalid/ui.tgz".into()
                    }
                    2 => value["assets"][0]["digest"] = "sha256:invalid".into(),
                    3 => value["assets"][0]["size"] = 0.into(),
                    4 => {
                        let asset = value["assets"][0].clone();
                        value["assets"].as_array_mut().unwrap().push(asset);
                    }
                    _ => value["tag_name"] = "../latest".into(),
                }
                assert!(
                    release(panel, &value, None).is_err(),
                    "{panel:?} variation {index}"
                );
            }
        }
    }

    #[test]
    fn pinned_panel_rejects_arbitrary_custom_version() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        assert_eq!(
            install(&ctx, "yacd", "v1.0.0").unwrap_err().key,
            "error.input"
        );
        assert_eq!(
            install(&ctx, "razord", "v9.9.9").unwrap_err().key,
            "error.input"
        );
    }

    #[test]
    fn tar_extraction_strips_required_root_and_skips_pax_global_header() {
        let bytes = tarball(&[
            (
                "pax_global_header",
                tar::EntryType::XGlobalHeader,
                b"52 comment=abc\n",
            ),
            (RAZORD_ROOT, tar::EntryType::Directory, b""),
            (
                &format!("{RAZORD_ROOT}/index.html"),
                tar::EntryType::Regular,
                b"<html>razord</html>",
            ),
            (
                &format!("{RAZORD_ROOT}/assets/app.js"),
                tar::EntryType::Regular,
                b"ok",
            ),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack_tar(&bytes, StripRoot::Required(RAZORD_ROOT), dir.path(), || {
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::read(dir.path().join("index.html")).unwrap(),
            b"<html>razord</html>"
        );
        assert_eq!(fs::read(dir.path().join("assets/app.js")).unwrap(), b"ok");
    }

    #[test]
    fn archive_paths_reject_traversal_and_windows_escapes() {
        assert_eq!(
            archive_path(b"./assets/main.js").unwrap(),
            PathBuf::from("assets/main.js")
        );
        for path in [
            "",
            "/",
            "../index.html",
            "/index.html",
            "assets/../../x",
            "C:/x",
            "a\\..\\x",
            "a/./b",
            "a//b",
            "a\nb",
        ] {
            assert!(archive_path(path.as_bytes()).is_err(), "{path}");
        }
    }

    #[test]
    fn extraction_refuses_oversized_files_before_allocating_payload() {
        let mut header = tar::Header::new_gnu();
        header.set_path("index.html").unwrap();
        header.set_size(MAX_PANEL_FILE + 1);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        header.set_cksum();
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(header.as_bytes()).unwrap();
        let bytes = gzip.finish().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let error = unpack_tar(&bytes, StripRoot::None, dir.path(), || Ok(())).unwrap_err();
        assert_eq!(error.detail, "panel file size limit");
        assert!(!dir.path().join("index.html").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires CQS_PANEL_SMOKE_DIR with official release.json/archive.tgz and CQS_PANEL_SMOKE_MIHOMO"]
    fn official_archive_publishes_and_passes_real_mihomo_validation() {
        use std::{
            io,
            net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
            time::Instant,
        };

        fn get(port: u16, path: &str) -> io::Result<Vec<u8>> {
            let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(200))?;
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            stream.set_write_timeout(Some(Duration::from_secs(2)))?;
            write!(
                stream,
                "GET {path} HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\nAccept-Encoding: identity\r\n\r\n"
            )?;
            let mut bytes = Vec::new();
            stream
                .take(MAX_PANEL_FILE + 64 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            assert!(bytes.len() as u64 <= MAX_PANEL_FILE + 64 * 1024);
            Ok(bytes)
        }

        fn body(response: &[u8]) -> &[u8] {
            let split = response
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .unwrap();
            let status = response.split(|byte| *byte == b'\n').next().unwrap();
            assert!(
                String::from_utf8_lossy(status).contains(" 200 "),
                "{}",
                String::from_utf8_lossy(status)
            );
            &response[split + 4..]
        }

        fn wait_gone(pid: u32) -> bool {
            let path = PathBuf::from(format!("/proc/{pid}"));
            let deadline = Instant::now() + Duration::from_secs(2);
            while path.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            !path.exists()
        }

        struct StopOnDrop {
            ctx: Context,
            pid: Option<u32>,
        }
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                let pid = self.pid.or_else(|| match state(&self.ctx) {
                    Ok(State::Running(pid)) => Some(pid),
                    _ => None,
                });
                if let Err(error) = stop(&self.ctx.recovery()) {
                    eprintln!("panel smoke cleanup: {error}");
                }
                let Some(pid) = pid else { return };
                if wait_gone(pid) {
                    return;
                }
                if !pid_matches(&self.ctx, pid) {
                    eprintln!("panel smoke cleanup: PID {pid} still exists but is no longer owned");
                    return;
                }
                if let Err(error) = signal(pid, true) {
                    eprintln!("panel smoke cleanup: {error}");
                    return;
                }
                if wait_gone(pid) {
                    let _ = storage::remove_file(&self.ctx.paths.clash_pid);
                    let _ = storage::remove_file(&self.ctx.paths.clash_pid_start);
                } else {
                    eprintln!("panel smoke cleanup: PID {pid} did not disappear after SIGKILL");
                }
            }
        }

        let cache = PathBuf::from(std::env::var_os("CQS_PANEL_SMOKE_DIR").unwrap());
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(cache.join("release.json")).unwrap()).unwrap();
        let package = release(Panel::MetaCubeXd, &metadata, None).unwrap();
        let archive = cache.join("archive.tgz");
        assert_eq!(fs::metadata(&archive).unwrap().len(), package.size);
        let bytes = fs::read(archive).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        ctx.paths.clash_binary = PathBuf::from(std::env::var_os("CQS_PANEL_SMOKE_MIHOMO").unwrap());
        assert!(ctx.paths.clash_binary.is_file());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_config, "mode: direct\nmixed-port: 0\n").unwrap();
        publish(&ctx, &package, &bytes).unwrap();
        let directory = configured_directory(&ctx).unwrap().unwrap();
        assert!(directory.join("index.html").is_file());
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        controller::configure(&ctx, &format!("127.0.0.1:{port}")).unwrap();
        let base = format!("http://127.0.0.1:{port}/ui/");
        assert_eq!(url(&ctx).unwrap().unwrap(), base);
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
        let mut cleanup = StopOnDrop {
            ctx: ctx.clone(),
            pid: None,
        };
        start_inner(&ctx, false).unwrap();
        let pid = match state(&ctx).unwrap() {
            State::Running(pid) => pid,
            other => panic!("Mihomo did not start: {other:?}"),
        };
        cleanup.pid = Some(pid);
        let deadline = Instant::now() + Duration::from_secs(5);
        let response = loop {
            match get(port, "/ui/") {
                Ok(bytes) => break bytes,
                Err(_) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(error) => panic!("Mihomo did not serve the panel: {error}"),
            }
        };
        let html = body(&response);
        assert_eq!(html, fs::read(directory.join("index.html")).unwrap());
        assert!(
            String::from_utf8_lossy(html)
                .to_ascii_lowercase()
                .contains("<html")
        );
        let script = regex::Regex::new(r#"<script[^>]*src="([^"]+)""#).unwrap();
        let html = String::from_utf8_lossy(html);
        let script = &script.captures(&html).expect("panel has a module script")[1];
        let asset = url::Url::parse(&base).unwrap().join(script).unwrap();
        let relative = asset
            .path()
            .strip_prefix("/ui/")
            .expect("panel module stays in /ui/");
        let response = get(port, asset.path()).unwrap();
        assert_eq!(body(&response), fs::read(directory.join(relative)).unwrap());
        stop(&ctx).unwrap();
        assert!(!pid_matches(&ctx, pid));
        assert!(wait_gone(pid), "Mihomo process {pid} was not reaped");
        assert!(!ctx.paths.clash_pid.exists());
        assert!(!ctx.paths.clash_pid_start.exists());
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
    }

    #[test]
    fn extraction_rejects_links_duplicates_missing_index_and_cancellation() {
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Fifo,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let bytes = tarball(&[("index.html", kind, b"")]);
            assert!(unpack_tar(&bytes, StripRoot::None, dir.path(), || Ok(())).is_err());
        }
        let valid = tarball(&[
            ("./index.html", tar::EntryType::Regular, b"<html>ok</html>"),
            ("./assets/app.js", tar::EntryType::Regular, b"export {}"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack_tar(&valid, StripRoot::None, dir.path(), || Ok(())).unwrap();
        assert_eq!(
            fs::read(dir.path().join("index.html")).unwrap(),
            b"<html>ok</html>"
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(
            unpack_tar(&valid, StripRoot::None, dir.path(), || Err(Error::new(
                "error.cancelled",
                "test"
            )))
            .is_err()
        );
        assert!(!dir.path().join("index.html").exists());
        let bytes = tarball(&[
            ("index.html", tar::EntryType::Regular, b"1"),
            ("index.html", tar::EntryType::Regular, b"2"),
        ]);
        assert!(
            unpack_tar(
                &bytes,
                StripRoot::None,
                tempfile::tempdir().unwrap().path(),
                || Ok(())
            )
            .is_err()
        );
        let bytes = tarball(&[("app.js", tar::EntryType::Regular, b"1")]);
        assert!(
            unpack_tar(
                &bytes,
                StripRoot::None,
                tempfile::tempdir().unwrap().path(),
                || Ok(())
            )
            .is_err()
        );
    }

    #[test]
    fn panel_inspect_reports_name_version_url_and_bind_scope_without_secret() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();

        // Not installed initially
        let empty = inspect(&ctx);
        assert!(!empty.installed);
        assert!(empty.url.is_none());

        // Installed Zashboard + loopback controller without secret
        let ui_dir = ctx.paths.clash_resources.join("panels/zashboard-v3.30.0-1");
        fs::create_dir_all(&ui_dir).unwrap();
        fs::write(ui_dir.join("index.html"), b"<html></html>").unwrap();
        fs::write(ui_dir.join(VERSION_MARKER), b"zashboard:v3.30.0\n").unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "external-ui: panels/zashboard-v3.30.0-1\nexternal-controller: 127.0.0.1:9090\n",
        )
        .unwrap();

        let local = inspect(&ctx);
        assert!(local.installed);
        assert_eq!(local.name.as_deref(), Some("Zashboard"));
        assert_eq!(local.version.as_deref(), Some("v3.30.0"));
        assert_eq!(local.url.as_deref(), Some("http://127.0.0.1:9090/ui/"));
        assert_eq!(local.bind_address.as_deref(), Some("127.0.0.1:9090"));
        assert!(local.loopback_only);

        // Non-loopback controller with secret -> loopback_only=false, URL never contains secret
        fs::write(
            &ctx.paths.clash_mixin,
            "external-ui: panels/zashboard-v3.30.0-1\nexternal-controller: 0.0.0.0:9090\nsecret: super-secret-xyz\n",
        )
        .unwrap();
        let lan = inspect(&ctx);
        assert!(lan.installed);
        assert!(!lan.loopback_only);
        assert_eq!(lan.bind_address.as_deref(), Some("0.0.0.0:9090"));
        assert_eq!(lan.url.as_deref(), Some("http://0.0.0.0:9090/ui/"));
        assert!(
            !lan.url
                .as_deref()
                .unwrap_or("")
                .contains("super-secret-xyz")
        );
    }
}

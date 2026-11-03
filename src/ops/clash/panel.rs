//! Verified MetaCubeXD static resources served by Mihomo's controller.

use super::*;
use std::{collections::BTreeSet, path::Component};

const REPOSITORY: &str = "MetaCubeX/metacubexd";
const ASSET: &str = "compressed-dist.tgz";
const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_PANEL_ARCHIVE: u64 = 32 * 1024 * 1024;
const MAX_PANEL_FILE: u64 = 32 * 1024 * 1024;
const MAX_PANEL_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PANEL_ENTRIES: usize = 8192;
const VERSION_MARKER: &str = ".cqs-panel-version";

#[derive(Debug)]
struct Release {
    version: String,
    url: String,
    digest: String,
    size: u64,
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

fn release(value: &serde_json::Value, requested: Option<&str>) -> Result<Release> {
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
    let mut matches = assets.iter().filter(|asset| asset["name"] == ASSET);
    let asset = matches
        .next()
        .ok_or_else(|| Error::new("error.release", "panel archive is missing"))?;
    let url = format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{ASSET}");
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
        version: tag,
        url,
        digest: digest.to_ascii_lowercase(),
        size,
    })
}

fn metadata(ctx: &Context, requested: Option<&str>) -> Result<Release> {
    let endpoint = match requested {
        Some(tag) => format!("tags/{tag}"),
        None => "latest".to_owned(),
    };
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/{endpoint}");
    let file = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    let target = path_string(file.path())?;
    ctx.run(
        "curl",
        &[
            "--proto",
            "=https",
            "--fail",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "15",
            "--max-time",
            "30",
            "--max-filesize",
            &MAX_METADATA.to_string(),
            "--header",
            "Accept: application/vnd.github+json",
            "--output",
            &target,
            &url,
        ],
    )?;
    if fs::metadata(file.path())?.len() > MAX_METADATA {
        return Err(Error::new("error.release", "panel metadata size"));
    }
    let value = serde_json::from_slice(&fs::read(file.path())?)?;
    release(&value, requested)
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

fn unpack(bytes: &[u8], destination: &Path, check: impl Fn() -> Result<()>) -> Result<()> {
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
        let path = archive_path(&entry.path_bytes())?;
        let kind = entry.header().entry_type();
        if kind != tar::EntryType::Regular && kind != tar::EntryType::Directory {
            return Err(Error::new(
                "error.release",
                "panel archive contains a link or special file",
            ));
        }
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

/// Each install publishes a fresh immutable directory; the Mixin transaction
/// switches the reference only after the complete archive passes validation.
pub fn install(ctx: &Context, requested: &str) -> Result<()> {
    ctx.check()?;
    ensure_layout(ctx)?;
    let requested = if requested.is_empty() || requested == "latest" {
        None
    } else {
        Some(version(requested)?)
    };
    if let Some(requested) = requested.as_deref()
        && installed_version(ctx)?.as_deref() == Some(requested)
    {
        ctx.message(
            "notice.clash_panel",
            format!("MetaCubeXD {requested}; already installed, download skipped"),
        );
        return Ok(());
    }
    ctx.require(&["curl"])?;
    let release = metadata(ctx, requested.as_deref())?;
    if installed_version(ctx)?.as_deref() == Some(release.version.as_str()) {
        ctx.message(
            "notice.clash_panel",
            format!(
                "MetaCubeXD {}; already installed, download skipped",
                release.version
            ),
        );
        return Ok(());
    }
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
        format!("MetaCubeXD {}; start Mihomo to serve /ui/", release.version),
    );
    Ok(())
}

fn publish(ctx: &Context, release: &Release, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 != release.size || sha256_hex(bytes) != release.digest {
        return Err(Error::new("error.digest", "panel archive"));
    }
    let panels = ctx.paths.clash_resources.join("ui");
    storage::private_dir(&panels)?;
    let destination = tempfile::Builder::new()
        .prefix(&format!("metacubexd-{}-", release.version))
        .tempdir_in(&panels)?;
    unpack(bytes, destination.path(), || ctx.check())?;
    storage::atomic_write(
        &destination.path().join(VERSION_MARKER),
        release.version.as_bytes(),
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

fn installed_version(ctx: &Context) -> Result<Option<String>> {
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
        && let Ok(version) = version(value.trim())
    {
        return Ok(Some(version));
    }
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let Some(value) = name.strip_prefix("metacubexd-") else {
        return Ok(None);
    };
    let Some((installed, _suffix)) = value.split_once('-') else {
        return Ok(None);
    };
    Ok(version(installed).ok())
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
    let detail = match (directory, address) {
        (Some(path), Some(address)) => format!(
            "{}; {address}; {}; loopback URLs require a browser on the server or an SSH tunnel",
            path.display(),
            state(ctx)?.label(),
        ),
        (Some(path), None) => format!(
            "{}; index.html or external-controller is not configured",
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

    fn fixture() -> serde_json::Value {
        serde_json::json!({
            "tag_name":"v1.2.3", "draft":false, "prerelease":false,
            "assets":[{"name":ASSET,"size":1234,"digest":format!("sha256:{}", "a".repeat(64)),
                "browser_download_url":format!("https://github.com/{REPOSITORY}/releases/download/v1.2.3/{ASSET}")}]
        })
    }

    fn package(bytes: &[u8]) -> Release {
        Release {
            version: "v1.2.3".into(),
            url: String::new(),
            digest: sha256_hex(bytes),
            size: bytes.len() as u64,
        }
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
        let mut invalid = package(&bytes);
        invalid.digest = "0".repeat(64);
        assert_eq!(
            publish(&ctx, &invalid, &bytes).unwrap_err().key,
            "error.digest"
        );
        assert_eq!(
            fs::read_to_string(&ctx.paths.clash_mixin).unwrap(),
            original
        );
        publish(&ctx, &package(&bytes), &bytes).unwrap();
        assert_eq!(fs::read(old.join("index.html")).unwrap(), b"previous panel");
        let current = configured_directory(&ctx).unwrap().unwrap();
        assert_eq!(fs::read(current.join("index.html")).unwrap(), b"new panel");
        let address = url(&ctx).unwrap().unwrap();
        assert_eq!(address, "http://127.0.0.1:9191/ui/");
        assert!(!address.contains("private-test-secret"));
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
    }

    #[test]
    fn installed_panel_version_reads_marker_and_legacy_directory_name() {
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
        assert_eq!(installed_version(&ctx).unwrap().as_deref(), Some("v1.2.3"));
        fs::remove_file(directory.join(VERSION_MARKER)).unwrap();
        assert_eq!(installed_version(&ctx).unwrap().as_deref(), Some("v1.2.3"));
        fs::write(directory.join(VERSION_MARKER), b"v2.3.4\n").unwrap();
        assert_eq!(installed_version(&ctx).unwrap().as_deref(), Some("v2.3.4"));
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
            ctx.environment
                .insert("CQS_TEST_RELEASE_JSON".into(), fixture().to_string());
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

            install(&ctx, requested).unwrap();

            let log = fs::read_to_string(temp.path().join("curl.log")).unwrap_or_default();
            assert_eq!(log, expected_log);
            assert_eq!(installed_version(&ctx).unwrap().as_deref(), Some("v1.2.3"));
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
        assert!(publish(&ctx, &package(&bytes), &bytes).is_err());
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
        publish(&ctx, &package(&bytes), &bytes).unwrap();
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
        assert_eq!(release(&fixture(), None).unwrap().version, "v1.2.3");
        assert!(release(&fixture(), Some("v2.0.0")).is_err());
        for index in 0..6 {
            let mut value = fixture();
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
            assert!(release(&value, None).is_err(), "variation {index}");
        }
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
        let error = unpack(&bytes, dir.path(), || Ok(())).unwrap_err();
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
                // Production owns Child::wait; only observe /proc so the
                // smoke test cannot race its background process reaper.
                if wait_gone(pid) {
                    return;
                }
                if !pid_matches(&self.ctx, pid) {
                    eprintln!("panel smoke cleanup: PID {pid} still exists but is no longer owned");
                    return;
                }
                // Only this test's matching runtime, binary, and start time
                // can reach the last-resort signal during panic cleanup.
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
        let package = release(&metadata, None).unwrap();
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
        // Fetch an actual module referenced by the shipped index as well, so
        // a bare HTML response cannot pass with broken static resource paths.
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
            assert!(unpack(&bytes, dir.path(), || Ok(())).is_err());
        }
        let valid = tarball(&[
            ("./index.html", tar::EntryType::Regular, b"<html>ok</html>"),
            ("./assets/app.js", tar::EntryType::Regular, b"export {}"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        unpack(&valid, dir.path(), || Ok(())).unwrap();
        assert_eq!(
            fs::read(dir.path().join("index.html")).unwrap(),
            b"<html>ok</html>"
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(
            unpack(&valid, dir.path(), || Err(Error::new(
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
        assert!(unpack(&bytes, tempfile::tempdir().unwrap().path(), || Ok(())).is_err());
        let bytes = tarball(&[("app.js", tar::EntryType::Regular, b"1")]);
        assert!(unpack(&bytes, tempfile::tempdir().unwrap().path(), || Ok(())).is_err());
    }
}

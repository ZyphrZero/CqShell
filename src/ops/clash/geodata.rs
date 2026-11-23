//! Verified GeoIP / GeoSite / MMDB / ASN database management from
//! `MetaCubeX/meta-rules-dat` (`latest` release tag).

use super::*;

const REPOSITORY: &str = "MetaCubeX/meta-rules-dat";
const RELEASE_TAG: &str = "latest";
const MAX_METADATA: u64 = 2 * 1024 * 1024;
const MAX_GEODATA_FILE: u64 = 64 * 1024 * 1024;

const KNOWN_FILES: &[&str] = &[
    "geoip.metadb",
    "geosite.dat",
    "country.mmdb",
    "geoip.dat",
    "GeoLite2-ASN.mmdb",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Standard,
    Lite,
    Full,
}

impl Preset {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "" | "standard" => Ok(Self::Standard),
            "lite" => Ok(Self::Lite),
            "full" => Ok(Self::Full),
            _ => Err(Error::new(
                "error.input",
                "geodata preset must be standard, lite or full",
            )),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Lite => "lite",
            Self::Full => "full",
        }
    }

    /// Pairs of `(upstream_asset_name, local_filename_in_clash_resources)`.
    fn entries(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Standard => &[
                ("geoip.metadb", "geoip.metadb"),
                ("geosite.dat", "geosite.dat"),
                ("country.mmdb", "country.mmdb"),
            ],
            Self::Lite => &[
                ("geoip-lite.metadb", "geoip.metadb"),
                ("geosite-lite.dat", "geosite.dat"),
                ("country-lite.mmdb", "country.mmdb"),
            ],
            Self::Full => &[
                ("geoip.metadb", "geoip.metadb"),
                ("geoip.dat", "geoip.dat"),
                ("geosite.dat", "geosite.dat"),
                ("country.mmdb", "country.mmdb"),
                ("GeoLite2-ASN.mmdb", "GeoLite2-ASN.mmdb"),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AssetSpec {
    remote_name: String,
    local_name: String,
    url: String,
    digest: String,
    size: u64,
}

fn manifest(value: &serde_json::Value, preset: Preset) -> Result<Vec<AssetSpec>> {
    if value["draft"] != false || value["prerelease"] != false {
        return Err(Error::new(
            "error.release",
            "GeoData release must be stable",
        ));
    }
    let assets = value["assets"]
        .as_array()
        .ok_or_else(|| Error::new("error.release", "GeoData release assets"))?;
    let mut specs = Vec::new();
    for &(remote_name, local_name) in preset.entries() {
        let mut matches = assets.iter().filter(|asset| asset["name"] == remote_name);
        let asset = matches.next().ok_or_else(|| {
            Error::new(
                "error.release",
                format!("GeoData asset missing: {remote_name}"),
            )
        })?;
        let url = format!(
            "https://github.com/{REPOSITORY}/releases/download/{RELEASE_TAG}/{remote_name}"
        );
        if matches.next().is_some() || asset["browser_download_url"] != url {
            return Err(Error::new(
                "error.release",
                format!("GeoData asset source mismatch: {remote_name}"),
            ));
        }
        let digest = asset["digest"]
            .as_str()
            .and_then(|value| value.strip_prefix("sha256:"))
            .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or_else(|| {
                Error::new(
                    "error.release",
                    format!("GeoData SHA-256 missing: {remote_name}"),
                )
            })?;
        let size = asset["size"]
            .as_u64()
            .filter(|size| (1..=MAX_GEODATA_FILE).contains(size))
            .ok_or_else(|| Error::new("error.release", format!("GeoData size: {remote_name}")))?;
        specs.push(AssetSpec {
            remote_name: remote_name.into(),
            local_name: local_name.into(),
            url,
            digest: digest.to_ascii_lowercase(),
            size,
        });
    }
    Ok(specs)
}

fn metadata(ctx: &Context, preset: Preset) -> Result<Vec<AssetSpec>> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/tags/{RELEASE_TAG}");
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
            return Err(Error::new("error.release", "GeoData metadata size"));
        }
        Ok(serde_json::from_slice(&fs::read(file.path())?)?)
    })?;
    manifest(&value, preset)
}

fn already_matches(path: &Path, size: u64, digest: &str) -> Result<bool> {
    storage::check_path(path, true)?;
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !meta.is_file() || meta.len() != size {
        return Ok(false);
    }
    let bytes = fs::read(path)?;
    Ok(sha256_hex(&bytes) == digest)
}

pub fn status(ctx: &Context) -> Result<()> {
    let mut rows = Vec::new();
    for name in KNOWN_FILES {
        let path = ctx.paths.clash_resources.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                rows.push(format!("{name}: symlink (unsafe)"));
            }
            Ok(meta) if meta.is_file() => {
                rows.push(format!("{name}: {}", format_bytes(meta.len())));
            }
            Ok(_) => rows.push(format!("{name}: not a regular file")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                rows.push(format!("{name}: missing"));
            }
            Err(error) => return Err(error.into()),
        }
    }
    ctx.message(
        "notice.clash_geodata",
        format!(
            "{}\n  {}",
            ctx.paths.clash_resources.display(),
            rows.join("\n  ")
        ),
    );
    Ok(())
}

pub fn update(ctx: &Context, preset: &str) -> Result<()> {
    ctx.check()?;
    let preset = Preset::parse(preset)?;
    ctx.require(&["curl"])?;
    ensure_layout(ctx)?;
    let specs = metadata(ctx, preset)?;
    let mut updated = Vec::new();
    let mut skipped = Vec::new();
    for spec in &specs {
        ctx.check()?;
        let destination = ctx.paths.clash_resources.join(&spec.local_name);
        if already_matches(&destination, spec.size, &spec.digest)? {
            skipped.push(spec.local_name.clone());
            continue;
        }
        let temp = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
        download_release_asset(ctx, &spec.url, temp.path(), spec.size)?;
        let bytes = fs::read(temp.path())?;
        if bytes.len() as u64 != spec.size || sha256_hex(&bytes) != spec.digest {
            return Err(Error::new("error.digest", &spec.remote_name));
        }
        storage::atomic_write(&destination, &bytes, 0o600)?;
        updated.push(format!("{} ({})", spec.local_name, format_bytes(spec.size)));
    }
    let summary = match (updated.is_empty(), skipped.is_empty()) {
        (true, _) => format!(
            "preset={}; all {} files already up to date",
            preset.id(),
            skipped.len()
        ),
        (false, true) => format!("preset={}; updated: {}", preset.id(), updated.join(", ")),
        (false, false) => format!(
            "preset={}; updated: {}; unchanged: {}",
            preset.id(),
            updated.join(", "),
            skipped.join(", ")
        ),
    };
    ctx.message("notice.clash_geodata", summary);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> serde_json::Value {
        let names = [
            "geoip.metadb",
            "geoip-lite.metadb",
            "geosite.dat",
            "geosite-lite.dat",
            "country.mmdb",
            "country-lite.mmdb",
            "geoip.dat",
            "GeoLite2-ASN.mmdb",
        ];
        let assets: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                serde_json::json!({
                    "name": name,
                    "size": (i as u64 + 1) * 1024,
                    "digest": format!("sha256:{}", "b".repeat(64)),
                    "browser_download_url": format!(
                        "https://github.com/{REPOSITORY}/releases/download/{RELEASE_TAG}/{name}"
                    )
                })
            })
            .collect();
        serde_json::json!({
            "tag_name": RELEASE_TAG,
            "draft": false,
            "prerelease": false,
            "assets": assets,
        })
    }

    #[test]
    fn presets_map_expected_upstream_and_local_filenames() {
        let standard = manifest(&fixture(), Preset::Standard).unwrap();
        assert_eq!(
            standard
                .iter()
                .map(|a| (a.remote_name.as_str(), a.local_name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("geoip.metadb", "geoip.metadb"),
                ("geosite.dat", "geosite.dat"),
                ("country.mmdb", "country.mmdb"),
            ]
        );
        let lite = manifest(&fixture(), Preset::Lite).unwrap();
        assert_eq!(
            lite.iter()
                .map(|a| (a.remote_name.as_str(), a.local_name.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("geoip-lite.metadb", "geoip.metadb"),
                ("geosite-lite.dat", "geosite.dat"),
                ("country-lite.mmdb", "country.mmdb"),
            ]
        );
        let full = manifest(&fixture(), Preset::Full).unwrap();
        assert_eq!(full.len(), 5);
        assert!(Preset::parse("unknown").is_err());
    }

    #[test]
    fn manifest_rejects_bad_url_missing_digest_duplicates_and_oversized_assets() {
        for variation in 0..5 {
            let mut value = fixture();
            match variation {
                0 => value["draft"] = true.into(),
                1 => {
                    value["assets"][0]["browser_download_url"] =
                        "https://evil.invalid/geoip.metadb".into()
                }
                2 => value["assets"][0]["digest"] = "md5:deadbeef".into(),
                3 => value["assets"][0]["size"] = (MAX_GEODATA_FILE + 1).into(),
                _ => {
                    let dup = value["assets"][0].clone();
                    value["assets"].as_array_mut().unwrap().push(dup);
                }
            }
            assert!(
                manifest(&value, Preset::Standard).is_err(),
                "variation {variation}"
            );
        }
    }

    #[test]
    fn already_matches_checks_exact_size_and_sha256() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("geoip.metadb");
        assert!(!already_matches(&file, 5, &sha256_hex(b"hello")).unwrap());
        fs::write(&file, b"hello").unwrap();
        assert!(already_matches(&file, 5, &sha256_hex(b"hello")).unwrap());
        assert!(!already_matches(&file, 6, &sha256_hex(b"hello")).unwrap());
        assert!(!already_matches(&file, 5, &sha256_hex(b"world")).unwrap());
    }

    #[test]
    fn status_reports_present_and_missing_databases() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            ctx.paths.clash_resources.join("geoip.metadb"),
            vec![0u8; 2048],
        )
        .unwrap();
        status(&ctx).unwrap();
    }
}

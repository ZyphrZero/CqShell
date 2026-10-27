//! LinuxMirrors owns repository generation; CqShell owns selection and recovery.
use crate::{
    error::{Error, Result},
    runtime::{Context, path, sha256},
    storage::{self, Lock, Saved},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

pub const REVISION: &str = "76fbf1738c22108cdbcc73973612bdb07ad6789b";
pub const SCRIPT_URL: &str = "https://raw.githubusercontent.com/SuperManito/LinuxMirrors/76fbf1738c22108cdbcc73973612bdb07ad6789b/ChangeMirrorsLite.sh";
pub const SCRIPT_SHA256: &str = "963c605bf114bdb7123eaa71d42c7034ac38916b96a55d9f03afa40f397b9fb6";
const SAMPLE: u64 = 1024 * 1024;
const SITES: &[&str] = &["official", "tuna", "ustc", "aliyun"];
const APT: &str = "/etc/apt";
const STATE: &str = ".cqs-mirrors";
type Files = BTreeMap<String, Option<Saved>>;

#[derive(Debug, Clone)]
struct Host {
    distro: String,
    codename: String,
    arch: String,
}

impl Host {
    fn parse(release: &str, arch: &str) -> Result<Self> {
        let values: BTreeMap<_, _> = release
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k, v.trim_matches(['\'', '"'])))
            .collect();
        let distro = values.get("ID").copied().unwrap_or("");
        let codename = values.get("VERSION_CODENAME").copied().unwrap_or("");
        // Only releases whose upstream generation contract is covered by fixtures.
        if !matches!(
            (distro, codename),
            ("debian", "bookworm" | "trixie") | ("ubuntu", "jammy" | "noble" | "resolute")
        ) || !matches!(arch, "amd64" | "arm64")
        {
            return Err(Error::new(
                "error.mirror_host",
                format!("{distro} {codename} {arch}"),
            ));
        }
        Ok(Self {
            distro: distro.into(),
            codename: codename.into(),
            arch: arch.into(),
        })
    }
    fn detect(ctx: &Context) -> Result<Self> {
        for marker in [
            "/etc/armbian-release",
            "/etc/pve/.version",
            "/etc/rpi-issue",
        ] {
            if Path::new(marker).exists() {
                return Err(Error::new("error.mirror_host", marker));
            }
        }
        Self::parse(
            &fs::read_to_string("/etc/os-release")?,
            &ctx.output("dpkg", &["--print-architecture"])?,
        )
    }
    fn branch(&self) -> &str {
        match (self.distro.as_str(), self.arch.as_str()) {
            ("debian", _) => "debian",
            (_, "arm64") => "ubuntu-ports",
            _ => "ubuntu",
        }
    }
    fn official(&self) -> &str {
        match (self.distro.as_str(), self.arch.as_str()) {
            ("debian", _) => "deb.debian.org",
            (_, "arm64") => "ports.ubuntu.com",
            _ => "archive.ubuntu.com",
        }
    }
    fn security(&self) -> (&str, &str) {
        match (self.distro.as_str(), self.arch.as_str()) {
            ("debian", _) => ("security.debian.org", "debian-security"),
            (_, "arm64") => ("ports.ubuntu.com", "ubuntu-ports"),
            _ => ("security.ubuntu.com", "ubuntu"),
        }
    }
    fn domain<'a>(&'a self, site: &str) -> Result<&'a str> {
        match site {
            "official" => Ok(self.official()),
            "tuna" => Ok("mirrors.tuna.tsinghua.edu.cn"),
            "ustc" => Ok("mirrors.ustc.edu.cn"),
            "aliyun" => Ok("mirrors.aliyun.com"),
            _ => Err(Error::new("error.input", "mirror_site")),
        }
    }
    fn base(&self, site: &str) -> Result<String> {
        Ok(format!("https://{}/{}/", self.domain(site)?, self.branch()))
    }
    fn keyring(&self) -> String {
        format!("/usr/share/keyrings/{}-archive-keyring.gpg", self.distro)
    }
    fn targets(&self) -> Vec<String> {
        vec![
            "sources.list".into(),
            format!("sources.list.d/{}.sources", self.distro),
        ]
    }
}

#[derive(Debug)]
struct Measurement {
    site: String,
    mib_per_second: f64,
    first_byte_ms: f64,
}

fn probe(ctx: &Context, url: &str) -> Result<(f64, f64)> {
    let work = tempfile::tempdir()?;
    let body = work.path().join("sample");
    let headers = work.path().join("headers");
    let out = ctx.output(
        "curl",
        &[
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "-fLsS",
            "--connect-timeout",
            "3",
            "--max-time",
            "8",
            "--retry",
            "0",
            "--range",
            "0-1048575",
            "--max-filesize",
            "1048576",
            "--dump-header",
            path(&headers)?,
            "--output",
            path(&body)?,
            "--write-out",
            "%{http_code}\t%{size_download}\t%{time_total}\t%{time_starttransfer}",
            url,
        ],
    )?;
    parse_measurement(&out, &fs::read(&body)?, &fs::read_to_string(headers)?)
}

fn parse_measurement(out: &str, body: &[u8], headers: &str) -> Result<(f64, f64)> {
    let fields: Vec<_> = out.trim().split('\t').collect();
    let fail = || Error::new("error.mirror_sample", out);
    if fields.len() != 4
        || fields[0] != "206"
        || body.len() as u64 != SAMPLE
        || !body.starts_with(b"\xfd7zXZ\0")
        || !headers
            .to_ascii_lowercase()
            .lines()
            .any(|line| line.starts_with("content-range: bytes 0-1048575/"))
    {
        return Err(fail());
    }
    let size = fields[1].parse::<f64>().map_err(|_| fail())?;
    let seconds = fields[2].parse::<f64>().map_err(|_| fail())?;
    let first = fields[3].parse::<f64>().map_err(|_| fail())?;
    if size != SAMPLE as f64
        || !seconds.is_finite()
        || seconds <= 0.0
        || !first.is_finite()
        || first < 0.0
        || first > seconds
    {
        return Err(fail());
    }
    Ok((1.0 / seconds, first * 1000.0))
}

fn benchmark(ctx: &Context, host: &Host) -> Result<Vec<Measurement>> {
    ctx.require(&["curl"])?;
    ctx.message(
        "notice.mirror_probe",
        format!("{} {} / {}", host.distro, host.codename, host.arch),
    );
    let mut results = Vec::new();
    for site in SITES {
        ctx.check()?;
        let url = format!(
            "{}dists/{}/main/binary-{}/Packages.xz",
            host.base(site)?,
            host.codename,
            host.arch
        );
        ctx.log(format!("{site}: {url}"));
        match probe(ctx, &url) {
            Ok((speed, first)) => {
                ctx.log(format!("{site}: {speed:.2} MiB/s; TTFB {first:.0} ms"));
                results.push(Measurement {
                    site: (*site).into(),
                    mib_per_second: speed,
                    first_byte_ms: first,
                });
            }
            Err(e) if e.key == "error.cancelled" => return Err(e),
            Err(e) => ctx.message("notice.mirror_failed", format!("{site}: {e}")),
        }
    }
    results.sort_by(|a, b| {
        b.mib_per_second
            .total_cmp(&a.mib_per_second)
            .then(a.first_byte_ms.total_cmp(&b.first_byte_ms))
    });
    for (i, result) in results.iter().enumerate() {
        ctx.log(format!(
            "{}. {}: {:.2} MiB/s; TTFB {:.0} ms",
            i + 1,
            result.site,
            result.mib_per_second,
            result.first_byte_ms
        ));
    }
    let best = results
        .first()
        .ok_or_else(|| Error::new("error.mirror_speed", ""))?;
    ctx.message(
        "notice.mirror_best",
        format!("{} ({})", best.site, host.base(&best.site)?),
    );
    Ok(results)
}

pub fn test(ctx: &Context) -> Result<()> {
    benchmark(ctx, &Host::detect(ctx)?).map(|_| ())
}

fn known_uri(host: &Host, value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let domain = url.host_str().unwrap_or("");
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (SITES
            .iter()
            .any(|site| host.domain(site).is_ok_and(|s| s == domain))
            || domain == host.security().0
            || (host.distro == "ubuntu" && domain.ends_with(".archive.ubuntu.com")))
        && [host.branch(), host.security().1].contains(&url.path().trim_matches('/'))
}

fn check_source(host: &Host, name: &str, text: &str) -> Result<()> {
    let fail = || Error::new("error.mirror_config", name);
    let suite_ok = |s: &str| {
        ["", "-updates", "-security", "-backports"]
            .iter()
            .any(|suffix| s == format!("{}{suffix}", host.codename))
    };
    let component_ok = |s: &str| match host.distro.as_str() {
        "debian" => ["main", "contrib", "non-free", "non-free-firmware"].contains(&s),
        _ => ["main", "restricted", "universe", "multiverse"].contains(&s),
    };
    if name.ends_with(".sources") {
        let mut fields = BTreeMap::new();
        for line in text.lines().chain(std::iter::once("")) {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if line.trim().is_empty() {
                if !fields.is_empty() {
                    for required in ["Types", "URIs", "Suites", "Components"] {
                        if !fields.contains_key(required) {
                            return Err(fail());
                        }
                    }
                    fields.clear();
                }
                continue;
            }
            let (key, value) = line.split_once(':').ok_or_else(fail)?;
            let words: Vec<_> = value.split_whitespace().collect();
            if words.is_empty() || fields.insert(key, ()).is_some() {
                return Err(fail());
            }
            let valid = match key {
                "Types" => words.iter().all(|s| matches!(*s, "deb" | "deb-src")),
                "URIs" => words.iter().all(|s| known_uri(host, s)),
                "Suites" => words.iter().all(|s| suite_ok(s)),
                "Components" => words.iter().all(|s| component_ok(s)),
                "Signed-By" => words == [host.keyring()],
                _ => false,
            };
            if !valid {
                return Err(fail());
            }
        }
    } else {
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut words: Vec<_> = line.split_whitespace().collect();
            if words
                .get(1)
                .is_some_and(|s| *s == format!("[signed-by={}]", host.keyring()))
            {
                words.remove(1);
            }
            if words.len() < 4
                || !matches!(words[0], "deb" | "deb-src")
                || !known_uri(host, words[1])
                || !suite_ok(words[2])
                || !words[3..].iter().all(|s| component_ok(s))
            {
                return Err(fail());
            }
        }
    }
    Ok(())
}

fn capture(apt: &Path, host: &Host) -> Result<Files> {
    host.targets()
        .into_iter()
        .map(|name| Ok((name.clone(), Saved::capture(&apt.join(name))?)))
        .collect()
}

fn preflight(apt: &Path, host: &Host) -> Result<Files> {
    let files = capture(apt, host)?;
    if files.values().all(Option::is_none) {
        return Err(Error::new("error.mirror_config", apt.display()));
    }
    for (name, saved) in &files {
        if let Some(saved) = saved {
            check_source(
                host,
                name,
                &String::from_utf8(saved.bytes()?).map_err(|e| Error::new("error.data", e))?,
            )?;
        }
    }
    Ok(files)
}

fn verify_result(apt: &Path, host: &Host, site: &str) -> Result<()> {
    let files = preflight(apt, host)?;
    let generated = format!("sources.list.d/{}.sources", host.distro);
    let saved = files[&generated]
        .as_ref()
        .or(files["sources.list"].as_ref())
        .ok_or_else(|| Error::new("error.mirror_config", apt.display()))?;
    let text = String::from_utf8(saved.bytes()?).map_err(|e| Error::new("error.data", e))?;
    let active = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let primary = host.base(site)?;
    let security = format!("https://{}/{}/", host.security().0, host.security().1);
    let words: Vec<_> = active.split_whitespace().collect();
    if !words
        .iter()
        .any(|word| word.trim_end_matches('/') == primary.trim_end_matches('/'))
        || !words
            .iter()
            .any(|word| word.trim_end_matches('/') == security.trim_end_matches('/'))
    {
        return Err(Error::new(
            "error.mirror_config",
            "LinuxMirrors did not generate the selected repositories",
        ));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    distro: String,
    before: Files,
    after: Option<Files>,
    previous: Option<Saved>,
}

fn read_record(file: &Path, host: &Host) -> Result<Record> {
    storage::check_path(file, true)?;
    let record: Record = serde_json::from_slice(&fs::read(file)?)?;
    let expected: Vec<_> = host.targets();
    if record.version != 1 || record.distro != host.distro {
        return Err(Error::new("error.snapshot", file.display()));
    }
    for files in std::iter::once(&record.before).chain(record.after.as_ref()) {
        if files.len() != expected.len() || expected.iter().any(|key| !files.contains_key(key)) {
            return Err(Error::new("error.snapshot", file.display()));
        }
        for saved in files.values().flatten() {
            saved.bytes()?;
        }
    }
    if let Some(saved) = &record.previous {
        saved.bytes()?;
    }
    Ok(record)
}

fn write_files(apt: &Path, files: &Files) -> Result<()> {
    // Validate all targets before the first write. Each file is replaced atomically.
    for name in files.keys() {
        storage::check_path(&apt.join(name), true)?;
    }
    for (name, saved) in files {
        Saved::write(&apt.join(name), saved)?;
    }
    Ok(())
}

fn rollback(apt: &Path, record: &Record) -> Result<()> {
    write_files(apt, &record.before)?;
    Saved::write(&apt.join(STATE).join("last.json"), &record.previous)?;
    storage::remove_file(&apt.join(STATE).join("pending.json"))
}

fn transaction(
    ctx: &Context,
    apt: &Path,
    host: &Host,
    before: Files,
    operation: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let state = apt.join(STATE);
    storage::private_dir(&state)?;
    let pending = state.join("pending.json");
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    if capture(apt, host)? != before {
        return Err(Error::new("error.mirror_changed", apt.display()));
    }
    let mut record = Record {
        version: 1,
        distro: host.distro.clone(),
        before,
        after: None,
        previous: Saved::capture(&state.join("last.json"))?,
    };
    storage::atomic_write(&pending, &serde_json::to_vec_pretty(&record)?, 0o600)?;
    let result = (|| {
        ctx.check()?;
        operation()?;
        ctx.check()?;
        record.after = Some(capture(apt, host)?);
        // Keep just one successful undo, rather than recursively embedding history.
        let last = Record {
            version: 1,
            distro: record.distro.clone(),
            before: record.before.clone(),
            after: record.after.clone(),
            previous: None,
        };
        storage::atomic_write(
            &state.join("last.json"),
            &serde_json::to_vec_pretty(&last)?,
            0o600,
        )?;
        storage::remove_file(&pending)
    })();
    if let Err(error) = result {
        rollback(apt, &record)
            .map_err(|recovery| error.clone().with_recovery(recovery, &pending))?;
        ctx.message("notice.mirror_rolled_back", apt.display());
        return Err(error);
    }
    Ok(())
}

fn arguments(host: &Host, site: &str) -> Result<Vec<String>> {
    Ok([
        "--source",
        host.domain(site)?,
        "--branch",
        host.branch(),
        "--source-security",
        host.security().0,
        "--branch-security",
        host.security().1,
        "--protocol",
        "https",
        "--use-official-source",
        "false",
        "--backup",
        "false",
        "--upgrade-software",
        "false",
        "--clean-cache",
        "false",
        "--clean-screen",
        "false",
        "--install-epel",
        "false",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect())
}

fn verify_script(ctx: &Context, script: &Path) -> Result<()> {
    if sha256(script)? != SCRIPT_SHA256 {
        return Err(Error::new("error.digest", "LinuxMirrors Lite"));
    }
    ctx.run("bash", &["-n", "--", path(script)?])
}

fn strict_update(ctx: &Context) -> Result<()> {
    super::packages::run(
        ctx,
        &[
            "update",
            "-o",
            "APT::Update::Error-Mode=any",
            "-o",
            "Acquire::Retries=0",
            "-o",
            "Acquire::http::Timeout=15",
            "-o",
            "Acquire::https::Timeout=15",
        ],
    )
}

pub fn apply(ctx: &Context, site: &str) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    ctx.require(&["curl", "bash", "apt-get", "dpkg"])?;
    let host = Host::detect(ctx)?;
    if site != "auto" {
        host.domain(site)?;
    }
    let apt = Path::new(APT);
    let pending = apt.join(STATE).join("pending.json");
    if pending.exists() {
        return Err(Error::new("error.pending", pending.display()));
    }
    let before = preflight(apt, &host)?;
    let site = if site == "auto" {
        benchmark(ctx, &host)?[0].site.clone()
    } else {
        site.to_owned()
    };
    let detail = format!(
        "{} {} ({})\n{}\nsecurity: https://{}/{}/\nLinuxMirrors {REVISION}\n{}",
        host.distro,
        host.codename,
        host.arch,
        host.base(&site)?,
        host.security().0,
        host.security().1,
        before
            .keys()
            .map(|name| apt.join(name).display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    ctx.confirm("confirm.mirror", detail, "YES")?;
    let work = tempfile::tempdir()?;
    let script = work.path().join("LinuxMirrors.sh");
    ctx.download(SCRIPT_URL, &script, 60)?;
    verify_script(ctx, &script)?;
    let config = work.path().join("apt.conf");
    fs::write(
        &config,
        "APT::Update::Error-Mode \"any\";\nAcquire::Retries \"0\";\nAcquire::http::Timeout \"15\";\nAcquire::https::Timeout \"15\";\n",
    )?;
    let mut execution = ctx.clone();
    for (key, value) in [
        ("APT_CONFIG", path(&config)?),
        ("BASH_ENV", "/dev/null"),
        ("ENV", "/dev/null"),
        ("DEBIAN_FRONTEND", "noninteractive"),
    ] {
        execution.environment.insert(key.into(), value.into());
    }
    let args = arguments(&host, &site)?;
    transaction(ctx, apt, &host, before, || {
        let mut argv = vec!["--", path(&script)?];
        argv.extend(args.iter().map(String::as_str));
        execution.run("bash", &argv)?;
        // Scope legacy entries to the distribution key, matching DEB822 generation.
        let legacy = apt.join("sources.list");
        if let Some(saved) = Saved::capture(&legacy)? {
            let text =
                String::from_utf8(saved.bytes()?).map_err(|e| Error::new("error.data", e))?;
            let mut new = String::new();
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("deb https://") {
                    new.push_str(&format!(
                        "deb [signed-by={}] https://{rest}\n",
                        host.keyring()
                    ));
                } else {
                    new.push_str(line);
                    new.push('\n');
                }
            }
            if new.as_bytes() != saved.bytes()? {
                storage::atomic_write(&legacy, new.as_bytes(), saved.mode)?;
            }
        }
        verify_result(apt, &host, &site)?;
        strict_update(&execution)
    })?;
    ctx.message("notice.mirror_applied", host.base(&site)?);
    Ok(())
}

fn restore_files(apt: &Path, host: &Host) -> Result<()> {
    let state = apt.join(STATE);
    let pending = state.join("pending.json");
    if pending.exists() {
        return rollback(apt, &read_record(&pending, host)?);
    }
    let last = state.join("last.json");
    let record = read_record(&last, host)?;
    if record.after.as_ref() != Some(&capture(apt, host)?) {
        return Err(Error::new("error.mirror_changed", apt.display()));
    }
    // This journal always contains the desired recovery target, so retry is safe.
    storage::atomic_write(&pending, &serde_json::to_vec_pretty(&record)?, 0o600)?;
    rollback(apt, &record)
}

pub fn restore(ctx: &Context) -> Result<()> {
    let _lock = Lock::acquire(&ctx.paths.kernel_lock)?;
    let host = Host::detect(ctx)?;
    restore_files(Path::new(APT), &host)?;
    ctx.message("notice.mirror_restored", APT);
    // A refresh failure must not put the unwanted mirror configuration back.
    strict_update(ctx)
}

#[cfg(test)]
mod tests;

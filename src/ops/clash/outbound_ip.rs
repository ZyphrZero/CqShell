//! Multi-source Mihomo outbound IP discovery and 4-wave hedged IP quality detection.
//!
//! Uses pure `std::thread` + `mpsc` + `curl -K -` (passing proxy config via stdin so proxy
//! credentials never appear in `/proc/<pid>/cmdline`, logs, or UI output) with per-probe
//! cancellation and bounded response sizes.

use super::model::{
    IpQuality, IpQualityFailure, IpQualitySource, IpQualitySourceStatus, IpType, OutboundIpInfo,
};
use crate::{
    error::{Error, Result},
    runtime::Context,
};
use serde_json::Value as Json;
use std::{
    io::{Read, Write},
    net::IpAddr,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_RESPONSE_BYTES: u64 = 64 * 1024;
const IP_CONNECT_TIMEOUT_SECS: &str = "5";
const IP_MAX_TIME_SECS: &str = "7";
const IP_HEDGE_DELAY: Duration = Duration::from_millis(1500);
const QUALITY_CONNECT_TIMEOUT_SECS: &str = "5";
const QUALITY_MAX_TIME_SECS: &str = "8";
const QUALITY_HEDGE_DELAY: Duration = Duration::from_secs(1);

/// Resolved proxy transport for probing through Mihomo (or direct when TUN-only).
#[derive(Debug, Clone)]
pub(crate) enum ProbeTransport {
    Proxy(String),
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutboundIpSource {
    CloudflareTrace,
    IpSb,
    IpWhoIs,
    IpInfoIo,
    GeoJs,
    CountryIs,
    IpQuery,
    IdentMe,
}

impl OutboundIpSource {
    fn label(self) -> &'static str {
        match self {
            Self::CloudflareTrace => "cloudflare.com",
            Self::IpSb => "ip.sb",
            Self::IpWhoIs => "ipwho.is",
            Self::IpInfoIo => "ipinfo.io",
            Self::GeoJs => "geojs.io",
            Self::CountryIs => "country.is",
            Self::IpQuery => "ipquery.io",
            Self::IdentMe => "ident.me",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Self::CloudflareTrace => "https://www.cloudflare.com/cdn-cgi/trace",
            Self::IpSb => "https://api.ip.sb/geoip",
            Self::IpWhoIs => "https://ipwho.is/",
            Self::IpInfoIo => "https://ipinfo.io/json",
            Self::GeoJs => "https://get.geojs.io/v1/ip/geo.json",
            Self::CountryIs => "https://api.country.is",
            Self::IpQuery => "https://api.ipquery.io/?format=json",
            Self::IdentMe => "https://ident.me/json",
        }
    }
}

const IP_WAVES: &[&[OutboundIpSource]] = &[
    &[
        OutboundIpSource::CloudflareTrace,
        OutboundIpSource::IpSb,
        OutboundIpSource::IpWhoIs,
    ],
    &[
        OutboundIpSource::IpInfoIo,
        OutboundIpSource::GeoJs,
        OutboundIpSource::CountryIs,
        OutboundIpSource::IpQuery,
        OutboundIpSource::IdentMe,
    ],
];

/// Later waves only launch while earlier ones remain unanswered, protecting sources with tighter
/// free quotas (`iplocate.io`, `ipapi.is`, `proxycheck.io`).
const QUALITY_WAVES: &[&[IpQualitySource]] = &[
    &[
        IpQualitySource::IdentMe,
        IpQualitySource::IpApiCom,
        IpQualitySource::IpQuery,
    ],
    &[IpQualitySource::IpLocate],
    &[IpQualitySource::IpApiIs],
    &[IpQualitySource::ProxyCheck],
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct QualityAnswer {
    quality: IpQuality,
    inferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HttpFetchError {
    Cancelled,
    Timeout,
    RateLimited,
    Failed,
}

/// Resolve the local Mihomo proxy endpoint used for outbound IP & quality probing.
pub(crate) fn resolve_transport(ctx: &Context) -> Result<ProbeTransport> {
    ctx.require(&["curl"])?;
    match super::proxy_config(ctx) {
        Ok(cfg) => {
            if let Some(port) = cfg.http_port {
                Ok(ProbeTransport::Proxy(super::proxy_url("http", &cfg, port)?))
            } else if let Some(port) = cfg.socks_port {
                Ok(ProbeTransport::Proxy(super::proxy_url(
                    "socks5h", &cfg, port,
                )?))
            } else if tun_enabled_in_runtime(ctx) {
                Ok(ProbeTransport::Direct)
            } else {
                Err(Error::new(
                    "error.data",
                    "Mihomo has no configured HTTP or SOCKS proxy port",
                ))
            }
        }
        Err(err) => {
            if tun_enabled_in_runtime(ctx) {
                Ok(ProbeTransport::Direct)
            } else {
                Err(err)
            }
        }
    }
}

fn tun_enabled_in_runtime(ctx: &Context) -> bool {
    let path = if ctx.paths.clash_runtime.is_file() {
        &ctx.paths.clash_runtime
    } else {
        &ctx.paths.clash_config
    };
    super::runtime_cache::load_file(path)
        .map(|cached| cached.tun_enabled)
        .unwrap_or(false)
}

fn is_cancelled(ctx: &Context, parent_cancel: &AtomicBool, race_cancel: &AtomicBool) -> bool {
    parent_cancel.load(Ordering::SeqCst)
        || race_cancel.load(Ordering::SeqCst)
        || ctx.cancel.load(Ordering::SeqCst)
}

/// Execute a bounded `curl` request, passing proxy and URL via stdin (`-K -`) so proxy credentials
/// never appear in command-line arguments or logs.
#[allow(clippy::too_many_arguments)]
fn curl_fetch_http(
    ctx: &Context,
    transport: &ProbeTransport,
    target_url: &str,
    allow_http: bool,
    connect_timeout: &str,
    max_time: &str,
    parent_cancel: &AtomicBool,
    race_cancel: &AtomicBool,
) -> std::result::Result<String, HttpFetchError> {
    if is_cancelled(ctx, parent_cancel, race_cancel) {
        return Err(HttpFetchError::Cancelled);
    }

    let mut cmd = Command::new("curl");
    let proto = if allow_http { "=http,https" } else { "=https" };
    cmd.args([
        "-q",
        "--proto",
        proto,
        "--proto-redir",
        proto,
        "-sSL",
        "--connect-timeout",
        connect_timeout,
        "--max-time",
        max_time,
        "-w",
        "\n%{http_code}",
        "-K",
        "-",
    ])
    .envs(&ctx.environment)
    .env("LC_ALL", "C")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = crate::runtime::retry_executable_busy(ctx, || cmd.spawn())
        .map_err(|_| HttpFetchError::Failed)?;

    if let Some(mut stdin) = child.stdin.take() {
        let mut cfg = String::new();
        if let ProbeTransport::Proxy(proxy_url) = transport {
            cfg.push_str(&format!(
                "proxy = \"{}\"\n",
                proxy_url.replace('\\', "\\\\").replace('"', "\\\"")
            ));
        } else {
            cfg.push_str("noproxy = \"*\"\n");
        }
        cfg.push_str(&format!(
            "url = \"{}\"\n",
            target_url.replace('\\', "\\\\").replace('"', "\\\"")
        ));
        let _ = stdin.write_all(cfg.as_bytes());
    }

    let out_pipe = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(out) = out_pipe {
            let _ = out.take(MAX_RESPONSE_BYTES + 32).read_to_end(&mut buf);
        }
        buf
    });

    let mut sleep_ms = 5u64;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                kill_child(&mut child);
                let _ = reader.join();
                return Err(HttpFetchError::Failed);
            }
        }
        if is_cancelled(ctx, parent_cancel, race_cancel) {
            kill_child(&mut child);
            let _ = reader.join();
            return Err(HttpFetchError::Cancelled);
        }
        thread::sleep(Duration::from_millis(sleep_ms));
        sleep_ms = (sleep_ms * 2).min(25);
    };

    let raw = reader.join().unwrap_or_default();
    if !status.success() {
        if status.code() == Some(28) {
            return Err(HttpFetchError::Timeout);
        }
        return Err(HttpFetchError::Failed);
    }

    let text = String::from_utf8_lossy(&raw);
    let trimmed = text.trim_end_matches(['\r', '\n']);
    let (body, code_str) = match trimmed.rsplit_once('\n') {
        Some((b, c)) => (b, c.trim()),
        None => ("", trimmed.trim()),
    };
    match code_str.parse::<u16>() {
        Ok(200) => Ok(body.trim().to_owned()),
        Ok(429) => Err(HttpFetchError::RateLimited),
        _ => Err(HttpFetchError::Failed),
    }
}

pub(crate) fn kill_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if pid > 0 {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Stage 1: Hedged multi-source outbound IP and country code discovery through Mihomo.
pub(crate) fn lookup_outbound_ip(
    ctx: &Context,
    transport: &ProbeTransport,
    cancel: &Arc<AtomicBool>,
) -> Result<OutboundIpInfo> {
    let ctx_base = ctx.clone();
    let transport = transport.clone();
    lookup_outbound_ip_with(
        IP_HEDGE_DELAY,
        cancel,
        move |source, parent_cancel, race_cancel| {
            let body = curl_fetch_http(
                &ctx_base,
                &transport,
                source.url(),
                false,
                IP_CONNECT_TIMEOUT_SECS,
                IP_MAX_TIME_SECS,
                parent_cancel,
                race_cancel,
            )
            .map_err(|_| ())?;
            parse_outbound_ip(source, &body).ok_or(())
        },
    )
}

fn lookup_outbound_ip_with<F>(
    hedge: Duration,
    parent_cancel: &Arc<AtomicBool>,
    fetch: F,
) -> Result<OutboundIpInfo>
where
    F: Fn(OutboundIpSource, &AtomicBool, &AtomicBool) -> std::result::Result<OutboundIpInfo, ()>
        + Send
        + Sync
        + 'static,
{
    let fetch = Arc::new(fetch);
    let race_cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<std::result::Result<OutboundIpInfo, ()>>();

    let mut in_flight = 0usize;
    let mut next_hedge: Option<Instant> = None;

    let spawn_wave =
        |wave: &[OutboundIpSource],
         in_flight: &mut usize,
         tx: &mpsc::Sender<std::result::Result<OutboundIpInfo, ()>>| {
            for &source in wave {
                *in_flight += 1;
                let tx = tx.clone();
                let fetch = fetch.clone();
                let rc = race_cancel.clone();
                let pc = parent_cancel.clone();
                thread::spawn(move || {
                    let res = if pc.load(Ordering::SeqCst) || rc.load(Ordering::SeqCst) {
                        Err(())
                    } else {
                        fetch(source, &pc, &rc)
                    };
                    let _ = tx.send(res);
                });
            }
        };

    // Launch wave 0 immediately.
    spawn_wave(IP_WAVES[0], &mut in_flight, &tx);
    let mut wave_idx = 1usize;
    if wave_idx < IP_WAVES.len() {
        next_hedge = Some(Instant::now() + hedge);
    }

    while in_flight > 0 || wave_idx < IP_WAVES.len() {
        if parent_cancel.load(Ordering::SeqCst) {
            race_cancel.store(true, Ordering::SeqCst);
            return Err(Error::new("error.cancelled", "outbound IP probe cancelled"));
        }

        if in_flight == 0 && wave_idx < IP_WAVES.len() {
            spawn_wave(IP_WAVES[wave_idx], &mut in_flight, &tx);
            wave_idx += 1;
            next_hedge = (wave_idx < IP_WAVES.len()).then(|| Instant::now() + hedge);
            continue;
        }

        let wait = next_hedge
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(80))
            .min(Duration::from_millis(80));

        match rx.recv_timeout(wait) {
            Ok(Ok(info)) => {
                race_cancel.store(true, Ordering::SeqCst);
                return Ok(info);
            }
            Ok(Err(())) => {
                in_flight = in_flight.saturating_sub(1);
                if in_flight == 0 && wave_idx < IP_WAVES.len() {
                    spawn_wave(IP_WAVES[wave_idx], &mut in_flight, &tx);
                    wave_idx += 1;
                    next_hedge = (wave_idx < IP_WAVES.len()).then(|| Instant::now() + hedge);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(deadline) = next_hedge
                    && Instant::now() >= deadline
                    && wave_idx < IP_WAVES.len()
                {
                    spawn_wave(IP_WAVES[wave_idx], &mut in_flight, &tx);
                    wave_idx += 1;
                    next_hedge = (wave_idx < IP_WAVES.len()).then(|| Instant::now() + hedge);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Err(Error::new(
        "error.command",
        "all outbound IP sources failed",
    ))
}

/// Stage 2: FlClash-aligned 4-wave hedged IP quality lookup for a validated IP address.
pub(crate) fn lookup_ip_quality(
    ctx: &Context,
    transport: &ProbeTransport,
    ip: &str,
    cancel: &Arc<AtomicBool>,
) -> std::result::Result<IpQuality, Vec<IpQualityFailure>> {
    let parsed_ip: IpAddr = match ip.trim().parse() {
        Ok(addr) => addr,
        Err(_) => {
            return Err(vec![IpQualityFailure {
                source: IpQualitySource::IdentMe,
                status: IpQualitySourceStatus::Failed,
            }]);
        }
    };
    let canonical_ip = parsed_ip.to_string();
    let ctx_base = ctx.clone();
    let transport = transport.clone();

    lookup_ip_quality_with(
        &canonical_ip,
        QUALITY_HEDGE_DELAY,
        cancel,
        move |source, target_ip, parent_cancel, race_cancel| {
            let url = quality_url(source, target_ip);
            // ip-api.com serves HTTPS only on its paid plan; all other sources require HTTPS.
            let allow_http = source == IpQualitySource::IpApiCom;
            let body = curl_fetch_http(
                &ctx_base,
                &transport,
                &url,
                allow_http,
                QUALITY_CONNECT_TIMEOUT_SECS,
                QUALITY_MAX_TIME_SECS,
                parent_cancel,
                race_cancel,
            )
            .map_err(|err| match err {
                HttpFetchError::Timeout => IpQualitySourceStatus::Timeout,
                HttpFetchError::RateLimited => IpQualitySourceStatus::RateLimited,
                HttpFetchError::Cancelled | HttpFetchError::Failed => IpQualitySourceStatus::Failed,
            })?;
            let json: Json =
                serde_json::from_str(&body).map_err(|_| IpQualitySourceStatus::Failed)?;
            parse_quality(source, target_ip, &json)
        },
    )
}

fn lookup_ip_quality_with<F>(
    ip: &str,
    hedge: Duration,
    parent_cancel: &Arc<AtomicBool>,
    fetch: F,
) -> std::result::Result<IpQuality, Vec<IpQualityFailure>>
where
    F: Fn(
            IpQualitySource,
            &str,
            &AtomicBool,
            &AtomicBool,
        ) -> std::result::Result<QualityAnswer, IpQualitySourceStatus>
        + Send
        + Sync
        + 'static,
{
    let fetch = Arc::new(fetch);
    let ip = ip.to_owned();
    let race_cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<(
        IpQualitySource,
        std::result::Result<QualityAnswer, IpQualitySourceStatus>,
    )>();

    let mut in_flight = 0usize;
    let mut next_hedge: Option<Instant> = None;
    let mut fallback: Option<IpQuality> = None;
    let mut failures: Vec<IpQualityFailure> = Vec::new();

    let spawn_wave = |wave: &[IpQualitySource],
                      in_flight: &mut usize,
                      tx: &mpsc::Sender<(
        IpQualitySource,
        std::result::Result<QualityAnswer, IpQualitySourceStatus>,
    )>| {
        for &source in wave {
            *in_flight += 1;
            let tx = tx.clone();
            let fetch = fetch.clone();
            let ip = ip.clone();
            let rc = race_cancel.clone();
            let pc = parent_cancel.clone();
            thread::spawn(move || {
                let res = if pc.load(Ordering::SeqCst) || rc.load(Ordering::SeqCst) {
                    Err(IpQualitySourceStatus::Failed)
                } else {
                    fetch(source, &ip, &pc, &rc)
                };
                let _ = tx.send((source, res));
            });
        }
    };

    spawn_wave(QUALITY_WAVES[0], &mut in_flight, &tx);
    let mut wave_idx = 1usize;
    if wave_idx < QUALITY_WAVES.len() {
        next_hedge = Some(Instant::now() + hedge);
    }

    while in_flight > 0 || wave_idx < QUALITY_WAVES.len() {
        if parent_cancel.load(Ordering::SeqCst) {
            race_cancel.store(true, Ordering::SeqCst);
            failures.sort_by_key(|f| f.source);
            return Err(failures);
        }

        if in_flight == 0 && wave_idx < QUALITY_WAVES.len() {
            spawn_wave(QUALITY_WAVES[wave_idx], &mut in_flight, &tx);
            wave_idx += 1;
            next_hedge = (wave_idx < QUALITY_WAVES.len()).then(|| Instant::now() + hedge);
            continue;
        }

        let wait = next_hedge
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(80))
            .min(Duration::from_millis(80));

        match rx.recv_timeout(wait) {
            Ok((_, Ok(answer))) => {
                in_flight = in_flight.saturating_sub(1);
                if answer.inferred {
                    if fallback.is_none() {
                        fallback = Some(answer.quality);
                    }
                    if in_flight == 0 && wave_idx < QUALITY_WAVES.len() {
                        spawn_wave(QUALITY_WAVES[wave_idx], &mut in_flight, &tx);
                        wave_idx += 1;
                        next_hedge =
                            (wave_idx < QUALITY_WAVES.len()).then(|| Instant::now() + hedge);
                    }
                } else {
                    race_cancel.store(true, Ordering::SeqCst);
                    return Ok(answer.quality);
                }
            }
            Ok((source, Err(status))) => {
                in_flight = in_flight.saturating_sub(1);
                failures.push(IpQualityFailure { source, status });
                if in_flight == 0 && wave_idx < QUALITY_WAVES.len() {
                    spawn_wave(QUALITY_WAVES[wave_idx], &mut in_flight, &tx);
                    wave_idx += 1;
                    next_hedge = (wave_idx < QUALITY_WAVES.len()).then(|| Instant::now() + hedge);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(deadline) = next_hedge
                    && Instant::now() >= deadline
                    && wave_idx < QUALITY_WAVES.len()
                {
                    spawn_wave(QUALITY_WAVES[wave_idx], &mut in_flight, &tx);
                    wave_idx += 1;
                    next_hedge = (wave_idx < QUALITY_WAVES.len()).then(|| Instant::now() + hedge);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if let Some(quality) = fallback {
        return Ok(quality);
    }
    failures.sort_by_key(|f| f.source);
    Err(failures)
}

// ── Outbound IP parsers (Stage 1) ───────────────────────────────────────────────────────────────

fn normalize_ip(raw: &str) -> Option<String> {
    raw.trim().parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

fn normalize_country(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() == 2 && trimmed.bytes().all(|b| b.is_ascii_alphabetic()) {
        trimmed.to_ascii_uppercase()
    } else {
        String::new()
    }
}

fn parse_outbound_ip(source: OutboundIpSource, body: &str) -> Option<OutboundIpInfo> {
    match source {
        OutboundIpSource::CloudflareTrace => parse_cloudflare_trace(body),
        OutboundIpSource::IpSb => parse_ip_sb(body),
        OutboundIpSource::IpWhoIs => parse_ipwho_is(body),
        OutboundIpSource::IpInfoIo => parse_ipinfo_io(body),
        OutboundIpSource::GeoJs => parse_geojs(body),
        OutboundIpSource::CountryIs => parse_country_is(body),
        OutboundIpSource::IpQuery => parse_ipquery_ip(body),
        OutboundIpSource::IdentMe => parse_ident_me_ip(body),
    }
}

fn parse_cloudflare_trace(body: &str) -> Option<OutboundIpInfo> {
    let mut ip = None;
    let mut loc = None;
    for line in body.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "ip" => ip = normalize_ip(v),
            "loc" => loc = Some(normalize_country(v)),
            _ => {}
        }
    }
    Some(OutboundIpInfo {
        ip: ip?,
        country: loc.unwrap_or_default(),
        ip_source: OutboundIpSource::CloudflareTrace.label().to_owned(),
        fallback_org: String::new(),
    })
}

fn parse_ip_sb(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("country_code")
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    let fallback_org = text_field(json.get("organization"))
        .or_else(|| text_field(json.get("isp")))
        .unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::IpSb.label().to_owned(),
        fallback_org,
    })
}

fn parse_ipwho_is(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    if json.get("success").and_then(Json::as_bool) == Some(false) {
        return None;
    }
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("country_code")
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    let fallback_org = json
        .get("connection")
        .and_then(|c| text_field(c.get("org")).or_else(|| text_field(c.get("isp"))))
        .unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::IpWhoIs.label().to_owned(),
        fallback_org,
    })
}

fn parse_ipinfo_io(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("country")
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    let fallback_org = text_field(json.get("org")).unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::IpInfoIo.label().to_owned(),
        fallback_org,
    })
}

fn parse_geojs(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("country_code")
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    let fallback_org = text_field(json.get("organization_name")).unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::GeoJs.label().to_owned(),
        fallback_org,
    })
}

fn parse_country_is(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("country")
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::CountryIs.label().to_owned(),
        fallback_org: String::new(),
    })
}

fn parse_ipquery_ip(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("location")
        .and_then(|l| l.get("country_code"))
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    let fallback_org = json
        .get("isp")
        .and_then(|isp| text_field(isp.get("org")).or_else(|| text_field(isp.get("isp"))))
        .unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::IpQuery.label().to_owned(),
        fallback_org,
    })
}

fn parse_ident_me_ip(body: &str) -> Option<OutboundIpInfo> {
    let json: Json = serde_json::from_str(body).ok()?;
    let ip = normalize_ip(json.get("ip")?.as_str()?)?;
    let country = json
        .get("cc")
        .and_then(Json::as_str)
        .map(normalize_country)
        .unwrap_or_default();
    let fallback_org = text_field(json.get("aso")).unwrap_or_default();
    Some(OutboundIpInfo {
        ip,
        country,
        ip_source: OutboundIpSource::IdentMe.label().to_owned(),
        fallback_org,
    })
}

// ── IP Quality parsers (Stage 2, aligned with FlClash) ──────────────────────────────────────────

fn quality_url(source: IpQualitySource, ip: &str) -> String {
    match source {
        IpQualitySource::IdentMe => "https://ident.me/json".to_owned(),
        IpQualitySource::IpApiCom => {
            format!("http://ip-api.com/json/{ip}?fields=status,isp,org,as,mobile,proxy,hosting")
        }
        IpQualitySource::IpQuery => format!("https://api.ipquery.io/{ip}?format=json"),
        IpQualitySource::IpLocate => format!("https://iplocate.io/api/lookup/{ip}"),
        IpQualitySource::IpApiIs => format!("https://api.ipapi.is/?q={ip}"),
        IpQualitySource::ProxyCheck => format!("https://proxycheck.io/v3/{ip}"),
    }
}

fn parse_quality(
    source: IpQualitySource,
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    match source {
        IpQualitySource::IdentMe => from_ident_me(ip, json),
        IpQualitySource::IpApiCom => from_ip_api_com(ip, json),
        IpQualitySource::IpQuery => from_ip_query(ip, json),
        IpQualitySource::IpLocate => from_ip_locate(ip, json),
        IpQualitySource::IpApiIs => from_ip_api_is(ip, json),
        IpQualitySource::ProxyCheck => from_proxy_check(ip, json),
    }
}

fn from_ident_me(
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    let reported = json.get("ip").and_then(Json::as_str).and_then(normalize_ip);
    if reported.as_deref() != Some(ip) {
        return Err(IpQualitySourceStatus::IpMismatch);
    }
    let ip_type = declared_type(json.get("type")).ok_or(IpQualitySourceStatus::NoType)?;
    Ok(QualityAnswer {
        quality: IpQuality {
            ip: ip.to_owned(),
            source: IpQualitySource::IdentMe,
            ip_type,
            organization: text_field(json.get("aso")),
            asn: parse_asn(json.get("asn")),
            is_proxy: false,
            is_vpn: false,
            is_tor: false,
            is_abuser: false,
        },
        inferred: false,
    })
}

fn from_ip_api_com(
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    if json.get("status").and_then(Json::as_str) != Some("success") {
        return Err(IpQualitySourceStatus::Failed);
    }
    let explicit = pick_type(
        bool_flag(json.get("hosting")),
        bool_flag(json.get("mobile")),
        None,
    );
    Ok(QualityAnswer {
        quality: IpQuality {
            ip: ip.to_owned(),
            source: IpQualitySource::IpApiCom,
            ip_type: explicit.unwrap_or(IpType::Residential),
            organization: text_field(json.get("org")).or_else(|| text_field(json.get("isp"))),
            asn: parse_asn(json.get("as")),
            is_proxy: bool_flag(json.get("proxy")),
            is_vpn: false,
            is_tor: false,
            is_abuser: false,
        },
        inferred: explicit.is_none(),
    })
}

fn from_ip_query(
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    let risk = json.get("risk");
    let isp = json.get("isp");
    let explicit = pick_type(
        bool_flag(risk.and_then(|r| r.get("is_datacenter"))),
        bool_flag(risk.and_then(|r| r.get("is_mobile"))),
        None,
    );
    Ok(QualityAnswer {
        quality: IpQuality {
            ip: ip.to_owned(),
            source: IpQualitySource::IpQuery,
            ip_type: explicit.unwrap_or(IpType::Residential),
            organization: text_field(isp.and_then(|i| i.get("org")))
                .or_else(|| text_field(isp.and_then(|i| i.get("isp")))),
            asn: parse_asn(isp.and_then(|i| i.get("asn"))),
            is_proxy: bool_flag(risk.and_then(|r| r.get("is_proxy"))),
            is_vpn: bool_flag(risk.and_then(|r| r.get("is_vpn"))),
            is_tor: bool_flag(risk.and_then(|r| r.get("is_tor"))),
            is_abuser: false,
        },
        inferred: explicit.is_none(),
    })
}

fn from_ip_locate(
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    let privacy = json.get("privacy");
    let company = json.get("company");
    let asn = json.get("asn");
    let ip_type = pick_type(
        bool_flag(privacy.and_then(|p| p.get("is_hosting"))),
        false,
        declared_type(company.and_then(|c| c.get("type")))
            .or_else(|| declared_type(asn.and_then(|a| a.get("type")))),
    )
    .ok_or(IpQualitySourceStatus::NoType)?;
    Ok(QualityAnswer {
        quality: IpQuality {
            ip: ip.to_owned(),
            source: IpQualitySource::IpLocate,
            ip_type,
            organization: text_field(company.and_then(|c| c.get("name")))
                .or_else(|| text_field(asn.and_then(|a| a.get("name")))),
            asn: parse_asn(asn.and_then(|a| a.get("asn"))),
            is_proxy: bool_flag(privacy.and_then(|p| p.get("is_proxy"))),
            is_vpn: bool_flag(privacy.and_then(|p| p.get("is_vpn"))),
            is_tor: bool_flag(privacy.and_then(|p| p.get("is_tor"))),
            is_abuser: bool_flag(privacy.and_then(|p| p.get("is_abuser"))),
        },
        inferred: false,
    })
}

fn from_ip_api_is(
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    let company = json.get("company");
    let asn = json.get("asn");
    let ip_type = pick_type(
        bool_flag(json.get("is_datacenter")),
        bool_flag(json.get("is_mobile")),
        declared_type(company.and_then(|c| c.get("type")))
            .or_else(|| declared_type(asn.and_then(|a| a.get("type")))),
    )
    .ok_or(IpQualitySourceStatus::NoType)?;
    Ok(QualityAnswer {
        quality: IpQuality {
            ip: ip.to_owned(),
            source: IpQualitySource::IpApiIs,
            ip_type,
            organization: text_field(company.and_then(|c| c.get("name")))
                .or_else(|| text_field(asn.and_then(|a| a.get("org")))),
            asn: parse_asn(asn.and_then(|a| a.get("asn"))),
            is_proxy: bool_flag(json.get("is_proxy")),
            is_vpn: bool_flag(json.get("is_vpn")),
            is_tor: bool_flag(json.get("is_tor")),
            is_abuser: bool_flag(json.get("is_abuser")),
        },
        inferred: false,
    })
}

fn from_proxy_check(
    ip: &str,
    json: &Json,
) -> std::result::Result<QualityAnswer, IpQualitySourceStatus> {
    let status = json.get("status").and_then(Json::as_str).unwrap_or("");
    if status == "denied" {
        return Err(IpQualitySourceStatus::RateLimited);
    }
    if status != "ok" && status != "warning" {
        return Err(IpQualitySourceStatus::Failed);
    }
    let result = json.get(ip);
    let network = result.and_then(|r| r.get("network"));
    let detections = result.and_then(|r| r.get("detections"));
    let net_type = network.and_then(|n| n.get("type")).and_then(Json::as_str);
    let declared = match net_type {
        Some("Residential") => Some(IpType::Residential),
        Some("Business") => Some(IpType::Business),
        _ => None,
    };
    let ip_type = pick_type(
        bool_flag(detections.and_then(|d| d.get("hosting"))) || net_type == Some("Hosting"),
        net_type == Some("Wireless"),
        declared,
    )
    .ok_or(IpQualitySourceStatus::NoType)?;
    Ok(QualityAnswer {
        quality: IpQuality {
            ip: ip.to_owned(),
            source: IpQualitySource::ProxyCheck,
            ip_type,
            organization: text_field(network.and_then(|n| n.get("organisation"))),
            asn: parse_asn(network.and_then(|n| n.get("asn"))),
            is_proxy: bool_flag(detections.and_then(|d| d.get("proxy"))),
            is_vpn: bool_flag(detections.and_then(|d| d.get("vpn"))),
            is_tor: bool_flag(detections.and_then(|d| d.get("tor"))),
            is_abuser: false,
        },
        inferred: false,
    })
}

fn pick_type(hosting: bool, mobile: bool, declared: Option<IpType>) -> Option<IpType> {
    if hosting || declared == Some(IpType::Hosting) {
        return Some(IpType::Hosting);
    }
    if mobile {
        return Some(IpType::Mobile);
    }
    declared
}

fn declared_type(value: Option<&Json>) -> Option<IpType> {
    match value.and_then(Json::as_str)? {
        "isp" => Some(IpType::Residential),
        "business" | "education" | "government" | "banking" => Some(IpType::Business),
        "hosting" => Some(IpType::Hosting),
        _ => None,
    }
}

fn bool_flag(value: Option<&Json>) -> bool {
    value.and_then(Json::as_bool) == Some(true)
}

fn text_field(value: Option<&Json>) -> Option<String> {
    value
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_asn(value: Option<&Json>) -> Option<u32> {
    let parsed = match value? {
        Json::Number(n) => n.as_u64().and_then(|v| u32::try_from(v).ok()),
        Json::String(text) => {
            let digits: String = text
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<u32>().ok()
        }
        _ => None,
    }?;
    (parsed != 0).then_some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::clash::model::IpQualityLevel;

    #[test]
    fn outbound_ip_parsers_handle_all_eight_sources_and_reject_invalid_ips() {
        let cf = parse_outbound_ip(
            OutboundIpSource::CloudflareTrace,
            "fl=114f12\nh=www.cloudflare.com\nip=203.0.113.106\nloc=sg\n",
        )
        .unwrap();
        assert_eq!(cf.ip, "203.0.113.106");
        assert_eq!(cf.country, "SG");
        assert_eq!(cf.ip_source, "cloudflare.com");

        let sb = parse_outbound_ip(
            OutboundIpSource::IpSb,
            r#"{"ip":"203.0.113.5","country_code":"jp","organization":"Example ISP"}"#,
        )
        .unwrap();
        assert_eq!(sb.ip, "203.0.113.5");
        assert_eq!(sb.country, "JP");
        assert_eq!(sb.fallback_org, "Example ISP");

        let who = parse_outbound_ip(
            OutboundIpSource::IpWhoIs,
            r#"{"success":true,"ip":"198.51.100.2","country_code":"US","connection":{"org":"Transit Corp"}}"#,
        )
        .unwrap();
        assert_eq!(who.ip, "198.51.100.2");
        assert_eq!(who.country, "US");
        assert_eq!(who.fallback_org, "Transit Corp");
        assert!(
            parse_outbound_ip(
                OutboundIpSource::IpWhoIs,
                r#"{"success":false,"ip":"198.51.100.2"}"#
            )
            .is_none()
        );

        let info = parse_outbound_ip(
            OutboundIpSource::IpInfoIo,
            r#"{"ip":"2001:db8::1","country":"DE","org":"AS64512 Example"}"#,
        )
        .unwrap();
        assert_eq!(info.ip, "2001:db8::1");
        assert_eq!(info.country, "DE");

        let geojs = parse_outbound_ip(
            OutboundIpSource::GeoJs,
            r#"{"ip":"192.0.2.9","country_code":"HK","organization_name":"HK Net"}"#,
        )
        .unwrap();
        assert_eq!(geojs.country, "HK");
        assert_eq!(geojs.fallback_org, "HK Net");

        let cis = parse_outbound_ip(
            OutboundIpSource::CountryIs,
            r#"{"ip":"192.0.2.10","country":"NL"}"#,
        )
        .unwrap();
        assert_eq!(cis.country, "NL");

        let ipq = parse_outbound_ip(
            OutboundIpSource::IpQuery,
            r#"{"ip":"192.0.2.11","location":{"country_code":"TW"},"isp":{"isp":"Chunghwa"}}"#,
        )
        .unwrap();
        assert_eq!(ipq.country, "TW");
        assert_eq!(ipq.fallback_org, "Chunghwa");

        let ident = parse_outbound_ip(
            OutboundIpSource::IdentMe,
            r#"{"ip":"192.0.2.12","cc":"GB","aso":"UK Transit"}"#,
        )
        .unwrap();
        assert_eq!(ident.country, "GB");
        assert_eq!(ident.fallback_org, "UK Transit");

        // Reject shell/URL injection payloads pretending to be IPs
        assert!(
            parse_outbound_ip(
                OutboundIpSource::CountryIs,
                r#"{"ip":"1.1.1.1/../../admin","country":"US"}"#
            )
            .is_none()
        );
    }

    #[test]
    fn ip_quality_parsers_match_flclash_classification_and_flags() {
        // ip-api.com hosting + proxy + AS64502
        let ip_api: Json = serde_json::from_str(
            r#"{"status":"success","isp":"Example Hosting","org":"Example Hosting","as":"AS64502 Example Hosting","mobile":false,"proxy":true,"hosting":true}"#,
        )
        .unwrap();
        let ans = parse_quality(IpQualitySource::IpApiCom, "203.0.113.106", &ip_api).unwrap();
        assert!(!ans.inferred);
        assert_eq!(ans.quality.ip_type, IpType::Hosting);
        assert_eq!(ans.quality.level(), IpQualityLevel::Normal);
        assert_eq!(ans.quality.organization.as_deref(), Some("Example Hosting"));
        assert_eq!(ans.quality.asn, Some(64502));
        assert!(ans.quality.is_proxy);

        // ip-api.com with hosting=false & mobile=false is inferred residential
        let inferred_json: Json = serde_json::from_str(
            r#"{"status":"success","isp":"Home ISP","as":"AS1234","mobile":false,"proxy":false,"hosting":false}"#,
        )
        .unwrap();
        let inf = parse_quality(IpQualitySource::IpApiCom, "203.0.113.1", &inferred_json).unwrap();
        assert!(inf.inferred);
        assert_eq!(inf.quality.ip_type, IpType::Residential);
        assert_eq!(inf.quality.level(), IpQualityLevel::Good);

        // ident.me IP mismatch detection
        let ident_bad: Json =
            serde_json::from_str(r#"{"ip":"198.51.100.9","type":"isp","aso":"ISP","asn":64500}"#)
                .unwrap();
        assert_eq!(
            parse_quality(IpQualitySource::IdentMe, "203.0.113.106", &ident_bad),
            Err(IpQualitySourceStatus::IpMismatch)
        );

        // ident.me declared business type
        let ident_ok: Json = serde_json::from_str(
            r#"{"ip":"203.0.113.106","type":"business","aso":"Corp Net","asn":64501}"#,
        )
        .unwrap();
        let ok = parse_quality(IpQualitySource::IdentMe, "203.0.113.106", &ident_ok).unwrap();
        assert_eq!(ok.quality.ip_type, IpType::Business);
        assert_eq!(ok.quality.level(), IpQualityLevel::Good);

        // iplocate.io abuser -> Risky
        let iploc: Json = serde_json::from_str(
            r#"{"privacy":{"is_hosting":false,"is_proxy":true,"is_vpn":true,"is_tor":false,"is_abuser":true},"company":{"name":"Bad Corp","type":"isp"},"asn":{"asn":"AS4242"}}"#,
        )
        .unwrap();
        let risky = parse_quality(IpQualitySource::IpLocate, "198.51.100.8", &iploc).unwrap();
        assert_eq!(risky.quality.level(), IpQualityLevel::Risky);
        assert_eq!(
            risky.quality.flag_keys(),
            vec![
                "clash.outbound.flag_abuser",
                "clash.outbound.flag_vpn",
                "clash.outbound.flag_proxy"
            ]
        );

        // proxycheck.io denied -> RateLimited; Wireless -> Mobile
        let denied: Json = serde_json::from_str(r#"{"status":"denied"}"#).unwrap();
        assert_eq!(
            parse_quality(IpQualitySource::ProxyCheck, "198.51.100.8", &denied),
            Err(IpQualitySourceStatus::RateLimited)
        );
        let mobile_pc: Json = serde_json::from_str(
            r#"{"status":"ok","198.51.100.8":{"network":{"type":"Wireless","organisation":"5G Carrier","asn":"AS9999"},"detections":{"hosting":false,"proxy":false,"vpn":false,"tor":false}}}"#,
        )
        .unwrap();
        let mob = parse_quality(IpQualitySource::ProxyCheck, "198.51.100.8", &mobile_pc).unwrap();
        assert_eq!(mob.quality.ip_type, IpType::Mobile);
        assert_eq!(mob.quality.asn, Some(9999));

        // ipapi.is Tor -> Risky
        let ipapi_is: Json = serde_json::from_str(
            r#"{"is_datacenter":true,"is_mobile":false,"is_proxy":false,"is_vpn":false,"is_tor":true,"is_abuser":false,"company":{"name":"Tor Exit","type":"hosting"},"asn":{"asn":13335}}"#,
        )
        .unwrap();
        let tor = parse_quality(IpQualitySource::IpApiIs, "198.51.100.9", &ipapi_is).unwrap();
        assert_eq!(tor.quality.level(), IpQualityLevel::Risky);
        assert!(tor.quality.is_tor);
    }

    #[test]
    fn hedged_quality_prefers_definitive_answer_over_inferred_and_sorts_failures() {
        let cancel = Arc::new(AtomicBool::new(false));
        // Wave 1 returns inferred residential from ip-api.com; Wave 2 (iplocate) returns declared Business.
        let result = lookup_ip_quality_with(
            "203.0.113.50",
            Duration::from_millis(20),
            &cancel,
            |source, ip, _pc, _rc| match source {
                IpQualitySource::IpApiCom => Ok(QualityAnswer {
                    quality: IpQuality {
                        ip: ip.to_owned(),
                        source,
                        ip_type: IpType::Residential,
                        organization: Some("Fallback ISP".into()),
                        asn: Some(100),
                        is_proxy: false,
                        is_vpn: false,
                        is_tor: false,
                        is_abuser: false,
                    },
                    inferred: true,
                }),
                IpQualitySource::IpLocate => Ok(QualityAnswer {
                    quality: IpQuality {
                        ip: ip.to_owned(),
                        source,
                        ip_type: IpType::Business,
                        organization: Some("Declared Corp".into()),
                        asn: Some(200),
                        is_proxy: false,
                        is_vpn: false,
                        is_tor: false,
                        is_abuser: false,
                    },
                    inferred: false,
                }),
                _ => Err(IpQualitySourceStatus::NoType),
            },
        )
        .unwrap();
        assert_eq!(result.source, IpQualitySource::IpLocate);
        assert_eq!(result.ip_type, IpType::Business);

        // When later waves fail, the inferred Wave 1 answer is returned as fallback.
        let fallback_res = lookup_ip_quality_with(
            "203.0.113.50",
            Duration::from_millis(10),
            &cancel,
            |source, ip, _pc, _rc| match source {
                IpQualitySource::IpQuery => Ok(QualityAnswer {
                    quality: IpQuality {
                        ip: ip.to_owned(),
                        source,
                        ip_type: IpType::Residential,
                        organization: Some("Residential Net".into()),
                        asn: Some(300),
                        is_proxy: false,
                        is_vpn: false,
                        is_tor: false,
                        is_abuser: false,
                    },
                    inferred: true,
                }),
                _ => Err(IpQualitySourceStatus::Timeout),
            },
        )
        .unwrap();
        assert_eq!(fallback_res.source, IpQualitySource::IpQuery);
        assert_eq!(fallback_res.ip_type, IpType::Residential);

        // When every source fails, failures are returned sorted by IpQualitySource order.
        let errs = lookup_ip_quality_with(
            "203.0.113.50",
            Duration::from_millis(10),
            &cancel,
            |source, _ip, _pc, _rc| match source {
                IpQualitySource::IdentMe => Err(IpQualitySourceStatus::IpMismatch),
                IpQualitySource::IpApiCom => Err(IpQualitySourceStatus::Timeout),
                IpQualitySource::ProxyCheck => Err(IpQualitySourceStatus::RateLimited),
                _ => Err(IpQualitySourceStatus::Failed),
            },
        )
        .unwrap_err();
        assert_eq!(errs.len(), 6);
        assert_eq!(errs[0].source, IpQualitySource::IdentMe);
        assert_eq!(errs[0].status, IpQualitySourceStatus::IpMismatch);
        assert_eq!(errs[1].source, IpQualitySource::IpApiCom);
        assert_eq!(errs[1].status, IpQualitySourceStatus::Timeout);
        assert_eq!(errs[5].source, IpQualitySource::ProxyCheck);
        assert_eq!(errs[5].status, IpQualitySourceStatus::RateLimited);
    }

    #[test]
    fn hedged_outbound_ip_returns_first_valid_source() {
        let cancel = Arc::new(AtomicBool::new(false));
        let info = lookup_outbound_ip_with(Duration::from_millis(20), &cancel, |source, _, _| {
            if source == OutboundIpSource::IpInfoIo {
                Ok(OutboundIpInfo {
                    ip: "203.0.113.106".into(),
                    country: "SG".into(),
                    ip_source: source.label().into(),
                    fallback_org: "Example Hosting".into(),
                })
            } else {
                Err(())
            }
        })
        .unwrap();
        assert_eq!(info.ip, "203.0.113.106");
        assert_eq!(info.country, "SG");
    }
}

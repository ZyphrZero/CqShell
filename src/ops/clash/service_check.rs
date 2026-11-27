//! FlClash-aligned 13-service availability, latency, region, and exit-IP probe engine.
//!
//! Runs purely on `std::thread` + bounded worker pool (`SERVICE_CHECK_CONCURRENCY = 4`) +
//! `curl -i -K -` (all proxy credentials, request headers, and POST payloads passed via stdin
//! so nothing sensitive enters `/proc/<pid>/cmdline`, logs, or UI output), with streaming
//! early-stop body scanning, single timeout retry, and immediate cancellation support.

use super::{
    model::{ProxyGroupItem, RuleItem, ServiceCheckItem, ServiceProbeStatus, ServiceTarget},
    outbound_ip::{self, ProbeTransport},
};
use crate::runtime::Context;
use regex::Regex;
use serde_json::Value as Json;
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::IpAddr,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub const SERVICE_CHECK_CONCURRENCY: usize = 4;
const DEFAULT_PROBE_TIMEOUT_SECS: u64 = 10;
const SERVICE_HTML_MAX_BODY: usize = 512 * 1024;
const SERVICE_SMALL_MAX_BODY: usize = 16 * 1024;
const SERVICE_SCAN_MAX_BODY: usize = 8 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const SCAN_CHUNK_BYTES: usize = 32 * 1024;
const SCAN_OVERLAP_BYTES: usize = 256;

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// Public browser client token Disney+ hands to every web visitor.
const DISNEY_AUTH_HEADER: &str =
    "Bearer ZGlzbmV5JmJyb3dzZXImMS4wLjA.Cu56AgSfBTDag5NiRA81oLHkDZfu5L3CKadnefEAY84";

const GEMINI_REGION_MARKER: &str = ",2,1,200,\"";
const CLAUDE_BLOCKED_REGIONS: &[&str] =
    &["AF", "BY", "CN", "CU", "HK", "IR", "KP", "MO", "RU", "SY"];
const GEMINI_BLOCKED_REGIONS: &[&str] = &["CHN", "RUS", "BLR", "CUB", "IRN", "PRK", "SYR"];

// Five bytes per entry, alpha-3 then alpha-2, sorted by alpha-3 (ISO 3166-1).
const ISO3166_PACKED: &str = concat!(
    "ABWAWAFGAFAGOAOAIAAIALAAXALBALANDADAREAEARGARARMAMASMASATAAQ",
    "ATFTFATGAGAUSAUAUTATAZEAZBDIBIBELBEBENBJBESBQBFABFBGDBDBGRBG",
    "BHRBHBHSBSBIHBABLMBLBLRBYBLZBZBMUBMBOLBOBRABRBRBBBBRNBNBTNBT",
    "BVTBVBWABWCAFCFCANCACCKCCCHECHCHLCLCHNCNCIVCICMRCMCODCDCOGCG",
    "COKCKCOLCOCOMKMCPVCVCRICRCUBCUCUWCWCXRCXCYMKYCYPCYCZECZDEUDE",
    "DJIDJDMADMDNKDKDOMDODZADZECUECEGYEGERIERESHEHESPESESTEEETHET",
    "FINFIFJIFJFLKFKFRAFRFROFOFSMFMGABGAGBRGBGEOGEGGYGGGHAGHGIBGI",
    "GINGNGLPGPGMBGMGNBGWGNQGQGRCGRGRDGDGRLGLGTMGTGUFGFGUMGUGUYGY",
    "HKGHKHMDHMHNDHNHRVHRHTIHTHUNHUIDNIDIMNIMINDINIOTIOIRLIEIRNIR",
    "IRQIQISLISISRILITAITJAMJMJEYJEJORJOJPNJPKAZKZKENKEKGZKGKHMKH",
    "KIRKIKNAKNKORKRKWTKWLAOLALBNLBLBRLRLBYLYLCALCLIELILKALKLSOLS",
    "LTULTLUXLULVALVMACMOMAFMFMARMAMCOMCMDAMDMDGMGMDVMVMEXMXMHLMH",
    "MKDMKMLIMLMLTMTMMRMMMNEMEMNGMNMNPMPMOZMZMRTMRMSRMSMTQMQMUSMU",
    "MWIMWMYSMYMYTYTNAMNANCLNCNERNENFKNFNGANGNICNINIUNUNLDNLNORNO",
    "NPLNPNRUNRNZLNZOMNOMPAKPKPANPAPCNPNPERPEPHLPHPLWPWPNGPGPOLPL",
    "PRIPRPRKKPPRTPTPRYPYPSEPSPYFPFQATQAREUREROURORUSRURWARWSAUSA",
    "SDNSDSENSNSGPSGSGSGSSHNSHSJMSJSLBSBSLESLSLVSVSMRSMSOMSOSPMPM",
    "SRBRSSSDSSSTPSTSURSRSVKSKSVNSISWESESWZSZSXMSXSYCSCSYRSYTCATC",
    "TCDTDTGOTGTHATHTJKTJTKLTKTKMTMTLSTLTONTOTTOTTTUNTNTURTRTUVTV",
    "TWNTWTZATZUGAUGUKRUAUMIUMURYUYUSAUSUZBUZVATVAVCTVCVENVEVGBVG",
    "VIRVIVNMVNVUTVUWLFWFWSMWSYEMYEZAFZAZMBZMZWEZW"
);

#[derive(Debug, Clone, Default)]
struct ProbeResponse {
    status_code: u16,
    delay_ms: Option<u64>,
    body: String,
    final_url: String,
    location: Option<String>,
    timed_out: bool,
    failed: bool,
}

#[derive(Debug, Clone)]
struct ProbeRequest<'a> {
    method: &'a str,
    url: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: Option<&'a str>,
    max_body: usize,
    no_redirect: bool,
    timeout_secs: u64,
}

impl<'a> ProbeRequest<'a> {
    fn get(url: &'a str, max_body: usize) -> Self {
        Self {
            method: "GET",
            url,
            headers: &[],
            body: None,
            max_body,
            no_redirect: false,
            timeout_secs: DEFAULT_PROBE_TIMEOUT_SECS,
        }
    }
}

/// Optional Mihomo routing context used to attach deterministic exit nodes when possible.
#[derive(Debug, Clone, Default)]
pub(crate) struct RouteContext {
    pub mode: String,
    pub rules: Vec<RuleItem>,
    pub groups: Vec<ProxyGroupItem>,
}

impl RouteContext {
    /// Resolve the deterministic exit leaf node for `target` without guessing past opaque
    /// rules (`RuleSet`, `GeoSite`, `GeoIP`, `DomainRegex`, `IpCidr`).
    pub fn resolve_exit_node(&self, target: ServiceTarget) -> Option<String> {
        let mode = self.mode.trim().to_ascii_lowercase();
        if mode == "direct" {
            return Some("DIRECT".to_owned());
        }
        if mode == "global" {
            return resolve_proxy_leaf("GLOBAL", &self.groups, 0);
        }
        if mode != "rule" || self.rules.is_empty() {
            return None;
        }
        let host = target.primary_host().to_ascii_lowercase();
        for rule in &self.rules {
            let kind = rule.kind.trim().to_ascii_lowercase();
            let payload = rule.payload.trim().to_ascii_lowercase();
            let matched = match kind.as_str() {
                "domain" => host == payload,
                "domainsuffix" | "domain-suffix" => {
                    host == payload
                        || (!payload.is_empty()
                            && host.ends_with(&payload)
                            && host.as_bytes().get(host.len() - payload.len() - 1) == Some(&b'.'))
                }
                "domainkeyword" | "domain-keyword" => {
                    !payload.is_empty() && host.contains(&payload)
                }
                "match" | "final" => true,
                // Opaque rules cannot be evaluated accurately from `/rules` payloads alone.
                _ => return None,
            };
            if matched {
                return resolve_proxy_leaf(rule.proxy.trim(), &self.groups, 0);
            }
        }
        None
    }
}

fn resolve_proxy_leaf(name: &str, groups: &[ProxyGroupItem], depth: usize) -> Option<String> {
    if name.is_empty() || depth > 12 {
        return None;
    }
    if let Some(group) = groups.iter().find(|g| g.name == name) {
        if let Some(last) = group.resolved_chain.last().filter(|s| !s.is_empty()) {
            return Some(last.clone());
        }
        if !group.now.is_empty() && group.now != name {
            return resolve_proxy_leaf(&group.now, groups, depth + 1)
                .or_else(|| Some(group.now.clone()));
        }
    }
    Some(name.to_owned())
}

/// Run a bounded concurrent sweep (`SERVICE_CHECK_CONCURRENCY = 4`) over `targets`, invoking
/// `on_item` progressively as each service completes so the TUI updates live in real time.
pub(crate) fn run_service_sweep<F>(
    ctx: &Context,
    transport: &ProbeTransport,
    route: &RouteContext,
    targets: &[ServiceTarget],
    cancel: &Arc<AtomicBool>,
    on_item: F,
) where
    F: Fn(ServiceCheckItem) + Send + Sync + 'static,
{
    if targets.is_empty() {
        return;
    }
    let queue = Arc::new(Mutex::new(targets.iter().copied().collect::<VecDeque<_>>()));
    let worker_count = SERVICE_CHECK_CONCURRENCY.min(targets.len());
    let on_item = Arc::new(on_item);
    let mut handles = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let q = queue.clone();
        let c = ctx.clone();
        let tr = transport.clone();
        let rt = route.clone();
        let cancel_flag = cancel.clone();
        let cb = on_item.clone();
        handles.push(thread::spawn(move || {
            loop {
                if is_cancelled(&c, &cancel_flag) {
                    break;
                }
                let next = {
                    let Ok(mut guard) = q.lock() else {
                        break;
                    };
                    guard.pop_front()
                };
                let Some(target) = next else {
                    break;
                };
                let item = check_single_service(&c, &tr, &rt, target, &cancel_flag);
                if is_cancelled(&c, &cancel_flag) {
                    break;
                }
                cb(item);
            }
        }));
    }

    for handle in handles {
        let _ = handle.join();
    }
}

fn is_cancelled(ctx: &Context, cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::SeqCst) || ctx.cancel.load(Ordering::SeqCst)
}

const EARLY_FAILURE_THRESHOLD: Duration = Duration::from_secs(3);
const EARLY_FAILURE_RETRY_DELAY: Duration = Duration::from_millis(1200);

fn sleep_cancelable(ctx: &Context, cancel: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if is_cancelled(ctx, cancel) {
            return false;
        }
        let step = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        thread::sleep(step);
    }
    !is_cancelled(ctx, cancel)
}

fn check_with_confirmation<F>(
    ctx: &Context,
    cancel: &AtomicBool,
    early_threshold: Duration,
    retry_delay: Duration,
    mut probe: F,
) -> ServiceCheckItem
where
    F: FnMut() -> ServiceCheckItem,
{
    let started = Instant::now();
    let item = probe();
    if is_cancelled(ctx, cancel) {
        return item;
    }
    // Retry once on timeout (aligned with FlClash core/service_check.go).
    if item.status == ServiceProbeStatus::Timeout {
        return probe();
    }
    // Early failure confirmation: a rapid connection reset right after a proxy switch or hot-reload
    // gets one brief confirmation retry after settling, preventing false red alerts.
    if item.status == ServiceProbeStatus::Failed
        && started.elapsed() < early_threshold
        && sleep_cancelable(ctx, cancel, retry_delay)
    {
        return probe();
    }
    item
}

fn check_single_service(
    ctx: &Context,
    transport: &ProbeTransport,
    route: &RouteContext,
    target: ServiceTarget,
    cancel: &AtomicBool,
) -> ServiceCheckItem {
    let env = CurlServiceEnv {
        ctx,
        transport,
        cancel,
    };
    let mut item = check_with_confirmation(
        ctx,
        cancel,
        EARLY_FAILURE_THRESHOLD,
        EARLY_FAILURE_RETRY_DELAY,
        || run_checker(&env, target),
    );
    if item.exit_node.is_none() {
        item.exit_node = route.resolve_exit_node(target);
    }
    item
}

trait ServiceHttpEnv {
    fn send(&self, req: &ProbeRequest<'_>) -> ProbeResponse;
}

struct CurlServiceEnv<'a> {
    ctx: &'a Context,
    transport: &'a ProbeTransport,
    cancel: &'a AtomicBool,
}

impl ServiceHttpEnv for CurlServiceEnv<'_> {
    fn send(&self, req: &ProbeRequest<'_>) -> ProbeResponse {
        curl_service_request(self.ctx, self.transport, req, self.cancel)
    }
}

fn curl_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn curl_service_request(
    ctx: &Context,
    transport: &ProbeTransport,
    req: &ProbeRequest<'_>,
    local_cancel: &AtomicBool,
) -> ProbeResponse {
    if is_cancelled(ctx, local_cancel) {
        return ProbeResponse {
            failed: true,
            ..Default::default()
        };
    }

    let timeout_str = req.timeout_secs.to_string();
    let connect_str = req.timeout_secs.min(6).to_string();
    let mut cmd = Command::new("curl");
    let mut args = vec![
        "-q",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "-sS",
        "-i",
        "--compressed",
        "--connect-timeout",
        &connect_str,
        "--max-time",
        &timeout_str,
    ];
    if !req.no_redirect {
        args.extend(["-L", "--max-redirs", "8"]);
    }
    if req.method != "GET" {
        args.extend(["-X", req.method]);
    }
    args.extend(["-K", "-"]);

    cmd.args(&args)
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

    let start = Instant::now();
    let mut child = match crate::runtime::retry_executable_busy(ctx, || cmd.spawn()) {
        Ok(c) => c,
        Err(_) => {
            return ProbeResponse {
                failed: true,
                ..Default::default()
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let mut cfg = String::new();
        match transport {
            ProbeTransport::Proxy(proxy_url) => {
                cfg.push_str(&format!("proxy = \"{}\"\n", curl_escape(proxy_url)));
            }
            ProbeTransport::Direct => {
                cfg.push_str("noproxy = \"*\"\n");
            }
        }
        cfg.push_str(&format!(
            "user-agent = \"{}\"\n",
            curl_escape(BROWSER_USER_AGENT)
        ));
        for &(name, value) in req.headers {
            cfg.push_str(&format!("header = \"{}: {}\"\n", name, curl_escape(value)));
        }
        if let Some(body) = req.body {
            cfg.push_str(&format!("data-binary = \"{}\"\n", curl_escape(body)));
        }
        cfg.push_str(&format!("url = \"{}\"\n", curl_escape(req.url)));
        let _ = stdin.write_all(cfg.as_bytes());
    }

    let Some(stdout) = child.stdout.take() else {
        outbound_ip::kill_child(&mut child);
        return ProbeResponse {
            failed: true,
            ..Default::default()
        };
    };

    let follow_redirects = !req.no_redirect;
    let initial_url = req.url.to_owned();
    let max_body = req.max_body;
    let early_stop = Arc::new(AtomicBool::new(false));
    let early_stop_flag = early_stop.clone();

    // Streaming reader runs in a scoped helper thread; main thread enforces cancellation and reaps.
    let (res_tx, res_rx) = mpsc::channel::<Option<ParsedStreamResponse>>();
    let reader_handle = thread::spawn(move || {
        let parsed = read_http_stream(stdout, &initial_url, follow_redirects, max_body, start);
        if parsed.as_ref().is_some_and(|p| p.stopped_early) {
            early_stop_flag.store(true, Ordering::SeqCst);
        }
        let _ = res_tx.send(parsed);
    });

    let mut sleep_ms = 5u64;
    let parsed_opt = loop {
        if let Ok(parsed) = res_rx.try_recv() {
            if early_stop.load(Ordering::SeqCst) {
                outbound_ip::kill_child(&mut child);
            }
            break parsed;
        }
        if is_cancelled(ctx, local_cancel) {
            outbound_ip::kill_child(&mut child);
            let _ = reader_handle.join();
            return ProbeResponse {
                failed: true,
                ..Default::default()
            };
        }
        thread::sleep(Duration::from_millis(sleep_ms));
        sleep_ms = (sleep_ms * 2).min(25);
    };

    let status = child.wait().ok();
    let _ = reader_handle.join();

    if let Some(parsed) = parsed_opt {
        ProbeResponse {
            status_code: parsed.status_code,
            delay_ms: Some(parsed.delay_ms),
            body: parsed.body,
            final_url: parsed.final_url,
            location: parsed.location,
            timed_out: false,
            failed: false,
        }
    } else {
        let timed_out = status.and_then(|s| s.code()) == Some(28);
        ProbeResponse {
            timed_out,
            failed: !timed_out,
            ..Default::default()
        }
    }
}

/// Variant of `curl_service_request` that stops reading the HTTP stream the instant `until(tail)`
/// matches (used for multi-megabyte HTML pages like Gemini, Prime Video, and TikTok).
fn curl_service_scan<F>(
    ctx: &Context,
    transport: &ProbeTransport,
    url: &str,
    max_body: usize,
    until: F,
    local_cancel: &AtomicBool,
) -> ProbeResponse
where
    F: Fn(&str) -> bool + Send + 'static,
{
    if is_cancelled(ctx, local_cancel) {
        return ProbeResponse {
            failed: true,
            ..Default::default()
        };
    }
    let timeout_str = DEFAULT_PROBE_TIMEOUT_SECS.to_string();
    let mut cmd = Command::new("curl");
    cmd.args([
        "-q",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "-sS",
        "-i",
        "-L",
        "--max-redirs",
        "8",
        "--compressed",
        "--connect-timeout",
        "6",
        "--max-time",
        &timeout_str,
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

    let start = Instant::now();
    let mut child = match crate::runtime::retry_executable_busy(ctx, || cmd.spawn()) {
        Ok(c) => c,
        Err(_) => {
            return ProbeResponse {
                failed: true,
                ..Default::default()
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let mut cfg = String::new();
        match transport {
            ProbeTransport::Proxy(proxy_url) => {
                cfg.push_str(&format!("proxy = \"{}\"\n", curl_escape(proxy_url)));
            }
            ProbeTransport::Direct => {
                cfg.push_str("noproxy = \"*\"\n");
            }
        }
        cfg.push_str(&format!(
            "user-agent = \"{}\"\nurl = \"{}\"\n",
            curl_escape(BROWSER_USER_AGENT),
            curl_escape(url)
        ));
        let _ = stdin.write_all(cfg.as_bytes());
    }

    let Some(stdout) = child.stdout.take() else {
        outbound_ip::kill_child(&mut child);
        return ProbeResponse {
            failed: true,
            ..Default::default()
        };
    };

    let initial_url = url.to_owned();
    let (res_tx, res_rx) = mpsc::channel::<Option<ParsedStreamResponse>>();
    let reader_handle = thread::spawn(move || {
        let parsed =
            read_http_stream_until(stdout, &initial_url, true, max_body, start, Some(&until));
        let _ = res_tx.send(parsed);
    });

    let mut sleep_ms = 5u64;
    let parsed_opt = loop {
        if let Ok(parsed) = res_rx.try_recv() {
            if parsed.as_ref().is_some_and(|p| p.stopped_early) {
                outbound_ip::kill_child(&mut child);
            }
            break parsed;
        }
        if is_cancelled(ctx, local_cancel) {
            outbound_ip::kill_child(&mut child);
            let _ = reader_handle.join();
            return ProbeResponse {
                failed: true,
                ..Default::default()
            };
        }
        thread::sleep(Duration::from_millis(sleep_ms));
        sleep_ms = (sleep_ms * 2).min(25);
    };

    let status = child.wait().ok();
    let _ = reader_handle.join();

    if let Some(parsed) = parsed_opt {
        ProbeResponse {
            status_code: parsed.status_code,
            delay_ms: Some(parsed.delay_ms),
            body: parsed.body,
            final_url: parsed.final_url,
            location: parsed.location,
            timed_out: false,
            failed: false,
        }
    } else {
        let timed_out = status.and_then(|s| s.code()) == Some(28);
        ProbeResponse {
            timed_out,
            failed: !timed_out,
            ..Default::default()
        }
    }
}

struct ParsedStreamResponse {
    status_code: u16,
    delay_ms: u64,
    final_url: String,
    location: Option<String>,
    body: String,
    stopped_early: bool,
}

fn read_http_stream<R: Read>(
    reader: R,
    initial_url: &str,
    follow_redirects: bool,
    max_body: usize,
    start: Instant,
) -> Option<ParsedStreamResponse> {
    read_http_stream_until(reader, initial_url, follow_redirects, max_body, start, None)
}

fn find_header_end(buf: &[u8]) -> Option<(usize, usize)> {
    for i in 0..buf.len() {
        if i + 3 < buf.len() && &buf[i..i + 4] == b"\r\n\r\n" {
            return Some((i, i + 4));
        }
        if i + 1 < buf.len() && &buf[i..i + 2] == b"\n\n" {
            return Some((i, i + 2));
        }
    }
    None
}

fn parse_status_line(line: &str) -> Option<(u16, String)> {
    let trimmed = line.trim();
    if !trimmed.starts_with("HTTP/") {
        return None;
    }
    let mut parts = trimmed.splitn(3, ' ');
    let _proto = parts.next()?;
    let code = parts.next()?.parse::<u16>().ok()?;
    let reason = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    Some((code, reason))
}

fn parse_location_header(header_text: &str) -> Option<String> {
    for line in header_text.lines().skip(1) {
        let line = line.trim_end_matches('\r');
        if let Some((name, val)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("location")
        {
            let loc = val.trim();
            if !loc.is_empty() {
                return Some(loc.to_owned());
            }
        }
    }
    None
}

fn read_http_stream_until<R: Read>(
    mut reader: R,
    initial_url: &str,
    follow_redirects: bool,
    max_body: usize,
    start: Instant,
    until: Option<&dyn Fn(&str) -> bool>,
) -> Option<ParsedStreamResponse> {
    let mut raw = Vec::with_capacity(4096);
    let mut chunk = vec![0u8; SCAN_CHUNK_BYTES];
    let mut current_url = initial_url.to_owned();
    let mut body_bytes = Vec::new();
    let mut stopped_early = false;

    // Phase 1: Consume proxy CONNECT / 1xx / 3xx redirect header blocks until final headers finish.
    let mut blocks = 0usize;
    let (status_code, final_location, header_done_ms) = loop {
        while find_header_end(&raw).is_none() {
            if raw.len() >= MAX_HEADER_BYTES {
                return None;
            }
            let n = reader.read(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..n]);
        }
        let (hdr_end, after_hdr) = find_header_end(&raw)?;
        let header_text = String::from_utf8_lossy(&raw[..hdr_end]).into_owned();
        let remainder = raw.split_off(after_hdr);
        raw = remainder;
        blocks += 1;
        if blocks > 16 {
            return None;
        }

        let first_line = header_text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let (code, reason) = parse_status_line(first_line)?;
        let loc = parse_location_header(&header_text);

        // Skip proxy tunnel CONNECT headers and 1xx informational responses.
        if (100..200).contains(&code) || (code == 200 && reason.contains("connection established"))
        {
            continue;
        }
        // When following redirects (`-L`), another response block follows 3xx + Location.
        if follow_redirects
            && matches!(code, 301 | 302 | 303 | 307 | 308)
            && let Some(loc_val) = loc.as_deref()
        {
            if let Ok(base) = url::Url::parse(&current_url)
                && let Ok(joined) = base.join(loc_val)
            {
                current_url = joined.to_string();
            }
            continue;
        }

        break (code, loc, start.elapsed().as_millis().max(1) as u64);
    };

    // Phase 2: Bounded / early-stop body read.
    if max_body == 0 {
        stopped_early = true;
    } else {
        let initial_take = raw.len().min(max_body);
        body_bytes.extend_from_slice(&raw[..initial_take]);
        if let Some(pred) = until {
            let tail = String::from_utf8_lossy(&body_bytes);
            if pred(&tail) {
                stopped_early = true;
            }
        }
        if !stopped_early && body_bytes.len() < max_body {
            loop {
                let remaining = max_body.saturating_sub(body_bytes.len());
                if remaining == 0 {
                    stopped_early = true;
                    break;
                }
                let to_read = remaining.min(chunk.len());
                let Ok(n) = reader.read(&mut chunk[..to_read]) else {
                    break;
                };
                if n == 0 {
                    break;
                }
                let scan_start = body_bytes.len().saturating_sub(SCAN_OVERLAP_BYTES);
                body_bytes.extend_from_slice(&chunk[..n]);
                if let Some(pred) = until {
                    let tail = String::from_utf8_lossy(&body_bytes[scan_start..]);
                    if pred(&tail) {
                        stopped_early = true;
                        break;
                    }
                }
            }
        }
    }

    Some(ParsedStreamResponse {
        status_code,
        delay_ms: header_done_ms,
        final_url: current_url,
        location: final_location,
        body: String::from_utf8_lossy(&body_bytes).into_owned(),
        stopped_early,
    })
}

// ── ISO 3166-1 alpha-3 -> alpha-2 normalization & shared helpers ────────────────────────────────

pub(crate) fn normalize_region(code: &str) -> Option<String> {
    let upper = code.trim().to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if !bytes.iter().all(u8::is_ascii_uppercase) {
        return None;
    }
    match bytes.len() {
        2 => Some(upper),
        3 => {
            let table = ISO3166_PACKED.as_bytes();
            let mut low = 0usize;
            let mut high = table.len() / 5;
            while low < high {
                let mid = (low + high) / 2;
                let entry = &table[mid * 5..mid * 5 + 3];
                match entry.cmp(bytes) {
                    std::cmp::Ordering::Less => low = mid + 1,
                    std::cmp::Ordering::Greater => high = mid,
                    std::cmp::Ordering::Equal => {
                        return std::str::from_utf8(&table[mid * 5 + 3..mid * 5 + 5])
                            .ok()
                            .map(str::to_owned);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn trace_value<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    for line in body.lines() {
        if let Some(rest) = line.trim_end_matches('\r').strip_prefix(&prefix) {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn trace_ip(body: &str) -> Option<String> {
    let raw = trace_value(body, "ip")?;
    let addr: IpAddr = raw.parse().ok()?;
    let unmapped = match addr {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        other => other,
    };
    Some(unmapped.to_string())
}

fn status_from_code(code: u16) -> ServiceProbeStatus {
    match code {
        200..=299 => ServiceProbeStatus::Available,
        401 | 403 | 429 | 451 => ServiceProbeStatus::Restricted,
        _ => ServiceProbeStatus::Unavailable,
    }
}

fn base_item(target: ServiceTarget, resp: &ProbeResponse) -> Option<ServiceCheckItem> {
    if resp.timed_out {
        return Some(ServiceCheckItem {
            target,
            status: ServiceProbeStatus::Timeout,
            delay_ms: None,
            region: None,
            ip: None,
            exit_node: None,
            checked_at: Instant::now(),
        });
    }
    if resp.failed || resp.status_code == 0 {
        return Some(ServiceCheckItem {
            target,
            status: ServiceProbeStatus::Failed,
            delay_ms: None,
            region: None,
            ip: None,
            exit_node: None,
            checked_at: Instant::now(),
        });
    }
    None
}

fn ok_item(
    target: ServiceTarget,
    status: ServiceProbeStatus,
    delay_ms: Option<u64>,
    region: Option<String>,
    ip: Option<String>,
) -> ServiceCheckItem {
    ServiceCheckItem {
        target,
        status,
        delay_ms,
        region,
        ip,
        exit_node: None,
        checked_at: Instant::now(),
    }
}

fn body_contains_any(lower_body: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| lower_body.contains(n))
}

fn first_capture(re: &Regex, body: &str) -> Option<String> {
    re.captures(body)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_owned())
}

// ── Cached compiled regexes (AGENTS.md §4.2: zero per-call regex compilation) ───────────────────

fn youtube_region_patterns() -> &'static [Regex] {
    static PATS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATS.get_or_init(|| {
        vec![
            Regex::new(r#"(?i)id=["']country-code["'][^>]*>\s*([A-Za-z]{2,3})\s*<"#).unwrap(),
            Regex::new(r#""GL"\s*:\s*"([A-Za-z]{2})""#).unwrap(),
            Regex::new(r#""countryCode"\s*:\s*"([A-Za-z]{2})""#).unwrap(),
            Regex::new(r#""country_code"\s*:\s*"([A-Za-z]{2})""#).unwrap(),
        ]
    })
}

fn tiktok_region_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#""webapp\.app-context"\s*:\s*\{[^{}]*?"region"\s*:\s*"([^"]*)""#).unwrap()
    })
}

fn prime_region_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""currentTerritory":"([^"]+)""#).unwrap())
}

fn disney_assertion_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""assertion"\s*:\s*"([^"]+)""#).unwrap())
}

fn disney_refresh_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""refresh_token"\s*:\s*"([^"]+)""#).unwrap())
}

fn disney_country_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""countryCode"\s*:\s*"([^"]+)""#).unwrap())
}

fn disney_supported_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""inSupportedLocation"\s*:\s*(false|true)"#).unwrap())
}

fn spotify_country_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""countryCode"\s*:\s*"([^"]+)""#).unwrap())
}

fn discord_country_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#""country_code"\s*:\s*"([^"]+)""#).unwrap())
}

fn bilibili_code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^\s*\{\s*"code"\s*:\s*(-?\d+)"#).unwrap())
}

// ── 13 Service Checkers (1:1 parity with FlClash core/service_rules.go) ─────────────────────────

fn run_checker(env: &CurlServiceEnv<'_>, target: ServiceTarget) -> ServiceCheckItem {
    match target {
        ServiceTarget::Google => {
            check_reachable(env, target, "https://www.google.com/generate_204")
        }
        ServiceTarget::GitHub => check_reachable(env, target, "https://github.com/"),
        ServiceTarget::YouTube => check_youtube(env),
        ServiceTarget::ChatGpt => check_chatgpt(env),
        ServiceTarget::Claude => check_claude(env),
        ServiceTarget::Gemini => {
            let resp = curl_service_scan(
                env.ctx,
                env.transport,
                "https://gemini.google.com",
                SERVICE_SCAN_MAX_BODY,
                |tail| {
                    tail.find(GEMINI_REGION_MARKER)
                        .is_some_and(|idx| idx + GEMINI_REGION_MARKER.len() + 3 <= tail.len())
                },
                env.cancel,
            );
            eval_gemini(&resp)
        }
        ServiceTarget::Netflix => check_netflix(env),
        ServiceTarget::DisneyPlus => check_disney_plus(env),
        ServiceTarget::PrimeVideo => {
            let resp = curl_service_scan(
                env.ctx,
                env.transport,
                "https://www.primevideo.com",
                SERVICE_SCAN_MAX_BODY,
                |tail| tail.contains("isServiceRestricted") || prime_region_re().is_match(tail),
                env.cancel,
            );
            eval_prime_video(&resp)
        }
        ServiceTarget::Spotify => check_spotify(env),
        ServiceTarget::TikTok => {
            let resp = curl_service_scan(
                env.ctx,
                env.transport,
                "https://www.tiktok.com/",
                SERVICE_SCAN_MAX_BODY,
                |tail| tiktok_region_re().is_match(tail),
                env.cancel,
            );
            eval_tiktok(&resp)
        }
        ServiceTarget::Discord => check_discord(env),
        ServiceTarget::Bilibili => check_bilibili(env),
    }
}

fn check_reachable(env: &dyn ServiceHttpEnv, target: ServiceTarget, url: &str) -> ServiceCheckItem {
    let resp = env.send(&ProbeRequest::get(url, 0));
    if let Some(err) = base_item(target, &resp) {
        return err;
    }
    ok_item(
        target,
        status_from_code(resp.status_code),
        resp.delay_ms,
        None,
        None,
    )
}

fn check_claude(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    let resp = env.send(&ProbeRequest::get(
        "https://claude.ai/cdn-cgi/trace",
        SERVICE_SMALL_MAX_BODY,
    ));
    eval_claude(&resp)
}

fn eval_claude(resp: &ProbeResponse) -> ServiceCheckItem {
    if let Some(err) = base_item(ServiceTarget::Claude, resp) {
        return err;
    }
    let ip = trace_ip(&resp.body);
    let Some(region) = trace_value(&resp.body, "loc").and_then(normalize_region) else {
        return ok_item(
            ServiceTarget::Claude,
            ServiceProbeStatus::Failed,
            resp.delay_ms,
            None,
            ip,
        );
    };
    let status = if CLAUDE_BLOCKED_REGIONS.contains(&region.as_str()) {
        ServiceProbeStatus::UnsupportedRegion
    } else {
        ServiceProbeStatus::Available
    };
    ok_item(
        ServiceTarget::Claude,
        status,
        resp.delay_ms,
        Some(region),
        ip,
    )
}

fn eval_gemini(resp: &ProbeResponse) -> ServiceCheckItem {
    if let Some(err) = base_item(ServiceTarget::Gemini, resp) {
        return err;
    }
    let Some(idx) = resp.body.find(GEMINI_REGION_MARKER) else {
        return ok_item(
            ServiceTarget::Gemini,
            ServiceProbeStatus::Failed,
            resp.delay_ms,
            None,
            None,
        );
    };
    let start = idx + GEMINI_REGION_MARKER.len();
    let Some(code) = resp.body.get(start..start + 3) else {
        return ok_item(
            ServiceTarget::Gemini,
            ServiceProbeStatus::Failed,
            resp.delay_ms,
            None,
            None,
        );
    };
    if !code.bytes().all(|b| b.is_ascii_uppercase()) {
        return ok_item(
            ServiceTarget::Gemini,
            ServiceProbeStatus::Failed,
            resp.delay_ms,
            None,
            None,
        );
    }
    let region = normalize_region(code);
    let status = if GEMINI_BLOCKED_REGIONS.contains(&code) {
        ServiceProbeStatus::UnsupportedRegion
    } else {
        ServiceProbeStatus::Available
    };
    ok_item(ServiceTarget::Gemini, status, resp.delay_ms, region, None)
}

fn eval_chatgpt_ios_status(body: &str) -> ServiceProbeStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("you may be connected to a disallowed isp") {
        ServiceProbeStatus::DisallowedIsp
    } else if lower.contains("sorry, you have been blocked") {
        ServiceProbeStatus::Blocked
    } else if lower.contains("request is not allowed. please try again later.") {
        ServiceProbeStatus::Available
    } else {
        ServiceProbeStatus::Failed
    }
}

fn check_chatgpt(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    let ios = env.send(&ProbeRequest::get(
        "https://ios.chat.openai.com/",
        SERVICE_SMALL_MAX_BODY,
    ));
    let mut item = match base_item(ServiceTarget::ChatGpt, &ios) {
        Some(err) => err,
        None => ok_item(
            ServiceTarget::ChatGpt,
            eval_chatgpt_ios_status(&ios.body),
            ios.delay_ms,
            None,
            None,
        ),
    };

    let trace = env.send(&ProbeRequest::get(
        "https://chat.openai.com/cdn-cgi/trace",
        SERVICE_SMALL_MAX_BODY,
    ));
    if base_item(ServiceTarget::ChatGpt, &trace).is_none() {
        item.region = trace_value(&trace.body, "loc").and_then(normalize_region);
        item.ip = trace_ip(&trace.body);
        if item.delay_ms.is_none() {
            item.delay_ms = trace.delay_ms;
        }
    }

    let web = env.send(&ProbeRequest::get(
        "https://api.openai.com/compliance/cookie_requirements",
        SERVICE_SMALL_MAX_BODY,
    ));
    if base_item(ServiceTarget::ChatGpt, &web).is_none()
        && web
            .body
            .to_ascii_lowercase()
            .contains("unsupported_country")
    {
        item.status = ServiceProbeStatus::UnsupportedRegion;
    }
    item
}

fn check_youtube(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    let resp = env.send(&ProbeRequest::get(
        "https://www.youtube.com/premium?hl=en",
        SERVICE_HTML_MAX_BODY,
    ));
    eval_youtube(&resp)
}

fn eval_youtube(resp: &ProbeResponse) -> ServiceCheckItem {
    if let Some(err) = base_item(ServiceTarget::YouTube, resp) {
        return err;
    }
    let region = youtube_region_patterns()
        .iter()
        .find_map(|pat| first_capture(pat, &resp.body).and_then(|c| normalize_region(&c)));
    let lower = resp.body.to_ascii_lowercase();
    let status = if body_contains_any(
        &lower,
        &[
            "youtube premium is not available in your country",
            "premium is not available in your country",
            "premium is not available in your region",
        ],
    ) {
        ServiceProbeStatus::UnsupportedRegion
    } else if (200..300).contains(&resp.status_code)
        && body_contains_any(
            &lower,
            &["youtube premium", "ad-free", "\"browseid\":\"spunlimited\""],
        )
    {
        ServiceProbeStatus::Available
    } else {
        status_from_code(resp.status_code)
    };
    ok_item(ServiceTarget::YouTube, status, resp.delay_ms, region, None)
}

fn check_spotify(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    let resp = env.send(&ProbeRequest::get(
        "https://www.spotify.com/api/content/v1/country-selector?platform=web&format=json",
        SERVICE_SMALL_MAX_BODY,
    ));
    eval_spotify(&resp)
}

fn spotify_region(resp: &ProbeResponse) -> Option<String> {
    if let Ok(parsed) = url::Url::parse(&resp.final_url) {
        let first = parsed
            .path()
            .trim_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        if !first.is_empty() && first != "api" {
            let candidate = first.split('-').next().unwrap_or("");
            if let Some(reg) = normalize_region(candidate) {
                return Some(reg);
            }
        }
    }
    first_capture(spotify_country_re(), &resp.body).and_then(|c| normalize_region(&c))
}

fn eval_spotify(resp: &ProbeResponse) -> ServiceCheckItem {
    if let Some(err) = base_item(ServiceTarget::Spotify, resp) {
        return err;
    }
    let region = spotify_region(resp);
    let lower = resp.body.to_ascii_lowercase();
    let status = if matches!(resp.status_code, 403 | 451) {
        ServiceProbeStatus::UnsupportedRegion
    } else if !(200..300).contains(&resp.status_code) {
        status_from_code(resp.status_code)
    } else if lower.contains("not available in your country") {
        ServiceProbeStatus::UnsupportedRegion
    } else {
        ServiceProbeStatus::Available
    };
    ok_item(ServiceTarget::Spotify, status, resp.delay_ms, region, None)
}

fn eval_tiktok(resp: &ProbeResponse) -> ServiceCheckItem {
    if let Some(err) = base_item(ServiceTarget::TikTok, resp) {
        return err;
    }
    let lower = resp.body.to_ascii_lowercase();
    let status = if matches!(resp.status_code, 403 | 451) {
        ServiceProbeStatus::UnsupportedRegion
    } else if !(200..300).contains(&resp.status_code) {
        ServiceProbeStatus::Failed
    } else if body_contains_any(
        &lower,
        &[
            "access denied",
            "not available in your region",
            "tiktok is not available",
        ],
    ) {
        ServiceProbeStatus::UnsupportedRegion
    } else {
        ServiceProbeStatus::Available
    };
    let region = first_capture(tiktok_region_re(), &resp.body).and_then(|c| normalize_region(&c));
    ok_item(ServiceTarget::TikTok, status, resp.delay_ms, region, None)
}

fn eval_bilibili_status(body: &str) -> ServiceProbeStatus {
    match first_capture(bilibili_code_re(), body).as_deref() {
        Some("0") => ServiceProbeStatus::Available,
        Some("-10403") => ServiceProbeStatus::UnsupportedRegion,
        _ => ServiceProbeStatus::Failed,
    }
}

fn check_bilibili(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    const MAINLAND: &str = "https://api.bilibili.com/pgc/player/web/playurl?avid=82846771&qn=0&type=&otype=json&ep_id=307247&fourk=1&fnver=0&fnval=16&module=bangumi";
    const HK_MO_TW: &str = "https://api.bilibili.com/pgc/player/web/playurl?avid=18281381&cid=29892777&qn=0&type=&otype=json&ep_id=183799&fourk=1&fnver=0&fnval=16&module=bangumi";
    let first = env.send(&ProbeRequest::get(MAINLAND, SERVICE_SMALL_MAX_BODY));
    if let Some(err) = base_item(ServiceTarget::Bilibili, &first) {
        return err;
    }
    match eval_bilibili_status(&first.body) {
        ServiceProbeStatus::Available => ok_item(
            ServiceTarget::Bilibili,
            ServiceProbeStatus::Available,
            first.delay_ms,
            Some("CN".to_owned()),
            None,
        ),
        ServiceProbeStatus::UnsupportedRegion => {
            let second = env.send(&ProbeRequest::get(HK_MO_TW, SERVICE_SMALL_MAX_BODY));
            if let Some(err) = base_item(ServiceTarget::Bilibili, &second) {
                return err;
            }
            ok_item(
                ServiceTarget::Bilibili,
                eval_bilibili_status(&second.body),
                second.delay_ms,
                None,
                None,
            )
        }
        other => ok_item(ServiceTarget::Bilibili, other, first.delay_ms, None, None),
    }
}

fn check_discord(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    let resp = env.send(&ProbeRequest::get(
        "https://discord.com/api/v9/auth/location-metadata",
        SERVICE_SMALL_MAX_BODY,
    ));
    if let Some(err) = base_item(ServiceTarget::Discord, &resp) {
        return err;
    }
    let status = status_from_code(resp.status_code);
    let region = if status == ServiceProbeStatus::Available {
        first_capture(discord_country_re(), &resp.body).and_then(|c| normalize_region(&c))
    } else {
        None
    };
    ok_item(ServiceTarget::Discord, status, resp.delay_ms, region, None)
}

fn eval_prime_video(resp: &ProbeResponse) -> ServiceCheckItem {
    if let Some(err) = base_item(ServiceTarget::PrimeVideo, resp) {
        return err;
    }
    if resp.body.contains("isServiceRestricted") {
        return ok_item(
            ServiceTarget::PrimeVideo,
            ServiceProbeStatus::UnsupportedRegion,
            resp.delay_ms,
            None,
            None,
        );
    }
    let Some(region) =
        first_capture(prime_region_re(), &resp.body).and_then(|c| normalize_region(&c))
    else {
        return ok_item(
            ServiceTarget::PrimeVideo,
            ServiceProbeStatus::Failed,
            resp.delay_ms,
            None,
            None,
        );
    };
    ok_item(
        ServiceTarget::PrimeVideo,
        ServiceProbeStatus::Available,
        resp.delay_ms,
        Some(region),
        None,
    )
}

fn check_netflix(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    const FAST_URL: &str = "https://api.fast.com/netflix/speedtest/v2?https=true&token=YXNkZmFzZGxmbnNkYWZoYXNkZmhrYWxm&urlCount=5";
    let cdn = env.send(&ProbeRequest::get(FAST_URL, SERVICE_SMALL_MAX_BODY));
    if base_item(ServiceTarget::Netflix, &cdn).is_none() {
        if cdn.status_code == 403 {
            return ok_item(
                ServiceTarget::Netflix,
                ServiceProbeStatus::Blocked,
                cdn.delay_ms,
                None,
                None,
            );
        }
        if let Ok(json) = serde_json::from_str::<Json>(&cdn.body)
            && let Some(country) = json
                .get("targets")
                .and_then(Json::as_array)
                .and_then(|arr| arr.first())
                .and_then(|t| t.get("location"))
                .and_then(|l| l.get("country"))
                .and_then(Json::as_str)
        {
            return ok_item(
                ServiceTarget::Netflix,
                ServiceProbeStatus::Available,
                cdn.delay_ms,
                normalize_region(country),
                None,
            );
        }
    }

    let first = env.send(&ProbeRequest::get(
        "https://www.netflix.com/title/81280792",
        0,
    ));
    if let Some(err) = base_item(ServiceTarget::Netflix, &first) {
        return err;
    }
    let second = env.send(&ProbeRequest::get(
        "https://www.netflix.com/title/70143836",
        0,
    ));
    if let Some(err) = base_item(ServiceTarget::Netflix, &second) {
        return err;
    }

    let (status, region) = if first.status_code == 404 && second.status_code == 404 {
        (ServiceProbeStatus::OriginalsOnly, None)
    } else if first.status_code == 403 || second.status_code == 403 {
        (ServiceProbeStatus::UnsupportedRegion, None)
    } else if matches!(first.status_code, 200 | 301) || matches!(second.status_code, 200 | 301) {
        let mut loc_req = ProbeRequest::get("https://www.netflix.com/title/80018499", 0);
        loc_req.no_redirect = true;
        let loc_resp = env.send(&loc_req);
        let reg = loc_resp.location.as_deref().and_then(|loc| {
            let seg = loc.split('/').nth(3)?;
            normalize_region(seg.split('-').next()?)
        });
        (ServiceProbeStatus::Available, reg)
    } else {
        (status_from_code(first.status_code), None)
    };

    ok_item(ServiceTarget::Netflix, status, first.delay_ms, region, None)
}

fn percent_encode_form(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn check_disney_plus(env: &dyn ServiceHttpEnv) -> ServiceCheckItem {
    let device = env.send(&ProbeRequest {
        method: "POST",
        url: "https://disney.api.edge.bamgrid.com/devices",
        headers: &[
            ("authorization", DISNEY_AUTH_HEADER),
            ("content-type", "application/json; charset=UTF-8"),
        ],
        body: Some(
            r#"{"deviceFamily":"browser","applicationRuntime":"chrome","deviceProfile":"windows","attributes":{}}"#,
        ),
        max_body: SERVICE_SMALL_MAX_BODY,
        no_redirect: false,
        timeout_secs: DEFAULT_PROBE_TIMEOUT_SECS,
    });
    if let Some(err) = base_item(ServiceTarget::DisneyPlus, &device) {
        return err;
    }
    if device.status_code == 403 {
        return ok_item(
            ServiceTarget::DisneyPlus,
            ServiceProbeStatus::Blocked,
            device.delay_ms,
            None,
            None,
        );
    }
    let Some(assertion) = first_capture(disney_assertion_re(), &device.body) else {
        return ok_item(
            ServiceTarget::DisneyPlus,
            ServiceProbeStatus::Failed,
            device.delay_ms,
            None,
            None,
        );
    };

    let form = format!(
        "grant_type=urn:ietf:params:oauth:grant-type:token-exchange&latitude=0&longitude=0&platform=browser&subject_token={}&subject_token_type=urn:bamtech:params:oauth:token-type:device",
        percent_encode_form(&assertion)
    );
    let token = env.send(&ProbeRequest {
        method: "POST",
        url: "https://disney.api.edge.bamgrid.com/token",
        headers: &[
            ("authorization", DISNEY_AUTH_HEADER),
            ("content-type", "application/x-www-form-urlencoded"),
        ],
        body: Some(&form),
        max_body: SERVICE_SMALL_MAX_BODY,
        no_redirect: false,
        timeout_secs: DEFAULT_PROBE_TIMEOUT_SECS,
    });
    if let Some(err) = base_item(ServiceTarget::DisneyPlus, &token) {
        return err;
    }
    if token.body.contains("forbidden-location") || token.body.contains("403 ERROR") {
        return ok_item(
            ServiceTarget::DisneyPlus,
            ServiceProbeStatus::Blocked,
            device.delay_ms,
            None,
            None,
        );
    }
    let Some(refresh) = first_capture(disney_refresh_re(), &token.body) else {
        return ok_item(
            ServiceTarget::DisneyPlus,
            ServiceProbeStatus::Failed,
            device.delay_ms,
            None,
            None,
        );
    };

    let query = format!(
        r#"{{"query":"mutation refreshToken($input: RefreshTokenInput!) {{ refreshToken(refreshToken: $input) {{ activeSession {{ sessionId }} }} }}","variables":{{"input":{{"refreshToken":"{}"}}}}}}"#,
        refresh.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let session = env.send(&ProbeRequest {
        method: "POST",
        url: "https://disney.api.edge.bamgrid.com/graph/v1/device/graphql",
        headers: &[
            ("authorization", DISNEY_AUTH_HEADER),
            ("content-type", "application/json"),
        ],
        body: Some(&query),
        max_body: SERVICE_SMALL_MAX_BODY,
        no_redirect: false,
        timeout_secs: DEFAULT_PROBE_TIMEOUT_SECS,
    });
    if base_item(ServiceTarget::DisneyPlus, &session).is_some()
        || session.body.is_empty()
        || session.status_code >= 400
    {
        return ok_item(
            ServiceTarget::DisneyPlus,
            ServiceProbeStatus::Failed,
            device.delay_ms,
            None,
            None,
        );
    }
    let Some(region) =
        first_capture(disney_country_re(), &session.body).and_then(|c| normalize_region(&c))
    else {
        return ok_item(
            ServiceTarget::DisneyPlus,
            ServiceProbeStatus::Unavailable,
            device.delay_ms,
            None,
            None,
        );
    };
    let status = match first_capture(disney_supported_re(), &session.body).as_deref() {
        Some("true") => ServiceProbeStatus::Available,
        Some("false") => ServiceProbeStatus::ComingSoon,
        _ => ServiceProbeStatus::Failed,
    };
    ok_item(
        ServiceTarget::DisneyPlus,
        status,
        device.delay_ms,
        Some(region),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::clash::model::ProxyGroupKind;
    use std::io::Cursor;

    #[test]
    fn normalize_region_handles_alpha2_alpha3_and_invalid_inputs() {
        for (input, want) in [
            ("US", Some("US")),
            ("jp", Some("JP")),
            ("CHN", Some("CN")),
            ("usa", Some("US")),
            ("HKG", Some("HK")),
            ("MAC", Some("MO")),
            ("ZZZ", None),
            ("C", None),
            ("CNAAA", None),
            ("", None),
            ("1A", None),
        ] {
            assert_eq!(normalize_region(input).as_deref(), want, "{input}");
        }
        for code in GEMINI_BLOCKED_REGIONS {
            assert!(normalize_region(code).is_some(), "missing alpha-3 {code}");
        }
        for code in CLAUDE_BLOCKED_REGIONS {
            assert_eq!(normalize_region(code).as_deref(), Some(*code));
        }
    }

    #[test]
    fn http_stream_parser_skips_connect_and_redirects_and_supports_early_stop() {
        let raw = concat!(
            "HTTP/1.1 200 Connection established\r\nProxy-Agent: mihomo\r\n\r\n",
            "HTTP/1.1 302 Found\r\nLocation: /de-de/\r\n\r\n",
            "HTTP/2 200\r\nContent-Type: text/html\r\n\r\n",
            "hello world ,2,1,200,\"JPN\" trailing ignored data"
        );
        let parsed = read_http_stream_until(
            Cursor::new(raw.as_bytes()),
            "https://www.spotify.com/api/content/v1/country-selector",
            true,
            1024,
            Instant::now(),
            Some(&|tail| tail.contains(",2,1,200,\"JPN\"")),
        )
        .unwrap();
        assert_eq!(parsed.status_code, 200);
        assert_eq!(parsed.final_url, "https://www.spotify.com/de-de/");
        assert!(parsed.stopped_early);
        assert!(parsed.body.contains(",2,1,200,\"JPN\""));
    }

    #[test]
    fn service_rule_evaluators_match_flclash_semantics() {
        // Claude trace IP + blocked region
        let claude_ok = eval_claude(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(120),
            body: "fl=1\nip=198.51.100.9\nloc=US\n".into(),
            ..Default::default()
        });
        assert_eq!(claude_ok.status, ServiceProbeStatus::Available);
        assert_eq!(claude_ok.region.as_deref(), Some("US"));
        assert_eq!(claude_ok.ip.as_deref(), Some("198.51.100.9"));

        let claude_hk = eval_claude(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(90),
            body: "ip=::ffff:203.0.113.7\nloc=HK\n".into(),
            ..Default::default()
        });
        assert_eq!(claude_hk.status, ServiceProbeStatus::UnsupportedRegion);
        assert_eq!(claude_hk.ip.as_deref(), Some("203.0.113.7"));

        // ChatGPT iOS statuses
        assert_eq!(
            eval_chatgpt_ios_status("Request is not allowed. Please try again later."),
            ServiceProbeStatus::Available
        );
        assert_eq!(
            eval_chatgpt_ios_status("You may be connected to a disallowed ISP"),
            ServiceProbeStatus::DisallowedIsp
        );
        assert_eq!(
            eval_chatgpt_ios_status("Sorry, you have been blocked"),
            ServiceProbeStatus::Blocked
        );
        assert_eq!(
            eval_chatgpt_ios_status("<html>unexpected</html>"),
            ServiceProbeStatus::Failed
        );

        // Gemini alpha-3 codes
        let gem_jp = eval_gemini(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(300),
            body: format!("[[{GEMINI_REGION_MARKER}JPN\"]]"),
            ..Default::default()
        });
        assert_eq!(gem_jp.status, ServiceProbeStatus::Available);
        assert_eq!(gem_jp.region.as_deref(), Some("JP"));

        let gem_cn = eval_gemini(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(300),
            body: format!("[[{GEMINI_REGION_MARKER}CHN\"]]"),
            ..Default::default()
        });
        assert_eq!(gem_cn.status, ServiceProbeStatus::UnsupportedRegion);
        assert_eq!(gem_cn.region.as_deref(), Some("CN"));

        // YouTube Premium
        let yt = eval_youtube(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(200),
            body: r#"<span id="country-code"> SG </span> YouTube Premium ad-free"#.into(),
            ..Default::default()
        });
        assert_eq!(yt.status, ServiceProbeStatus::Available);
        assert_eq!(yt.region.as_deref(), Some("SG"));

        // TikTok app-context region vs serving cluster
        let tt = eval_tiktok(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(150),
            body: r#"{"region":"EU-TTP2"}{"webapp.app-context":{"language":"en","region":"GB"}}"#
                .into(),
            ..Default::default()
        });
        assert_eq!(tt.status, ServiceProbeStatus::Available);
        assert_eq!(tt.region.as_deref(), Some("GB"));

        // Spotify redirect path preferred over JSON body
        let sp = eval_spotify(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(180),
            final_url: "https://www.spotify.com/de-de/".into(),
            body: r#"{"countryCode":"US"}"#.into(),
            ..Default::default()
        });
        assert_eq!(sp.status, ServiceProbeStatus::Available);
        assert_eq!(sp.region.as_deref(), Some("DE"));

        // Prime Video restricted vs available
        let pv = eval_prime_video(&ProbeResponse {
            status_code: 200,
            delay_ms: Some(210),
            body: r#"{"currentTerritory":"NL"}"#.into(),
            ..Default::default()
        });
        assert_eq!(pv.status, ServiceProbeStatus::Available);
        assert_eq!(pv.region.as_deref(), Some("NL"));

        // Bilibili status codes
        assert_eq!(
            eval_bilibili_status(r#"{"code":0,"message":"success"}"#),
            ServiceProbeStatus::Available
        );
        assert_eq!(
            eval_bilibili_status(r#"{"code":-10403,"message":"region locked"}"#),
            ServiceProbeStatus::UnsupportedRegion
        );
        assert_eq!(
            eval_bilibili_status(r#"{"code":-412}"#),
            ServiceProbeStatus::Failed
        );
    }

    #[test]
    fn route_context_resolves_deterministic_exit_and_stops_at_opaque_rules() {
        let groups = vec![
            ProxyGroupItem {
                name: "GLOBAL".into(),
                kind: ProxyGroupKind::Selector,
                now: "Proxy".into(),
                resolved_chain: vec!["Proxy".into(), "SG-01".into()],
                fixed: None,
                hidden: false,
                members: vec![],
            },
            ProxyGroupItem {
                name: "AI".into(),
                kind: ProxyGroupKind::Selector,
                now: "US-01".into(),
                resolved_chain: vec!["US-01".into()],
                fixed: None,
                hidden: false,
                members: vec![],
            },
        ];
        let direct_ctx = RouteContext {
            mode: "direct".into(),
            rules: vec![],
            groups: groups.clone(),
        };
        assert_eq!(
            direct_ctx
                .resolve_exit_node(ServiceTarget::Google)
                .as_deref(),
            Some("DIRECT")
        );

        let global_ctx = RouteContext {
            mode: "global".into(),
            rules: vec![],
            groups: groups.clone(),
        };
        assert_eq!(
            global_ctx
                .resolve_exit_node(ServiceTarget::GitHub)
                .as_deref(),
            Some("SG-01")
        );

        let rule_ctx = RouteContext {
            mode: "rule".into(),
            rules: vec![
                RuleItem {
                    index: 1,
                    kind: "DomainSuffix".into(),
                    payload: "openai.com".into(),
                    proxy: "AI".into(),
                },
                RuleItem {
                    index: 2,
                    kind: "RuleSet".into(),
                    payload: "proxy".into(),
                    proxy: "GLOBAL".into(),
                },
                RuleItem {
                    index: 3,
                    kind: "Match".into(),
                    payload: String::new(),
                    proxy: "GLOBAL".into(),
                },
            ],
            groups,
        };
        // Explicit rule before RuleSet resolves cleanly.
        assert_eq!(
            rule_ctx
                .resolve_exit_node(ServiceTarget::ChatGpt)
                .as_deref(),
            Some("US-01")
        );
        // Host falling past opaque RuleSet returns None instead of guessing.
        assert_eq!(rule_ctx.resolve_exit_node(ServiceTarget::GitHub), None);
    }

    #[test]
    fn early_failure_confirmation_retries_once_and_recovers() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let cancel = AtomicBool::new(false);
        let mut attempts = 0usize;
        let item = check_with_confirmation(
            &ctx,
            &cancel,
            Duration::from_secs(3),
            Duration::from_millis(10),
            || {
                attempts += 1;
                if attempts == 1 {
                    ok_item(
                        ServiceTarget::Google,
                        ServiceProbeStatus::Failed,
                        None,
                        None,
                        None,
                    )
                } else {
                    ok_item(
                        ServiceTarget::Google,
                        ServiceProbeStatus::Available,
                        Some(42),
                        None,
                        None,
                    )
                }
            },
        );
        assert_eq!(attempts, 2);
        assert_eq!(item.status, ServiceProbeStatus::Available);

        // Slow failures past the early threshold are not retried redundantly.
        let mut slow_attempts = 0usize;
        let slow = check_with_confirmation(
            &ctx,
            &cancel,
            Duration::ZERO,
            Duration::from_millis(10),
            || {
                slow_attempts += 1;
                ok_item(
                    ServiceTarget::Google,
                    ServiceProbeStatus::Failed,
                    None,
                    None,
                    None,
                )
            },
        );
        assert_eq!(slow_attempts, 1);
        assert_eq!(slow.status, ServiceProbeStatus::Failed);
    }
}

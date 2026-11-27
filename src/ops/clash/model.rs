//! Domain data model and pure formatting/parsing helpers for the dedicated Mihomo TUI workspace.

use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, time::Instant};

/// Maximum 1-second traffic history samples retained for real-time traffic graphs (5 minutes).
pub const MAX_TRAFFIC_HISTORY: usize = 300;
/// Maximum closed connections retained in memory for the Requests tab.
pub const MAX_CLOSED_CONNECTIONS: usize = 200;
/// Maximum redacted log lines loaded into the embedded Tools log viewer.
pub const MAX_CLASH_LOG_LINES: usize = 500;

/// Top-level tabs inside the dedicated Mihomo workspace (6-tab FlClash-inspired cadence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClashTab {
    Overview,
    Proxies,
    Profiles,
    Connections,
    Rules,
    Tools,
}

impl ClashTab {
    pub const ALL: [Self; 6] = [
        Self::Overview,
        Self::Proxies,
        Self::Profiles,
        Self::Connections,
        Self::Rules,
        Self::Tools,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Proxies => 1,
            Self::Profiles => 2,
            Self::Connections => 3,
            Self::Rules => 4,
            Self::Tools => 5,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        Self::ALL.get(idx).copied().unwrap_or(Self::Overview)
    }

    pub fn from_cli(value: &str) -> Option<Self> {
        match value {
            "overview" => Some(Self::Overview),
            "proxies" => Some(Self::Proxies),
            "profiles" => Some(Self::Profiles),
            "connections" => Some(Self::Connections),
            "rules" => Some(Self::Rules),
            "tools" => Some(Self::Tools),
            _ => None,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Overview => "clash.tab.overview",
            Self::Proxies => "clash.tab.proxies",
            Self::Profiles => "clash.tab.profiles",
            Self::Connections => "clash.tab.connections",
            Self::Rules => "clash.tab.rules",
            Self::Tools => "clash.tab.tools",
        }
    }
}

/// Reachability state of the local Mihomo core and REST controller.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ClashAvailability {
    #[default]
    Loading,
    NotInstalled,
    Stopped,
    ControllerUnconfigured,
    ControllerUnreachable(String),
    UnknownProcess,
    Ready,
}

impl ClashAvailability {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready)
    }
}

/// Subscription quota and expiration metadata parsed from the `subscription-userinfo` HTTP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SubscriptionUserinfo {
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    #[serde(default)]
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire: Option<u64>,
}

impl SubscriptionUserinfo {
    /// Parse a Clash/Mihomo `subscription-userinfo` header value such as
    /// `upload=123; download=456; total=789; expire=1795046400`.
    pub fn parse_header(raw: &str) -> Option<Self> {
        let mut upload = 0u64;
        let mut download = 0u64;
        let mut total = 0u64;
        let mut expire = None;
        let mut seen = false;

        let trimmed = raw.trim().trim_matches(|c| matches!(c, '"' | '\''));
        let parts: Vec<&str> = if trimmed.contains(';') || trimmed.contains(',') {
            trimmed.split([';', ',']).collect()
        } else {
            trimmed.split_whitespace().collect()
        };

        for part in parts {
            let Some((key, value)) = part.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let val_str = value.trim().trim_matches(|c| matches!(c, '"' | '\''));
            // Some providers emit scientific or decimal strings (e.g. `12345.0`); truncate safely.
            let parsed = val_str.parse::<u64>().ok().or_else(|| {
                val_str
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0 && *v <= u64::MAX as f64)
                    .map(|v| v as u64)
            });
            let Some(num) = parsed else {
                continue;
            };
            match key.as_str() {
                "upload" => {
                    upload = num;
                    seen = true;
                }
                "download" => {
                    download = num;
                    seen = true;
                }
                "total" => {
                    total = num;
                    seen = true;
                }
                "expire" => {
                    if num > 0 {
                        // Normalize 13-digit millisecond timestamps some panels emit.
                        let secs = if num >= 10_000_000_000 {
                            num / 1000
                        } else {
                            num
                        };
                        if secs > 0 {
                            expire = Some(secs);
                        }
                    }
                    seen = true;
                }
                _ => {}
            }
        }

        if seen && (total > 0 || upload > 0 || download > 0 || expire.is_some()) {
            Some(Self {
                upload,
                download,
                total,
                expire,
            })
        } else {
            None
        }
    }

    /// Extract the last `subscription-userinfo` header value from a raw HTTP header dump
    /// (which may contain multiple redirect blocks), falling back to `x-subscription-userinfo`
    /// or `profile-userinfo` when the canonical header is absent.
    pub fn extract_from_http_headers(raw_headers: &str) -> Option<Self> {
        let mut primary = None;
        let mut fallback = None;
        for line in raw_headers.lines() {
            let line = line.trim_end_matches('\r');
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if name.eq_ignore_ascii_case("subscription-userinfo") {
                if let Some(parsed) = Self::parse_header(value) {
                    primary = Some(parsed);
                }
            } else if (name.eq_ignore_ascii_case("x-subscription-userinfo")
                || name.eq_ignore_ascii_case("profile-userinfo"))
                && let Some(parsed) = Self::parse_header(value)
            {
                fallback = Some(parsed);
            }
        }
        primary.or(fallback)
    }

    pub fn used(&self) -> u64 {
        self.upload.saturating_add(self.download)
    }

    pub fn remaining(&self) -> u64 {
        self.total.saturating_sub(self.used())
    }

    pub fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used() as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }

    pub fn expire_date(&self) -> Option<String> {
        self.expire.map(format_epoch_ymd)
    }

    pub fn days_until_expire(&self, now_epoch_secs: u64) -> Option<u64> {
        self.expire
            .map(|exp| exp.saturating_sub(now_epoch_secs) / 86_400)
    }
}

/// Origin of the subscription quota shown for the active Mihomo profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserinfoSource {
    Profile,
    Provider(String),
    Providers(usize),
}

/// Local subscription profile summary (never exposes raw subscription URL tokens).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSummary {
    pub name: String,
    pub host: String,
    pub is_local: bool,
    pub active: bool,
    pub updated: String,
    pub updated_epoch: Option<u64>,
    pub userinfo: Option<SubscriptionUserinfo>,
    pub file_bytes: Option<u64>,
}

/// Mihomo policy group classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyGroupKind {
    Selector,
    UrlTest,
    Fallback,
    LoadBalance,
    Relay,
    Other(String),
}

impl ProxyGroupKind {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "selector" | "select" => Self::Selector,
            "urltest" | "url-test" => Self::UrlTest,
            "fallback" => Self::Fallback,
            "loadbalance" | "load-balance" => Self::LoadBalance,
            "relay" => Self::Relay,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Selector => "Selector",
            Self::UrlTest => "URLTest",
            Self::Fallback => "Fallback",
            Self::LoadBalance => "LoadBalance",
            Self::Relay => "Relay",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Manual selection is supported on `Selector`, and modern Mihomo also supports temporary
    /// node pinning on `URLTest` and `Fallback`.
    pub fn supports_select(&self) -> bool {
        matches!(self, Self::Selector | Self::UrlTest | Self::Fallback)
    }

    /// Whether `DELETE /proxies/{group}` can reset a temporary pinned selection back to automatic.
    pub fn supports_unfix(&self) -> bool {
        matches!(self, Self::UrlTest | Self::Fallback)
    }
}

/// One node or nested group member inside a policy group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyNodeItem {
    pub name: String,
    pub kind: String,
    pub delay_ms: Option<u64>,
    /// True when the node has a recorded test result (including timeout = 0ms).
    pub tested: bool,
    pub is_group: bool,
    pub now: Option<String>,
    pub udp: bool,
    pub alive: bool,
    pub original_index: usize,
}

/// One policy group returned by Mihomo's `GET /proxies`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyGroupItem {
    pub name: String,
    pub kind: ProxyGroupKind,
    pub now: String,
    pub resolved_chain: Vec<String>,
    pub fixed: Option<String>,
    pub hidden: bool,
    pub members: Vec<ProxyNodeItem>,
}

/// External proxy provider returned by `GET /providers/proxies`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyProviderItem {
    pub name: String,
    pub vehicle_type: String,
    pub node_count: usize,
    pub updated_at: String,
    pub userinfo: Option<SubscriptionUserinfo>,
}

/// External rule provider returned by `GET /providers/rules`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleProviderItem {
    pub name: String,
    pub behavior: String,
    pub vehicle_type: String,
    pub rule_count: usize,
    pub updated_at: String,
}

/// One routing rule returned by `GET /rules`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleItem {
    pub index: usize,
    pub kind: String,
    pub payload: String,
    pub proxy: String,
}

/// Active or recently closed Mihomo connection entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionItem {
    pub id: String,
    pub network: String,
    pub conn_type: String,
    pub source: String,
    pub destination: String,
    pub host: String,
    pub process: String,
    pub rule: String,
    pub rule_payload: String,
    pub chains: Vec<String>,
    pub exit_node: String,
    pub upload: u64,
    pub download: u64,
    pub up_speed: u64,
    pub down_speed: u64,
    pub start: String,
    pub closed: bool,
}

impl ConnectionItem {
    pub fn total_bytes(&self) -> u64 {
        self.upload.saturating_add(self.download)
    }

    pub fn display_process(&self) -> &str {
        if !self.process.is_empty() {
            &self.process
        } else if !self.conn_type.is_empty() {
            &self.conn_type
        } else {
            "-"
        }
    }

    pub fn display_target(&self) -> &str {
        if !self.host.is_empty() {
            &self.host
        } else if !self.destination.is_empty() {
            &self.destination
        } else {
            "-"
        }
    }
}

/// Single 1-second traffic speed point for real-time traffic graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrafficSample {
    pub up_bps: u64,
    pub down_bps: u64,
    pub active_conns: u32,
}

/// Rolling summary statistics computed over a recent traffic history window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrafficWindowStats {
    pub avg_up: u64,
    pub avg_down: u64,
    pub peak_up: u64,
    pub peak_down: u64,
    pub sample_count: usize,
}

/// Aggregated live throughput and cumulative volume for one exit node, rule, or process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficBucket {
    pub name: String,
    pub down_speed: u64,
    pub up_speed: u64,
    pub download: u64,
    pub upload: u64,
    pub conn_count: usize,
}

impl TrafficBucket {
    pub fn total_speed(&self) -> u64 {
        self.down_speed.saturating_add(self.up_speed)
    }

    pub fn total_bytes(&self) -> u64 {
        self.download.saturating_add(self.upload)
    }
}

/// Runtime metadata merged from local Mihomo state/config and `/version` + `/configs`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClashRuntimeSummary {
    pub version: String,
    pub mode: String,
    pub mixed_port: Option<u16>,
    pub http_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub allow_lan: bool,
    pub tun_enabled: bool,
    pub tun_stack: String,
    pub controller_url: Option<String>,
    pub controller_has_secret: bool,
    pub uptime_secs: Option<u64>,
    pub pid: Option<u32>,
    pub socks5_listen: Option<String>,
    pub socks5_auth: bool,
    pub panel_installed: bool,
    pub panel_name: Option<String>,
    pub panel_version: Option<String>,
    pub panel_url: Option<String>,
    pub panel_bind: Option<String>,
    pub panel_loopback: bool,
    pub active_profile: Option<String>,
}

/// IP network classification reported by third-party IP intelligence APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IpType {
    Residential,
    Mobile,
    Business,
    Hosting,
}

impl IpType {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Residential => "clash.outbound.type_residential",
            Self::Mobile => "clash.outbound.type_mobile",
            Self::Business => "clash.outbound.type_business",
            Self::Hosting => "clash.outbound.type_hosting",
        }
    }
}

/// Overall IP quality grade derived from risk flags and network type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IpQualityLevel {
    Good,
    Normal,
    Risky,
}

impl IpQualityLevel {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Good => "clash.outbound.level_good",
            Self::Normal => "clash.outbound.level_normal",
            Self::Risky => "clash.outbound.level_risky",
        }
    }
}

/// Third-party IP quality sources queried in hedged waves (tightest quotas last).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum IpQualitySource {
    IdentMe,
    IpApiCom,
    IpQuery,
    IpLocate,
    IpApiIs,
    ProxyCheck,
}

impl IpQualitySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::IdentMe => "ident.me",
            Self::IpApiCom => "ip-api.com",
            Self::IpQuery => "ipquery.io",
            Self::IpLocate => "iplocate.io",
            Self::IpApiIs => "ipapi.is",
            Self::ProxyCheck => "proxycheck.io",
        }
    }
}

/// Per-source failure classification shown when all IP quality sources fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IpQualitySourceStatus {
    NoType,
    Timeout,
    RateLimited,
    Failed,
    IpMismatch,
}

impl IpQualitySourceStatus {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::NoType => "clash.outbound.status_no_type",
            Self::Timeout => "clash.outbound.status_timeout",
            Self::RateLimited => "clash.outbound.status_rate_limited",
            Self::Failed => "clash.outbound.status_failed",
            Self::IpMismatch => "clash.outbound.status_ip_mismatch",
        }
    }
}

/// One failed third-party IP quality query attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpQualityFailure {
    pub source: IpQualitySource,
    pub status: IpQualitySourceStatus,
}

/// Resolved third-party IP quality and network ownership metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpQuality {
    pub ip: String,
    pub source: IpQualitySource,
    pub ip_type: IpType,
    pub organization: Option<String>,
    pub asn: Option<u32>,
    pub is_proxy: bool,
    pub is_vpn: bool,
    pub is_tor: bool,
    pub is_abuser: bool,
}

impl IpQuality {
    pub fn level(&self) -> IpQualityLevel {
        if self.is_tor || self.is_abuser {
            return IpQualityLevel::Risky;
        }
        match self.ip_type {
            IpType::Residential | IpType::Mobile | IpType::Business => IpQualityLevel::Good,
            IpType::Hosting => IpQualityLevel::Normal,
        }
    }

    pub fn asn_label(&self) -> Option<String> {
        self.asn.map(|n| format!("AS{n}"))
    }

    /// Active risk/proxy flag i18n keys ordered highest-severity first.
    pub fn flag_keys(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.is_tor {
            out.push("clash.outbound.flag_tor");
        }
        if self.is_abuser {
            out.push("clash.outbound.flag_abuser");
        }
        if self.is_vpn {
            out.push("clash.outbound.flag_vpn");
        }
        if self.is_proxy {
            out.push("clash.outbound.flag_proxy");
        }
        out
    }
}

/// Current state of the IP quality lookup for a resolved outbound IP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpQualityOutcome {
    Loading,
    Ready(IpQuality),
    Failed(Vec<IpQualityFailure>),
}

/// Stage-1 outbound IP discovery result returned as soon as the first source answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundIpInfo {
    pub ip: String,
    pub country: String,
    pub ip_source: String,
    pub fallback_org: String,
}

/// Outbound IP, quality classification, and connectivity latency probe result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitProbeResult {
    pub ip: String,
    pub country: String,
    pub ip_source: String,
    pub org: String,
    pub quality: IpQualityOutcome,
    pub google_ms: Option<u64>,
    pub github_ms: Option<u64>,
    pub checked_at: Instant,
}

impl ExitProbeResult {
    pub fn from_ip_info(info: OutboundIpInfo) -> Self {
        Self {
            ip: info.ip,
            country: info.country,
            ip_source: info.ip_source,
            org: info.fallback_org,
            quality: IpQualityOutcome::Loading,
            google_ms: None,
            github_ms: None,
            checked_at: Instant::now(),
        }
    }

    pub fn quality_ready(&self) -> Option<&IpQuality> {
        match &self.quality {
            IpQualityOutcome::Ready(q) => Some(q),
            _ => None,
        }
    }

    pub fn is_quality_loading(&self) -> bool {
        matches!(self.quality, IpQualityOutcome::Loading)
    }

    /// Best available organization / ASN display string for overview cards and copy actions.
    pub fn display_org(&self) -> Option<String> {
        if let Some(q) = self.quality_ready() {
            if let Some(org) = q.organization.as_deref().filter(|s| !s.is_empty()) {
                return Some(org.to_owned());
            }
            if let Some(asn) = q.asn_label() {
                return Some(asn);
            }
        }
        if !self.org.is_empty() {
            Some(self.org.clone())
        } else {
            None
        }
    }

    /// Raw organization name (excluding ASN fallback) when available.
    pub fn organization_name(&self) -> Option<&str> {
        self.quality_ready()
            .and_then(|q| q.organization.as_deref())
            .filter(|s| !s.is_empty())
            .or_else(|| (!self.org.is_empty()).then_some(self.org.as_str()))
    }
}

/// Built-in service reachability and streaming/AI unlock probe targets (aligned with FlClash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ServiceTarget {
    Google,
    GitHub,
    YouTube,
    ChatGpt,
    Claude,
    Gemini,
    Netflix,
    DisneyPlus,
    PrimeVideo,
    Spotify,
    TikTok,
    Discord,
    Bilibili,
}

impl ServiceTarget {
    pub const ALL: [Self; 13] = [
        Self::Google,
        Self::GitHub,
        Self::YouTube,
        Self::ChatGpt,
        Self::Claude,
        Self::Gemini,
        Self::Netflix,
        Self::DisneyPlus,
        Self::PrimeVideo,
        Self::Spotify,
        Self::TikTok,
        Self::Discord,
        Self::Bilibili,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::GitHub => "github",
            Self::YouTube => "youtube",
            Self::ChatGpt => "chatgpt",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::Netflix => "netflix",
            Self::DisneyPlus => "disney-plus",
            Self::PrimeVideo => "prime-video",
            Self::Spotify => "spotify",
            Self::TikTok => "tiktok",
            Self::Discord => "discord",
            Self::Bilibili => "bilibili",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::GitHub => "GitHub",
            Self::YouTube => "YouTube",
            Self::ChatGpt => "ChatGPT",
            Self::Claude => "Claude",
            Self::Gemini => "Gemini",
            Self::Netflix => "Netflix",
            Self::DisneyPlus => "Disney+",
            Self::PrimeVideo => "Prime Video",
            Self::Spotify => "Spotify",
            Self::TikTok => "TikTok",
            Self::Discord => "Discord",
            Self::Bilibili => "Bilibili",
        }
    }

    pub fn primary_host(self) -> &'static str {
        match self {
            Self::Google => "www.google.com",
            Self::GitHub => "github.com",
            Self::YouTube => "www.youtube.com",
            Self::ChatGpt => "chat.openai.com",
            Self::Claude => "claude.ai",
            Self::Gemini => "gemini.google.com",
            Self::Netflix => "www.netflix.com",
            Self::DisneyPlus => "disney.api.edge.bamgrid.com",
            Self::PrimeVideo => "www.primevideo.com",
            Self::Spotify => "www.spotify.com",
            Self::TikTok => "www.tiktok.com",
            Self::Discord => "discord.com",
            Self::Bilibili => "api.bilibili.com",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|t| t.id() == id)
    }

    /// Return all targets ordered by `saved_order`, appending any newly added targets in default order.
    pub fn ordered(saved_order: &[String]) -> Vec<Self> {
        let mut out = Vec::with_capacity(Self::ALL.len());
        for id in saved_order {
            if let Some(t) = Self::from_id(id)
                && !out.contains(&t)
            {
                out.push(t);
            }
        }
        for t in Self::ALL {
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }

    /// Return enabled targets in preferred order, falling back to all ordered targets if every item
    /// was disabled so the dashboard always retains at least one target.
    pub fn enabled(saved_order: &[String], disabled: &[String]) -> Vec<Self> {
        let all = Self::ordered(saved_order);
        let kept: Vec<Self> = all
            .iter()
            .copied()
            .filter(|t| !disabled.iter().any(|d| d == t.id()))
            .collect();
        if kept.is_empty() { all } else { kept }
    }
}

/// Service availability / regional unlock classification (aligned with FlClash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServiceProbeStatus {
    Available,
    Restricted,
    DisallowedIsp,
    Blocked,
    UnsupportedRegion,
    OriginalsOnly,
    ComingSoon,
    Unavailable,
    Timeout,
    Failed,
}

impl ServiceProbeStatus {
    pub fn label_key(self) -> &'static str {
        match self {
            Self::Available => "clash.service.status_available",
            Self::Restricted => "clash.service.status_restricted",
            Self::DisallowedIsp => "clash.service.status_disallowed_isp",
            Self::Blocked => "clash.service.status_blocked",
            Self::UnsupportedRegion => "clash.service.status_unsupported_region",
            Self::OriginalsOnly => "clash.service.status_originals_only",
            Self::ComingSoon => "clash.service.status_coming_soon",
            Self::Unavailable => "clash.service.status_unavailable",
            Self::Timeout => "clash.service.status_timeout",
            Self::Failed => "clash.service.status_failed",
        }
    }

    pub fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    pub fn is_warning(self) -> bool {
        matches!(
            self,
            Self::Restricted
                | Self::DisallowedIsp
                | Self::Blocked
                | Self::UnsupportedRegion
                | Self::OriginalsOnly
                | Self::ComingSoon
        )
    }

    pub fn is_error(self) -> bool {
        matches!(self, Self::Unavailable | Self::Timeout | Self::Failed)
    }
}

/// Result of checking a single service through Mihomo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCheckItem {
    pub target: ServiceTarget,
    pub status: ServiceProbeStatus,
    pub delay_ms: Option<u64>,
    pub region: Option<String>,
    pub ip: Option<String>,
    pub exit_node: Option<String>,
    pub checked_at: Instant,
}

/// Per-service live state tracked by the Mihomo TUI workspace.
#[derive(Debug, Clone, Default)]
pub struct ServiceEntryState {
    pub probing: bool,
    pub result: Option<ServiceCheckItem>,
}

/// Full immutable snapshot delivered by `ClashMonitor` to the TUI thread.
#[derive(Debug, Clone, Default)]
pub struct ClashSnapshot {
    pub availability: ClashAvailability,
    pub summary: ClashRuntimeSummary,
    pub groups: Vec<ProxyGroupItem>,
    pub profiles: Vec<ProfileSummary>,
    pub proxy_providers: Vec<ProxyProviderItem>,
    pub rule_providers: Vec<RuleProviderItem>,
    pub rules: Vec<RuleItem>,
    pub connections: Vec<ConnectionItem>,
    pub closed_connections: VecDeque<ConnectionItem>,
    pub upload_total: u64,
    pub download_total: u64,
    pub up_speed: u64,
    pub down_speed: u64,
    pub peak_up_speed: u64,
    pub peak_down_speed: u64,
    pub memory_bytes: Option<u64>,
    pub traffic_history: VecDeque<TrafficSample>,
    pub fetched_rules: bool,
    pub fetched_providers: bool,
    pub updated_at: Option<Instant>,
    pub last_error: Option<String>,
}

impl ClashSnapshot {
    pub fn visible_groups(&self) -> Vec<&ProxyGroupItem> {
        self.groups.iter().filter(|g| !g.hidden).collect()
    }

    pub fn active_profile(&self) -> Option<&ProfileSummary> {
        self.profiles.iter().find(|p| p.active)
    }

    /// Best available subscription quota for the active Mihomo session: prefers the active
    /// profile's `subscription-userinfo` header and falls back to external `proxy-providers`
    /// `subscriptionInfo` exposed by Mihomo's controller when the active profile has none.
    pub fn effective_active_userinfo(&self) -> Option<(SubscriptionUserinfo, UserinfoSource)> {
        if let Some(info) = self.active_profile().and_then(|p| p.userinfo) {
            return Some((info, UserinfoSource::Profile));
        }
        let with_info: Vec<(&str, SubscriptionUserinfo)> = self
            .proxy_providers
            .iter()
            .filter_map(|p| p.userinfo.map(|u| (p.name.as_str(), u)))
            .collect();
        match with_info.as_slice() {
            [] => None,
            [(name, info)] => Some((*info, UserinfoSource::Provider((*name).to_owned()))),
            many => {
                let mut agg = SubscriptionUserinfo::default();
                let mut earliest_expire: Option<u64> = None;
                for (_, u) in many {
                    agg.upload = agg.upload.saturating_add(u.upload);
                    agg.download = agg.download.saturating_add(u.download);
                    agg.total = agg.total.saturating_add(u.total);
                    if let Some(exp) = u.expire {
                        earliest_expire = Some(earliest_expire.map_or(exp, |cur| cur.min(exp)));
                    }
                }
                agg.expire = earliest_expire;
                Some((agg, UserinfoSource::Providers(many.len())))
            }
        }
    }

    pub fn tcp_count(&self) -> usize {
        self.connections
            .iter()
            .filter(|c| c.network.eq_ignore_ascii_case("tcp"))
            .count()
    }

    pub fn udp_count(&self) -> usize {
        self.connections
            .iter()
            .filter(|c| c.network.eq_ignore_ascii_case("udp"))
            .count()
    }

    /// Compute rolling average and peak throughput over the most recent `window_secs` samples.
    pub fn traffic_window_stats(&self, window_secs: usize) -> TrafficWindowStats {
        let take = window_secs.max(1).min(self.traffic_history.len());
        if take == 0 {
            return TrafficWindowStats::default();
        }
        let skip = self.traffic_history.len() - take;
        let mut sum_up = 0u128;
        let mut sum_down = 0u128;
        let mut peak_up = 0u64;
        let mut peak_down = 0u64;
        for sample in self.traffic_history.iter().skip(skip) {
            sum_up += u128::from(sample.up_bps);
            sum_down += u128::from(sample.down_bps);
            peak_up = peak_up.max(sample.up_bps);
            peak_down = peak_down.max(sample.down_bps);
        }
        TrafficWindowStats {
            avg_up: (sum_up / take as u128) as u64,
            avg_down: (sum_down / take as u128) as u64,
            peak_up,
            peak_down,
            sample_count: take,
        }
    }

    fn aggregate_connections<F>(&self, limit: usize, mut key_fn: F) -> Vec<TrafficBucket>
    where
        F: FnMut(&ConnectionItem) -> String,
    {
        if limit == 0 || self.connections.is_empty() {
            return Vec::new();
        }
        let mut map: std::collections::BTreeMap<String, TrafficBucket> =
            std::collections::BTreeMap::new();
        for conn in &self.connections {
            let key = key_fn(conn);
            let entry = map.entry(key.clone()).or_insert_with(|| TrafficBucket {
                name: key,
                down_speed: 0,
                up_speed: 0,
                download: 0,
                upload: 0,
                conn_count: 0,
            });
            entry.down_speed = entry.down_speed.saturating_add(conn.down_speed);
            entry.up_speed = entry.up_speed.saturating_add(conn.up_speed);
            entry.download = entry.download.saturating_add(conn.download);
            entry.upload = entry.upload.saturating_add(conn.upload);
            entry.conn_count += 1;
        }
        let mut out: Vec<TrafficBucket> = map.into_values().collect();
        out.sort_by(|a, b| {
            b.total_speed()
                .cmp(&a.total_speed())
                .then_with(|| b.total_bytes().cmp(&a.total_bytes()))
                .then_with(|| b.conn_count.cmp(&a.conn_count))
                .then_with(|| a.name.cmp(&b.name))
        });
        out.truncate(limit);
        out
    }

    /// Active connection traffic aggregated by outbound exit node.
    pub fn traffic_by_exit(&self, limit: usize) -> Vec<TrafficBucket> {
        self.aggregate_connections(limit, |c| {
            if c.exit_node.is_empty() {
                "-".to_owned()
            } else {
                c.exit_node.clone()
            }
        })
    }

    /// Active connection traffic aggregated by matched routing rule.
    pub fn traffic_by_rule(&self, limit: usize) -> Vec<TrafficBucket> {
        self.aggregate_connections(limit, |c| {
            if c.rule.is_empty() {
                "-".to_owned()
            } else if c.rule_payload.is_empty() {
                c.rule.clone()
            } else {
                format!("{}({})", c.rule, c.rule_payload)
            }
        })
    }

    /// Active connection traffic aggregated by client process / inbound type.
    pub fn traffic_by_process(&self, limit: usize) -> Vec<TrafficBucket> {
        self.aggregate_connections(limit, |c| c.display_process().to_owned())
    }
}

/// Round a peak byte/sec value up to a clean, human-readable Y-axis ceiling (minimum `1 KB/s`).
pub fn nice_speed_ceiling(max_bps: u64) -> u64 {
    const STEPS: [u64; 9] = [1, 2, 5, 10, 25, 50, 100, 250, 500];
    let target = max_bps.max(1024);
    let mut unit = 1024u64;
    for _ in 0..5 {
        for &mult in &STEPS {
            if let Some(candidate) = unit.checked_mul(mult)
                && candidate >= target
            {
                return candidate;
            }
        }
        let Some(next) = unit.checked_mul(1024) else {
            break;
        };
        unit = next;
    }
    target
}

/// Compact 7-bit ASCII Y-axis speed tick label (at most ~7 columns wide).
pub fn format_speed_axis(bps: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bps == 0 {
        return "0".to_owned();
    }
    if bps >= GB {
        if bps.is_multiple_of(GB) || bps >= 10 * GB {
            format!("{}G/s", bps / GB)
        } else {
            format!("{:.1}G/s", bps as f64 / GB as f64)
        }
    } else if bps >= MB {
        if bps.is_multiple_of(MB) || bps >= 10 * MB {
            format!("{}M/s", bps / MB)
        } else {
            format!("{:.1}M/s", bps as f64 / MB as f64)
        }
    } else if bps >= KB {
        if bps.is_multiple_of(KB) || bps >= 10 * KB {
            format!("{}K/s", bps / KB)
        } else {
            format!("{:.1}K/s", bps as f64 / KB as f64)
        }
    } else {
        format!("{bps}B/s")
    }
}

/// Human-friendly byte count (7-bit ASCII).
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let v = bytes as f64;
    if v >= TB {
        format!("{:.2} TB", v / TB)
    } else if v >= GB {
        format!("{:.1} GB", v / GB)
    } else if v >= MB {
        format!("{:.1} MB", v / MB)
    } else if v >= KB {
        format!("{:.1} KB", v / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Compact byte count without space for tight table columns (7-bit ASCII).
pub fn format_bytes_compact(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let v = bytes as f64;
    if v >= TB {
        format!("{:.1}TB", v / TB)
    } else if v >= GB {
        format!("{:.1}GB", v / GB)
    } else if v >= MB {
        format!("{:.1}MB", v / MB)
    } else if v >= KB {
        format!("{:.0}KB", v / KB)
    } else {
        format!("{bytes}B")
    }
}

/// Human-friendly transfer speed (7-bit ASCII).
pub fn format_speed(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}

/// Compact transfer speed without space for tight table columns (7-bit ASCII).
pub fn format_speed_compact(bps: u64) -> String {
    format!("{}/s", format_bytes_compact(bps))
}

/// Format process uptime seconds as `HH:MM:SS` or `Dd HH:MM`.
pub fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if days > 0 {
        format!("{days}d {hours:02}:{mins:02}")
    } else {
        format!("{hours:02}:{mins:02}:{secs:02}")
    }
}

/// Format Unix epoch seconds into UTC `YYYY-MM-DD` (Howard Hinnant's civil-from-days algorithm).
pub fn format_epoch_ymd(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = y + i64::from(m <= 2);
    format!("{year:04}-{m:02}-{d:02}")
}

/// Format a relative age from a past Unix epoch timestamp (7-bit ASCII).
pub fn format_relative_epoch(epoch_secs: u64, now_epoch_secs: u64) -> String {
    let diff = now_epoch_secs.saturating_sub(epoch_secs);
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_userinfo_parses_headers_and_redirects() {
        let headers = "\
HTTP/1.1 302 Found\r\n\
Location: https://cdn.example/sub\r\n\
\r\n\
HTTP/1.1 200 OK\r\n\
Content-Type: text/yaml\r\n\
Subscription-Userinfo: upload=1073741824; download=2147483648; total=10737418240; expire=1795046400\r\n\
\r\n";
        let info = SubscriptionUserinfo::extract_from_http_headers(headers).unwrap();
        assert_eq!(info.upload, 1_073_741_824);
        assert_eq!(info.download, 2_147_483_648);
        assert_eq!(info.total, 10_737_418_240);
        assert_eq!(info.used(), 3_221_225_472);
        assert!((info.ratio() - 0.3).abs() < 1e-6);
        assert_eq!(info.expire_date().as_deref(), Some("2026-11-19"));
        assert_eq!(info.remaining(), 7_516_192_768);
    }

    #[test]
    fn subscription_userinfo_handles_lenient_formats_ms_expire_and_fallback_headers() {
        let info = SubscriptionUserinfo::parse_header(
            "\"upload=1.5e3, download=2500.0, total=10000, expire=1795046400000\"",
        )
        .unwrap();
        assert_eq!(info.upload, 1500);
        assert_eq!(info.download, 2500);
        assert_eq!(info.total, 10_000);
        assert_eq!(info.used(), 4000);
        assert_eq!(info.remaining(), 6000);
        assert_eq!(info.expire, Some(1_795_046_400));
        assert_eq!(info.days_until_expire(1_795_046_400 - 3 * 86_400), Some(3));

        let no_exp =
            SubscriptionUserinfo::parse_header("upload=100 download=200 total=1000 expire=")
                .unwrap();
        assert_eq!(no_exp.used(), 300);
        assert_eq!(no_exp.expire, None);

        let null_exp =
            SubscriptionUserinfo::parse_header("upload=10; download=20; total=0; expire=null")
                .unwrap();
        assert_eq!(null_exp.used(), 30);
        assert_eq!(null_exp.total, 0);
        assert_eq!(null_exp.expire, None);

        let fallback_headers =
            "HTTP/1.1 200 OK\r\nX-Subscription-Userinfo: upload=50; download=70; total=500\r\n\r\n";
        let extracted = SubscriptionUserinfo::extract_from_http_headers(fallback_headers).unwrap();
        assert_eq!(extracted.used(), 120);
        assert_eq!(extracted.total, 500);

        // Canonical header takes precedence over fallback header even when fallback comes later.
        let mixed = "HTTP/1.1 200 OK\r\nSubscription-Userinfo: upload=1; download=2; total=10\r\nProfile-Userinfo: upload=9; download=9; total=99\r\n\r\n";
        let chosen = SubscriptionUserinfo::extract_from_http_headers(mixed).unwrap();
        assert_eq!(chosen.total, 10);
    }

    #[test]
    fn service_target_ordering_and_enabled_fallback_work() {
        let custom = vec![
            "claude".to_owned(),
            "chatgpt".to_owned(),
            "unknown".to_owned(),
        ];
        let ordered = ServiceTarget::ordered(&custom);
        assert_eq!(ordered.len(), ServiceTarget::ALL.len());
        assert_eq!(ordered[0], ServiceTarget::Claude);
        assert_eq!(ordered[1], ServiceTarget::ChatGpt);
        assert_eq!(ordered[2], ServiceTarget::Google);

        let disabled = vec!["claude".to_owned(), "google".to_owned()];
        let enabled = ServiceTarget::enabled(&custom, &disabled);
        assert!(!enabled.contains(&ServiceTarget::Claude));
        assert!(!enabled.contains(&ServiceTarget::Google));
        assert_eq!(enabled[0], ServiceTarget::ChatGpt);

        // Disabling every service safely falls back to the full ordered list.
        let all_ids: Vec<String> = ServiceTarget::ALL
            .iter()
            .map(|t| t.id().to_owned())
            .collect();
        let fallback = ServiceTarget::enabled(&custom, &all_ids);
        assert_eq!(fallback, ordered);
    }

    #[test]
    fn effective_active_userinfo_prefers_profile_and_falls_back_to_providers() {
        let mut snap = ClashSnapshot::default();
        snap.profiles.push(ProfileSummary {
            name: "main".into(),
            host: "sub.example.net".into(),
            is_local: false,
            active: true,
            updated: String::new(),
            updated_epoch: None,
            userinfo: None,
            file_bytes: None,
        });
        assert!(snap.effective_active_userinfo().is_none());

        snap.proxy_providers.push(ProxyProviderItem {
            name: "NodePoolA".into(),
            vehicle_type: "HTTP".into(),
            node_count: 10,
            updated_at: String::new(),
            userinfo: Some(SubscriptionUserinfo {
                upload: 100,
                download: 200,
                total: 1000,
                expire: Some(1_800_000_000),
            }),
        });
        let (info, src) = snap.effective_active_userinfo().unwrap();
        assert_eq!(info.used(), 300);
        assert_eq!(src, UserinfoSource::Provider("NodePoolA".into()));

        snap.proxy_providers.push(ProxyProviderItem {
            name: "NodePoolB".into(),
            vehicle_type: "HTTP".into(),
            node_count: 8,
            updated_at: String::new(),
            userinfo: Some(SubscriptionUserinfo {
                upload: 50,
                download: 150,
                total: 500,
                expire: Some(1_795_000_000),
            }),
        });
        let (agg, src) = snap.effective_active_userinfo().unwrap();
        assert_eq!(agg.used(), 500);
        assert_eq!(agg.total, 1500);
        assert_eq!(agg.expire, Some(1_795_000_000));
        assert_eq!(src, UserinfoSource::Providers(2));

        // Active profile's own header always wins over provider fallback.
        snap.profiles[0].userinfo = Some(SubscriptionUserinfo {
            upload: 10,
            download: 20,
            total: 100,
            expire: None,
        });
        let (prof_info, src) = snap.effective_active_userinfo().unwrap();
        assert_eq!(prof_info.total, 100);
        assert_eq!(src, UserinfoSource::Profile);
    }

    #[test]
    fn format_helpers_are_pure_ascii() {
        assert_eq!(format_epoch_ymd(0), "1970-01-01");
        assert_eq!(format_epoch_ymd(1_795_046_400), "2026-11-19");
        assert_eq!(format_uptime(1323), "00:22:03");
        assert!(format_bytes(1_500_000_000).is_ascii());
        assert!(format_speed(2_097_152).is_ascii());
        assert_eq!(nice_speed_ceiling(0), 1024);
        assert_eq!(nice_speed_ceiling(128_000), 250 * 1024);
        assert_eq!(nice_speed_ceiling(1_782_579), 2 * 1024 * 1024);
        assert_eq!(format_speed_axis(0), "0");
        assert_eq!(format_speed_axis(250 * 1024), "250K/s");
        assert_eq!(format_speed_axis(2 * 1024 * 1024), "2M/s");
        assert!(format_speed_axis(1_572_864).is_ascii());
    }

    #[test]
    fn traffic_window_stats_and_connection_buckets_aggregate_accurately() {
        let mut snap = ClashSnapshot::default();
        for (u, d) in [(100, 1000), (300, 3000), (500, 5000)] {
            snap.traffic_history.push_back(TrafficSample {
                up_bps: u,
                down_bps: d,
                active_conns: 2,
            });
        }
        let all = snap.traffic_window_stats(60);
        assert_eq!(all.sample_count, 3);
        assert_eq!(all.avg_up, 300);
        assert_eq!(all.avg_down, 3000);
        assert_eq!(all.peak_up, 500);
        assert_eq!(all.peak_down, 5000);

        let tail2 = snap.traffic_window_stats(2);
        assert_eq!(tail2.sample_count, 2);
        assert_eq!(tail2.avg_down, 4000);

        snap.connections = vec![
            ConnectionItem {
                id: "1".into(),
                network: "TCP".into(),
                conn_type: "HTTP".into(),
                source: String::new(),
                destination: String::new(),
                host: "a.example:443".into(),
                process: "curl".into(),
                rule: "DomainSuffix".into(),
                rule_payload: "example.com".into(),
                chains: vec!["Proxy".into(), "JP-01".into()],
                exit_node: "JP-01".into(),
                upload: 10,
                download: 200,
                up_speed: 50,
                down_speed: 500,
                start: String::new(),
                closed: false,
            },
            ConnectionItem {
                id: "2".into(),
                network: "TCP".into(),
                conn_type: "HTTP".into(),
                source: String::new(),
                destination: String::new(),
                host: "b.example:443".into(),
                process: "curl".into(),
                rule: "DomainSuffix".into(),
                rule_payload: "example.com".into(),
                chains: vec!["Proxy".into(), "JP-01".into()],
                exit_node: "JP-01".into(),
                upload: 20,
                download: 300,
                up_speed: 100,
                down_speed: 700,
                start: String::new(),
                closed: false,
            },
            ConnectionItem {
                id: "3".into(),
                network: "UDP".into(),
                conn_type: "TUN".into(),
                source: String::new(),
                destination: String::new(),
                host: "cn.example:443".into(),
                process: "chrome".into(),
                rule: "GeoIP".into(),
                rule_payload: "CN".into(),
                chains: vec!["DIRECT".into()],
                exit_node: "DIRECT".into(),
                upload: 5,
                download: 40,
                up_speed: 10,
                down_speed: 90,
                start: String::new(),
                closed: false,
            },
        ];
        let by_exit = snap.traffic_by_exit(5);
        assert_eq!(by_exit.len(), 2);
        assert_eq!(by_exit[0].name, "JP-01");
        assert_eq!(by_exit[0].conn_count, 2);
        assert_eq!(by_exit[0].down_speed, 1200);
        assert_eq!(by_exit[1].name, "DIRECT");

        let by_rule = snap.traffic_by_rule(5);
        assert_eq!(by_rule[0].name, "DomainSuffix(example.com)");
        assert_eq!(by_rule[1].name, "GeoIP(CN)");

        let by_proc = snap.traffic_by_process(5);
        assert_eq!(by_proc[0].name, "curl");
        assert_eq!(by_proc[0].conn_count, 2);
    }
}

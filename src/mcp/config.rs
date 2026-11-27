//! Persistent configuration, `0600` Bearer token lifecycle, and multi-client snippet generation
//! for CqShell's native MCP stdio and Streamable HTTP interfaces.

use crate::{
    api::ExecutionPolicy,
    error::{Error, Result},
    runtime::Context,
    storage::{self, Lock},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read},
    net::IpAddr,
    path::{Path, PathBuf},
};

pub const DEFAULT_LISTEN: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 18765;
pub const DEFAULT_PATH: &str = "/mcp";
pub const DEFAULT_SERVER_NAME: &str = "cqs";
pub const TOKEN_PREFIX: &str = "cqs_mcp_";
pub const REDACTED_TOKEN: &str = "<MCP_TOKEN>";
pub const MASKED_TOKEN: &str = "************************";

const MAX_TOKEN_FILE_BYTES: u64 = 512;
const TOKEN_RANDOM_LEN: usize = 36;

pub fn mcp_dir(ctx: &Context) -> PathBuf {
    ctx.paths.state.join("mcp")
}

pub fn config_path(ctx: &Context) -> PathBuf {
    mcp_dir(ctx).join("config.toml")
}

pub fn token_path(ctx: &Context) -> PathBuf {
    mcp_dir(ctx).join("mcp.token")
}

pub fn lock_path(ctx: &Context) -> PathBuf {
    mcp_dir(ctx).join("mcp.lock")
}

pub fn backups_dir(ctx: &Context) -> PathBuf {
    mcp_dir(ctx).join("backups")
}

/// Permission tier persisted for the MCP HTTP service or client snippet generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    #[default]
    ReadOnly,
    Standard,
    AllowDanger,
}

impl PolicyMode {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "read_only" | "read-only" | "readonly" => Ok(Self::ReadOnly),
            "standard" | "default" => Ok(Self::Standard),
            "allow_danger" | "allow-danger" | "danger" => Ok(Self::AllowDanger),
            other => Err(Error::new(
                "error.input",
                format!("MCP policy mode must be read_only, standard, or allow_danger: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Standard => "standard",
            Self::AllowDanger => "allow_danger",
        }
    }

    pub fn cli_flag(self) -> Option<&'static str> {
        match self {
            Self::ReadOnly => Some("--read-only"),
            Self::Standard => None,
            Self::AllowDanger => Some("--allow-danger"),
        }
    }
}

/// Transport mode for client configuration snippets and installers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    Http,
    SshStdio,
    LocalStdio,
}

impl TransportMode {
    pub const ALL: [Self; 3] = [Self::Http, Self::SshStdio, Self::LocalStdio];

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "http" | "streamable_http" | "streamable-http" => Ok(Self::Http),
            "ssh" | "ssh_stdio" | "ssh-stdio" => Ok(Self::SshStdio),
            "local" | "stdio" | "local_stdio" | "local-stdio" => Ok(Self::LocalStdio),
            other => Err(Error::new(
                "error.input",
                format!("MCP transport must be http, ssh_stdio, or local_stdio: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::SshStdio => "ssh_stdio",
            Self::LocalStdio => "local_stdio",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Http => Self::SshStdio,
            Self::SshStdio => Self::LocalStdio,
            Self::LocalStdio => Self::Http,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Http => Self::LocalStdio,
            Self::SshStdio => Self::Http,
            Self::LocalStdio => Self::SshStdio,
        }
    }
}

/// Output format for client configuration export / clipboard copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientFormat {
    Json,
    Claude,
    Codex,
    Cursor,
}

impl ClientFormat {
    pub const ALL: [Self; 4] = [Self::Json, Self::Claude, Self::Codex, Self::Cursor];

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "json" | "generic" | "desktop" => Ok(Self::Json),
            "claude" | "claude-code" | "claude_code" => Ok(Self::Claude),
            "codex" | "codex-cli" | "codex_cli" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            other => Err(Error::new(
                "error.input",
                format!("MCP client format must be json, claude, codex, or cursor: {other}"),
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Json => Self::Claude,
            Self::Claude => Self::Codex,
            Self::Codex => Self::Cursor,
            Self::Cursor => Self::Json,
        }
    }
}

/// Persistent non-secret configuration for CqShell's MCP HTTP server and access templates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpHttpConfig {
    pub listen: String,
    pub port: u16,
    pub endpoint_path: String,
    pub auth_enabled: bool,
    pub public_url: String,
    pub ssh_target: String,
    pub server_name: String,
    pub policy_mode: PolicyMode,
    pub allow_groups: Vec<String>,
    pub deny_actions: Vec<String>,
    pub allowed_origins: Vec<String>,
}

impl Default for McpHttpConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN.into(),
            port: DEFAULT_PORT,
            endpoint_path: DEFAULT_PATH.into(),
            auth_enabled: true,
            public_url: String::new(),
            ssh_target: String::new(),
            server_name: DEFAULT_SERVER_NAME.into(),
            policy_mode: PolicyMode::ReadOnly,
            allow_groups: Vec::new(),
            deny_actions: Vec::new(),
            allowed_origins: Vec::new(),
        }
    }
}

pub fn normalize_listen(value: &str) -> Result<(String, IpAddr)> {
    let trimmed = value.trim();
    let candidate = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let ip = candidate.parse::<IpAddr>().map_err(|_| {
        Error::new(
            "error.input",
            "MCP listen address must be a valid IPv4 or IPv6 address (e.g. 127.0.0.1, 0.0.0.0, ::1, ::)",
        )
    })?;
    Ok((ip.to_string(), ip))
}

pub fn is_loopback_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

pub fn format_host_bracketed(host: &str) -> String {
    let trimmed = host.trim_matches(['[', ']']);
    if trimmed.contains(':') {
        format!("[{trimmed}]")
    } else {
        trimmed.into()
    }
}

pub fn validate_port(value: &str) -> Result<u16> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 5 || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::new(
            "error.input",
            "MCP port must be an integer between 1 and 65535",
        ));
    }
    let port = trimmed
        .parse::<u16>()
        .map_err(|_| Error::new("error.input", "MCP port must be between 1 and 65535"))?;
    if port == 0 {
        return Err(Error::new(
            "error.input",
            "MCP port must be between 1 and 65535",
        ));
    }
    Ok(port)
}

pub fn validate_endpoint_path(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 64
        || !trimmed.starts_with('/')
        || trimmed.contains("..")
        || trimmed.contains('?')
        || trimmed.contains('#')
        || trimmed.contains('\\')
        || !trimmed.bytes().all(|b| (0x21..=0x7e).contains(&b))
    {
        return Err(Error::new(
            "error.input",
            "MCP endpoint path must start with '/' and contain 1-64 printable ASCII non-whitespace characters without '?' or '#'",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn validate_server_name(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 48
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(Error::new(
            "error.input",
            "MCP server name must be 1-48 ASCII characters in [A-Za-z0-9._-]",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn validate_public_url(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.len() > 256 || trimmed.chars().any(char::is_control) {
        return Err(Error::new("error.url", "invalid MCP public URL"));
    }
    let parsed = url::Url::parse(trimmed).map_err(|e| Error::new("error.url", e))?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err(Error::new(
            "error.url",
            "MCP public URL must use https:// or http://",
        ));
    }
    if parsed.host_str().is_none() {
        return Err(Error::new("error.url", "MCP public URL requires a host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::new(
            "error.url",
            "MCP public URL must not embed credentials",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn validate_ssh_target(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.len() > 128
        || trimmed.starts_with('-')
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b'-' | b':'))
    {
        return Err(Error::new(
            "error.input",
            "SSH target must look like user@host or an SSH alias without spaces or leading '-'",
        ));
    }
    Ok(trimmed.to_owned())
}

pub fn parse_csv_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
            out.push(trimmed.to_owned());
        }
    }
    out
}

impl McpHttpConfig {
    pub fn validate(&self) -> Result<IpAddr> {
        let (_, ip) = normalize_listen(&self.listen)?;
        if self.port == 0 {
            return Err(Error::new(
                "error.input",
                "MCP port must be between 1 and 65535",
            ));
        }
        validate_endpoint_path(&self.endpoint_path)?;
        validate_server_name(&self.server_name)?;
        validate_public_url(&self.public_url)?;
        validate_ssh_target(&self.ssh_target)?;
        if !is_loopback_ip(&ip) && !self.auth_enabled {
            return Err(Error::new(
                "error.input",
                "non-loopback MCP HTTP listener requires Bearer token authentication; set auth on or bind 127.0.0.1",
            ));
        }
        for origin in &self.allowed_origins {
            let parsed = url::Url::parse(origin)
                .map_err(|_| Error::new("error.input", format!("invalid Origin URL: {origin}")))?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                return Err(Error::new(
                    "error.input",
                    format!("invalid Origin URL: {origin}"),
                ));
            }
        }
        self.execution_policy().validate()?;
        Ok(ip)
    }

    pub fn is_loopback(&self) -> bool {
        normalize_listen(&self.listen)
            .map(|(_, ip)| is_loopback_ip(&ip))
            .unwrap_or(false)
    }

    pub fn execution_policy(&self) -> ExecutionPolicy {
        ExecutionPolicy {
            read_only: self.policy_mode == PolicyMode::ReadOnly,
            allow_danger: self.policy_mode == PolicyMode::AllowDanger,
            allow_groups: self.allow_groups.clone(),
            deny_actions: self.deny_actions.clone(),
            block_mcp_control_mutations: true,
        }
    }

    pub fn policy_args(&self, mode_override: Option<PolicyMode>) -> Vec<String> {
        let mode = mode_override.unwrap_or(self.policy_mode);
        let mut args = Vec::new();
        if let Some(flag) = mode.cli_flag() {
            args.push(flag.to_owned());
        }
        if !self.allow_groups.is_empty() {
            args.push("--allow-group".into());
            args.push(self.allow_groups.join(","));
        }
        if !self.deny_actions.is_empty() {
            args.push("--deny-action".into());
            args.push(self.deny_actions.join(","));
        }
        args
    }

    pub fn listen_endpoint(&self) -> String {
        let host = normalize_listen(&self.listen)
            .map(|(h, _)| format_host_bracketed(&h))
            .unwrap_or_else(|_| self.listen.clone());
        format!("{host}:{}", self.port)
    }

    pub fn effective_http_url(&self, detected_host: Option<&str>) -> String {
        if !self.public_url.trim().is_empty() {
            return self.public_url.trim().to_owned();
        }
        let host = match normalize_listen(&self.listen) {
            Ok((_, ip)) if ip.is_unspecified() => match detected_host.filter(|s| !s.is_empty()) {
                Some(h) => format_host_bracketed(h),
                None => "<SERVER_IP>".into(),
            },
            Ok((norm, _)) => format_host_bracketed(&norm),
            Err(_) => "<SERVER_IP>".into(),
        };
        format!("http://{host}:{}{}", self.port, self.endpoint_path)
    }

    pub fn effective_ssh_target(&self, detected_host: Option<&str>) -> String {
        if !self.ssh_target.trim().is_empty() {
            return self.ssh_target.trim().to_owned();
        }
        let user = std::env::var("USER")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "root".into());
        let host = detected_host
            .filter(|s| !s.is_empty())
            .unwrap_or("<SERVER_IP>");
        format!("{user}@{host}")
    }

    pub fn ssh_tunnel_command(&self, detected_host: Option<&str>) -> String {
        let target = self.effective_ssh_target(detected_host);
        let local_bind = match normalize_listen(&self.listen) {
            Ok((_, ip)) if ip.is_ipv6() && ip.is_loopback() => "[::1]".to_owned(),
            Ok((norm, ip)) if ip.is_loopback() => norm,
            _ => "127.0.0.1".to_owned(),
        };
        format!(
            "ssh -N -L {}:{local_bind}:{} {target}",
            self.port, self.port
        )
    }
}

pub fn load_config(ctx: &Context) -> Result<McpHttpConfig> {
    let path = config_path(ctx);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(McpHttpConfig::default()),
        Err(e) => return Err(e.into()),
    }
    storage::check_path(&path, true)?;
    let raw = fs::read_to_string(&path)?;
    let cfg: McpHttpConfig = toml::from_str(&raw)
        .map_err(|e| Error::new("error.config", format!("{}: {e}", path.display())))?;
    cfg.validate()?;
    Ok(cfg)
}

pub fn save_config(ctx: &Context, cfg: &McpHttpConfig) -> Result<PathBuf> {
    let ip = cfg.validate()?;
    let mut normalized = cfg.clone();
    normalized.listen = ip.to_string();
    normalized.endpoint_path = validate_endpoint_path(&cfg.endpoint_path)?;
    normalized.server_name = validate_server_name(&cfg.server_name)?;
    normalized.public_url = validate_public_url(&cfg.public_url)?;
    normalized.ssh_target = validate_ssh_target(&cfg.ssh_target)?;

    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;
    let path = config_path(ctx);
    let body = toml::to_string_pretty(&normalized)?;
    storage::atomic_write(&path, body.as_bytes(), 0o600)?;
    Ok(path)
}

pub fn validate_token(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if !(24..=256).contains(&trimmed.len()) || !trimmed.bytes().all(|b| (0x21..=0x7e).contains(&b))
    {
        return Err(Error::new(
            "error.input",
            "MCP Bearer token must be 24-256 printable ASCII characters without spaces",
        ));
    }
    Ok(trimmed.to_owned())
}

/// Constant-time token comparison via SHA-256 digest XOR fold (eliminates length and byte timing).
pub fn constant_time_token_eq(expected: &str, provided: &str) -> bool {
    let a = Sha256::digest(expected.as_bytes());
    let b = Sha256::digest(provided.as_bytes());
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn fill_csprng(buf: &mut [u8]) -> Result<()> {
    #[cfg(unix)]
    {
        let mut urandom = fs::File::open("/dev/urandom")?;
        urandom.read_exact(buf)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
        #[link(name = "bcrypt")]
        unsafe extern "system" {
            fn BCryptGenRandom(
                h_algorithm: *mut std::ffi::c_void,
                pb_buffer: *mut u8,
                cb_buffer: u32,
                dw_flags: u32,
            ) -> i32;
        }
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(Error::new(
                "error.io",
                format!(
                    "BCryptGenRandom failed with NTSTATUS 0x{:08x}",
                    status as u32
                ),
            ));
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = buf;
        Err(Error::new(
            "error.platform",
            "CSPRNG token generation is unsupported on this platform",
        ))
    }
}

/// Generate a high-entropy Bearer token (`cqs_mcp_<36 base62 chars>`, ~214 bits of entropy).
pub fn generate_token() -> Result<String> {
    const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(TOKEN_PREFIX.len() + TOKEN_RANDOM_LEN);
    out.push_str(TOKEN_PREFIX);
    let mut buf = [0u8; 64];
    while out.len() < TOKEN_PREFIX.len() + TOKEN_RANDOM_LEN {
        fill_csprng(&mut buf)?;
        for &b in &buf {
            // 62 * 4 = 248: reject 248..=255 for zero modulo bias.
            if b < 248 {
                out.push(ALPHABET[usize::from(b % 62)] as char);
                if out.len() == TOKEN_PREFIX.len() + TOKEN_RANDOM_LEN {
                    break;
                }
            }
        }
    }
    Ok(out)
}

/// Generate a 128-bit random lowercase hex session identifier for Streamable HTTP.
pub fn generate_session_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    fill_csprng(&mut bytes)?;
    let mut hex = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(hex)
}

fn check_private_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let mode = storage::mode(path)?;
        if mode & 0o077 != 0 {
            return Err(Error::new(
                "error.path",
                format!(
                    "{} has overly broad permissions {:04o}; expected mode 0600 (chmod 600)",
                    path.display(),
                    mode
                ),
            ));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Read the active MCP Bearer token from the `0600` token file, returning `None` if absent.
pub fn read_token(ctx: &Context) -> Result<Option<String>> {
    let path = token_path(ctx);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    storage::check_path(&path, true)?;
    check_private_file_permissions(&path)?;
    let meta = fs::metadata(&path)?;
    if meta.len() > MAX_TOKEN_FILE_BYTES {
        return Err(Error::new(
            "error.data",
            format!("MCP token file exceeds {MAX_TOKEN_FILE_BYTES} bytes"),
        ));
    }
    let raw = fs::read_to_string(&path)?;
    Ok(Some(validate_token(&raw)?))
}

fn write_token_unlocked(ctx: &Context, token: &str) -> Result<PathBuf> {
    let validated = validate_token(token)?;
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let path = token_path(ctx);
    let mut bytes = validated.into_bytes();
    bytes.push(b'\n');
    storage::atomic_write(&path, &bytes, 0o600)?;
    Ok(path)
}

/// Ensure a valid `0600` Bearer token exists; returns `(token, created_new)`.
pub fn ensure_token(ctx: &Context) -> Result<(String, bool)> {
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;
    if let Some(existing) = read_token(ctx)? {
        return Ok((existing, false));
    }
    let token = generate_token()?;
    write_token_unlocked(ctx, &token)?;
    Ok((token, true))
}

/// Rotate the MCP Bearer token atomically in `0600` `mcp.token`.
pub fn rotate_token(ctx: &Context) -> Result<String> {
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;
    let token = generate_token()?;
    write_token_unlocked(ctx, &token)?;
    Ok(token)
}

/// Import a custom MCP Bearer token from a `0600` private file.
pub fn set_token_from_file(ctx: &Context, source: &Path) -> Result<PathBuf> {
    storage::check_path(source, true)?;
    check_private_file_permissions(source)?;
    let meta = fs::metadata(source)?;
    if meta.len() > MAX_TOKEN_FILE_BYTES {
        return Err(Error::new(
            "error.input",
            format!("secret file exceeds {MAX_TOKEN_FILE_BYTES} bytes"),
        ));
    }
    let raw = fs::read_to_string(source)?;
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("");
    let token = validate_token(line)?;
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;
    write_token_unlocked(ctx, &token)
}

/// Import a custom MCP Bearer token from stdin (never via CLI arguments).
pub fn set_token_from_reader<R: Read>(ctx: &Context, reader: R) -> Result<PathBuf> {
    let mut buf = Vec::new();
    reader
        .take(MAX_TOKEN_FILE_BYTES + 1)
        .read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_TOKEN_FILE_BYTES {
        return Err(Error::new(
            "error.input",
            format!("stdin token exceeds {MAX_TOKEN_FILE_BYTES} bytes"),
        ));
    }
    let text = String::from_utf8(buf).map_err(|e| Error::new("error.input", e))?;
    let token = validate_token(&text)?;
    let dir = mcp_dir(ctx);
    storage::private_dir(&dir)?;
    let _lock = Lock::acquire(&lock_path(ctx))?;
    write_token_unlocked(ctx, &token)
}

/// Redacted token metadata safe for logs, CLI status, and MCP read-only status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStatus {
    pub exists: bool,
    pub path: PathBuf,
    pub mode_octal: Option<String>,
    pub fingerprint: Option<String>,
}

pub fn token_status(ctx: &Context) -> Result<TokenStatus> {
    let path = token_path(ctx);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(TokenStatus {
                exists: false,
                path,
                mode_octal: None,
                fingerprint: None,
            });
        }
        Err(e) => return Err(e.into()),
    }
    let mode = storage::mode(&path).ok().map(|m| format!("{m:04o}"));
    let token = read_token(ctx)?;
    let fingerprint = token.map(|t| {
        let digest = Sha256::digest(t.as_bytes());
        format!(
            "sha256:{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            digest[0], digest[1], digest[2], digest[3], digest[4], digest[5]
        )
    });
    Ok(TokenStatus {
        exists: true,
        path,
        mode_octal: mode,
        fingerprint,
    })
}

/// Redacted MCP summary safe for TUI Inspector, row tags, and `ai.mcp.status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSummary {
    pub config: McpHttpConfig,
    pub config_exists: bool,
    pub token_exists: bool,
    pub token_fingerprint: Option<String>,
    pub token_file: PathBuf,
    pub config_file: PathBuf,
}

pub fn inspect(ctx: &Context) -> Result<McpSummary> {
    let cfg_path = config_path(ctx);
    let config_exists = cfg_path.is_file();
    let config = load_config(ctx)?;
    let tok = token_status(ctx)?;
    Ok(McpSummary {
        config,
        config_exists,
        token_exists: tok.exists,
        token_fingerprint: tok.fingerprint,
        token_file: tok.path,
        config_file: cfg_path,
    })
}

/// Preferred `cqs` executable command string for local stdio snippets and installers.
pub fn preferred_cqs_command(ctx: &Context) -> String {
    if ctx.paths.install.is_file() {
        return ctx.paths.install.display().to_string();
    }
    if ctx.require(&["cqs"]).is_ok() {
        return "cqs".into();
    }
    std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "cqs".into())
}

fn shell_single_quote(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-' | b':' | b'@'))
        && !value.is_empty()
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn toml_quote(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

/// Render a client configuration snippet for `transport` and `format`.
/// `token_text` should be `REDACTED_TOKEN` (`<MCP_TOKEN>`), `MASKED_TOKEN` (`****`), or the
/// real token (only for OSC 52 copy or explicit CLI `--reveal`).
pub fn render_snippet(
    cfg: &McpHttpConfig,
    transport: TransportMode,
    format: ClientFormat,
    cqs_cmd: &str,
    detected_host: Option<&str>,
    policy_override: Option<PolicyMode>,
    token_text: Option<&str>,
) -> String {
    let name = &cfg.server_name;
    let url = cfg.effective_http_url(detected_host);
    let ssh_target = cfg.effective_ssh_target(detected_host);
    let policy_args = cfg.policy_args(policy_override);

    match format {
        ClientFormat::Json | ClientFormat::Cursor => {
            let entry = build_json_server_entry(
                cfg,
                transport,
                cqs_cmd,
                &url,
                &ssh_target,
                &policy_args,
                token_text,
            );
            let mut servers = serde_json::Map::new();
            servers.insert(name.clone(), entry);
            serde_json::to_string_pretty(&json!({ "mcpServers": servers }))
                .unwrap_or_else(|_| "{}".into())
        }
        ClientFormat::Claude => match transport {
            TransportMode::Http => {
                let mut cmd = format!(
                    "claude mcp add --transport http {} {}",
                    shell_single_quote(name),
                    shell_single_quote(&url)
                );
                if cfg.auth_enabled {
                    let tok = token_text.unwrap_or(REDACTED_TOKEN);
                    cmd.push_str(&format!(
                        " --header {}",
                        shell_single_quote(&format!("Authorization: Bearer {tok}"))
                    ));
                }
                cmd
            }
            TransportMode::SshStdio => {
                let mut parts = vec![
                    "claude".into(),
                    "mcp".into(),
                    "add".into(),
                    shell_single_quote(name),
                    "--".into(),
                    "ssh".into(),
                    "-T".into(),
                    shell_single_quote(&ssh_target),
                    "cqs".into(),
                    "mcp".into(),
                ];
                parts.extend(policy_args.into_iter().map(|a| shell_single_quote(&a)));
                parts.join(" ")
            }
            TransportMode::LocalStdio => {
                let mut parts = vec![
                    "claude".into(),
                    "mcp".into(),
                    "add".into(),
                    shell_single_quote(name),
                    "--".into(),
                    shell_single_quote(cqs_cmd),
                    "mcp".into(),
                ];
                parts.extend(policy_args.into_iter().map(|a| shell_single_quote(&a)));
                parts.join(" ")
            }
        },
        ClientFormat::Codex => {
            let header = format!("[mcp_servers.{}]", codex_table_key(name));
            match transport {
                TransportMode::Http => {
                    let mut out = format!("{header}\nurl = {}\n", toml_quote(&url));
                    if cfg.auth_enabled {
                        let tok = token_text.unwrap_or(REDACTED_TOKEN);
                        out.push_str(&format!(
                            "\n[mcp_servers.{}.http_headers]\nAuthorization = {}\n",
                            codex_table_key(name),
                            toml_quote(&format!("Bearer {tok}"))
                        ));
                    }
                    out.trim_end().to_owned()
                }
                TransportMode::SshStdio => {
                    let mut args = vec!["-T".to_owned(), ssh_target, "cqs".into(), "mcp".into()];
                    args.extend(policy_args);
                    let args_toml = args
                        .iter()
                        .map(|a| toml_quote(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{header}\ncommand = \"ssh\"\nargs = [{args_toml}]")
                }
                TransportMode::LocalStdio => {
                    let mut args = vec!["mcp".to_owned()];
                    args.extend(policy_args);
                    let args_toml = args
                        .iter()
                        .map(|a| toml_quote(a))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "{header}\ncommand = {}\nargs = [{args_toml}]",
                        toml_quote(cqs_cmd)
                    )
                }
            }
        }
    }
}

fn codex_table_key(name: &str) -> String {
    if name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        name.to_owned()
    } else {
        toml_quote(name)
    }
}

pub(crate) fn build_json_server_entry(
    cfg: &McpHttpConfig,
    transport: TransportMode,
    cqs_cmd: &str,
    url: &str,
    ssh_target: &str,
    policy_args: &[String],
    token_text: Option<&str>,
) -> Value {
    match transport {
        TransportMode::Http => {
            let mut map = serde_json::Map::new();
            map.insert("url".into(), json!(url));
            if cfg.auth_enabled {
                let tok = token_text.unwrap_or(REDACTED_TOKEN);
                map.insert(
                    "headers".into(),
                    json!({ "Authorization": format!("Bearer {tok}") }),
                );
            }
            Value::Object(map)
        }
        TransportMode::SshStdio => {
            let mut args = vec![
                "-T".to_owned(),
                ssh_target.to_owned(),
                "cqs".into(),
                "mcp".into(),
            ];
            args.extend_from_slice(policy_args);
            json!({
                "command": "ssh",
                "args": args,
            })
        }
        TransportMode::LocalStdio => {
            let mut args = vec!["mcp".to_owned()];
            args.extend_from_slice(policy_args);
            json!({
                "command": cqs_cmd,
                "args": args,
            })
        }
    }
}

/// Transient in-memory MCP access bundle for TUI OSC 52 clipboard copy only.
/// Raw Bearer token is never rendered on screen, printed to logs, or included in `Debug` output.
#[derive(Clone, PartialEq, Eq)]
pub struct RevealedMcpAccess {
    pub config: McpHttpConfig,
    pub detected_host: Option<String>,
    pub cqs_cmd: String,
    pub token: Option<String>,
    pub token_file: PathBuf,
    pub config_file: PathBuf,
    pub service_summary: String,
}

impl std::fmt::Debug for RevealedMcpAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevealedMcpAccess")
            .field("config", &self.config)
            .field("detected_host", &self.detected_host)
            .field("cqs_cmd", &self.cqs_cmd)
            .field("token", &self.token.as_ref().map(|_| "[redacted]"))
            .field("token_file", &self.token_file)
            .field("config_file", &self.config_file)
            .field("service_summary", &self.service_summary)
            .finish()
    }
}

impl RevealedMcpAccess {
    pub fn load(ctx: &Context, detected_host: Option<&str>) -> Result<Self> {
        let config = load_config(ctx)?;
        let token = if config.auth_enabled {
            let (tok, _) = ensure_token(ctx)?;
            Some(tok)
        } else {
            read_token(ctx)?
        };
        let service_summary = super::service::brief_status(ctx);
        Ok(Self {
            config,
            detected_host: detected_host.filter(|s| !s.is_empty()).map(str::to_owned),
            cqs_cmd: preferred_cqs_command(ctx),
            token,
            token_file: token_path(ctx),
            config_file: config_path(ctx),
            service_summary,
        })
    }

    pub fn http_url(&self) -> String {
        self.config
            .effective_http_url(self.detected_host.as_deref())
    }

    pub fn ssh_target(&self) -> String {
        self.config
            .effective_ssh_target(self.detected_host.as_deref())
    }

    pub fn ssh_tunnel_cmd(&self) -> String {
        self.config
            .ssh_tunnel_command(self.detected_host.as_deref())
    }

    pub fn needs_plaintext_warning(&self) -> bool {
        !self.config.is_loopback() && self.config.public_url.trim().is_empty()
    }

    /// Screen-safe snippet where any Bearer token is replaced with fixed-width `****`.
    pub fn masked_snippet(&self, transport: TransportMode, format: ClientFormat) -> String {
        render_snippet(
            &self.config,
            transport,
            format,
            &self.cqs_cmd,
            self.detected_host.as_deref(),
            None,
            Some(MASKED_TOKEN),
        )
    }

    /// Copy-ready snippet containing the live Bearer token (OSC 52 clipboard only).
    pub fn copy_snippet(&self, transport: TransportMode, format: ClientFormat) -> String {
        let tok = self.token.as_deref().unwrap_or(REDACTED_TOKEN);
        render_snippet(
            &self.config,
            transport,
            format,
            &self.cqs_cmd,
            self.detected_host.as_deref(),
            None,
            Some(tok),
        )
    }
}

/// Action implementation for `ai.mcp.configure`.
#[allow(clippy::too_many_arguments)]
pub fn action_configure(
    ctx: &Context,
    listen: &str,
    port_raw: &str,
    path_raw: &str,
    auth_on: bool,
    policy_raw: &str,
    public_url: &str,
    ssh_target: &str,
    allow_groups_raw: &str,
    deny_actions_raw: &str,
) -> Result<()> {
    let (norm_listen, _) = normalize_listen(listen)?;
    let port = validate_port(port_raw)?;
    let endpoint_path = validate_endpoint_path(path_raw)?;
    let policy_mode = PolicyMode::parse(policy_raw)?;
    let public_url = validate_public_url(public_url)?;
    let ssh_target = validate_ssh_target(ssh_target)?;
    let allow_groups = parse_csv_list(allow_groups_raw);
    let deny_actions = parse_csv_list(deny_actions_raw);

    let existing = load_config(ctx).unwrap_or_default();
    let cfg = McpHttpConfig {
        listen: norm_listen,
        port,
        endpoint_path,
        auth_enabled: auth_on,
        public_url,
        ssh_target,
        server_name: existing.server_name,
        policy_mode,
        allow_groups,
        deny_actions,
        allowed_origins: existing.allowed_origins,
    };
    cfg.validate()?;
    let saved = save_config(ctx, &cfg)?;
    let mut created_token = false;
    if cfg.auth_enabled {
        let (_, created) = ensure_token(ctx)?;
        created_token = created;
    }
    ctx.message(
        "notice.mcp_configured",
        format!(
            "{}{} (auth={}, policy={}) -> {}",
            cfg.listen_endpoint(),
            cfg.endpoint_path,
            if cfg.auth_enabled { "on" } else { "off" },
            cfg.policy_mode.as_str(),
            saved.display()
        ),
    );
    if created_token {
        ctx.message("notice.mcp_token_ready", token_path(ctx).display());
    }
    if !cfg.is_loopback() && cfg.public_url.is_empty() {
        ctx.message(
            "notice.mcp_plaintext_warn",
            format!("{}{}", cfg.listen_endpoint(), cfg.endpoint_path),
        );
    }
    Ok(())
}

/// Action implementation for `ai.mcp.status` (redacted; never prints Bearer tokens).
pub fn action_status(ctx: &Context) -> Result<()> {
    let summary = inspect(ctx)?;
    let cfg = &summary.config;
    ctx.message(
        "notice.mcp_status",
        format!(
            "endpoint={}{} | auth={} | policy={} | token={} | service={}",
            cfg.listen_endpoint(),
            cfg.endpoint_path,
            if cfg.auth_enabled { "on" } else { "off" },
            cfg.policy_mode.as_str(),
            summary
                .token_fingerprint
                .as_deref()
                .unwrap_or("not initialized"),
            super::service::brief_status(ctx),
        ),
    );
    if !cfg.public_url.is_empty() {
        ctx.log(format!("public_url={}", cfg.public_url));
    }
    if !cfg.ssh_target.is_empty() {
        ctx.log(format!("ssh_target={}", cfg.ssh_target));
    }
    ctx.log(format!("config_file={}", summary.config_file.display()));
    ctx.log(format!("token_file={}", summary.token_file.display()));
    let installs = super::install::detect_installations(ctx, &cfg.server_name);
    for item in installs {
        let state = if item.installed {
            format!(
                "installed ({})",
                item.transport.as_deref().unwrap_or("configured")
            )
        } else if item.config_exists {
            "not registered".into()
        } else {
            "config absent".into()
        };
        ctx.log(format!(
            "client={} state={} path={}",
            item.client,
            state,
            item.path.display()
        ));
    }
    Ok(())
}

/// Action implementation for `ai.mcp.token.rotate` (never echoes token to logs).
pub fn action_token_rotate(ctx: &Context) -> Result<()> {
    let _token = rotate_token(ctx)?;
    let status = token_status(ctx)?;
    ctx.message(
        "notice.mcp_token_rotated",
        format!(
            "{} ({})",
            status.path.display(),
            status.fingerprint.as_deref().unwrap_or("updated")
        ),
    );
    Ok(())
}

/// Action implementation for `ai.mcp.token.set` (reads from a `0600` file, never echoes token).
pub fn action_token_set(ctx: &Context, secret_file: &str) -> Result<()> {
    let path = set_token_from_file(ctx, Path::new(secret_file))?;
    let status = token_status(ctx)?;
    ctx.message(
        "notice.mcp_token_ready",
        format!(
            "{} ({})",
            path.display(),
            status.fingerprint.as_deref().unwrap_or("saved")
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_without_auth_is_rejected() {
        let cfg = McpHttpConfig {
            listen: "0.0.0.0".into(),
            auth_enabled: false,
            ..McpHttpConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert_eq!(err.key, "error.input");

        let loopback_no_auth = McpHttpConfig {
            listen: "127.0.0.1".into(),
            auth_enabled: false,
            ..McpHttpConfig::default()
        };
        assert!(loopback_no_auth.validate().is_ok());
    }

    #[test]
    fn config_and_token_roundtrip_and_constant_time_eq() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());

        let cfg = McpHttpConfig {
            listen: "127.0.0.1".into(),
            port: 19001,
            endpoint_path: "/mcp".into(),
            auth_enabled: true,
            public_url: "https://mcp.example.com/mcp".into(),
            ssh_target: "ops@server.example.com".into(),
            server_name: "cqs".into(),
            policy_mode: PolicyMode::Standard,
            allow_groups: vec!["docker".into(), "clash".into()],
            deny_actions: vec!["ssh.port".into()],
            allowed_origins: vec![],
        };
        save_config(&ctx, &cfg).unwrap();
        let loaded = load_config(&ctx).unwrap();
        assert_eq!(loaded, cfg);

        let (tok1, created) = ensure_token(&ctx).unwrap();
        assert!(created);
        assert!(tok1.starts_with(TOKEN_PREFIX));
        assert!(constant_time_token_eq(&tok1, &tok1));
        assert!(!constant_time_token_eq(&tok1, "wrong-token"));

        let (tok1_again, created2) = ensure_token(&ctx).unwrap();
        assert!(!created2);
        assert_eq!(tok1, tok1_again);

        let tok2 = rotate_token(&ctx).unwrap();
        assert_ne!(tok1, tok2);
        assert_eq!(read_token(&ctx).unwrap().as_deref(), Some(tok2.as_str()));

        // Debug representation of RevealedMcpAccess never leaks the real token.
        let access = RevealedMcpAccess::load(&ctx, Some("198.51.100.10")).unwrap();
        let dbg = format!("{access:?}");
        assert!(!dbg.contains(&tok2));
        assert!(dbg.contains("[redacted]"));
        let masked = access.masked_snippet(TransportMode::Http, ClientFormat::Json);
        assert!(!masked.contains(&tok2));
        assert!(masked.contains(MASKED_TOKEN));
        let copy = access.copy_snippet(TransportMode::Http, ClientFormat::Json);
        assert!(copy.contains(&tok2));
    }

    #[test]
    fn snippets_render_valid_json_claude_and_codex_across_transports() {
        let cfg = McpHttpConfig {
            public_url: "https://mcp.example.com/mcp".into(),
            ssh_target: "admin@server.example.com".into(),
            policy_mode: PolicyMode::ReadOnly,
            ..McpHttpConfig::default()
        };
        for transport in TransportMode::ALL {
            let j = render_snippet(
                &cfg,
                transport,
                ClientFormat::Json,
                "/usr/local/bin/cqs",
                Some("198.51.100.10"),
                None,
                Some(REDACTED_TOKEN),
            );
            let parsed: Value = serde_json::from_str(&j).unwrap();
            assert!(parsed["mcpServers"]["cqs"].is_object());

            let claude = render_snippet(
                &cfg,
                transport,
                ClientFormat::Claude,
                "/usr/local/bin/cqs",
                Some("198.51.100.10"),
                None,
                Some(REDACTED_TOKEN),
            );
            assert!(claude.starts_with("claude mcp add "));

            let codex = render_snippet(
                &cfg,
                transport,
                ClientFormat::Codex,
                "/usr/local/bin/cqs",
                Some("198.51.100.10"),
                None,
                Some(REDACTED_TOKEN),
            );
            let parsed_toml: toml::Value = toml::from_str(&codex).unwrap();
            assert!(parsed_toml["mcp_servers"]["cqs"].is_table());
        }
    }

    #[test]
    fn ssh_target_rejects_option_injection_and_spaces() {
        assert!(validate_ssh_target("-oProxyCommand=id").is_err());
        assert!(validate_ssh_target("user@host rm -rf").is_err());
        assert!(validate_ssh_target("admin@server.example.com").is_ok());
    }
}

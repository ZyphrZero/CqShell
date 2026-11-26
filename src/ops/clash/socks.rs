//! Dedicated Mihomo SOCKS5 / SOCKS5h inbound listener (`listeners` entry `cqs-socks5`)
//! with enforced non-loopback authentication, private `0600` credential card output,
//! and zero-downtime controller hot-reload when Mihomo is already running.

use super::*;
use serde_yaml::Mapping;
use std::net::IpAddr;

pub const LISTENER_NAME: &str = "cqs-socks5";
pub const DEFAULT_PORT: u16 = 10808;
pub const DEFAULT_LISTEN: &str = "0.0.0.0";
pub const DEFAULT_USER: &str = "cqs";

const MAX_CREDENTIAL_FILE: u64 = 512;

fn key(name: &str) -> Value {
    Value::String(name.into())
}

pub(crate) fn auth_path(ctx: &Context) -> PathBuf {
    ctx.paths.clash_data.join("socks5.auth")
}

/// Redacted snapshot of the managed `cqs-socks5` listener for TUI forms, row badges, and status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocksConfig {
    pub enabled: bool,
    pub port: u16,
    pub listen: String,
    pub udp: bool,
    pub outbound: String,
    pub has_auth: bool,
    pub username: Option<String>,
}

impl Default for SocksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            listen: DEFAULT_LISTEN.into(),
            udp: true,
            outbound: "direct".into(),
            has_auth: false,
            username: Some(DEFAULT_USER.into()),
        }
    }
}

impl SocksConfig {
    /// Target host portion for client URI display (`<SERVER_IP>` or detected public IP for wildcards).
    pub fn display_host(&self, detected_public_ip: Option<&str>) -> String {
        match normalize_listen(&self.listen) {
            Ok((_, ip)) if ip.is_unspecified() => {
                match detected_public_ip.filter(|s| !s.is_empty()) {
                    Some(pub_ip) => format_host_bracketed(pub_ip),
                    None => "<SERVER_IP>".into(),
                }
            }
            Ok((normalized, _)) => format_host_bracketed(&normalized),
            Err(_) => "<SERVER_IP>".into(),
        }
    }

    /// Format a display-safe `socks5h://` or `socks5://` URI template without exposing secrets.
    pub fn uri_template(&self, scheme: &str, secret_placeholder: &str, host: &str) -> String {
        if self.has_auth {
            let user = self.username.as_deref().unwrap_or(DEFAULT_USER);
            format!(
                "{scheme}://{user}:{secret_placeholder}@{host}:{}",
                self.port
            )
        } else {
            format!("{scheme}://{host}:{}", self.port)
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

pub(crate) fn validate_port(value: &str) -> Result<u16> {
    if value.is_empty() || value.len() > 5 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::new(
            "error.input",
            "SOCKS5 port must be between 1 and 65535",
        ));
    }
    let port = value
        .parse::<u16>()
        .map_err(|_| Error::new("error.input", "SOCKS5 port must be between 1 and 65535"))?;
    if port == 0 {
        return Err(Error::new(
            "error.input",
            "SOCKS5 port must be between 1 and 65535",
        ));
    }
    Ok(port)
}

pub(crate) fn normalize_listen(value: &str) -> Result<(String, IpAddr)> {
    let trimmed = value.trim();
    let candidate = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    let ip = candidate.parse::<IpAddr>().map_err(|_| {
        Error::new(
            "error.input",
            "listen address must be a valid IPv4 or IPv6 address (e.g. 0.0.0.0, ::, 127.0.0.1)",
        )
    })?;
    Ok((ip.to_string(), ip))
}

pub(crate) fn is_loopback_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

pub(crate) fn validate_username(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(Error::new(
            "error.input",
            "SOCKS5 username must be 1-64 ASCII characters in [A-Za-z0-9._-]",
        ));
    }
    Ok(())
}

pub(crate) fn validate_password(value: &str) -> Result<()> {
    if !(8..=256).contains(&value.len()) || !value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return Err(Error::new(
            "error.input",
            "SOCKS5 password must be 8-256 printable ASCII characters without spaces",
        ));
    }
    Ok(())
}

pub(crate) fn validate_form(
    enabled: bool,
    port: &str,
    listen: &str,
    outbound: &str,
    auth: &str,
    user: &str,
) -> Result<()> {
    validate_port(port)?;
    let (_, ip) = normalize_listen(listen)?;
    if !matches!(outbound, "direct" | "rule") {
        return Err(Error::new(
            "error.input",
            "SOCKS5 outbound must be direct or rule",
        ));
    }
    if !matches!(auth, "auto" | "keep" | "off") {
        return Err(Error::new(
            "error.input",
            "SOCKS5 authentication mode must be auto, keep or off",
        ));
    }
    if !user.is_empty() {
        validate_username(user)?;
    }
    if enabled && auth == "off" && !is_loopback_ip(&ip) {
        return Err(Error::new(
            "error.input",
            "non-loopback SOCKS5 listener requires authentication; set socks_auth to auto/keep or bind to 127.0.0.1",
        ));
    }
    Ok(())
}

/// Enforce port uniqueness and non-loopback authentication on `cqs-socks5` across all runtime builds.
pub(crate) fn validate_runtime(value: &Value) -> Result<()> {
    let Some(root) = value.as_mapping() else {
        return Err(Error::new(
            "error.data",
            "Clash configuration root must be a map",
        ));
    };
    let Some(listeners_val) = root.get(key("listeners")) else {
        return Ok(());
    };
    let listeners = listeners_val
        .as_sequence()
        .ok_or_else(|| Error::new("error.data", "Clash listeners must be a list"))?;

    let mut managed = None;
    let mut other_ports = Vec::new();
    for top_key in [
        "mixed-port",
        "port",
        "socks-port",
        "redir-port",
        "tproxy-port",
    ] {
        if let Some(p) = scalar_port(root.get(key(top_key)))? {
            other_ports.push((top_key.to_owned(), p));
        }
    }

    for entry in listeners {
        let Some(map) = entry.as_mapping() else {
            continue;
        };
        let name = map.get(key("name")).and_then(Value::as_str).unwrap_or("");
        if name == LISTENER_NAME {
            if managed.replace(map).is_some() {
                return Err(Error::new(
                    "error.data",
                    "duplicate cqs-socks5 listener entry",
                ));
            }
        } else if let Ok(Some(p)) = scalar_port(map.get(key("port"))) {
            let label = if name.is_empty() {
                "listener".to_owned()
            } else {
                format!("listener:{name}")
            };
            other_ports.push((label, p));
        }
    }

    let Some(map) = managed else {
        return Ok(());
    };
    if map.get(key("type")).and_then(Value::as_str) != Some("socks") {
        return Err(Error::new(
            "error.data",
            "cqs-socks5 listener must keep type: socks",
        ));
    }
    let port = scalar_port(map.get(key("port")))?
        .ok_or_else(|| Error::new("error.data", "cqs-socks5 listener requires a valid port"))?;
    if let Some((conflict, _)) = other_ports.iter().find(|(_, p)| *p == port) {
        return Err(Error::new(
            "error.input",
            format!("SOCKS5 port {port} conflicts with {conflict}"),
        ));
    }
    let listen_raw = map
        .get(key("listen"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_LISTEN);
    let (_, ip) = normalize_listen(listen_raw)?;
    if let Some(proxy) = map.get(key("proxy")).and_then(Value::as_str)
        && proxy != "DIRECT"
    {
        return Err(Error::new(
            "error.data",
            "cqs-socks5 proxy override only supports DIRECT",
        ));
    }

    let users = map.get(key("users")).and_then(Value::as_sequence);
    let has_valid_users = match users {
        Some(list) if !list.is_empty() => {
            for item in list {
                let user_map = item.as_mapping().ok_or_else(|| {
                    Error::new("error.data", "cqs-socks5 user entry must be a map")
                })?;
                let u = user_map
                    .get(key("username"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let p = user_map
                    .get(key("password"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                validate_username(u)?;
                validate_password(p)?;
            }
            true
        }
        _ => false,
    };

    if !is_loopback_ip(&ip) && !has_valid_users {
        return Err(Error::new(
            "error.data",
            "non-loopback cqs-socks5 listener requires user authentication",
        ));
    }
    Ok(())
}

struct ExistingCredentials {
    config: SocksConfig,
    password: Option<String>,
}

fn read_existing(ctx: &Context) -> Result<ExistingCredentials> {
    let mixin = read_mixin(ctx)?;
    let Some(root) = mixin.as_mapping() else {
        return Err(Error::new("error.data", "Mixin root must be a map"));
    };
    let Some(listeners) = root.get(key("listeners")).and_then(Value::as_sequence) else {
        return Ok(ExistingCredentials {
            config: SocksConfig::default(),
            password: None,
        });
    };
    let Some(entry) = listeners.iter().find_map(|item| {
        let map = item.as_mapping()?;
        (map.get(key("name")).and_then(Value::as_str) == Some(LISTENER_NAME)).then_some(map)
    }) else {
        return Ok(ExistingCredentials {
            config: SocksConfig::default(),
            password: None,
        });
    };

    let port = scalar_port(entry.get(key("port")))?.unwrap_or(DEFAULT_PORT);
    let listen = entry
        .get(key("listen"))
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_LISTEN)
        .to_owned();
    let udp = entry
        .get(key("udp"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let outbound = match entry.get(key("proxy")).and_then(Value::as_str) {
        Some("DIRECT") => "direct".into(),
        _ => "rule".into(),
    };
    let first_user = entry
        .get(key("users"))
        .and_then(Value::as_sequence)
        .and_then(|list| list.first())
        .and_then(Value::as_mapping);
    let username = first_user
        .and_then(|u| u.get(key("username")).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let password = first_user
        .and_then(|u| u.get(key("password")).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let has_auth = username.is_some() && password.is_some();

    Ok(ExistingCredentials {
        config: SocksConfig {
            enabled: true,
            port,
            listen,
            udp,
            outbound,
            has_auth,
            username: username.or_else(|| Some(DEFAULT_USER.into())),
        },
        password,
    })
}

/// Read the current redacted `cqs-socks5` configuration from Mixin.
pub fn inspect(ctx: &Context) -> Result<SocksConfig> {
    Ok(read_existing(ctx)?.config)
}

/// Full in-memory SOCKS5 / SOCKS5h credentials for transient TUI reveal and OSC 52 copy only.
/// Never printed to CLI stdout/stderr or written to task logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealedCredentials {
    pub listen: String,
    pub port: u16,
    pub host: String,
    pub udp: bool,
    pub outbound: String,
    pub has_auth: bool,
    pub username: Option<String>,
    pub password: Option<String>,
    pub socks5h_uri: String,
    pub socks5_uri: String,
    pub curl_cmd: String,
    pub auth_file: PathBuf,
}

impl RevealedCredentials {
    /// Format a display-masked `socks5h://` or `socks5://` URI when hiding the password on screen.
    pub fn masked_uri(&self, scheme: &str) -> String {
        if self.has_auth {
            let user = self.username.as_deref().unwrap_or(DEFAULT_USER);
            format!("{scheme}://{user}:********@{}:{}", self.host, self.port)
        } else {
            format!("{scheme}://{}:{}", self.host, self.port)
        }
    }

    /// Format a display-masked `curl` command when hiding the password on screen.
    pub fn masked_curl_cmd(&self) -> String {
        format!(
            "curl -x '{}' https://ifconfig.me",
            self.masked_uri("socks5h")
        )
    }
}

/// Load the configured `cqs-socks5` credentials directly from Mixin for TUI-only transient reveal.
/// Returns `Ok(None)` when the dedicated listener is not configured.
pub fn reveal(
    ctx: &Context,
    detected_public_ip: Option<&str>,
) -> Result<Option<RevealedCredentials>> {
    let existing = read_existing(ctx)?;
    if !existing.config.enabled {
        return Ok(None);
    }
    let cfg = existing.config;
    let host = cfg.display_host(detected_public_ip);
    let (socks5h_uri, socks5_uri, username, password) = if cfg.has_auth {
        let user = cfg.username.unwrap_or_else(|| DEFAULT_USER.into());
        let pass = existing.password.unwrap_or_default();
        let uri_h = format_secret_uri("socks5h", &user, &pass, &host, cfg.port)?;
        let uri_5 = format_secret_uri("socks5", &user, &pass, &host, cfg.port)?;
        (uri_h, uri_5, Some(user), Some(pass))
    } else {
        (
            format!("socks5h://{host}:{}", cfg.port),
            format!("socks5://{host}:{}", cfg.port),
            None,
            None,
        )
    };
    let curl_cmd = format!("curl -x '{socks5h_uri}' https://ifconfig.me");
    Ok(Some(RevealedCredentials {
        listen: cfg.listen,
        port: cfg.port,
        host,
        udp: cfg.udp,
        outbound: cfg.outbound,
        has_auth: cfg.has_auth,
        username,
        password,
        socks5h_uri,
        socks5_uri,
        curl_cmd,
        auth_file: auth_path(ctx),
    }))
}

fn generate_password() -> Result<String> {
    #[cfg(unix)]
    {
        const ALPHABET: &[u8; 62] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut out = String::with_capacity(24);
        let mut urandom = fs::File::open("/dev/urandom")?;
        let mut buf = [0u8; 64];
        while out.len() < 24 {
            urandom.read_exact(&mut buf)?;
            for &b in &buf {
                // 62 * 4 == 248: reject 248..=255 for zero modulo bias.
                if b < 248 {
                    out.push(ALPHABET[usize::from(b % 62)] as char);
                    if out.len() == 24 {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }
    #[cfg(not(unix))]
    Err(Error::new(
        "error.platform",
        "SOCKS5 password generation requires Unix; use clash.socks.auth.set with a private file",
    ))
}

fn format_secret_uri(
    scheme: &str,
    username: &str,
    password: &str,
    host: &str,
    port: u16,
) -> Result<String> {
    let placeholder_host = host == "<SERVER_IP>";
    let parse_host = if placeholder_host {
        "example.invalid"
    } else {
        host
    };
    let mut url = url::Url::parse(&format!("{scheme}://{parse_host}:{port}"))
        .map_err(|e| Error::new("error.url", e))?;
    url.set_username(username)
        .map_err(|_| Error::new("error.input", "invalid SOCKS5 username"))?;
    url.set_password(Some(password))
        .map_err(|_| Error::new("error.input", "invalid SOCKS5 password"))?;
    let mut rendered = url.to_string();
    // `url::Url` appends no path for custom schemes like `socks5h`, or `/` for known schemes; trim trailing `/`.
    if rendered.ends_with('/') {
        rendered.pop();
    }
    if placeholder_host {
        rendered = rendered.replacen("@example.invalid:", "@<SERVER_IP>:", 1);
    }
    Ok(rendered)
}

fn render_auth_card(
    port: u16,
    listen: &str,
    ip: &IpAddr,
    udp: bool,
    outbound: &str,
    username: &str,
    password: &str,
) -> Result<Vec<u8>> {
    let host = if ip.is_unspecified() {
        "<SERVER_IP>".into()
    } else {
        format_host_bracketed(listen)
    };
    let socks5h = format_secret_uri("socks5h", username, password, &host, port)?;
    let socks5 = format_secret_uri("socks5", username, password, &host, port)?;
    let text = format!(
        "# CqShell managed SOCKS5 / SOCKS5h credentials (mode 0600)\n\
         # Replace <SERVER_IP> with this server's public or LAN IP when connecting remotely.\n\
         listen={listen}:{port}\n\
         udp={udp}\n\
         outbound={outbound}\n\
         username={username}\n\
         password={password}\n\
         socks5h_uri={socks5h}\n\
         socks5_uri={socks5}\n"
    );
    Ok(text.into_bytes())
}

fn build_listener_value(
    port: u16,
    listen: &str,
    udp: bool,
    outbound: &str,
    credentials: Option<(&str, &str)>,
) -> Value {
    let mut map = Mapping::new();
    map.insert(key("name"), key(LISTENER_NAME));
    map.insert(key("type"), key("socks"));
    map.insert(key("port"), Value::Number(port.into()));
    map.insert(key("listen"), key(listen));
    map.insert(key("udp"), Value::Bool(udp));
    if outbound == "direct" {
        map.insert(key("proxy"), key("DIRECT"));
    }
    let users = match credentials {
        Some((username, password)) => {
            let mut u = Mapping::new();
            u.insert(key("username"), key(username));
            u.insert(key("password"), key(password));
            Value::Sequence(vec![Value::Mapping(u)])
        }
        None => Value::Sequence(Vec::new()),
    };
    map.insert(key("users"), users);
    Value::Mapping(map)
}

fn upsert_mixin_listener(root: &mut Mapping, listener: Option<Value>) -> Result<()> {
    let listeners_key = key("listeners");
    let mut list = match root.remove(&listeners_key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Sequence(items)) => items,
        Some(_) => {
            return Err(Error::new(
                "error.data",
                "Mixin listeners section must be a list",
            ));
        }
    };
    let mut replaced = false;
    let mut next = Vec::with_capacity(list.len() + 1);
    for item in list.drain(..) {
        let is_target = item
            .as_mapping()
            .and_then(|m| m.get(key("name")))
            .and_then(Value::as_str)
            == Some(LISTENER_NAME);
        if is_target {
            if let Some(ref updated) = listener
                && !replaced
            {
                next.push(updated.clone());
                replaced = true;
            }
        } else {
            next.push(item);
        }
    }
    if !replaced && let Some(updated) = listener {
        next.push(updated);
    }
    if !next.is_empty() {
        root.insert(listeners_key, Value::Sequence(next));
    }
    Ok(())
}

/// Apply an update to `cqs-socks5` in Mixin + runtime + `socks5.auth`, hot-reloading when running.
fn commit_socks_change(
    ctx: &Context,
    listener: Option<Value>,
    auth_card: Option<Vec<u8>>,
) -> Result<bool> {
    ctx.check()?;
    let _lock = Lock::acquire(&ctx.paths.clash_lock)?;
    recover_interrupted_files(ctx)?;
    let current_state = state(ctx)?;
    let reload_target = match current_state {
        State::Unknown => return Err(unknown_process()),
        State::Running(_) => Some(controller::settings(ctx).map_err(|_| {
            Error::new(
                "error.busy",
                "Mihomo is running without external-controller; stop Mihomo before changing SOCKS5 settings",
            )
        })?),
        State::Stopped | State::NotInstalled => None,
    };

    ensure_layout(ctx)?;
    let cred_path = auth_path(ctx);
    storage::check_path(&ctx.paths.clash_mixin, true)?;
    storage::check_path(&ctx.paths.clash_runtime, true)?;
    storage::check_path(&cred_path, true)?;

    let prev_mixin = storage::Saved::capture(&ctx.paths.clash_mixin)?;
    let prev_runtime = storage::Saved::capture(&ctx.paths.clash_runtime)?;
    let prev_auth = storage::Saved::capture(&cred_path)?;

    let mut mixin = read_mixin(ctx)?;
    let root = mixin
        .as_mapping_mut()
        .ok_or_else(|| Error::new("error.data", "Mixin root must be a map"))?;
    upsert_mixin_listener(root, listener)?;

    let runtime = runtime_with_mixin(ctx, mixin.clone())?;
    let candidate = tempfile::NamedTempFile::new_in(&ctx.paths.clash_data)?;
    fs::write(candidate.path(), &runtime)?;
    validate_config(ctx, candidate.path(), true)?;
    if ctx.paths.clash_binary.is_file() {
        require_binary(ctx)?;
        let output = ctx.command(
            &path_string(&ctx.paths.clash_binary)?,
            &[
                "-d",
                &path_string(&ctx.paths.clash_resources)?,
                "-f",
                &path_string(candidate.path())?,
                "-t",
            ],
            false,
        )?;
        if !output.status.success() {
            return Err(Error::new(
                "error.verify",
                "Mihomo rejected SOCKS5 settings; previous configuration retained",
            ));
        }
    }

    let mixin_bytes = serde_yaml::to_string(&mixin)?.into_bytes();
    let updates = vec![
        (
            ctx.paths.clash_mixin.clone(),
            Some(storage::Saved::new(&mixin_bytes, 0o600)),
        ),
        (
            ctx.paths.clash_runtime.clone(),
            Some(storage::Saved::new(&runtime, 0o600)),
        ),
        (
            cred_path.clone(),
            auth_card.map(|bytes| storage::Saved::new(&bytes, 0o600)),
        ),
    ];
    commit_files(ctx, &updates)?;

    if let Some(ctrl_settings) = reload_target {
        if let Err(err) = controller::push_runtime(ctx, &ctrl_settings, &runtime) {
            let rollback = vec![
                (ctx.paths.clash_mixin.clone(), prev_mixin),
                (ctx.paths.clash_runtime.clone(), prev_runtime),
                (cred_path, prev_auth),
            ];
            let _ = commit_files(&ctx.recovery(), &rollback);
            return Err(err);
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Display the current dedicated SOCKS5 / SOCKS5h listener status without exposing passwords.
pub fn status(ctx: &Context) -> Result<()> {
    let cfg = inspect(ctx)?;
    if !cfg.enabled {
        ctx.message("notice.clash_socks", "not configured");
        return Ok(());
    }
    let mihomo = state(ctx)?.label();
    let auth_desc = if cfg.has_auth {
        format!(
            "user:{} (redacted; file: {})",
            cfg.username.as_deref().unwrap_or(DEFAULT_USER),
            auth_path(ctx).display()
        )
    } else {
        "none (loopback only)".into()
    };
    let host = cfg.display_host(None);
    let socks5h = cfg.uri_template("socks5h", "[redacted]", &host);
    let socks5 = cfg.uri_template("socks5", "[redacted]", &host);
    ctx.message(
        "notice.clash_socks",
        format!(
            "enabled=true; listen={}:{}; udp={}; outbound={}; auth={auth_desc}; mihomo={mihomo}\n  socks5h (remote DNS): {socks5h}\n  socks5  (local DNS):  {socks5}",
            format_host_bracketed(&cfg.listen),
            cfg.port,
            cfg.udp,
            cfg.outbound,
        ),
    );
    Ok(())
}

/// Configure or disable the dedicated `cqs-socks5` SOCKS5 / SOCKS5h inbound listener.
#[allow(clippy::too_many_arguments)]
pub fn configure(
    ctx: &Context,
    enabled: bool,
    port_raw: &str,
    listen_raw: &str,
    udp: bool,
    outbound: &str,
    auth_mode: &str,
    user_raw: &str,
) -> Result<()> {
    validate_form(enabled, port_raw, listen_raw, outbound, auth_mode, user_raw)?;
    if !enabled {
        let reloaded = commit_socks_change(ctx, None, None)?;
        ctx.message(
            "notice.clash_socks",
            if reloaded {
                "disabled and hot-reloaded into running Mihomo"
            } else {
                "disabled in Mixin; start Mihomo to apply"
            },
        );
        return Ok(());
    }

    let port = validate_port(port_raw)?;
    let (listen, ip) = normalize_listen(listen_raw)?;
    let existing = read_existing(ctx)?;
    let username = if user_raw.trim().is_empty() {
        existing
            .config
            .username
            .as_deref()
            .unwrap_or(DEFAULT_USER)
            .to_owned()
    } else {
        user_raw.trim().to_owned()
    };
    validate_username(&username)?;

    let credentials = match auth_mode {
        "off" => None,
        "keep" => {
            let password = existing.password.ok_or_else(|| {
                Error::new(
                    "error.input",
                    "no existing SOCKS5 password found; choose socks_auth=auto or run clash.socks.auth.set",
                )
            })?;
            validate_password(&password)?;
            Some((username, password))
        }
        "auto" => {
            let password = match existing.password {
                Some(pw) if validate_password(&pw).is_ok() => pw,
                _ => generate_password()?,
            };
            Some((username, password))
        }
        _ => unreachable!(),
    };

    let auth_card = match &credentials {
        Some((u, p)) => Some(render_auth_card(port, &listen, &ip, udp, outbound, u, p)?),
        None => None,
    };
    let listener = build_listener_value(
        port,
        &listen,
        udp,
        outbound,
        credentials.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
    );
    let reloaded = commit_socks_change(ctx, Some(listener), auth_card)?;

    let apply_note = if reloaded {
        "saved and hot-reloaded into running Mihomo"
    } else {
        "saved; start Mihomo to apply"
    };
    let cred_note = if credentials.is_some() {
        format!("; credentials written to {}", auth_path(ctx).display())
    } else {
        String::new()
    };
    ctx.message(
        "notice.clash_socks",
        format!(
            "listen={}:{port}; udp={udp}; outbound={outbound}; {apply_note}{cred_note}",
            format_host_bracketed(&listen)
        ),
    );
    Ok(())
}

/// Generate/rotate a fresh 24-character CSPRNG password for `cqs-socks5`.
pub fn auth_generate(ctx: &Context) -> Result<()> {
    ctx.check()?;
    let existing = read_existing(ctx)?;
    if !existing.config.enabled {
        return Err(Error::new(
            "error.input",
            "SOCKS5 listener is not configured yet; run clash.socks.configure first",
        ));
    }
    let password = generate_password()?;
    let username = existing
        .config
        .username
        .unwrap_or_else(|| DEFAULT_USER.into());
    validate_username(&username)?;
    let (listen, ip) = normalize_listen(&existing.config.listen)?;
    let port = existing.config.port;
    let udp = existing.config.udp;
    let outbound = existing.config.outbound;

    let auth_card = render_auth_card(port, &listen, &ip, udp, &outbound, &username, &password)?;
    let listener =
        build_listener_value(port, &listen, udp, &outbound, Some((&username, &password)));
    let reloaded = commit_socks_change(ctx, Some(listener), Some(auth_card))?;
    ctx.message(
        "notice.clash_socks",
        format!(
            "rotated password for user '{username}' (saved to {}); {}",
            auth_path(ctx).display(),
            if reloaded {
                "hot-reloaded into running Mihomo"
            } else {
                "start Mihomo to apply"
            }
        ),
    );
    Ok(())
}

/// Load `username:password` (or `password`) from a `0600` private file into `cqs-socks5`.
pub fn auth_set(ctx: &Context, source: &str) -> Result<()> {
    ctx.check()?;
    let existing = read_existing(ctx)?;
    if !existing.config.enabled {
        return Err(Error::new(
            "error.input",
            "SOCKS5 listener is not configured yet; run clash.socks.configure first",
        ));
    }
    let path = Path::new(source);
    storage::check_path(path, true)?;
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_CREDENTIAL_FILE {
        return Err(Error::new(
            "error.input",
            "credential file must be a regular file of at most 512 bytes",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::new(
                "error.input",
                "credential file must not be accessible by group or others (chmod 600)",
            ));
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_CREDENTIAL_FILE + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CREDENTIAL_FILE {
        return Err(Error::new(
            "error.input",
            "credential file exceeds 512 bytes",
        ));
    }
    let raw = String::from_utf8(bytes)
        .map_err(|_| Error::new("error.input", "cannot read credential file as UTF-8"))?;
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        return Err(Error::new("error.input", "credential file is empty"));
    }
    let (username, password) = match trimmed.split_once(':') {
        Some((u, p)) => (u.to_owned(), p.to_owned()),
        None => (
            existing
                .config
                .username
                .unwrap_or_else(|| DEFAULT_USER.into()),
            trimmed.to_owned(),
        ),
    };
    validate_username(&username)?;
    validate_password(&password)?;

    let (listen, ip) = normalize_listen(&existing.config.listen)?;
    let port = existing.config.port;
    let udp = existing.config.udp;
    let outbound = existing.config.outbound;
    let auth_card = render_auth_card(port, &listen, &ip, udp, &outbound, &username, &password)?;
    let listener =
        build_listener_value(port, &listen, udp, &outbound, Some((&username, &password)));
    let reloaded = commit_socks_change(ctx, Some(listener), Some(auth_card))?;
    ctx.message(
        "notice.clash_socks",
        format!(
            "updated credentials for user '{username}' (saved to {}); {}",
            auth_path(ctx).display(),
            if reloaded {
                "hot-reloaded into running Mihomo"
            } else {
                "start Mihomo to apply"
            }
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_loopback_listener_requires_authentication_in_form_and_runtime() {
        for bad_listen in ["0.0.0.0", "::", "[::]", "192.168.1.10", "203.0.113.5"] {
            assert!(
                validate_form(true, "10808", bad_listen, "direct", "off", "cqs").is_err(),
                "{bad_listen}"
            );
        }
        for loopback in ["127.0.0.1", "::1", "[::1]", "::ffff:127.0.0.1"] {
            validate_form(true, "10808", loopback, "direct", "off", "").unwrap();
        }
        // Disabled listener permits any valid address/auth combo so disabling never gets stuck.
        validate_form(false, "10808", "0.0.0.0", "direct", "off", "").unwrap();

        let unauth_public: Value = serde_yaml::from_str(
            "mixed-port: 7890\nmode: direct\nlisteners:\n  - name: cqs-socks5\n    type: socks\n    port: 10808\n    listen: 0.0.0.0\n    users: []\n",
        )
        .unwrap();
        assert!(validate_runtime(&unauth_public).is_err());

        let port_collision: Value = serde_yaml::from_str(
            "mixed-port: 7890\nmode: direct\nlisteners:\n  - name: cqs-socks5\n    type: socks\n    port: 7890\n    listen: 127.0.0.1\n    users: []\n",
        )
        .unwrap();
        assert!(validate_runtime(&port_collision).is_err());
    }

    #[test]
    fn configure_auth_set_redaction_and_disable_preserve_other_settings() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "log-level: warning\nlisteners:\n  - name: other-in\n    type: mixed\n    port: 17891\n    listen: 127.0.0.1\n",
        )
        .unwrap();

        // Loopback unauthenticated first (works on all platforms including Windows).
        configure(
            &ctx,
            true,
            "10808",
            "127.0.0.1",
            true,
            "direct",
            "off",
            "cqs",
        )
        .unwrap();
        let cfg = inspect(&ctx).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.port, 10808);
        assert_eq!(cfg.listen, "127.0.0.1");
        assert_eq!(cfg.outbound, "direct");
        assert!(!cfg.has_auth);
        assert!(!auth_path(&ctx).exists());

        // Set credentials from a private file, then switch to 0.0.0.0 with socks_auth=keep.
        let secret_file = temp.path().join("creds.txt");
        fs::write(&secret_file, "alice:S3cret#Pass!99\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&secret_file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        auth_set(&ctx, secret_file.to_str().unwrap()).unwrap();
        configure(&ctx, true, "21080", "0.0.0.0", false, "rule", "keep", "").unwrap();

        let cfg = inspect(&ctx).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.port, 21080);
        assert_eq!(cfg.listen, "0.0.0.0");
        assert!(!cfg.udp);
        assert_eq!(cfg.outbound, "rule");
        assert!(cfg.has_auth);
        assert_eq!(cfg.username.as_deref(), Some("alice"));

        let card = fs::read_to_string(auth_path(&ctx)).unwrap();
        assert!(
            card.contains("socks5h_uri=socks5h://alice:S3cret%23Pass!99@<SERVER_IP>:21080"),
            "{card}"
        );
        assert!(
            card.contains("socks5_uri=socks5://alice:S3cret%23Pass!99@<SERVER_IP>:21080"),
            "{card}"
        );

        // Verify redaction in logs and mixin_status never leaks the password.
        let redacted_log = logs::redact(&ctx, "auth failed for S3cret#Pass!99").unwrap();
        assert!(!redacted_log.contains("S3cret#Pass!99"));
        assert!(redacted_log.contains("[redacted]"));

        // Other mixin keys and other listeners remain intact.
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["log-level"], "warning");
        assert_eq!(mixin["listeners"].as_sequence().unwrap().len(), 2);

        // Disabling removes only `cqs-socks5` and cleans `socks5.auth`.
        configure(
            &ctx, false, "21080", "0.0.0.0", false, "rule", "off", "alice",
        )
        .unwrap();
        assert!(!inspect(&ctx).unwrap().enabled);
        assert!(!auth_path(&ctx).exists());
        let mixin: Value =
            serde_yaml::from_slice(&fs::read(&ctx.paths.clash_mixin).unwrap()).unwrap();
        assert_eq!(mixin["listeners"].as_sequence().unwrap().len(), 1);
        assert_eq!(mixin["listeners"][0]["name"], "other-in");
    }

    #[cfg(unix)]
    #[test]
    fn unix_auto_auth_generates_csprng_password_and_enforces_private_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();

        configure(
            &ctx, true, "10808", "0.0.0.0", true, "direct", "auto", "cqs",
        )
        .unwrap();
        let first = read_existing(&ctx).unwrap().password.unwrap();
        assert_eq!(first.len(), 24);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()));
        let mode = fs::metadata(auth_path(&ctx)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        // Rotate password via `auth_generate`
        auth_generate(&ctx).unwrap();
        let second = read_existing(&ctx).unwrap().password.unwrap();
        assert_eq!(second.len(), 24);
        assert_ne!(first, second);

        // Reject world-readable credential file in `auth_set`
        let insecure = temp.path().join("insecure.txt");
        fs::write(&insecure, "bob:ValidPass123\n").unwrap();
        fs::set_permissions(&insecure, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(auth_set(&ctx, insecure.to_str().unwrap()).is_err());
    }

    #[test]
    fn reveal_returns_none_when_unconfigured_and_formats_authenticated_or_loopback_uris() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        ensure_layout(&ctx).unwrap();

        assert_eq!(reveal(&ctx, Some("203.0.113.88")).unwrap(), None);

        // Loopback unauthenticated listener
        configure(
            &ctx,
            true,
            "10808",
            "127.0.0.1",
            false,
            "rule",
            "off",
            "cqs",
        )
        .unwrap();
        let loop_creds = reveal(&ctx, Some("203.0.113.88")).unwrap().unwrap();
        assert!(!loop_creds.has_auth);
        assert_eq!(loop_creds.username, None);
        assert_eq!(loop_creds.password, None);
        assert_eq!(loop_creds.socks5h_uri, "socks5h://127.0.0.1:10808");
        assert_eq!(loop_creds.socks5_uri, "socks5://127.0.0.1:10808");
        assert_eq!(
            loop_creds.masked_uri("socks5h"),
            "socks5h://127.0.0.1:10808"
        );

        // Authenticated wildcard listener with special chars in password
        let secret_file = temp.path().join("creds.txt");
        fs::write(&secret_file, "alice:S3cret#Pass!99\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&secret_file, fs::Permissions::from_mode(0o600)).unwrap();
        }
        auth_set(&ctx, secret_file.to_str().unwrap()).unwrap();
        configure(&ctx, true, "21080", "0.0.0.0", true, "direct", "keep", "").unwrap();

        let with_ip = reveal(&ctx, Some("203.0.113.88")).unwrap().unwrap();
        assert!(with_ip.has_auth);
        assert_eq!(with_ip.username.as_deref(), Some("alice"));
        assert_eq!(with_ip.password.as_deref(), Some("S3cret#Pass!99"));
        assert_eq!(
            with_ip.socks5h_uri,
            "socks5h://alice:S3cret%23Pass!99@203.0.113.88:21080"
        );
        assert_eq!(
            with_ip.socks5_uri,
            "socks5://alice:S3cret%23Pass!99@203.0.113.88:21080"
        );
        assert_eq!(
            with_ip.masked_uri("socks5h"),
            "socks5h://alice:********@203.0.113.88:21080"
        );
        assert!(with_ip.masked_curl_cmd().contains("********"));
        assert!(!with_ip.masked_curl_cmd().contains("S3cret"));

        let without_ip = reveal(&ctx, None).unwrap().unwrap();
        assert_eq!(
            without_ip.socks5h_uri,
            "socks5h://alice:S3cret%23Pass!99@<SERVER_IP>:21080"
        );
    }
}

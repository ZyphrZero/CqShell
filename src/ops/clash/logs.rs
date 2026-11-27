use super::*;
use regex::Regex;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{Seek, SeekFrom},
    sync::{Arc, Mutex, OnceLock},
};

const WINDOW: u64 = 1024 * 1024;
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
const LIVE_ROTATE_BYTES: u64 = 2 * MAX_LOG_BYTES;
const LIVE_KEEP_TAIL_BYTES: usize = 256 * 1024;
const MAX_CACHED_AUTOMATA: usize = 16;

fn rotated_log_path(ctx: &Context) -> PathBuf {
    let mut name = ctx.paths.clash_log.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

fn rotate_on_open_with_limit(ctx: &Context, max_bytes: u64) -> Result<bool> {
    storage::check_path(&ctx.paths.clash_log, true)?;
    let meta = match fs::symlink_metadata(&ctx.paths.clash_log) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if !meta.is_file() || meta.len() <= max_bytes {
        return Ok(false);
    }
    let backup = rotated_log_path(ctx);
    storage::check_path(&backup, true)?;
    let _ = storage::remove_file(&backup);
    fs::rename(&ctx.paths.clash_log, &backup)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

/// Safely rotate a live `O_APPEND` Mihomo log file if it grows beyond `2 * MAX_LOG_BYTES`.
/// Preserves the trailing 256 KiB in `mihomo.log.1` (`0600`) and truncates the active file
/// in place so a running Mihomo daemon keeps appending seamlessly from offset 0.
pub(crate) fn rotate_if_oversized(ctx: &Context) -> Result<bool> {
    rotate_if_oversized_with_limit(ctx, LIVE_ROTATE_BYTES, LIVE_KEEP_TAIL_BYTES)
}

fn rotate_if_oversized_with_limit(
    ctx: &Context,
    max_bytes: u64,
    keep_tail_bytes: usize,
) -> Result<bool> {
    storage::check_path(&ctx.paths.clash_log, true)?;
    let meta = match fs::symlink_metadata(&ctx.paths.clash_log) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err.into()),
    };
    if !meta.is_file() || meta.len() <= max_bytes {
        return Ok(false);
    }
    let backup = rotated_log_path(ctx);
    storage::check_path(&backup, true)?;
    let mut reader = fs::File::open(&ctx.paths.clash_log)?;
    let start = meta.len().saturating_sub(keep_tail_bytes as u64);
    reader.seek(SeekFrom::Start(start))?;
    let mut tail_bytes = Vec::new();
    reader
        .take(keep_tail_bytes as u64)
        .read_to_end(&mut tail_bytes)?;
    storage::atomic_write(&backup, &tail_bytes, 0o600)?;

    let mut opts = OpenOptions::new();
    opts.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let active = opts.open(&ctx.paths.clash_log)?;
    active.set_len(0)?;
    Ok(true)
}

pub(super) fn open_private_log(ctx: &Context) -> Result<fs::File> {
    let _ = rotate_on_open_with_limit(ctx, MAX_LOG_BYTES)?;
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(&ctx.paths.clash_log)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        Ok(file)
    }
    #[cfg(not(unix))]
    Ok(options.open(&ctx.paths.clash_log)?)
}

/// Linear-time multi-pattern matcher (Aho-Corasick trie + dictionary suffix links) with
/// leftmost-longest non-overlapping single-pass replacement, replacing dynamic `Regex::new`
/// alternation so 1,000+ node subscriptions never hit regex compilation limits.
#[derive(Debug)]
struct AcNode {
    edges: Vec<(u8, u32)>,
    fail: u32,
    depth: usize,
    match_len: usize,
    out_link: u32,
}

impl AcNode {
    fn new(depth: usize) -> Self {
        Self {
            edges: Vec::new(),
            fail: 0,
            depth,
            match_len: 0,
            out_link: 0,
        }
    }

    fn get_edge(&self, byte: u8) -> Option<u32> {
        self.edges
            .binary_search_by_key(&byte, |&(b, _)| b)
            .ok()
            .map(|idx| self.edges[idx].1)
    }

    fn insert_edge(&mut self, byte: u8, target: u32) {
        match self.edges.binary_search_by_key(&byte, |&(b, _)| b) {
            Ok(idx) => self.edges[idx].1 = target,
            Err(idx) => self.edges.insert(idx, (byte, target)),
        }
    }
}

#[derive(Debug)]
struct SecretAutomaton {
    nodes: Vec<AcNode>,
}

impl SecretAutomaton {
    fn build(patterns: &[String]) -> Self {
        let mut nodes = vec![AcNode::new(0)];
        for pat in patterns {
            let bytes = pat.as_bytes();
            if bytes.is_empty() {
                continue;
            }
            let mut cur = 0usize;
            for (depth_idx, &b) in bytes.iter().enumerate() {
                let next = if let Some(existing) = nodes[cur].get_edge(b) {
                    existing as usize
                } else {
                    let id = nodes.len() as u32;
                    nodes.push(AcNode::new(depth_idx + 1));
                    nodes[cur].insert_edge(b, id);
                    id as usize
                };
                cur = next;
            }
            if bytes.len() > nodes[cur].match_len {
                nodes[cur].match_len = bytes.len();
            }
        }

        // BFS to build failure and dictionary-suffix links.
        let mut queue = VecDeque::new();
        for &(_, child) in &nodes[0].edges {
            queue.push_back(child);
        }
        while let Some(u) = queue.pop_front() {
            let edges = nodes[u as usize].edges.clone();
            for (byte, v) in edges {
                let mut f = nodes[u as usize].fail;
                while f != 0 && nodes[f as usize].get_edge(byte).is_none() {
                    f = nodes[f as usize].fail;
                }
                let next_fail = nodes[f as usize].get_edge(byte).unwrap_or(0);
                let out_link = if nodes[next_fail as usize].match_len > 0 {
                    next_fail
                } else {
                    nodes[next_fail as usize].out_link
                };
                nodes[v as usize].fail = next_fail;
                nodes[v as usize].out_link = out_link;
                queue.push_back(v);
            }
        }

        Self { nodes }
    }

    fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    fn step(&self, mut state: u32, byte: u8) -> u32 {
        loop {
            if let Some(next) = self.nodes[state as usize].get_edge(byte) {
                return next;
            }
            if state == 0 {
                return 0;
            }
            state = self.nodes[state as usize].fail;
        }
    }

    /// Replace all non-overlapping occurrences using leftmost-longest semantics in a single pass.
    fn replace_all(&self, text: &str, replacement: &str) -> String {
        if self.is_empty() || text.is_empty() {
            return text.to_owned();
        }
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0usize;
        // Pending candidate matches keyed by start byte -> longest observed end byte.
        let mut pending: BTreeMap<usize, usize> = BTreeMap::new();
        let mut state = 0u32;

        let flush_before = |limit: usize,
                            cursor: &mut usize,
                            pending: &mut BTreeMap<usize, usize>,
                            out: &mut String| {
            while let Some((&start, &end)) = pending.first_key_value() {
                if start >= limit {
                    break;
                }
                pending.pop_first();
                if start < *cursor {
                    continue;
                }
                out.push_str(&text[*cursor..start]);
                out.push_str(replacement);
                *cursor = end;
            }
        };

        for (idx, &b) in bytes.iter().enumerate() {
            state = self.step(state, b);
            let pos = idx + 1;

            let mut u = if self.nodes[state as usize].match_len > 0 {
                state
            } else {
                self.nodes[state as usize].out_link
            };
            while u != 0 {
                let len = self.nodes[u as usize].match_len;
                let start = pos - len;
                if start >= cursor {
                    let entry = pending.entry(start).or_insert(pos);
                    if pos > *entry {
                        *entry = pos;
                    }
                }
                u = self.nodes[u as usize].out_link;
            }

            // No future match can start earlier than `pos - depth[state]`.
            let s_min = pos - self.nodes[state as usize].depth;
            flush_before(s_min, &mut cursor, &mut pending, &mut out);
        }

        flush_before(usize::MAX, &mut cursor, &mut pending, &mut out);
        out.push_str(&text[cursor..]);
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    mtime_secs: u64,
    mtime_nanos: u32,
    ino: u64,
}

fn file_stamp(path: &Path) -> Result<Option<FileStamp>> {
    storage::check_path(path, true)?;
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let (mtime_secs, mtime_nanos) = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs(), d.subsec_nanos()))
        .unwrap_or((0, 0));
    #[cfg(unix)]
    let ino = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    #[cfg(not(unix))]
    let ino = 0u64;
    Ok(Some(FileStamp {
        len: meta.len(),
        mtime_secs,
        mtime_nanos,
        ino,
    }))
}

type AutomatonCacheKey = [PathBuf; 4];
type AutomatonCacheEntry = ([Option<FileStamp>; 4], Arc<SecretAutomaton>);

fn automaton_cache() -> &'static Mutex<HashMap<AutomatonCacheKey, AutomatonCacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<AutomatonCacheKey, AutomatonCacheEntry>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fields_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?im)((?:authorization|secret|password|passwd|token|uuid|psk|auth)[\"']?\s*[:=]\s*)[^\r\n]+"#,
        )
        .expect("constant credential regex")
    })
}

fn urls_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(?:https?|socks4a?|socks5h?)://[^\s\"'<>]+"#).expect("constant URL regex")
    })
}

fn collect_sensitive(value: &Value, sensitive: bool, secrets: &mut Vec<String>) {
    match value {
        Value::String(text) if sensitive && !text.is_empty() => {
            secrets.push(text.clone());
            if let Some((_, password)) = text.split_once(':')
                && !password.is_empty()
            {
                secrets.push(password.into());
            }
        }
        Value::Sequence(items) => {
            for item in items {
                collect_sensitive(item, sensitive, secrets);
            }
        }
        Value::Mapping(map) => {
            for (key, item) in map {
                let key = key.as_str().unwrap_or_default().to_ascii_lowercase();
                collect_sensitive(
                    item,
                    sensitive
                        || matches!(
                            key.as_str(),
                            "secret"
                                | "password"
                                | "passwd"
                                | "token"
                                | "authentication"
                                | "authorization"
                                | "private-key"
                                | "uuid"
                                | "psk"
                                | "auth"
                                | "url"
                        ),
                    secrets,
                );
            }
        }
        _ => (),
    }
}

fn load_secret_automaton(ctx: &Context) -> Result<Arc<SecretAutomaton>> {
    let paths: AutomatonCacheKey = [
        ctx.paths.clash_runtime.clone(),
        ctx.paths.clash_mixin.clone(),
        ctx.paths.clash_config.clone(),
        ctx.paths.clash_profiles.clone(),
    ];
    let stamps: [Option<FileStamp>; 4] = [
        file_stamp(&paths[0])?,
        file_stamp(&paths[1])?,
        file_stamp(&paths[2])?,
        file_stamp(&paths[3])?,
    ];
    if let Ok(guard) = automaton_cache().lock()
        && let Some((cached_stamps, automaton)) = guard.get(&paths)
        && *cached_stamps == stamps
    {
        return Ok(Arc::clone(automaton));
    }

    let mut secrets = Vec::new();
    for path in &paths {
        storage::check_path(path, true)?;
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        let mut bytes = Vec::new();
        file.take(8 * WINDOW + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > 8 * WINDOW {
            return Err(Error::new(
                "error.data",
                "configuration too large for safe log redaction",
            ));
        }
        let value: Value = serde_yaml::from_slice(&bytes).map_err(|_| {
            Error::new(
                "error.data",
                "invalid YAML; cannot safely redact Mihomo logs",
            )
        })?;
        collect_sensitive(&value, false, &mut secrets);
    }
    let encoded = secrets
        .iter()
        .map(|secret| {
            let encoded = serde_json::to_string(secret)?;
            Ok(encoded[1..encoded.len() - 1].to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    secrets.extend(encoded);
    secrets
        .sort_unstable_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    secrets.dedup();
    let automaton = Arc::new(SecretAutomaton::build(&secrets));

    if let Ok(mut guard) = automaton_cache().lock() {
        if guard.len() >= MAX_CACHED_AUTOMATA && !guard.contains_key(&paths) {
            guard.clear();
        }
        guard.insert(paths, (stamps, Arc::clone(&automaton)));
    }
    Ok(automaton)
}

/// Never fall back to raw logs if configuration cannot be read for redaction.
pub(crate) fn redact(ctx: &Context, text: &str) -> Result<String> {
    let automaton = load_secret_automaton(ctx)?;
    let mut output = automaton.replace_all(text, "[redacted]");
    output = fields_regex()
        .replace_all(&output, "${1}[redacted]")
        .into_owned();
    // Credentials and subscription query strings can also appear in old logs.
    output = urls_regex()
        .replace_all(
            &output,
            |caps: &regex::Captures<'_>| match url::Url::parse(&caps[0]) {
                Ok(mut url) => {
                    if !url.username().is_empty() || url.password().is_some() {
                        let _ = url.set_username("redacted");
                        let _ = url.set_password(None);
                    }
                    if url.query().is_some() {
                        url.set_query(Some("redacted"));
                    }
                    if url.fragment().is_some() {
                        url.set_fragment(Some("redacted"));
                    }
                    url.to_string()
                }
                Err(_) => "[redacted URL]".into(),
            },
        )
        .into_owned();
    Ok(output)
}

pub(super) fn startup_tail(ctx: &Context) -> Result<String> {
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut file = fs::File::open(&ctx.paths.clash_log)?;
    let start = file.metadata()?.len().saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024).read_to_end(&mut bytes)?;
    redact(ctx, &String::from_utf8_lossy(&bytes))
}

/// Read up to `max_lines` trailing lines from `clash_log`, strictly redacted through [`redact`].
/// Returns an empty list if the log file does not exist yet.
pub(crate) fn read_redacted_lines(ctx: &Context, max_lines: usize) -> Result<Vec<String>> {
    let max_lines = max_lines.clamp(1, 2000);
    ctx.check()?;
    let _ = rotate_if_oversized(ctx);
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut file = match fs::File::open(&ctx.paths.clash_log) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let length = file.metadata()?.len();
    let start = length.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut file).take(WINDOW).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, tail)| tail)
    } else {
        &text
    };
    let mut rows: Vec<&str> = text.lines().rev().take(max_lines).collect();
    rows.reverse();
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let redacted = redact(ctx, &rows.join("\n"))?;
    Ok(redacted.lines().map(str::to_owned).collect())
}

pub fn logs(ctx: &Context, lines: &str, follow: bool) -> Result<()> {
    let lines = lines
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=2000).contains(n))
        .ok_or_else(|| Error::new("error.input", "lines must be between 1 and 2000"))?;
    ctx.check()?;
    storage::check_path(&ctx.paths.clash_log, true)?;
    let mut file = fs::File::open(&ctx.paths.clash_log)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(WINDOW);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    (&mut file).take(WINDOW).read_to_end(&mut bytes)?;
    let mut offset = start + bytes.len() as u64;
    let mut pending = Vec::new();
    let end = if follow && !bytes.ends_with(b"\n") {
        let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        pending.extend_from_slice(&bytes[end..]);
        end
    } else {
        bytes.len()
    };
    let text = String::from_utf8_lossy(&bytes[..end]);
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, tail)| tail)
    } else {
        &text
    };
    let rows: Vec<_> = text.lines().rev().take(lines).collect();
    ctx.log(redact(
        ctx,
        &rows.into_iter().rev().collect::<Vec<_>>().join("\n"),
    )?);
    if !follow {
        return Ok(());
    }
    loop {
        ctx.check()?;
        thread::sleep(Duration::from_millis(200));
        storage::check_path(&ctx.paths.clash_log, true)?;
        let mut current = fs::File::open(&ctx.paths.clash_log)?;
        let metadata = current.metadata()?;
        #[cfg(unix)]
        let replaced = {
            use std::os::unix::fs::MetadataExt;
            let previous = file.metadata()?;
            previous.ino() != metadata.ino() || previous.dev() != metadata.dev()
        };
        #[cfg(not(unix))]
        let replaced = false;
        if replaced || metadata.len() < offset {
            offset = 0;
            pending.clear();
        }
        current.seek(SeekFrom::Start(offset))?;
        bytes.clear();
        (&mut current).take(WINDOW).read_to_end(&mut bytes)?;
        offset += bytes.len() as u64;
        pending.extend_from_slice(&bytes);
        if let Some(last) = pending.iter().rposition(|b| *b == b'\n') {
            let text = String::from_utf8_lossy(&pending[..=last]);
            ctx.log(redact(ctx, &text)?);
            pending.drain(..=last);
        }
        if pending.len() as u64 > WINDOW {
            return Err(Error::new("error.data", "Mihomo log line exceeds 1 MiB"));
        }
        #[cfg(unix)]
        {
            file = current;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::context;

    #[test]
    fn logs_redact_secrets_proxy_passwords_and_subscription_queries() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "secret: controller-secret\nauthentication: ['user:proxy-password']\n",
        )
        .unwrap();
        let output = redact(&ctx, "controller-secret proxy-password http://user:old-password@localhost:9090/ui https://example.invalid/sub?token=old-token socks5h://user:old-socks-password@localhost:7890").unwrap();
        for secret in [
            "controller-secret",
            "proxy-password",
            "old-password",
            "old-token",
            "old-socks-password",
        ] {
            assert!(!output.contains(secret));
        }
        assert!(output.contains("localhost:9090/ui"));
    }

    #[test]
    fn malformed_config_prevents_raw_log_output() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "secret: [unterminated-secret").unwrap();
        let error = redact(&ctx, "unterminated-secret").unwrap_err();
        assert!(!error.to_string().contains("unterminated-secret"));
    }

    #[test]
    fn private_log_rejects_invalid_line_count_and_is_private() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        assert!(logs(&ctx, "0", false).is_err());
        assert!(logs(&ctx, "2001", false).is_err());
        ensure_layout(&ctx).unwrap();
        open_private_log(&ctx).unwrap();
        assert_eq!(storage::mode(&ctx.paths.clash_log).unwrap(), 0o600);
    }

    #[test]
    fn old_bearer_tokens_and_json_escaped_secrets_are_redacted() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "secret: 'quoted\"token'\n").unwrap();
        let text = redact(&ctx, "Authorization: Bearer old-rotated-secret\n{\"name\":\"quoted\\\"token\"}\npassword=old-password\n").unwrap();
        assert!(!text.contains("old-rotated-secret"));
        assert!(!text.contains("old-password"));
        assert!(!text.contains("quoted"));
    }

    #[test]
    fn proxy_credentials_are_redacted_in_config_and_old_logs() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "proxies:\n  - name: node\n    type: vless\n    uuid: 00000000-0000-4000-8000-000000000001\n    auth: private-auth\n    psk: private-psk\n",
        )
        .unwrap();
        let config = fs::read_to_string(&ctx.paths.clash_mixin).unwrap();
        let text = redact(&ctx, &config).unwrap();
        for secret in [
            "00000000-0000-4000-8000-000000000001",
            "private-auth",
            "private-psk",
        ] {
            assert!(!text.contains(secret));
            assert_eq!(redact(&ctx, secret).unwrap(), "[redacted]");
        }
        let old = redact(&ctx, "uuid=old-uuid\nauth=old-auth\npsk=old-psk\n").unwrap();
        assert_eq!(old, "uuid=[redacted]\nauth=[redacted]\npsk=[redacted]\n");
    }

    #[test]
    fn short_credentials_do_not_expand_the_whole_log() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "proxies:\n  - password: r\n  - password: e\n  - password: d\n  - password: a\n  - password: c\n  - password: t\n",
        )
        .unwrap();
        let output = redact(&ctx, "re").unwrap();
        assert_eq!(output, "[redacted][redacted]");
        assert_eq!(redact(&ctx, &"r".repeat(1024)).unwrap().len(), 1024 * 10);
    }

    #[test]
    fn overlapping_and_json_escaped_credentials_are_replaced_once() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(
            &ctx.paths.clash_mixin,
            "proxies:\n  - password: 'quoted'\n  - password: 'quoted\"value'\n  - password: 'quoted\\'\n",
        )
        .unwrap();
        assert_eq!(
            redact(&ctx, "quoted\"value quoted\\\"value quoted\\\\ quoted").unwrap(),
            "[redacted] [redacted] [redacted] [redacted]"
        );
    }

    #[test]
    fn secret_automaton_handles_overlapping_suffix_matches_leftmost_longest() {
        let auto = SecretAutomaton::build(&[
            "012".into(),
            "1234".into(),
            "34".into(),
            "abc".into(),
            "a".into(),
        ]);
        // `012` wins leftmost, leaving `34` intact and matched next.
        assert_eq!(auto.replace_all("01234", "X"), "XX");
        // `abc` beats shorter `a` at index 0.
        assert_eq!(auto.replace_all("abcd", "X"), "Xd");
    }

    #[test]
    fn large_subscription_with_1200_nodes_redacts_linearly_without_regex_limit() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        let mut yaml_buf = String::from("proxies:\n");
        for i in 0..1200usize {
            yaml_buf.push_str(&format!(
                "  - name: node-{i}\n    type: vless\n    server: 192.0.2.{}\n    port: 443\n    uuid: 00000000-0000-4000-8000-{i:012}\n",
                (i % 250) + 1
            ));
        }
        fs::write(&ctx.paths.clash_config, &yaml_buf).unwrap();
        let sample = "connect ok uuid-leak 00000000-0000-4000-8000-000000000000 and 00000000-0000-4000-8000-000000001199 done";
        let redacted = redact(&ctx, sample).unwrap();
        assert!(!redacted.contains("000000000000"));
        assert!(!redacted.contains("000000001199"));
        assert_eq!(
            redacted,
            "connect ok uuid-leak [redacted] and [redacted] done"
        );
        // Second call hits fingerprint cache with identical output.
        assert_eq!(redact(&ctx, sample).unwrap(), redacted);
    }

    #[test]
    fn oversized_log_rotates_on_open_and_live_with_private_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_log, "a".repeat(2048)).unwrap();
        assert!(rotate_on_open_with_limit(&ctx, 1024).unwrap());
        let backup = rotated_log_path(&ctx);
        assert!(backup.is_file());
        assert_eq!(fs::metadata(&backup).unwrap().len(), 2048);
        assert!(!ctx.paths.clash_log.exists());
        #[cfg(unix)]
        assert_eq!(storage::mode(&backup).unwrap(), 0o600);

        // Live rotation preserves tail in `.1` and truncates active log in place.
        fs::write(
            &ctx.paths.clash_log,
            format!("{}TAIL-MARKER\n", "x".repeat(4000)),
        )
        .unwrap();
        assert!(rotate_if_oversized_with_limit(&ctx, 1024, 64).unwrap());
        assert_eq!(fs::metadata(&ctx.paths.clash_log).unwrap().len(), 0);
        let kept = fs::read_to_string(&backup).unwrap();
        assert!(kept.ends_with("TAIL-MARKER\n"));
        #[cfg(unix)]
        assert_eq!(storage::mode(&backup).unwrap(), 0o600);
    }

    #[test]
    fn follow_buffers_incomplete_secret_lines_and_cancels() {
        use std::{
            io::Write,
            sync::{atomic::Ordering, mpsc},
        };
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = context(temp.path());
        ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_mixin, "secret: split-private-token\n").unwrap();
        let mut file = open_private_log(&ctx).unwrap();
        write!(file, "ready\nsplit-private").unwrap();
        file.flush().unwrap();
        let (tx, rx) = mpsc::channel();
        ctx.events = Some(tx);
        let worker_ctx = ctx.clone();
        let worker = thread::spawn(move || logs(&worker_ctx, "100", true));
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(first, crate::runtime::Event::Log(ref text) if text == "ready"));
        writeln!(file, "-token").unwrap();
        file.flush().unwrap();
        let next = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        ctx.cancel.store(true, Ordering::SeqCst);
        let result = worker.join().unwrap();
        assert!(
            matches!(next, crate::runtime::Event::Log(ref text) if text.contains("[redacted]") && !text.contains("private"))
        );
        assert_eq!(result.unwrap_err().key, "error.cancelled");
    }
}

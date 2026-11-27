use crate::{
    config::Paths,
    error::{Error, Result},
    i18n::I18n,
};
use std::{
    fs::File,
    io::{BufRead, BufReader, IsTerminal, Read, Write},
    path::Path,
    process::{ChildStdout, Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread,
    time::Duration,
};

#[derive(Debug)]
pub enum Event {
    Log(String),
    Confirm {
        text: String,
        danger: bool,
        reply: Sender<bool>,
    },
    Step(Step),
    Finished(Result<()>),
}

/// One job of a batch starting or ending, reported through [`Context::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// Zero-based position of the job in the batch; shown as `index + 1`.
    pub index: usize,
    pub total: usize,
    pub id: &'static str,
    pub state: StepState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Started,
    Finished { outcome: Outcome, elapsed: Duration },
}

/// How a job ended: `error.cancelled` is Cancelled, any other error is Failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    pub i18n: I18n,
    pub cancel: Arc<AtomicBool>,
    pub events: Option<Sender<Event>>,
    pub yes: bool,
    pub interactive: bool,
    /// Optional normalized HTTPS prefix for GitHub acceleration proxy (e.g. `https://gh-proxy.com/`).
    pub github_proxy: Option<String>,
    pub(crate) environment: std::collections::BTreeMap<String, String>,
    pub(crate) batch_confirmed: bool,
    recovering: bool,
}

/// Parses an interactive confirmation reply: `y`/`yes` accepts, `n`/`no` or empty rejects,
/// and unrecognized input returns `None` so the caller can re-prompt.
pub(crate) fn parse_confirm_reply(input: &str) -> Option<bool> {
    match input.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Some(true),
        "n" | "no" | "" => Some(false),
        _ => None,
    }
}

fn read_confirm_reply(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    i18n: &I18n,
    check_cancel: impl Fn() -> Result<()>,
) -> Result<bool> {
    let mut prompt_key = "ui.confirm_prompt";
    loop {
        check_cancel()?;
        write!(writer, "{} ", i18n.text(prompt_key))?;
        writer.flush()?;
        let mut input = String::new();
        if reader.read_line(&mut input)? == 0 {
            return Ok(false);
        }
        if let Some(accepted) = parse_confirm_reply(&input) {
            return Ok(accepted);
        }
        prompt_key = "ui.confirm_invalid";
    }
}

impl Context {
    pub fn new(paths: Paths, i18n: I18n, yes: bool) -> Self {
        Self {
            paths,
            i18n,
            cancel: Arc::new(AtomicBool::new(false)),
            events: None,
            yes,
            interactive: std::io::stdin().is_terminal(),
            github_proxy: None,
            environment: std::collections::BTreeMap::new(),
            batch_confirmed: false,
            recovering: false,
        }
    }
    /// Rewrites canonical GitHub HTTPS URLs through the configured acceleration proxy and logs
    /// the active proxy prefix whenever rewriting occurs.
    pub fn github_fetch_url(&self, url: &str) -> String {
        let rewritten = crate::github_proxy::rewrite(url, self.github_proxy.as_deref());
        if rewritten != url
            && let Some(proxy) = &self.github_proxy
        {
            self.message("notice.github_proxy_using", proxy);
        }
        rewritten
    }
    /// Runs `fetch` against the proxied GitHub URL when acceleration is active.
    /// For `api.github.com` metadata endpoints, if the proxy returns an error (for example
    /// HTTP 403 from shared-IP rate limiting or a mirror that only serves download/raw paths),
    /// logs a notice and retries directly against `https://api.github.com/...` once.
    /// Cancellations (`error.cancelled`) are never retried.
    pub(crate) fn with_github_url<T>(
        &self,
        url: &str,
        mut fetch: impl FnMut(&str) -> Result<T>,
    ) -> Result<T> {
        let effective = self.github_fetch_url(url);
        match fetch(&effective) {
            Ok(value) => Ok(value),
            Err(error)
                if error.key != "error.cancelled"
                    && effective != url
                    && crate::github_proxy::is_github_api_url(url) =>
            {
                self.message("notice.github_proxy_api_fallback", url);
                fetch(url)
            }
            Err(error) => Err(error),
        }
    }
    pub fn recovery(&self) -> Self {
        let mut ctx = self.clone();
        ctx.recovering = true;
        ctx
    }
    /// Return a background-safe clone whose `log` / `message` calls discard into a closed event
    /// channel instead of printing to `stdout`. Prevents TUI monitor threads from corrupting the
    /// active alternate-screen terminal when reusing CLI operations.
    pub fn silent(&self) -> Self {
        let mut ctx = self.clone();
        let (tx, _rx) = mpsc::channel();
        ctx.events = Some(tx);
        ctx.interactive = false;
        ctx
    }
    pub fn check(&self) -> Result<()> {
        if !self.recovering && self.cancel.load(Ordering::SeqCst) {
            Err(Error::new("error.cancelled", ""))
        } else {
            Ok(())
        }
    }
    pub fn log(&self, text: impl AsRef<str>) {
        let text = sanitize(text.as_ref());
        if let Some(tx) = &self.events {
            let _ = tx.send(Event::Log(text));
        } else {
            println!("{text}");
        }
    }
    pub fn message(&self, key: &str, detail: impl ToString) {
        self.log(self.i18n.format(key, &[("detail", &detail.to_string())]));
    }
    /// Report a batch job starting or ending. An event receiver gets only the structured
    /// step; without one the CLI prints the progress and completion lines it always printed.
    pub fn step(&self, step: Step) {
        if let Some(tx) = &self.events {
            let _ = tx.send(Event::Step(step));
        } else if let Some(line) = self.step_line(&step) {
            self.log(line);
        }
    }
    /// The log line for `step`: `notice.progress` with `{i}/{n} {label}` when it starts and
    /// `notice.success` when it is done. Failures and cancellations have no line of their
    /// own; the batch logs their error text.
    pub fn step_line(&self, step: &Step) -> Option<String> {
        // The same key as `actions::Spec::label`.
        let label = self.i18n.text(&format!("action.{}", step.id));
        match step.state {
            StepState::Started => Some(self.i18n.format(
                "notice.progress",
                &[(
                    "detail",
                    &format!("{}/{} {label}", step.index + 1, step.total),
                )],
            )),
            StepState::Finished {
                outcome: Outcome::Done,
                ..
            } => Some(self.i18n.format("notice.success", &[("detail", label)])),
            StepState::Finished { .. } => None,
        }
    }
    /// Standard pre-execution confirmation (`y`/`yes` or `n`/`no`). Skipped when `--yes` is set
    /// or when the enclosing batch has already been confirmed by the user.
    pub fn confirm(&self, key: &str, detail: impl ToString) -> Result<()> {
        self.confirm_inner(key, detail, false, true)
    }
    /// High-risk pre-execution confirmation (`y`/`yes` or `n`/`no`, danger-styled in TUI).
    /// Skipped when `--yes` is set or when the enclosing batch has already been confirmed.
    pub fn confirm_danger(&self, key: &str, detail: impl ToString) -> Result<()> {
        self.confirm_inner(key, detail, true, true)
    }
    /// Mid- or post-operation checkpoint confirmation (`y`/`yes` or `n`/`no`) that cannot be
    /// subsumed by an upfront batch confirmation (for example, post-key SSH verification,
    /// benchmark-selected mirror review, or post-install host reboot).
    pub fn confirm_step(&self, key: &str, detail: impl ToString, danger: bool) -> Result<()> {
        self.confirm_inner(key, detail, danger, false)
    }
    fn confirm_inner(
        &self,
        key: &str,
        detail: impl ToString,
        danger: bool,
        allow_batch_skip: bool,
    ) -> Result<()> {
        self.check()?;
        let text = self.i18n.format(key, &[("detail", &detail.to_string())]);
        if self.yes {
            self.log(text);
            return Ok(());
        }
        if allow_batch_skip && self.batch_confirmed {
            return Ok(());
        }
        let accepted = if let Some(tx) = &self.events {
            let (reply, receive) = mpsc::channel();
            tx.send(Event::Confirm {
                text,
                danger,
                reply,
            })
            .map_err(|e| Error::new("error.io", e))?;
            loop {
                self.check()?;
                match receive.recv_timeout(Duration::from_millis(100)) {
                    Ok(value) => break value,
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(e) => return Err(Error::new("error.io", e)),
                }
            }
        } else if self.interactive {
            println!("{text}");
            let stdin = std::io::stdin();
            let mut reader = stdin.lock();
            let mut stdout = std::io::stdout();
            read_confirm_reply(&mut reader, &mut stdout, &self.i18n, || self.check())?
        } else {
            return Err(Error::new("error.confirm", "--yes"));
        };
        if accepted {
            Ok(())
        } else {
            Err(Error::new("error.cancelled", ""))
        }
    }
    pub fn root(&self) -> Result<()> {
        self.platform()?;
        #[cfg(unix)]
        if unsafe { libc::geteuid() } != 0 {
            return Err(Error::new("error.root", "sudo cqs"));
        }
        self.check()
    }
    pub fn platform(&self) -> Result<()> {
        if !cfg!(target_os = "linux") {
            return Err(Error::new("error.platform", std::env::consts::OS));
        }
        let os = std::fs::read_to_string("/etc/os-release")?;
        let id = os
            .lines()
            .find_map(|l| l.strip_prefix("ID="))
            .unwrap_or("")
            .trim_matches('"');
        if !matches!(id, "debian" | "ubuntu") {
            return Err(Error::new("error.platform", id));
        }
        Ok(())
    }
    pub fn require(&self, names: &[&str]) -> Result<()> {
        let search = self.search_path();
        for name in names {
            if !available_on(name, &search) {
                return Err(Error::new("error.dependency", name));
            }
        }
        Ok(())
    }
    fn search_path(&self) -> std::ffi::OsString {
        system_search_path(
            self.environment
                .get("PATH")
                .map(std::ffi::OsString::from)
                .or_else(|| std::env::var_os("PATH"))
                .unwrap_or_default(),
        )
    }
    pub fn output(&self, program: &str, args: &[&str]) -> Result<String> {
        let out = self.command(program, args, false)?;
        self.success(program, args, &out)?;
        String::from_utf8(out.stdout)
            .map(|s| s.trim_end().to_owned())
            .map_err(|e| Error::new("error.data", e))
    }
    pub fn run(&self, program: &str, args: &[&str]) -> Result<()> {
        let out = self.command(program, args, true)?;
        self.success(program, args, &out)
    }
    fn success(&self, program: &str, args: &[&str], out: &Output) -> Result<()> {
        if !out.status.success() {
            return Err(Error::new(
                "error.command",
                format!(
                    "{program} {}: {}\n{}",
                    args.join(" "),
                    out.status,
                    String::from_utf8_lossy(&out.stderr)
                ),
            ));
        }
        Ok(())
    }
    pub fn active(&self, service: &str) -> Result<bool> {
        let out = self.command("systemctl", &["is-active", "--quiet", service], false)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(3 | 4) => Ok(false),
            _ => {
                self.success("systemctl", &["is-active", service], &out)?;
                Ok(false)
            }
        }
    }
    pub fn command(&self, program: &str, args: &[&str], stream: bool) -> Result<Output> {
        self.command_with_stdout(program, args, stream, move |stdout, ctx| {
            read_output(stdout, ctx, stream)
        })
    }
    fn command_with_stdout(
        &self,
        program: &str,
        args: &[&str],
        stream: bool,
        read_stdout: impl FnOnce(ChildStdout, &Self) -> Result<Vec<u8>> + Send + 'static,
    ) -> Result<Output> {
        self.check()?;
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(&self.environment)
            .env("PATH", self.search_path())
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Captured commands must not read the TUI through /dev/tty. setsid
            // also gives cancellation a private process group. The post-fork
            // hook calls only an async-signal-safe syscall before exec.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = retry_executable_busy(self, || command.spawn())
            .map_err(|e| Error::new("error.command", format!("{program}: {e}")))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let ctx1 = self.clone();
        let ctx2 = self.clone();
        let read_failed = Arc::new(AtomicBool::new(false));
        let stdout_failed = read_failed.clone();
        let a = thread::spawn(move || {
            let result = read_stdout(stdout, &ctx1);
            stdout_failed.store(result.is_err(), Ordering::SeqCst);
            result
        });
        let b = thread::spawn(move || read_output(stderr, &ctx2, stream));
        let mut interrupted = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if (!self.recovering && self.cancel.load(Ordering::SeqCst)
                || read_failed.load(Ordering::SeqCst))
                && !interrupted
            {
                interrupted = true;
                // SIGINT lets package managers finish their own cleanup. Never escalate to SIGKILL.
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGINT);
                }
                #[cfg(not(unix))]
                child.kill()?;
            }
            thread::sleep(Duration::from_millis(50));
        };
        let stdout = a.join();
        let stderr = b.join();
        let stdout = stdout.map_err(|_| Error::new("error.io", "stdout reader panicked"))??;
        let stderr = stderr.map_err(|_| Error::new("error.io", "stderr reader panicked"))??;
        if interrupted {
            return Err(Error::new("error.cancelled", program));
        }
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
    /// Run a third-party interactive tool only after the TUI has handed back the terminal.
    pub fn passthrough(&self, program: &str, args: &[&str]) -> Result<()> {
        self.check()?;
        if !self.interactive {
            return Err(Error::new("error.tty", program));
        }
        if self.events.is_some() {
            return Err(Error::new("error.tty", "terminal ownership"));
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(&self.environment)
            .env("PATH", self.search_path());
        let status = retry_executable_busy(self, || command.status())?;
        if !status.success() {
            return Err(Error::new("error.command", format!("{program}: {status}")));
        }
        self.check()
    }
    pub fn download(&self, url: &str, target: &Path, seconds: u32) -> Result<()> {
        self.download_inner(url, target, None, seconds, None, true)
    }
    pub(crate) fn download_exact(
        &self,
        url: &str,
        target: &Path,
        seconds: u32,
        size: u64,
    ) -> Result<()> {
        self.check()?;
        validate_https_url(url)?;
        let effective = self.github_fetch_url(url);
        validate_https_url(&effective)?;
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let download = tempfile::NamedTempFile::new_in(parent)?;
        let output = download.reopen()?;
        let seconds = seconds.to_string();
        let max_size = size.to_string();
        let args = [
            "-q",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-fsSL",
            "--connect-timeout",
            "15",
            "--max-time",
            &seconds,
            "--max-filesize",
            &max_size,
            "--output",
            "-",
            &effective,
        ];
        let result = self.command_with_stdout("curl", &args, true, move |stdout, _| {
            copy_bounded(stdout, output, size)?;
            Ok(Vec::new())
        })?;
        self.success("curl", &args, &result)?;
        self.check()?;
        let actual_size = download.as_file().metadata()?.len();
        if actual_size != size {
            return Err(Error::new(
                "error.release",
                format!("asset size: expected {size}, received {actual_size}"),
            ));
        }
        download
            .persist(target)
            .map_err(|error| Error::from(error.error))?;
        Ok(())
    }
    /// Like [`Self::download`], identifying the client to servers that pick the response
    /// format by User-Agent. `None` keeps curl's own identity. Never forwards private
    /// subscription URLs through a third-party GitHub proxy.
    pub fn download_as(
        &self,
        url: &str,
        target: &Path,
        seconds: u32,
        agent: Option<&str>,
    ) -> Result<()> {
        self.download_inner(url, target, None, seconds, agent, false)
    }
    /// Like [`Self::download_as`], additionally writing HTTP response headers to `headers_target`
    /// so callers can inspect safe metadata such as `subscription-userinfo`.
    pub(crate) fn download_as_with_headers(
        &self,
        url: &str,
        target: &Path,
        headers_target: &Path,
        seconds: u32,
        agent: Option<&str>,
    ) -> Result<()> {
        self.download_inner(url, target, Some(headers_target), seconds, agent, false)
    }
    fn download_inner(
        &self,
        url: &str,
        target: &Path,
        headers_target: Option<&Path>,
        seconds: u32,
        agent: Option<&str>,
        accelerate_github: bool,
    ) -> Result<()> {
        validate_https_url(url)?;
        let fetch = |effective: &str| -> Result<()> {
            validate_https_url(effective)?;
            let seconds = seconds.to_string();
            let mut args = vec![
                "-q",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "-fsSL",
                "--connect-timeout",
                "15",
                "--max-time",
                &seconds,
            ];
            if let Some(agent) = agent {
                args.extend(["--user-agent", agent]);
            }
            if let Some(headers) = headers_target {
                args.extend(["--dump-header", path(headers)?]);
            }
            args.extend(["--output", path(target)?, effective]);
            self.run("curl", &args)?;
            if std::fs::metadata(target)?.len() == 0 {
                return Err(Error::new("error.data", url));
            }
            Ok(())
        };
        if accelerate_github {
            self.with_github_url(url, fetch)
        } else {
            fetch(url)
        }
    }
}

fn validate_https_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| Error::new("error.url", e))?;
    if parsed.scheme() != "https" || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::new("error.url", url));
    }
    Ok(())
}

/// Retry transient `ETXTBSY` (`ExecutableFileBusy`) errors that can occur on Linux when another
/// thread forks while this thread has just finished writing a newly unpacked executable.
pub(crate) fn retry_executable_busy<T>(
    ctx: &Context,
    mut op: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    const BACKOFF_MS: [u64; 6] = [5, 10, 20, 40, 80, 160];
    let mut attempt = 0usize;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err)
                if err.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && attempt < BACKOFF_MS.len()
                    && (ctx.recovering || !ctx.cancel.load(Ordering::SeqCst)) =>
            {
                thread::sleep(Duration::from_millis(BACKOFF_MS[attempt]));
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

fn copy_bounded(mut input: impl Read, mut output: impl Write, limit: u64) -> Result<()> {
    std::io::copy(&mut input.by_ref().take(limit), &mut output)?;
    if std::io::copy(&mut input.take(1), &mut std::io::sink())? != 0 {
        return Err(Error::new(
            "error.release",
            format!("asset exceeds expected size of {limit} bytes"),
        ));
    }
    Ok(())
}

fn read_output(input: impl Read, ctx: &Context, stream: bool) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(input);
    let mut bytes = Vec::new();
    let mut line = Vec::new();
    let mut overflow = false;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if stream {
            ctx.log(String::from_utf8_lossy(&line).trim_end());
        }
        if bytes.len() + line.len() <= 64 * 1024 * 1024 {
            bytes.extend_from_slice(&line);
        } else {
            overflow = true;
        }
    }
    if overflow {
        return Err(Error::new("error.data", "command output exceeds 64 MiB"));
    }
    Ok(bytes)
}

pub fn available(program: &str) -> bool {
    available_on(
        program,
        &system_search_path(std::env::var_os("PATH").unwrap_or_default()),
    )
}
fn system_search_path(paths: std::ffi::OsString) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        let mut directories: Vec<_> = std::env::split_paths(&paths).collect();
        // Non-root Debian login shells may omit sbin, including read-only tools such as sysctl.
        for directory in ["/usr/local/sbin", "/usr/sbin", "/sbin"] {
            let directory = std::path::PathBuf::from(directory);
            if !directories.contains(&directory) {
                directories.push(directory);
            }
        }
        std::env::join_paths(directories).expect("PATH components are already split")
    }
    #[cfg(not(unix))]
    {
        paths
    }
}
fn available_on(program: &str, paths: &std::ffi::OsStr) -> bool {
    std::env::split_paths(paths).any(|dir| {
        let p = dir.join(program);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            p.is_file() || p.with_extension("exe").is_file()
        }
    })
}
pub fn path(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| Error::new("error.path", path.display()))
}
pub fn sanitize(text: &str) -> String {
    enum State {
        Text,
        Escape,
        Intermediate,
        Csi,
        String,
        StringEscape,
    }
    let mut state = State::Text;
    let mut clean = String::with_capacity(text.len());
    for c in text.chars() {
        state = match state {
            State::Text => match c {
                '\x1b' => State::Escape,
                '\u{9b}' => State::Csi,
                '\u{90}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => State::String,
                _ => {
                    if !c.is_control() || matches!(c, '\n' | '\t') {
                        clean.push(c);
                    }
                    State::Text
                }
            },
            State::Escape => match c {
                '[' => State::Csi,
                ']' | 'P' | 'X' | '^' | '_' => State::String,
                '\x1b' => State::Escape,
                '\x20'..='\x2f' => State::Intermediate,
                _ => State::Text,
            },
            State::Intermediate => {
                if ('\x30'..='\x7e').contains(&c) {
                    State::Text
                } else {
                    State::Intermediate
                }
            }
            State::Csi => {
                if ('\x40'..='\x7e').contains(&c) {
                    State::Text
                } else {
                    State::Csi
                }
            }
            State::String => match c {
                '\x07' | '\u{9c}' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
            State::StringEscape => match c {
                '\\' | '\x07' | '\u{9c}' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
        };
    }
    clean
}
pub fn sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_copy_never_writes_or_reads_past_its_budget() {
        for limit in [0, 1, 16, 65536, 65537] {
            let bytes = vec![0x5a; limit + 512];
            let mut input = std::io::Cursor::new(&bytes);
            let mut output = Vec::new();
            let error = copy_bounded(&mut input, &mut output, limit as u64).unwrap_err();
            assert_eq!(error.key, "error.release");
            assert_eq!(output, bytes[..limit]);
            assert_eq!(input.position(), limit as u64 + 1);
        }
    }

    #[test]
    fn bounded_copy_preserves_exact_and_short_payloads() {
        for length in [0, 15, 16] {
            let bytes = (0..length).map(|index| index * 17).collect::<Vec<u8>>();
            let mut output = Vec::new();
            copy_bounded(bytes.as_slice(), &mut output, 16).unwrap();
            assert_eq!(output, bytes);
        }
    }

    #[test]
    fn bounded_copy_propagates_io_errors() {
        let error = copy_bounded(b"bytes".as_slice(), [].as_mut_slice(), 16).unwrap_err();
        assert_eq!(error.key, "error.io");
        struct FailedReader;
        impl Read for FailedReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("fixture read failure"))
            }
        }
        let error = copy_bounded(FailedReader, std::io::sink(), 16).unwrap_err();
        assert_eq!(error.key, "error.io");
        assert!(error.detail.contains("fixture read failure"));
    }

    #[test]
    fn cancelled_download_preserves_the_target_without_creating_files() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let target = temp.path().join("asset");
        std::fs::write(&target, b"previous asset").unwrap();
        let files_before = std::fs::read_dir(temp.path()).unwrap().count();
        ctx.cancel.store(true, Ordering::SeqCst);
        let error = ctx
            .download_exact("https://example.invalid/asset", &target, 30, 16)
            .unwrap_err();
        assert_eq!(error.key, "error.cancelled");
        assert_eq!(std::fs::read(target).unwrap(), b"previous asset");
        assert_eq!(
            std::fs::read_dir(temp.path()).unwrap().count(),
            files_before
        );
    }

    #[cfg(unix)]
    #[test]
    fn bounded_copy_failure_stops_and_reaps_the_process() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        let pid_path = temp.path().join("pid");
        let started = std::time::Instant::now();
        let error = ctx
            .command_with_stdout(
                "sh",
                &[
                    "-c",
                    "echo $$ > \"$1\"; printf 'too much data'; exec sleep 30",
                    "fixture",
                    path(&pid_path).unwrap(),
                ],
                false,
                |stdout, _| {
                    copy_bounded(stdout, std::io::sink(), 1)?;
                    Ok(Vec::new())
                },
            )
            .unwrap_err();
        assert_eq!(error.key, "error.release");
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid: i32 = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    fn step(index: usize, state: StepState) -> Step {
        Step {
            index,
            total: 3,
            id: "tcp.tune",
            state,
        }
    }

    #[test]
    fn step_lines_are_the_lines_batch_printed_before_steps_existed() {
        let elapsed = Duration::from_millis(1500);
        for (locale, started, done) in [
            (
                "en-US",
                "Running 2/3 Tune TCP buffers",
                "Completed: Tune TCP buffers",
            ),
            (
                "zh-CN",
                "正在执行 2/3 TCP 缓冲区调优",
                "已完成：TCP 缓冲区调优",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let mut ctx = crate::tests::context(temp.path());
            ctx.i18n = I18n::load(Some(locale), None).unwrap();
            assert_eq!(
                ctx.step_line(&step(1, StepState::Started)).as_deref(),
                Some(started)
            );
            let finished = |outcome| step(1, StepState::Finished { outcome, elapsed });
            assert_eq!(
                ctx.step_line(&finished(Outcome::Done)).as_deref(),
                Some(done)
            );
            // The batch logs the error text of failed and cancelled jobs itself.
            for outcome in [Outcome::Failed, Outcome::Cancelled] {
                assert_eq!(ctx.step_line(&finished(outcome)), None);
            }
        }
    }

    #[test]
    fn an_event_receiver_gets_only_the_structured_step() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        let (tx, rx) = mpsc::channel();
        ctx.events = Some(tx);
        let steps = [
            step(0, StepState::Started),
            step(
                0,
                StepState::Finished {
                    outcome: Outcome::Done,
                    elapsed: Duration::from_secs(2),
                },
            ),
        ];
        for sent in steps {
            ctx.step(sent);
            assert!(matches!(rx.try_recv(), Ok(Event::Step(received)) if received == sent));
            // No log line alongside the step, not even for a completed one.
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn with_github_url_falls_back_directly_only_for_github_api_non_cancelled_errors() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        ctx.github_proxy = Some(crate::github_proxy::DEFAULT_URL.into());

        // 1. Proxied api.github.com 403 error retries directly and succeeds.
        let mut seen = Vec::new();
        let api = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
        let result = ctx
            .with_github_url(api, |url| {
                seen.push(url.to_owned());
                if url.starts_with(crate::github_proxy::DEFAULT_URL) {
                    Err(Error::new("error.command", "curl: (22) 403"))
                } else {
                    Ok("direct-ok")
                }
            })
            .unwrap();
        assert_eq!(result, "direct-ok");
        assert_eq!(
            seen,
            vec![
                format!("{}{api}", crate::github_proxy::DEFAULT_URL),
                api.into()
            ]
        );

        // 2. Non-API GitHub download failures stay on the proxy and do not silently bypass it.
        let mut seen = Vec::new();
        let asset = "https://github.com/MetaCubeX/mihomo/releases/download/v1.0.0/mihomo.gz";
        let err = ctx
            .with_github_url(asset, |url| {
                seen.push(url.to_owned());
                Err::<(), _>(Error::new("error.command", "curl: (22) 403"))
            })
            .unwrap_err();
        assert_eq!(err.key, "error.command");
        assert_eq!(
            seen,
            vec![format!("{}{asset}", crate::github_proxy::DEFAULT_URL)]
        );

        // 3. Cancellation never triggers a retry even for api.github.com.
        let mut seen = Vec::new();
        let err = ctx
            .with_github_url(api, |url| {
                seen.push(url.to_owned());
                Err::<(), _>(Error::new("error.cancelled", "curl"))
            })
            .unwrap_err();
        assert_eq!(err.key, "error.cancelled");
        assert_eq!(
            seen,
            vec![format!("{}{api}", crate::github_proxy::DEFAULT_URL)]
        );
    }

    #[test]
    fn retry_executable_busy_recovers_transient_etxtbsy_and_preserves_other_errors() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());

        // Transient ExecutableFileBusy succeeds after brief retries.
        let mut attempts = 0usize;
        let value = retry_executable_busy(&ctx, || {
            attempts += 1;
            if attempts < 3 {
                Err(std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy))
            } else {
                Ok("spawned")
            }
        })
        .unwrap();
        assert_eq!(value, "spawned");
        assert_eq!(attempts, 3);

        // Non-busy errors fail immediately without retrying.
        let mut attempts = 0usize;
        let err = retry_executable_busy(&ctx, || {
            attempts += 1;
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(attempts, 1);

        // Cancelled context does not retry ExecutableFileBusy.
        ctx.cancel.store(true, Ordering::SeqCst);
        let mut attempts = 0usize;
        let err = retry_executable_busy(&ctx, || {
            attempts += 1;
            Err::<(), _>(std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy))
        })
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ExecutableFileBusy);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn confirm_reply_parser_accepts_yes_no_and_reprompts_on_invalid_input() {
        for accepted in ["y", "Y", "yes", "YES", "Yes", "  y \r\n", "\tyes\n"] {
            assert_eq!(parse_confirm_reply(accepted), Some(true), "{accepted:?}");
        }
        for rejected in ["n", "N", "no", "NO", "No", "", "   \r\n"] {
            assert_eq!(parse_confirm_reply(rejected), Some(false), "{rejected:?}");
        }
        for invalid in ["ye", "ok", "RUN", "REMOVE", "1"] {
            assert_eq!(parse_confirm_reply(invalid), None, "{invalid:?}");
        }

        let i18n = I18n::load(Some("en-US"), None).unwrap();
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"maybe\ny\n".as_slice());
        assert!(read_confirm_reply(&mut input, &mut out, &i18n, || Ok(())).unwrap());
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains(i18n.text("ui.confirm_prompt")));
        assert!(rendered.contains(i18n.text("ui.confirm_invalid")));

        // EOF after invalid input safely cancels without looping.
        let mut out = Vec::new();
        let mut input = std::io::Cursor::new(b"nope\n".as_slice());
        assert!(!read_confirm_reply(&mut input, &mut out, &i18n, || Ok(())).unwrap());
    }

    #[test]
    fn silent_context_attaches_discard_channel_and_disables_interactive_prompts() {
        let temp = tempfile::tempdir().unwrap();
        let mut base = crate::tests::context(temp.path());
        base.interactive = true;
        base.yes = false;
        let bg = base.silent();
        assert!(bg.events.is_some());
        assert!(!bg.interactive);
        // Logging and messaging discard cleanly instead of writing to stdout.
        bg.log("background log line");
        bg.message("notice.clash_subscription", "updated demo");
        // Interactive confirmation fails fast instead of blocking on stdin.
        assert!(bg.confirm("notice.clash_subscription", "demo").is_err());
    }
}

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
        token: String,
        reply: Sender<bool>,
    },
    Finished(Result<()>),
}

#[derive(Clone)]
pub struct Context {
    pub paths: Paths,
    pub i18n: I18n,
    pub cancel: Arc<AtomicBool>,
    pub events: Option<Sender<Event>>,
    pub yes: bool,
    pub interactive: bool,
    pub(crate) environment: std::collections::BTreeMap<String, String>,
    recovering: bool,
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
            environment: std::collections::BTreeMap::new(),
            recovering: false,
        }
    }
    pub fn recovery(&self) -> Self {
        let mut ctx = self.clone();
        ctx.recovering = true;
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
    pub fn confirm(&self, key: &str, detail: impl ToString, token: &str) -> Result<()> {
        self.check()?;
        let text = self.i18n.format(key, &[("detail", &detail.to_string())]);
        if self.yes {
            self.log(text);
            return Ok(());
        }
        let accepted = if let Some(tx) = &self.events {
            let (reply, receive) = mpsc::channel();
            tx.send(Event::Confirm {
                text,
                token: token.into(),
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
            print!(
                "{} ",
                self.i18n.format("ui.type_confirm", &[("token", token)])
            );
            std::io::stdout().flush()?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)? > 0 && input.trim() == token
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
        let mut child = command
            .spawn()
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
        let status = Command::new(program)
            .args(args)
            .envs(&self.environment)
            .env("PATH", self.search_path())
            .status()?;
        if !status.success() {
            return Err(Error::new("error.command", format!("{program}: {status}")));
        }
        self.check()
    }
    pub fn download(&self, url: &str, target: &Path, seconds: u32) -> Result<()> {
        self.download_as(url, target, seconds, None)
    }
    pub(crate) fn download_exact(
        &self,
        url: &str,
        target: &Path,
        seconds: u32,
        size: u64,
    ) -> Result<()> {
        self.check()?;
        let parsed = url::Url::parse(url).map_err(|error| Error::new("error.url", error))?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(Error::new("error.url", url));
        }
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
            "-fSL",
            "--connect-timeout",
            "15",
            "--max-time",
            &seconds,
            "--max-filesize",
            &max_size,
            "--output",
            "-",
            url,
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
    /// format by User-Agent. `None` keeps curl's own identity.
    pub fn download_as(
        &self,
        url: &str,
        target: &Path,
        seconds: u32,
        agent: Option<&str>,
    ) -> Result<()> {
        let parsed = url::Url::parse(url).map_err(|e| Error::new("error.url", e))?;
        if parsed.scheme() != "https"
            || !parsed.username().is_empty()
            || parsed.password().is_some()
        {
            return Err(Error::new("error.url", url));
        }
        let seconds = seconds.to_string();
        let mut args = vec![
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "-fSL",
            "--connect-timeout",
            "15",
            "--max-time",
            &seconds,
        ];
        if let Some(agent) = agent {
            args.extend(["--user-agent", agent]);
        }
        args.extend(["--output", path(target)?, url]);
        self.run("curl", &args)?;
        if std::fs::metadata(target)?.len() == 0 {
            return Err(Error::new("error.data", url));
        }
        Ok(())
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
}

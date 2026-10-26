use crate::{
    error::{Error, Result},
    ops,
    runtime::Context,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub type Parameters = BTreeMap<String, String>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    PublicIp,
    Update,
    Clean,
    Timezone,
    SwapCreate,
    SwapAuto,
    SwapRemove,
    Fail2ban,
    SshPort,
    SshKey,
    BbrStatus,
    BbrEnable,
    BbrRestore,
    TcpTune,
    TcpRestore,
    KernelList,
    KernelInstall,
    KernelRemove,
    ToolsInstall,
    ToolInstall(&'static str),
    AiInstall(ops::ai::Tool),
    AiStatus,
    TerminalInstall,
    TerminalApply,
    TerminalStatus,
    TerminalRestore,
    DockerInstall,
    DockerStatus,
    DockerClean,
    DockerMirror,
    DockerIpv6,
    DockerUninstall,
    Speedtest,
    IpTest,
    RouteTest,
    Performance,
    Connectivity,
    Io,
    Install,
    Uninstall,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: &'static str,
    pub default: &'static str,
    pub choices: &'static [&'static str],
    pub required: bool,
}
impl Field {
    pub fn label(&self) -> String {
        format!("field.{}", self.name)
    }
}
fn field(
    name: &'static str,
    default: &'static str,
    choices: &'static [&'static str],
    required: bool,
) -> Field {
    Field {
        name,
        default,
        choices,
        required,
    }
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub id: &'static str,
    pub group: &'static str,
    pub kind: Kind,
    pub root: bool,
    pub changes: bool,
    pub external: bool,
    pub fields: Vec<Field>,
}
impl Spec {
    pub fn label(&self) -> String {
        format!("action.{}", self.id)
    }
    pub fn description(&self) -> String {
        format!("description.{}", self.id)
    }
    pub fn defaults(&self) -> Parameters {
        self.fields
            .iter()
            .map(|f| (f.name.into(), f.default.into()))
            .collect()
    }
    pub fn parameters(&self, supplied: &Parameters) -> Result<Parameters> {
        let mut parameters = self.defaults();
        for (key, value) in supplied {
            let field = self
                .fields
                .iter()
                .find(|f| f.name == key)
                .ok_or_else(|| Error::new("error.input", format!("{}: {key}", self.id)))?;
            if value.chars().any(char::is_control)
                || (!field.choices.is_empty() && !field.choices.contains(&value.as_str()))
            {
                return Err(Error::new("error.input", format!("{key}={value}")));
            }
            parameters.insert(key.clone(), value.clone());
        }
        for field in &self.fields {
            if field.required && parameters[field.name].is_empty() {
                return Err(Error::new("error.required", field.name));
            }
        }
        self.validate_values(&parameters)?;
        Ok(parameters)
    }
    fn validate_values(&self, p: &Parameters) -> Result<()> {
        let valid = match self.kind {
            Kind::SshPort => {
                let port = &p["port"];
                port.len() <= 5
                    && port.bytes().all(|c| c.is_ascii_digit())
                    && port.parse::<u16>().is_ok_and(|n| n > 0)
            }
            Kind::SwapCreate => {
                p["size_mb"].bytes().all(|c| c.is_ascii_digit())
                    && p["size_mb"]
                        .parse::<u64>()
                        .is_ok_and(|n| (1..=1_048_576).contains(&n))
            }
            Kind::TcpTune => {
                ops::memory::tcp_buffer(&p["bandwidth"], &p["rtt"], 65536)?;
                true
            }
            Kind::KernelInstall => {
                p["version"].is_empty()
                    || regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+$")
                        .unwrap()
                        .is_match(&p["version"])
            }
            Kind::KernelRemove => {
                regex::Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+-joeyblog-bbrv3(-max)?$")
                    .unwrap()
                    .is_match(&p["release"])
            }
            Kind::Timezone => {
                !p["timezone"].starts_with('/')
                    && !p["timezone"]
                        .split('/')
                        .any(|s| matches!(s, "" | "." | ".."))
            }
            Kind::DockerMirror => {
                ops::docker::candidate("{}", ops::docker::Change::Mirror(&p["mirror"]), "")?;
                true
            }
            Kind::SshKey | Kind::Io => {
                let path = Path::new(
                    &p[if self.kind == Kind::SshKey {
                        "public_key"
                    } else {
                        "directory"
                    }],
                );
                path.is_absolute()
                    && !path
                        .components()
                        .any(|c| c == std::path::Component::ParentDir)
            }
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(Error::new("error.input", self.id))
        }
    }
}

pub const GROUPS: &[&str] = &[
    "system", "network", "kernel", "ssh", "docker", "tools", "ai", "terminal", "tests", "app",
];
pub const OPTIMIZATION: &[&str] = &[
    "system.update",
    "system.clean",
    "tcp.tune",
    "bbr.enable",
    "system.timezone",
    "swap.auto",
    "system.fail2ban",
    "tools.install",
    "docker.install",
    "ssh.port",
    "ssh.key",
];

pub fn specs() -> Vec<Spec> {
    use Kind::*;
    let mut items = Vec::new();
    let mut add = |id, group, kind, root, changes, external, fields| {
        items.push(Spec {
            id,
            group,
            kind,
            root,
            changes,
            external,
            fields,
        })
    };
    add("system.info", "system", Info, false, false, false, vec![]);
    add(
        "system.public_ip",
        "system",
        PublicIp,
        false,
        false,
        false,
        vec![],
    );
    add("system.update", "system", Update, true, true, false, vec![]);
    add(
        "system.clean",
        "system",
        Clean,
        true,
        true,
        false,
        vec![field("scope", "standard", &["standard", "full"], true)],
    );
    add(
        "system.timezone",
        "system",
        Timezone,
        true,
        true,
        false,
        vec![field("timezone", "Asia/Shanghai", &[], true)],
    );
    add(
        "swap.create",
        "system",
        SwapCreate,
        true,
        true,
        false,
        vec![field("size_mb", "2048", &[], true)],
    );
    add("swap.auto", "system", SwapAuto, true, true, false, vec![]);
    add(
        "swap.remove",
        "system",
        SwapRemove,
        true,
        true,
        false,
        vec![],
    );
    add(
        "system.fail2ban",
        "system",
        Fail2ban,
        true,
        true,
        false,
        vec![],
    );
    add(
        "ssh.port",
        "ssh",
        SshPort,
        true,
        true,
        false,
        vec![field("port", "55520", &[], true)],
    );
    add(
        "ssh.key",
        "ssh",
        SshKey,
        true,
        true,
        false,
        vec![field("public_key", "", &[], true)],
    );
    add(
        "bbr.status",
        "network",
        BbrStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "bbr.enable",
        "network",
        BbrEnable,
        true,
        true,
        false,
        vec![field(
            "qdisc",
            "fq",
            &["fq", "fq_codel", "fq_pie", "cake"],
            true,
        )],
    );
    add(
        "bbr.restore",
        "network",
        BbrRestore,
        true,
        true,
        false,
        vec![],
    );
    add(
        "tcp.tune",
        "network",
        TcpTune,
        true,
        true,
        false,
        vec![
            field("bandwidth", "1000", &[], true),
            field("rtt", "50", &[], true),
        ],
    );
    add(
        "tcp.restore",
        "network",
        TcpRestore,
        true,
        true,
        false,
        vec![],
    );
    add(
        "kernel.list",
        "kernel",
        KernelList,
        false,
        false,
        false,
        vec![],
    );
    add(
        "kernel.install",
        "kernel",
        KernelInstall,
        true,
        true,
        false,
        vec![
            field("profile", "standard", &["standard", "max"], true),
            field("version", "", &[], false),
        ],
    );
    add(
        "kernel.remove",
        "kernel",
        KernelRemove,
        true,
        true,
        false,
        vec![field("release", "", &[], true)],
    );
    for (id, package) in [
        ("tools.curl", "curl"),
        ("tools.wget", "wget"),
        ("tools.nano", "nano"),
        ("tools.unzip", "unzip"),
        ("tools.tar", "tar"),
        ("tools.tmux", "tmux"),
        ("tools.iftop", "iftop"),
        ("tools.btop", "btop"),
        ("tools.gdu", "gdu"),
        ("tools.fzf", "fzf"),
    ] {
        add(id, "tools", ToolInstall(package), true, true, false, vec![]);
    }
    add(
        "tools.install",
        "tools",
        ToolsInstall,
        true,
        true,
        false,
        vec![field(
            "package",
            "all",
            &[
                "all", "curl", "wget", "nano", "unzip", "tar", "tmux", "iftop", "btop", "gdu",
                "fzf",
            ],
            true,
        )],
    );
    for (id, tool) in [
        ("ai.pi.install", ops::ai::Tool::Pi),
        ("ai.claude.install", ops::ai::Tool::Claude),
        ("ai.codex.install", ops::ai::Tool::Codex),
    ] {
        add(id, "ai", AiInstall(tool), false, true, false, vec![]);
    }
    add("ai.status", "ai", AiStatus, false, false, false, vec![]);
    add(
        "terminal.install",
        "terminal",
        TerminalInstall,
        true,
        true,
        false,
        vec![],
    );
    add(
        "terminal.apply",
        "terminal",
        TerminalApply,
        false,
        true,
        false,
        vec![
            field("shell", "bash", &["bash", "zsh", "both"], true),
            field("style", "full", &["compact", "full"], true),
            field("color", "cyan", &["cyan", "green", "magenta"], true),
            field("time", "off", &["on", "off"], true),
            field("git", "on", &["on", "off"], true),
        ],
    );
    add(
        "terminal.status",
        "terminal",
        TerminalStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "terminal.restore",
        "terminal",
        TerminalRestore,
        false,
        true,
        false,
        vec![],
    );
    add(
        "docker.install",
        "docker",
        DockerInstall,
        true,
        true,
        true,
        vec![field("source", "official", &["official", "china"], true)],
    );
    add(
        "docker.status",
        "docker",
        DockerStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "docker.clean",
        "docker",
        DockerClean,
        true,
        true,
        false,
        vec![],
    );
    add(
        "docker.mirror",
        "docker",
        DockerMirror,
        true,
        true,
        false,
        vec![field("mirror", "", &[], true)],
    );
    add(
        "docker.ipv6",
        "docker",
        DockerIpv6,
        true,
        true,
        false,
        vec![field("enabled", "on", &["on", "off"], true)],
    );
    add(
        "docker.uninstall",
        "docker",
        DockerUninstall,
        true,
        true,
        false,
        vec![field("purge_data", "off", &["on", "off"], true)],
    );
    add("test.speed", "tests", Speedtest, false, true, true, vec![]);
    add(
        "test.ip",
        "tests",
        IpTest,
        false,
        true,
        true,
        vec![
            field("ip_family", "dual", &["dual", "ipv4", "ipv6"], true),
            field("full_ip", "off", &["off", "on"], true),
        ],
    );
    add("test.route", "tests", RouteTest, true, true, true, vec![]);
    add(
        "test.performance",
        "tests",
        Performance,
        false,
        true,
        true,
        vec![],
    );
    add(
        "test.connectivity",
        "tests",
        Connectivity,
        false,
        false,
        false,
        vec![],
    );
    add(
        "test.io",
        "tests",
        Io,
        false,
        true,
        false,
        vec![field("directory", "/var/tmp", &[], true)],
    );
    add("app.install", "app", Install, true, true, false, vec![]);
    add("app.uninstall", "app", Uninstall, true, true, false, vec![]);
    items
}
pub fn find(id: &str) -> Result<Spec> {
    specs()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| Error::new("error.action", id))
}

pub fn execute(ctx: &Context, spec: &Spec, supplied: &Parameters) -> Result<()> {
    let p = spec.parameters(supplied)?;
    ctx.platform()?;
    if spec.root {
        ctx.root()?;
    }
    if spec.changes
        && !matches!(
            spec.kind,
            Kind::KernelInstall | Kind::KernelRemove | Kind::AiInstall(_)
        )
    {
        let detail = format!(
            "{}\n{}\n{}",
            ctx.i18n.text(&spec.label()),
            ctx.i18n.text(&spec.description()),
            p.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        ctx.confirm(
            "confirm.action",
            detail,
            if spec.kind == Kind::TerminalRestore {
                "RESTORE"
            } else {
                "YES"
            },
        )?;
    }
    use Kind::*;
    match spec.kind {
        Info => ops::system::info(ctx),
        PublicIp => ops::system::public_ip(ctx),
        Update => ops::system::update(ctx),
        Clean => ops::system::clean(ctx, &p["scope"]),
        Timezone => ops::system::timezone(ctx, &p["timezone"]),
        SwapCreate => ops::system::swap(ctx, "create", &p["size_mb"]),
        SwapAuto => ops::system::swap(ctx, "auto", ""),
        SwapRemove => ops::system::swap(ctx, "remove", ""),
        Fail2ban => ops::system::fail2ban(ctx),
        SshPort => ops::ssh::port(ctx, &p["port"]),
        SshKey => ops::ssh::key_login(ctx, Path::new(&p["public_key"])),
        BbrStatus => ops::network::status(ctx),
        BbrEnable => ops::network::enable_bbr(ctx, &p["qdisc"]),
        BbrRestore => ops::network::restore(ctx, &ctx.paths.bbr),
        TcpTune => ops::network::tune(ctx, &p["bandwidth"], &p["rtt"]),
        TcpRestore => ops::network::restore(ctx, &ctx.paths.tcp),
        KernelList => ops::kernel::list(ctx),
        KernelInstall => ops::kernel::install(ctx, &p["profile"], &p["version"]),
        KernelRemove => ops::kernel::remove(ctx, &p["release"]),
        ToolsInstall => ops::tools::packages(ctx, &p["package"]),
        ToolInstall(package) => ops::tools::packages(ctx, package),
        AiInstall(tool) => ops::ai::install(ctx, tool),
        AiStatus => ops::ai::status(ctx),
        TerminalInstall => ops::tools::terminal_install(ctx),
        TerminalApply => ops::terminal::apply(
            ctx,
            &ops::terminal::Options {
                shell: p["shell"].clone(),
                style: p["style"].clone(),
                color: p["color"].clone(),
                time: p["time"].clone(),
                git: p["git"].clone(),
            },
        ),
        TerminalStatus => ops::terminal::status(ctx),
        TerminalRestore => ops::terminal::restore(ctx),
        DockerInstall => ops::docker::install(ctx, &p["source"]),
        DockerStatus => ops::docker::status(ctx),
        DockerClean => ctx.run("docker", &["system", "prune", "-af", "--volumes"]),
        DockerMirror => ops::docker::configure(ctx, ops::docker::Change::Mirror(&p["mirror"])),
        DockerIpv6 => ops::docker::configure(ctx, ops::docker::Change::Ipv6(p["enabled"] == "on")),
        DockerUninstall => ops::docker::uninstall(ctx, p["purge_data"] == "on"),
        Speedtest => ops::tools::speedtest(ctx),
        IpTest => ops::ip_quality::run(ctx, &p["ip_family"], &p["full_ip"]),
        RouteTest => {
            ops::tools::remote(ctx, "https://nxtrace.org/nt")?;
            ctx.passthrough("nexttrace", &["--fast-trace", "--tcp"])
        }
        Performance => ops::tools::remote(ctx, "https://yabs.sh"),
        Connectivity => ops::tools::connectivity(ctx),
        Io => ops::tools::io_test(ctx, Path::new(&p["directory"])),
        Install => ops::system::install_self(ctx),
        Uninstall => ops::system::uninstall_self(ctx),
    }
}

pub fn batch(ctx: &Context, jobs: &[(Spec, Parameters)]) -> Result<()> {
    if jobs.is_empty() {
        return Err(Error::new("error.input", "empty selection"));
    }
    let mut unique = BTreeSet::new();
    for (spec, p) in jobs {
        if !unique.insert(spec.id) {
            return Err(Error::new("error.input", spec.id));
        }
        spec.parameters(p)?;
    }
    ctx.confirm(
        "confirm.batch",
        jobs.iter()
            .map(|(s, _)| ctx.i18n.text(&s.label()))
            .collect::<Vec<_>>()
            .join("\n"),
        "YES",
    )?;
    let mut failures = Vec::new();
    for (index, (spec, p)) in jobs.iter().enumerate() {
        ctx.check()?;
        ctx.message(
            "notice.progress",
            format!(
                "{}/{} {}",
                index + 1,
                jobs.len(),
                ctx.i18n.text(&spec.label())
            ),
        );
        match execute(ctx, spec, p) {
            Ok(()) => ctx.message("notice.success", ctx.i18n.text(&spec.label())),
            Err(e) => {
                ctx.log(ctx.i18n.error(&e));
                failures.push(spec.id);
                if e.key == "error.cancelled"
                    || !matches!(spec.kind, Kind::TcpTune | Kind::BbrEnable)
                {
                    return Err(e);
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::new("error.batch", failures.join(", ")))
    }
}

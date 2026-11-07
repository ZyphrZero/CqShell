use crate::{
    error::{Error, Result},
    ops,
    runtime::{Context, Outcome, Step, StepState},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::{Duration, Instant},
};

pub type Parameters = BTreeMap<String, String>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    PublicIp,
    Update,
    MirrorTest,
    MirrorApply,
    MirrorRestore,
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
    ClashInstall,
    ClashStatus,
    ClashStart,
    ClashStop,
    ClashSubscriptions,
    ClashSubscriptionAdd,
    ClashSubscriptionUse,
    ClashSubscriptionUpdate,
    ClashSubscriptionRemove,
    ClashControllerStatus,
    ClashControllerConfigure,
    ClashProxies,
    ClashProxySelect,
    ClashSecretGenerate,
    ClashSecretSet,
    ClashSecretClear,
    ClashPanelStatus,
    ClashPanelInstall,
    ClashTunStatus,
    ClashTunConfigure,
    ClashLogs,
    ClashServiceStatus,
    ClashServiceInstall,
    ClashServiceRemove,
    KernelList,
    KernelInstall,
    KernelBoot,
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

/// Live system data a parameter can name, read when its form opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    Subscriptions,
    ProxyGroups,
    BootKernels,
    RemovableKernels,
}

/// Where a parameter's existing values come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Listing(Listing),
    /// The children of the choice made in the named, earlier field.
    Children(&'static str),
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: &'static str,
    pub default: &'static str,
    pub choices: &'static [&'static str],
    pub allow_custom: bool,
    pub required: bool,
    /// Values that already exist on this host. The CLI still accepts free text, and the
    /// operation validates the value either way.
    pub source: Option<Source>,
}
impl Field {
    pub fn editable(&self) -> bool {
        self.choices.is_empty() || self.allow_custom
    }

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
        allow_custom: false,
        required,
        source: None,
    }
}
/// A required parameter that names an existing item.
fn listed(name: &'static str, source: Source) -> Field {
    Field {
        source: Some(source),
        ..field(name, "", &[], true)
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
    /// Whether a reviewed Run-mode form submission may skip the generic confirmation.
    /// Set from [`REVERSIBLE`]; never shown by `cqs list`.
    pub reversible: bool,
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
                || (!field.editable() && !field.choices.contains(&value.as_str()))
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
            Kind::KernelBoot => regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9.+_~-]{0,127}$")
                .unwrap()
                .is_match(&p["release"]),
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
    "clash",
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

/// Actions whose effect the user can undo, so a reviewed form submission runs them without
/// the generic confirmation. Each entry was reviewed individually (design, component 8);
/// every other action is not reversible.
const REVERSIBLE: &[&str] = &[
    "bbr.enable",
    "tcp.tune",
    "system.timezone",
    "terminal.apply",
    "clash.proxy.select",
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
            reversible: REVERSIBLE.contains(&id),
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
        "system.mirror.test",
        "system",
        MirrorTest,
        false,
        false,
        false,
        vec![],
    );
    add(
        "system.mirror.apply",
        "system",
        MirrorApply,
        true,
        true,
        false,
        vec![field(
            "mirror_site",
            "auto",
            &["auto", "official", "tuna", "ustc", "aliyun"],
            true,
        )],
    );
    add(
        "system.mirror.restore",
        "system",
        MirrorRestore,
        true,
        true,
        false,
        vec![],
    );
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
        vec![Field {
            allow_custom: true,
            ..field(
                "timezone",
                "Asia/Shanghai",
                &[
                    "Asia/Shanghai",
                    "Asia/Hong_Kong",
                    "Asia/Taipei",
                    "Asia/Singapore",
                    "Asia/Tokyo",
                    "Asia/Seoul",
                    "Asia/Bangkok",
                    "Asia/Kolkata",
                    "Asia/Dubai",
                    "America/New_York",
                    "America/Chicago",
                    "America/Denver",
                    "America/Los_Angeles",
                    "America/Toronto",
                    "Europe/London",
                    "Europe/Berlin",
                    "Europe/Paris",
                    "Europe/Amsterdam",
                    "Europe/Helsinki",
                    "Europe/Moscow",
                    "Australia/Sydney",
                    "Australia/Perth",
                    "Pacific/Auckland",
                    "UTC",
                ],
                true,
            )
        }],
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
        "clash.install",
        "clash",
        ClashInstall,
        false,
        true,
        false,
        vec![field("version", "", &[], false)],
    );
    add(
        "clash.status",
        "clash",
        ClashStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "clash.start",
        "clash",
        ClashStart,
        false,
        true,
        false,
        vec![],
    );
    add("clash.stop", "clash", ClashStop, false, true, false, vec![]);
    add(
        "clash.sub.list",
        "clash",
        ClashSubscriptions,
        false,
        false,
        false,
        vec![],
    );
    add(
        "clash.sub.add",
        "clash",
        ClashSubscriptionAdd,
        false,
        true,
        false,
        vec![
            field("name", "", &[], true),
            field("url", "", &[], true),
            field("use", "off", &["on", "off"], true),
        ],
    );
    add(
        "clash.sub.use",
        "clash",
        ClashSubscriptionUse,
        false,
        true,
        false,
        vec![listed("name", Source::Listing(Listing::Subscriptions))],
    );
    add(
        "clash.sub.update",
        "clash",
        ClashSubscriptionUpdate,
        false,
        true,
        false,
        vec![listed("name", Source::Listing(Listing::Subscriptions))],
    );
    add(
        "clash.sub.remove",
        "clash",
        ClashSubscriptionRemove,
        false,
        true,
        false,
        vec![listed("name", Source::Listing(Listing::Subscriptions))],
    );
    add(
        "clash.controller.status",
        "clash",
        ClashControllerStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "clash.controller.configure",
        "clash",
        ClashControllerConfigure,
        false,
        true,
        false,
        vec![field("controller_address", "127.0.0.1:9090", &[], true)],
    );
    add(
        "clash.proxies",
        "clash",
        ClashProxies,
        false,
        false,
        false,
        vec![],
    );
    add(
        "clash.proxy.select",
        "clash",
        ClashProxySelect,
        false,
        true,
        false,
        vec![
            listed("proxy_group", Source::Listing(Listing::ProxyGroups)),
            listed("proxy_node", Source::Children("proxy_group")),
        ],
    );
    add(
        "clash.secret.generate",
        "clash",
        ClashSecretGenerate,
        false,
        true,
        false,
        vec![],
    );
    add(
        "clash.secret.set",
        "clash",
        ClashSecretSet,
        false,
        true,
        false,
        vec![field("secret_file", "", &[], true)],
    );
    add(
        "clash.secret.clear",
        "clash",
        ClashSecretClear,
        false,
        true,
        false,
        vec![],
    );
    add(
        "clash.panel.status",
        "clash",
        ClashPanelStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "clash.panel.install",
        "clash",
        ClashPanelInstall,
        false,
        true,
        false,
        vec![field("version", "", &[], false)],
    );
    add(
        "clash.tun.status",
        "clash",
        ClashTunStatus,
        false,
        false,
        false,
        vec![],
    );
    add(
        "clash.tun.configure",
        "clash",
        ClashTunConfigure,
        false,
        true,
        false,
        vec![
            field("enabled", "off", &["on", "off"], true),
            field("tun_stack", "mixed", &["system", "gvisor", "mixed"], true),
            field("tun_device", "cqs-tun", &[], true),
            field("auto_route", "on", &["on", "off"], true),
            field("dns_hijack", "on", &["on", "off"], true),
        ],
    );
    add(
        "clash.logs",
        "clash",
        ClashLogs,
        false,
        false,
        false,
        vec![
            field("lines", "100", &[], true),
            field("follow", "off", &["on", "off"], true),
        ],
    );
    add(
        "clash.service.status",
        "clash",
        ClashServiceStatus,
        false,
        false,
        false,
        vec![field("service_scope", "user", &["user", "system"], true)],
    );
    add(
        "clash.service.install",
        "clash",
        ClashServiceInstall,
        false,
        true,
        false,
        vec![field("service_scope", "user", &["user", "system"], true)],
    );
    add(
        "clash.service.remove",
        "clash",
        ClashServiceRemove,
        false,
        true,
        false,
        vec![field("service_scope", "user", &["user", "system"], true)],
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
        "kernel.boot",
        "kernel",
        KernelBoot,
        true,
        true,
        false,
        vec![listed("release", Source::Listing(Listing::BootKernels))],
    );
    add(
        "kernel.remove",
        "kernel",
        KernelRemove,
        true,
        true,
        false,
        vec![listed(
            "release",
            Source::Listing(Listing::RemovableKernels),
        )],
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

/// The existing items a listed parameter can name. Values that `Spec::parameters` would
/// reject, such as names containing control characters, are left out: choosing one could
/// never succeed, and they must not reach the terminal.
pub fn choices(ctx: &Context, listing: Listing) -> Result<Vec<ops::Choice>> {
    let mut choices = match listing {
        Listing::Subscriptions => ops::clash::subscription_choices(ctx)?,
        Listing::ProxyGroups => ops::clash::controller::group_choices(ctx)?,
        Listing::BootKernels => ops::kernel::boot_choices(ctx)?,
        Listing::RemovableKernels => ops::kernel::removable_choices(ctx)?,
    };
    retain_valid(&mut choices);
    Ok(choices)
}
fn retain_valid(choices: &mut Vec<ops::Choice>) {
    choices
        .retain(|choice| !choice.value.is_empty() && !choice.value.chars().any(char::is_control));
    for choice in choices {
        retain_valid(&mut choice.children);
    }
}

/// How the entry point that runs an action has already involved the user, which decides
/// whether the generic `confirm.action` is still shown. Specific confirmations requested
/// by the operation itself are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompt {
    /// `cqs run` and TUI activation without a form.
    Generic,
    /// Submission of a Run-mode parameter form the user has just reviewed.
    Reviewed,
    /// One item of a batch the user has confirmed as a whole.
    Batch,
}

/// Actions that ask for their own, more specific confirmation instead of the generic one.
fn self_confirming(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::KernelInstall
            | Kind::KernelRemove
            | Kind::KernelBoot
            | Kind::AiInstall(_)
            | Kind::MirrorApply
            | Kind::ClashInstall
            | Kind::ClashStart
    )
}

/// Whether running `spec` from an entry point of mode `prompt` shows `confirm.action`.
fn needs_generic(spec: &Spec, prompt: Prompt) -> bool {
    spec.changes
        && !self_confirming(spec.kind)
        && match prompt {
            Prompt::Generic => true,
            // Skipping the prompt for reviewed reversible actions is the rule itself, not
            // a fallback path.
            Prompt::Reviewed => !spec.reversible,
            // The batch confirmation covers every item; restoring the terminal baseline
            // still asks for its RESTORE token.
            Prompt::Batch => spec.kind == Kind::TerminalRestore,
        }
}

/// Run `spec` with the generic confirmation rule of `cqs run` and form-less TUI activation.
pub fn execute(ctx: &Context, spec: &Spec, supplied: &Parameters) -> Result<()> {
    run(ctx, spec, supplied, Prompt::Generic)
}

/// Run `spec` after the user submitted its Run-mode parameter form.
pub fn execute_reviewed(ctx: &Context, spec: &Spec, supplied: &Parameters) -> Result<()> {
    run(ctx, spec, supplied, Prompt::Reviewed)
}

fn run(ctx: &Context, spec: &Spec, supplied: &Parameters, prompt: Prompt) -> Result<()> {
    let p = spec.parameters(supplied)?;
    ctx.platform()?;
    if spec.root {
        ctx.root()?;
    }
    if needs_generic(spec, prompt) {
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
        MirrorTest => ops::mirrors::test(ctx),
        MirrorApply => ops::mirrors::apply(ctx, &p["mirror_site"]),
        MirrorRestore => ops::mirrors::restore(ctx),
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
        ClashInstall => ops::clash::install(ctx, &p["version"]),
        ClashStatus => ops::clash::status(ctx),
        ClashStart => ops::clash::start(ctx),
        ClashStop => ops::clash::stop(ctx),
        ClashSubscriptions => ops::clash::subscriptions(ctx),
        ClashSubscriptionAdd => {
            ops::clash::subscription_add(ctx, &p["name"], &p["url"], p["use"] == "on")
        }
        ClashSubscriptionUse => ops::clash::subscription_use(ctx, &p["name"]),
        ClashSubscriptionUpdate => ops::clash::subscription_update(ctx, &p["name"]),
        ClashSubscriptionRemove => ops::clash::subscription_remove(ctx, &p["name"]),
        ClashControllerStatus => ops::clash::controller::status(ctx),
        ClashControllerConfigure => {
            ops::clash::controller::configure(ctx, &p["controller_address"])
        }
        ClashProxies => ops::clash::controller::proxies(ctx),
        ClashProxySelect => {
            ops::clash::controller::select(ctx, &p["proxy_group"], &p["proxy_node"])
        }
        ClashSecretGenerate => ops::clash::controller::secret_generate(ctx),
        ClashSecretSet => ops::clash::controller::secret_set(ctx, &p["secret_file"]),
        ClashSecretClear => ops::clash::controller::secret_clear(ctx),
        ClashPanelStatus => ops::clash::panel::status(ctx),
        ClashPanelInstall => ops::clash::panel::install(ctx, &p["version"]),
        ClashTunStatus => ops::clash::tun::status(ctx),
        ClashTunConfigure => ops::clash::tun::configure(
            ctx,
            p["enabled"] == "on",
            &p["tun_stack"],
            &p["tun_device"],
            p["auto_route"] == "on",
            p["dns_hijack"] == "on",
        ),
        ClashLogs => ops::clash::logs(ctx, &p["lines"], p["follow"] == "on"),
        ClashServiceStatus => ops::clash::service::status(ctx, &p["service_scope"]),
        ClashServiceInstall => ops::clash::service::install(ctx, &p["service_scope"]),
        ClashServiceRemove => ops::clash::service::remove(ctx, &p["service_scope"]),
        KernelList => ops::kernel::list(ctx),
        KernelInstall => ops::kernel::install(ctx, &p["profile"], &p["version"]),
        KernelBoot => ops::kernel::boot(ctx, &p["release"]),
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

/// What happened to one job of a batch. Both fields are `None` when the job never started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    pub id: &'static str,
    pub outcome: Option<Outcome>,
    pub elapsed: Option<Duration>,
}

/// The result of a batch with one record per job, in job order, so a batch that ran without
/// live events (after handing over the terminal) can still be replayed.
#[derive(Debug)]
pub struct BatchReport {
    pub steps: Vec<StepRecord>,
    pub result: Result<()>,
}

/// [`batch_report`] without the per-job records, for `cqs batch`.
pub fn batch(ctx: &Context, jobs: &[(Spec, Parameters)]) -> Result<()> {
    batch_report(ctx, jobs).result
}

/// Run `jobs` after a single `confirm.batch`. Each job runs with [`Prompt::Batch`], so the
/// only confirmations left are the ones an operation asks for itself and terminal.restore's
/// RESTORE. Every job reports its start and end through [`Context::step`].
pub fn batch_report(ctx: &Context, jobs: &[(Spec, Parameters)]) -> BatchReport {
    let mut steps = jobs
        .iter()
        .map(|(spec, _)| StepRecord {
            id: spec.id,
            outcome: None,
            elapsed: None,
        })
        .collect::<Vec<_>>();
    let result = run_batch(ctx, jobs, &mut steps);
    BatchReport { steps, result }
}

fn run_batch(ctx: &Context, jobs: &[(Spec, Parameters)], steps: &mut [StepRecord]) -> Result<()> {
    if jobs.is_empty() {
        return Err(Error::new("error.input", "empty selection"));
    }
    let mut unique = BTreeSet::new();
    let mut lines = Vec::new();
    for (n, (spec, p)) in jobs.iter().enumerate() {
        if !unique.insert(spec.id) {
            return Err(Error::new("error.input", spec.id));
        }
        // The batch confirmation replaces each item's generic one, so it lists every
        // parameter, defaults included.
        let p = spec.parameters(p)?;
        let label = ctx.i18n.text(&spec.label());
        lines.push(if p.is_empty() {
            format!("{}. {label}", n + 1)
        } else {
            let values = p
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>();
            format!("{}. {label} ({})", n + 1, values.join(", "))
        });
    }
    ctx.confirm("confirm.batch", lines.join("\n"), "YES")?;
    let mut failures = Vec::new();
    for (index, (spec, p)) in jobs.iter().enumerate() {
        ctx.check()?;
        let step = |state| Step {
            index,
            total: jobs.len(),
            id: spec.id,
            state,
        };
        ctx.step(step(StepState::Started));
        let started = Instant::now();
        let result = run(ctx, spec, p, Prompt::Batch);
        let elapsed = started.elapsed();
        let outcome = match &result {
            Ok(()) => Outcome::Done,
            Err(e) if e.key == "error.cancelled" => Outcome::Cancelled,
            Err(_) => Outcome::Failed,
        };
        if let Err(e) = &result {
            ctx.log(ctx.i18n.error(e));
        }
        steps[index].outcome = Some(outcome);
        steps[index].elapsed = Some(elapsed);
        ctx.step(step(StepState::Finished { outcome, elapsed }));
        if let Err(e) = result {
            failures.push(spec.id);
            // A failed TCP tune or BBR enable does not stop the batch; a cancellation or any
            // other failure does.
            if outcome == Outcome::Cancelled
                || !matches!(spec.kind, Kind::TcpTune | Kind::BbrEnable)
            {
                return Err(e);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::new("error.batch", failures.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversible_actions_exist_take_parameters_and_change_the_system() {
        let specs = specs();
        for id in REVERSIBLE {
            let spec = specs
                .iter()
                .find(|s| s.id == *id)
                .unwrap_or_else(|| panic!("{id} is not an action"));
            assert!(!spec.fields.is_empty(), "{id} has no parameter form");
            assert!(spec.changes, "{id} does not change the system");
        }
        // Undeclared actions are not reversible.
        let marked: BTreeSet<_> = specs
            .iter()
            .filter(|s| s.reversible)
            .map(|s| s.id)
            .collect();
        assert_eq!(marked, REVERSIBLE.iter().copied().collect::<BTreeSet<_>>());
    }

    // Feature: tui-redesign, Property 15: 确认规则
    // **Validates: Requirements 11.2, 11.7, 12.3, 12.4, 12.5**
    //
    // The domain is finite, so every spec is checked under every mode instead of sampling.
    #[test]
    fn generic_confirmation_follows_the_entry_point_table() {
        // Written out from the design rather than derived from the implementation: actions
        // with their own confirmation, and the reviewed reversible actions.
        const OWN_CONFIRMATION: &[&str] = &[
            "kernel.install",
            "kernel.remove",
            "kernel.boot",
            "ai.pi.install",
            "ai.claude.install",
            "ai.codex.install",
            "system.mirror.apply",
            "clash.install",
            "clash.start",
        ];
        const REVIEWED_REVERSIBLE: &[&str] = &[
            "bbr.enable",
            "tcp.tune",
            "system.timezone",
            "terminal.apply",
            "clash.proxy.select",
        ];
        // Component 8, "generic confirmation per entry point".
        let expected = |spec: &Spec, prompt: Prompt| {
            let entry = match prompt {
                // `cqs run`, TUI activation without a form: always.
                Prompt::Generic => true,
                // TUI Run-mode form submission: only when not reversible.
                Prompt::Reviewed => !REVIEWED_REVERSIBLE.contains(&spec.id),
                // Each batch item: only terminal.restore.
                Prompt::Batch => spec.id == "terminal.restore",
            };
            spec.changes && !OWN_CONFIRMATION.contains(&spec.id) && entry
        };
        let specs = specs();
        for id in OWN_CONFIRMATION.iter().chain(REVIEWED_REVERSIBLE) {
            assert!(specs.iter().any(|s| s.id == *id), "{id} is not an action");
        }
        let mut cases = 0;
        for spec in &specs {
            for prompt in [Prompt::Generic, Prompt::Reviewed, Prompt::Batch] {
                cases += 1;
                let shown = needs_generic(spec, prompt);
                assert_eq!(shown, expected(spec, prompt), "{} {prompt:?}", spec.id);
                match prompt {
                    Prompt::Generic => assert_eq!(
                        shown,
                        spec.changes && !self_confirming(spec.kind),
                        "{} changed its existing confirmation",
                        spec.id
                    ),
                    Prompt::Reviewed => {
                        if spec.reversible {
                            assert!(!shown, "{} asks again after its form", spec.id);
                        }
                    }
                    Prompt::Batch => assert!(
                        !shown || spec.kind == Kind::TerminalRestore,
                        "{} asks inside a batch",
                        spec.id
                    ),
                }
            }
        }
        assert!(cases >= 100, "only {cases} combinations");
    }

    /// What a batch sent to its event receiver.
    #[derive(Default)]
    struct Sent {
        confirmations: Vec<(String, String)>,
        logs: Vec<String>,
        steps: Vec<Step>,
    }

    /// Run a batch on a worker, as the TUI does, answering every confirmation with `accept`.
    fn batch_with_events(
        mut ctx: Context,
        jobs: Vec<(Spec, Parameters)>,
        accept: bool,
    ) -> (BatchReport, Sent) {
        use crate::runtime::Event;
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        ctx.events = Some(tx);
        let worker = std::thread::spawn(move || batch_report(&ctx, &jobs));
        let mut sent = Sent::default();
        // The channel closes when the worker drops its context.
        loop {
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(Event::Confirm { text, token, reply }) => {
                    sent.confirmations.push((text, token));
                    reply.send(accept).unwrap();
                }
                Ok(Event::Log(text)) => sent.logs.push(text),
                Ok(Event::Step(step)) => sent.steps.push(step),
                Ok(Event::Finished(_)) => panic!("batch_report does not send Finished"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("batch did not finish"),
            }
        }
        (worker.join().unwrap(), sent)
    }

    fn not_run(id: &'static str) -> StepRecord {
        StepRecord {
            id,
            outcome: None,
            elapsed: None,
        }
    }

    #[test]
    fn one_batch_confirmation_lists_all_parameters_and_rejection_runs_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        ctx.yes = false;
        ctx.interactive = false;
        let i18n = ctx.i18n.clone();
        let jobs = vec![
            (
                find("tcp.tune").unwrap(),
                Parameters::from([("rtt".into(), "80".into())]),
            ),
            (find("tcp.restore").unwrap(), Parameters::new()),
        ];
        let (report, sent) = batch_with_events(ctx, jobs, false);
        // The defaulted bandwidth is listed too; a job without parameters shows its name.
        let detail = "1. Tune TCP buffers (bandwidth=1000, rtt=80)\n2. Restore TCP settings";
        assert_eq!(
            sent.confirmations,
            [(
                i18n.format("confirm.batch", &[("detail", detail)]),
                "YES".to_owned()
            )]
        );
        assert_eq!(report.result.unwrap_err().key, "error.cancelled");
        assert_eq!(report.steps, [not_run("tcp.tune"), not_run("tcp.restore")]);
        assert!(sent.steps.is_empty() && sent.logs.is_empty());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn tcp_tune_failures_continue_and_other_failures_stop_the_batch() {
        use crate::storage::{Lock, suffix};
        for (ids, failed, stops) in [
            // tcp.tune fails and the batch goes on; tcp.restore fails and it stops.
            (&["tcp.tune", "tcp.restore", "bbr.restore"][..], 2, true),
            // Only continuable failures: error.batch names them.
            (&["tcp.tune"][..], 1, false),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let ctx = crate::tests::context(temp.path());
            let i18n = ctx.i18n.clone();
            // Every job fails before it changes anything: at the platform check, the root
            // check, or, where both pass, at these configuration locks.
            let _tcp = Lock::acquire(&suffix(&ctx.paths.tcp, ".cqs-lock")).unwrap();
            let _bbr = Lock::acquire(&suffix(&ctx.paths.bbr, ".cqs-lock")).unwrap();
            let jobs = ids
                .iter()
                .map(|id| (find(id).unwrap(), Parameters::new()))
                .collect();
            let (report, sent) = batch_with_events(ctx, jobs, true);
            assert!(sent.confirmations.is_empty(), "--yes accepts confirm.batch");
            let ran = &report.steps[..failed];
            assert!(
                ran.iter()
                    .all(|s| s.outcome == Some(Outcome::Failed) && s.elapsed.is_some()),
                "{:?}",
                report.steps
            );
            let skipped = report.steps[failed..].iter().map(|s| s.id);
            assert!(skipped.eq(ids[failed..].iter().copied()));
            assert!(report.steps[failed..].iter().all(|s| *s == not_run(s.id)));
            // Each job that ran sent Started, then Finished with its record's values.
            let expected = ran
                .iter()
                .enumerate()
                .flat_map(|(index, record)| {
                    let step = |state| Step {
                        index,
                        total: ids.len(),
                        id: record.id,
                        state,
                    };
                    [
                        step(StepState::Started),
                        step(StepState::Finished {
                            outcome: Outcome::Failed,
                            elapsed: record.elapsed.unwrap(),
                        }),
                    ]
                })
                .collect::<Vec<_>>();
            assert_eq!(sent.steps, expected);
            let error = report.result.unwrap_err();
            if stops {
                // The stopping job's own error, also logged as the batch's last line.
                assert_ne!(error.key, "error.batch");
                assert_eq!(sent.logs.last(), Some(&i18n.error(&error)));
            } else {
                assert_eq!(
                    (error.key, error.detail.as_str()),
                    ("error.batch", "tcp.tune")
                );
            }
        }
    }
}

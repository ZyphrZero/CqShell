//! Typed Docker mutation operations, confirmation risk tiers, CLI previews, and safe
//! interactive `docker exec -it` shell detection/handoff.

use crate::{
    error::{Error, Result},
    runtime::Context,
};
use std::process::{Command, Stdio};

/// Tiered confirmation policy matching the design's L0–L3 safety matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmPolicy {
    /// L0: read-only or harmless recovery (no prompt).
    None,
    /// L1 / L2: reversible or scoped removal (Yes/No confirmation, default Cancel).
    YesNo,
    /// L3: irreversible data destruction or large batch deletion (typed token required).
    Token(&'static str),
}

/// Interactive terminal session specification (`docker exec -it`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    pub container_id: String,
    pub container_name: String,
    /// Empty or `"auto"` triggers static `test -x` detection (`/bin/bash` -> `/bin/ash` -> `/bin/sh`).
    pub shell: String,
    pub user: String,
    pub workdir: String,
}

/// Single source of truth for all Docker TUI mutations: drives CLI preview, confirmation policy,
/// i18n title keys, and argument-array execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerOp {
    ContainerStart {
        ids: Vec<String>,
        names: Vec<String>,
    },
    ContainerStop {
        ids: Vec<String>,
        names: Vec<String>,
    },
    ContainerRestart {
        ids: Vec<String>,
        names: Vec<String>,
    },
    ContainerPause {
        ids: Vec<String>,
        names: Vec<String>,
    },
    ContainerUnpause {
        ids: Vec<String>,
        names: Vec<String>,
    },
    ContainerRemove {
        ids: Vec<String>,
        names: Vec<String>,
        force: bool,
    },
    ComposeUp {
        project: String,
        config_files: Vec<String>,
    },
    ComposeStart {
        project: String,
        config_files: Vec<String>,
    },
    ComposeStop {
        project: String,
        config_files: Vec<String>,
    },
    ComposeRestart {
        project: String,
        config_files: Vec<String>,
    },
    ComposePull {
        project: String,
        config_files: Vec<String>,
    },
    ComposeDown {
        project: String,
        config_files: Vec<String>,
        volumes: bool,
    },
    ImagePull {
        reference: String,
    },
    ImageRemove {
        ids: Vec<String>,
        labels: Vec<String>,
        force: bool,
    },
    VolumeRemove {
        names: Vec<String>,
    },
    NetworkCreate {
        name: String,
        subnet: String,
        ipv6: bool,
    },
    NetworkRemove {
        ids: Vec<String>,
        names: Vec<String>,
    },
    Prune {
        containers: bool,
        images: bool,
        all_unused_images: bool,
        volumes: bool,
        networks: bool,
        build_cache: bool,
    },
    ServiceStart,
    ServiceStop,
    ServiceRestart,
}

impl DockerOp {
    pub fn title_key(&self) -> &'static str {
        match self {
            Self::ContainerStart { .. } => "docker.op.start",
            Self::ContainerStop { .. } => "docker.op.stop",
            Self::ContainerRestart { .. } => "docker.op.restart",
            Self::ContainerPause { .. } => "docker.op.pause",
            Self::ContainerUnpause { .. } => "docker.op.unpause",
            Self::ContainerRemove { .. } => "docker.op.remove_container",
            Self::ComposeUp { .. } => "docker.op.compose_up",
            Self::ComposeStart { .. } => "docker.op.compose_start",
            Self::ComposeStop { .. } => "docker.op.compose_stop",
            Self::ComposeRestart { .. } => "docker.op.compose_restart",
            Self::ComposePull { .. } => "docker.op.compose_pull",
            Self::ComposeDown { .. } => "docker.op.compose_down",
            Self::ImagePull { .. } => "docker.op.image_pull",
            Self::ImageRemove { .. } => "docker.op.remove_image",
            Self::VolumeRemove { .. } => "docker.op.remove_volume",
            Self::NetworkCreate { .. } => "docker.op.create_network",
            Self::NetworkRemove { .. } => "docker.op.remove_network",
            Self::Prune { .. } => "docker.op.prune",
            Self::ServiceStart => "docker.op.service_start",
            Self::ServiceStop => "docker.op.service_stop",
            Self::ServiceRestart => "docker.op.service_restart",
        }
    }

    /// Confirmation risk policy (L0 None, L1/L2 YesNo, L3 Typed Token).
    pub fn confirm_policy(&self) -> ConfirmPolicy {
        match self {
            Self::ContainerStart { .. } | Self::ContainerUnpause { .. } => ConfirmPolicy::None,
            Self::ContainerStop { .. }
            | Self::ContainerRestart { .. }
            | Self::ContainerPause { .. }
            | Self::ComposeUp { .. }
            | Self::ComposeStart { .. }
            | Self::ComposeStop { .. }
            | Self::ComposeRestart { .. }
            | Self::ComposePull { .. }
            | Self::ImagePull { .. }
            | Self::NetworkCreate { .. }
            | Self::ServiceStart
            | Self::ServiceStop
            | Self::ServiceRestart => ConfirmPolicy::YesNo,
            Self::ContainerRemove { ids, .. } => {
                if ids.len() >= 3 {
                    ConfirmPolicy::Token("REMOVE")
                } else {
                    ConfirmPolicy::YesNo
                }
            }
            Self::ImageRemove { ids, .. } => {
                if ids.len() >= 3 {
                    ConfirmPolicy::Token("REMOVE")
                } else {
                    ConfirmPolicy::YesNo
                }
            }
            Self::NetworkRemove { ids, .. } => {
                if ids.len() >= 3 {
                    ConfirmPolicy::Token("REMOVE")
                } else {
                    ConfirmPolicy::YesNo
                }
            }
            Self::VolumeRemove { .. } => ConfirmPolicy::Token("REMOVE"),
            Self::ComposeDown { volumes, .. } => {
                if *volumes {
                    ConfirmPolicy::Token("REMOVE")
                } else {
                    ConfirmPolicy::YesNo
                }
            }
            Self::Prune { volumes, .. } => {
                if *volumes {
                    ConfirmPolicy::Token("PRUNE")
                } else {
                    ConfirmPolicy::YesNo
                }
            }
        }
    }

    pub fn requires_root(&self) -> bool {
        matches!(
            self,
            Self::ServiceStart | Self::ServiceStop | Self::ServiceRestart
        )
    }

    /// Human-readable summary of target resource names for confirmation dialogs.
    pub fn target_summary(&self) -> String {
        match self {
            Self::ContainerStart { names, .. }
            | Self::ContainerStop { names, .. }
            | Self::ContainerRestart { names, .. }
            | Self::ContainerPause { names, .. }
            | Self::ContainerUnpause { names, .. }
            | Self::ContainerRemove { names, .. }
            | Self::NetworkRemove { names, .. } => names.join(", "),
            Self::ComposeUp { project, .. }
            | Self::ComposeStart { project, .. }
            | Self::ComposeStop { project, .. }
            | Self::ComposeRestart { project, .. }
            | Self::ComposePull { project, .. }
            | Self::ComposeDown { project, .. } => project.clone(),
            Self::ImagePull { reference } => reference.clone(),
            Self::ImageRemove { labels, .. } => labels.join(", "),
            Self::VolumeRemove { names } => names.join(", "),
            Self::NetworkCreate { name, .. } => name.clone(),
            Self::Prune {
                containers,
                images,
                all_unused_images,
                volumes,
                networks,
                build_cache,
            } => {
                let mut parts = Vec::new();
                if *containers {
                    parts.push("containers");
                }
                if *images {
                    parts.push(if *all_unused_images {
                        "images (all unused)"
                    } else {
                        "images (dangling)"
                    });
                }
                if *volumes {
                    parts.push("volumes");
                }
                if *networks {
                    parts.push("networks");
                }
                if *build_cache {
                    parts.push("build-cache");
                }
                parts.join(", ")
            }
            Self::ServiceStart | Self::ServiceStop | Self::ServiceRestart => {
                "docker.service".into()
            }
        }
    }

    /// Exact command preview shown in confirmations and copied via OSC 52.
    pub fn cli_preview(&self) -> String {
        let cmds = self.commands();
        cmds.into_iter()
            .map(|(prog, args)| {
                let rendered = args
                    .into_iter()
                    .map(|a| shell_quote_display(&a))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("{prog} {rendered}")
            })
            .collect::<Vec<_>>()
            .join(" && ")
    }

    /// Structured `(program, args)` sequence executed with zero shell interpolation.
    pub fn commands(&self) -> Vec<(&'static str, Vec<String>)> {
        let compose_base = |project: &str, files: &[String]| {
            let mut args = vec!["compose".into(), "-p".into(), project.to_owned()];
            for f in files {
                args.push("-f".into());
                args.push(f.clone());
            }
            args
        };
        match self {
            Self::ContainerStart { ids, .. } => {
                let mut a = vec!["start".into()];
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::ContainerStop { ids, .. } => {
                let mut a = vec!["stop".into()];
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::ContainerRestart { ids, .. } => {
                let mut a = vec!["restart".into()];
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::ContainerPause { ids, .. } => {
                let mut a = vec!["pause".into()];
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::ContainerUnpause { ids, .. } => {
                let mut a = vec!["unpause".into()];
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::ContainerRemove { ids, force, .. } => {
                let mut a = vec!["rm".into()];
                if *force {
                    a.push("-f".into());
                }
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::ComposeUp {
                project,
                config_files,
            } => {
                let mut a = compose_base(project, config_files);
                a.extend(["up".into(), "-d".into()]);
                vec![("docker", a)]
            }
            Self::ComposeStart {
                project,
                config_files,
            } => {
                let mut a = compose_base(project, config_files);
                a.push("start".into());
                vec![("docker", a)]
            }
            Self::ComposeStop {
                project,
                config_files,
            } => {
                let mut a = compose_base(project, config_files);
                a.push("stop".into());
                vec![("docker", a)]
            }
            Self::ComposeRestart {
                project,
                config_files,
            } => {
                let mut a = compose_base(project, config_files);
                a.push("restart".into());
                vec![("docker", a)]
            }
            Self::ComposePull {
                project,
                config_files,
            } => {
                let mut a = compose_base(project, config_files);
                a.push("pull".into());
                vec![("docker", a)]
            }
            Self::ComposeDown {
                project,
                config_files,
                volumes,
            } => {
                let mut a = compose_base(project, config_files);
                a.push("down".into());
                if *volumes {
                    a.push("--volumes".into());
                }
                vec![("docker", a)]
            }
            Self::ImagePull { reference } => {
                vec![("docker", vec!["pull".into(), reference.clone()])]
            }
            Self::ImageRemove { ids, force, .. } => {
                let mut a = vec!["image".into(), "rm".into()];
                if *force {
                    a.push("-f".into());
                }
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::VolumeRemove { names } => {
                let mut a = vec!["volume".into(), "rm".into()];
                a.extend(names.clone());
                vec![("docker", a)]
            }
            Self::NetworkCreate { name, subnet, ipv6 } => {
                let mut a = vec!["network".into(), "create".into()];
                if !subnet.trim().is_empty() {
                    a.push("--subnet".into());
                    a.push(subnet.trim().to_owned());
                }
                if *ipv6 {
                    a.push("--ipv6".into());
                }
                a.push(name.clone());
                vec![("docker", a)]
            }
            Self::NetworkRemove { ids, .. } => {
                let mut a = vec!["network".into(), "rm".into()];
                a.extend(ids.clone());
                vec![("docker", a)]
            }
            Self::Prune {
                containers,
                images,
                all_unused_images,
                volumes,
                networks,
                build_cache,
            } => {
                let mut cmds = Vec::new();
                if *containers {
                    cmds.push((
                        "docker",
                        vec!["container".into(), "prune".into(), "-f".into()],
                    ));
                }
                if *images {
                    let mut a = vec!["image".into(), "prune".into(), "-f".into()];
                    if *all_unused_images {
                        a.push("-a".into());
                    }
                    cmds.push(("docker", a));
                }
                if *volumes {
                    cmds.push(("docker", vec!["volume".into(), "prune".into(), "-f".into()]));
                }
                if *networks {
                    cmds.push((
                        "docker",
                        vec!["network".into(), "prune".into(), "-f".into()],
                    ));
                }
                if *build_cache {
                    cmds.push((
                        "docker",
                        vec!["builder".into(), "prune".into(), "-f".into()],
                    ));
                }
                cmds
            }
            Self::ServiceStart => vec![("systemctl", vec!["start".into(), "docker".into()])],
            Self::ServiceStop => vec![("systemctl", vec!["stop".into(), "docker".into()])],
            Self::ServiceRestart => vec![("systemctl", vec!["restart".into(), "docker".into()])],
        }
    }

    pub fn validate(&self) -> Result<()> {
        let check_token = |val: &str, field: &str| -> Result<()> {
            if val.is_empty() || val.starts_with('-') || val.chars().any(char::is_control) {
                return Err(Error::new("error.input", format!("{field}: {val}")));
            }
            Ok(())
        };
        match self {
            Self::ContainerStart { ids, .. }
            | Self::ContainerStop { ids, .. }
            | Self::ContainerRestart { ids, .. }
            | Self::ContainerPause { ids, .. }
            | Self::ContainerUnpause { ids, .. }
            | Self::ContainerRemove { ids, .. }
            | Self::ImageRemove { ids, .. }
            | Self::NetworkRemove { ids, .. } => {
                if ids.is_empty() {
                    return Err(Error::new("error.required", "target"));
                }
                for id in ids {
                    check_token(id, "id")?;
                }
            }
            Self::VolumeRemove { names } => {
                if names.is_empty() {
                    return Err(Error::new("error.required", "volume"));
                }
                for name in names {
                    check_token(name, "volume")?;
                }
            }
            Self::ImagePull { reference } => {
                let r = reference.trim();
                if r.is_empty()
                    || r.starts_with('-')
                    || r.chars().any(|c| c.is_control() || c.is_whitespace())
                {
                    return Err(Error::new("error.input", format!("image: {reference}")));
                }
            }
            Self::NetworkCreate { name, subnet, .. } => {
                check_token(name.trim(), "network")?;
                if name.trim().chars().any(char::is_whitespace) {
                    return Err(Error::new("error.input", format!("network: {name}")));
                }
                let s = subnet.trim();
                if !s.is_empty()
                    && (s.starts_with('-')
                        || s.chars().any(|c| c.is_control() || c.is_whitespace())
                        || !s.contains('/'))
                {
                    return Err(Error::new("error.input", format!("subnet: {subnet}")));
                }
            }
            Self::ComposeUp { project, .. }
            | Self::ComposeStart { project, .. }
            | Self::ComposeStop { project, .. }
            | Self::ComposeRestart { project, .. }
            | Self::ComposePull { project, .. }
            | Self::ComposeDown { project, .. } => {
                check_token(project, "project")?;
            }
            Self::Prune {
                containers,
                images,
                volumes,
                networks,
                build_cache,
                ..
            } => {
                if !(*containers || *images || *volumes || *networks || *build_cache) {
                    return Err(Error::new("error.required", "prune target"));
                }
            }
            Self::ServiceStart | Self::ServiceStop | Self::ServiceRestart => {}
        }
        Ok(())
    }

    pub fn execute(&self, ctx: &Context) -> Result<()> {
        self.validate()?;
        ctx.platform()?;
        if self.requires_root() {
            ctx.root()?;
        }
        match self.confirm_policy() {
            ConfirmPolicy::None => {}
            ConfirmPolicy::YesNo => {
                let detail = format!(
                    "{}\n{}\n\n$ {}",
                    ctx.i18n.text(self.title_key()),
                    self.target_summary(),
                    self.cli_preview()
                );
                ctx.confirm("confirm.action", detail, "YES")?;
            }
            ConfirmPolicy::Token(token) => {
                let detail = format!(
                    "{}\n{}\n\n$ {}",
                    ctx.i18n.text(self.title_key()),
                    self.target_summary(),
                    self.cli_preview()
                );
                ctx.confirm("confirm.action", detail, token)?;
            }
        }
        for (prog, args) in self.commands() {
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            ctx.run(prog, &refs)?;
        }
        Ok(())
    }
}

/// Probes available shells inside a running container using pure `docker exec <id> test -x <path>`
/// without ever spawning `sh -c`.
pub fn detect_container_shell(ctx: &Context, container_id: &str) -> String {
    for candidate in ["/bin/bash", "/bin/ash", "/bin/sh"] {
        if let Ok(out) = ctx.command(
            "docker",
            &["exec", container_id, "test", "-x", candidate],
            false,
        ) && out.status.success()
        {
            return candidate.to_owned();
        }
    }
    "/bin/sh".to_owned()
}

/// Validates and constructs argument vector for `docker exec -it`.
pub fn build_exec_args(spec: &ExecSpec, resolved_shell: &str) -> Result<Vec<String>> {
    if spec.container_id.is_empty()
        || spec.container_id.starts_with('-')
        || spec.container_id.chars().any(char::is_control)
    {
        return Err(Error::new("error.input", &spec.container_id));
    }
    if resolved_shell.is_empty()
        || resolved_shell.starts_with('-')
        || resolved_shell
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(Error::new("error.input", resolved_shell));
    }
    let mut args = vec!["exec".into(), "-it".into()];
    let user = spec.user.trim();
    if !user.is_empty() {
        if user.starts_with('-') || user.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(Error::new("error.input", user));
        }
        args.push("-u".into());
        args.push(user.to_owned());
    }
    let workdir = spec.workdir.trim();
    if !workdir.is_empty() {
        if workdir.starts_with('-') || workdir.chars().any(char::is_control) {
            return Err(Error::new("error.input", workdir));
        }
        args.push("-w".into());
        args.push(workdir.to_owned());
    }
    args.push(spec.container_id.clone());
    args.push(resolved_shell.to_owned());
    Ok(args)
}

/// Runs an interactive `docker exec -it` inheriting the user's real TTY (called after TUI `leave()`).
pub fn run_interactive_exec(ctx: &Context, spec: &ExecSpec) -> Result<()> {
    ctx.platform()?;
    let shell = if spec.shell.trim().is_empty() || spec.shell.trim() == "auto" {
        detect_container_shell(ctx, &spec.container_id)
    } else {
        spec.shell.trim().to_owned()
    };
    let args = build_exec_args(spec, &shell)?;
    let status = Command::new("docker")
        .args(&args)
        .envs(&ctx.environment)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| Error::new("error.command", format!("docker exec: {e}")))?;
    if !status.success() && !matches!(status.code(), Some(0 | 130)) {
        return Err(Error::new(
            "error.command",
            format!("docker exec exited with {status}"),
        ));
    }
    Ok(())
}

fn shell_quote_display(arg: &str) -> String {
    if arg.is_empty()
        || arg.bytes().any(|b| {
            matches!(
                b,
                b' ' | b'\t' | b'"' | b'\'' | b'$' | b'\\' | b';' | b'|' | b'&'
            )
        })
    {
        format!("'{}'", arg.replace('\'', "'\\''"))
    } else {
        arg.to_owned()
    }
}

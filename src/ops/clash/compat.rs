use crate::{
    error::{Error, Result},
    i18n::I18n,
    runtime::Context,
    storage,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::{fs, path::Path};

const ALIASES: &[(&str, &str)] = &[
    ("clashon", "on"),
    ("clashoff", "off"),
    ("clashstart", "start"),
    ("clashstop", "stop"),
    ("clashstatus", "status"),
    ("clashui", "ui"),
    ("clashsub", "sub"),
    ("clashnode", "node"),
    ("clashtun", "tun"),
    ("clashmixin", "mixin"),
    ("clashsecret", "secret"),
    ("clashlog", "log"),
    ("clashupgrade", "upgrade"),
    ("clashupdate", "update"),
    ("clashversion", "version"),
    ("clashhelp", "help"),
];

fn flag(name: &'static str, short: Option<char>) -> Arg {
    let argument = Arg::new(name).long(name).action(ArgAction::SetTrue);
    match short {
        Some(short) => argument.short(short),
        None => argument,
    }
}

fn group_node(command: Command) -> Command {
    command
        .arg(Arg::new("group").required(true))
        .arg(Arg::new("node").required(true))
}

pub fn command(i18n: &I18n) -> Command {
    let mut command = Command::new("clashctl")
        .bin_name("clashctl")
        .override_usage("clashctl [OPTIONS] [COMMAND]")
        .about(i18n.text("cli.clashctl").to_owned())
        .after_help(i18n.text("cli.clashctl_boundary").to_owned())
        .disable_help_subcommand(true)
        .arg(flag("version", Some('v')));
    for (name, action) in [
        ("start", "clash.start"),
        ("stop", "clash.stop"),
        ("status", "clash.status"),
        ("ui", "clash.panel.status"),
    ] {
        command = command.subcommand(
            Command::new(name).about(i18n.text(&format!("action.{action}")).to_owned()),
        );
    }
    command
        .subcommand(Command::new("help"))
        .subcommand(Command::new("version"))
        .subcommand(Command::new("on").about(i18n.text("cli.clashctl_on").to_owned()))
        .subcommand(
            Command::new("off")
                .about(i18n.text("cli.clashctl_off").to_owned())
                .arg(flag("stop", None)),
        )
        .subcommand(
            Command::new("shell-init")
                .about(i18n.text("cli.clashctl_init").to_owned())
                .arg(
                    Arg::new("shell")
                        .long("shell")
                        .default_value("bash")
                        .value_parser(["bash", "zsh", "fish"]),
                ),
        )
        .subcommand(
            Command::new("sub")
                .subcommand(Command::new("list").visible_alias("ls"))
                .subcommand(
                    Command::new("add")
                        .arg(Arg::new("url").required(true))
                        .arg(Arg::new("name").short('n').long("name"))
                        .arg(flag("use", Some('u')))
                        .arg(flag("raw", None)),
                )
                .subcommand(Command::new("use").arg(Arg::new("name").required(true)))
                .subcommand(
                    Command::new("update")
                        .arg(Arg::new("name").conflicts_with("all"))
                        .arg(flag("all", None))
                        .arg(flag("raw", None)),
                )
                .subcommand(
                    Command::new("del")
                        .visible_alias("delete")
                        .arg(Arg::new("name").required(true)),
                )
                .subcommand(
                    Command::new("rename")
                        .arg(Arg::new("old").required(true))
                        .arg(Arg::new("new").required(true)),
                )
                .subcommand(Command::new("log")),
        )
        .subcommand(
            Command::new("node")
                .subcommand_precedence_over_arg(true)
                .arg(Arg::new("group"))
                .arg(Arg::new("node").requires("group"))
                .subcommand(group_node(Command::new("use")))
                .subcommand(
                    Command::new("list")
                        .visible_alias("ls")
                        .arg(Arg::new("group")),
                )
                .subcommand(
                    Command::new("delay").arg(
                        Arg::new("arguments")
                            .num_args(0..)
                            .trailing_var_arg(true)
                            .allow_hyphen_values(true),
                    ),
                ),
        )
        .subcommand(Command::new("tun").arg(Arg::new("state").value_parser(["on", "off"])))
        .subcommand(
            Command::new("mixin")
                .arg(flag("edit", Some('e')).conflicts_with_all(["runtime", "base-config"]))
                .arg(flag("runtime", Some('r')).conflicts_with("base-config"))
                .arg(flag("base-config", Some('c'))),
        )
        .subcommand(
            Command::new("secret")
                .arg(Arg::new("legacy-value"))
                .arg(Arg::new("file").long("file").conflicts_with_all([
                    "generate",
                    "clear",
                    "legacy-value",
                ]))
                .arg(flag("generate", None).conflicts_with_all(["clear", "legacy-value"]))
                .arg(flag("clear", None).conflicts_with("legacy-value")),
        )
        .subcommand(
            Command::new("log")
                .arg(
                    Arg::new("lines")
                        .short('n')
                        .long("lines")
                        .default_value("100"),
                )
                .arg(flag("follow", Some('f'))),
        )
        .subcommand(
            Command::new("upgrade")
                .arg(flag("release", Some('r')).conflicts_with("alpha"))
                .arg(flag("alpha", Some('a')))
                .arg(flag("verbose", Some('v'))),
        )
        .subcommand(Command::new("update"))
}

pub fn normalize_arguments(arguments: &mut Vec<String>) {
    let Some(name) = arguments
        .first()
        .and_then(|argument| Path::new(argument).file_stem())
        .and_then(|name| name.to_str())
    else {
        return;
    };
    if name == "clashctl" {
        arguments[0] = "cqs".into();
        arguments.insert(1, "clashctl".into());
    } else if let Some((_, operation)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
        arguments[0] = "cqs".into();
        arguments.insert(1, (*operation).into());
        arguments.insert(1, "clashctl".into());
    }
}

fn required<'a>(arguments: &'a ArgMatches, name: &str) -> &'a str {
    arguments.get_one::<String>(name).unwrap()
}

fn unsupported(detail: &str) -> Error {
    Error::new("error.cli", detail)
}

pub fn dispatch(ctx: &Context, arguments: &ArgMatches) -> Result<()> {
    if arguments.get_flag("version") || arguments.subcommand_name() == Some("version") {
        if arguments.get_flag("version") && arguments.subcommand().is_some() {
            return Err(unsupported(
                "clashctl --version cannot be combined with a command",
            ));
        }
        return version(ctx);
    }
    match arguments.subcommand() {
        None | Some(("help", _)) => {
            command(&ctx.i18n).print_long_help()?;
            println!();
            return Ok(());
        }
        Some(("shell-init", arguments)) => {
            print!("{}", shell_init(ctx, required(arguments, "shell"))?);
            return Ok(());
        }
        Some(("on" | "off", _)) => {
            return Err(Error::new(
                "error.clashctl_shell",
                "cqs clashctl shell-init --shell bash|zsh|fish",
            ));
        }
        Some(("update", _)) => {
            return Err(unsupported(
                "clashctl update cannot update the Rust binary; rerun the CqShell installer",
            ));
        }
        Some(("secret", arguments)) if arguments.get_one::<String>("legacy-value").is_some() => {
            return Err(unsupported(
                "secrets in command arguments are not supported; use clashctl secret --file PRIVATE_FILE",
            ));
        }
        Some(("mixin", arguments)) if arguments.get_flag("edit") => {
            return Err(unsupported(
                "interactive Mixin editing is not implemented; stop Mihomo and edit CQS_CLASH_MIXIN with a trusted editor",
            ));
        }
        Some(("upgrade", arguments)) if arguments.get_flag("alpha") => {
            return Err(unsupported(
                "alpha kernel upgrades are not supported; only verified stable releases are accepted",
            ));
        }
        Some(("node", arguments)) if arguments.subcommand_name() == Some("delay") => {
            return Err(unsupported("node delay measurement is not implemented"));
        }
        Some(("sub", arguments)) if arguments.subcommand_name() == Some("log") => {
            return Err(unsupported(
                "subscription operation logs are not implemented; clashctl log reads Mihomo kernel logs only",
            ));
        }
        _ => (),
    }
    ctx.platform()?;
    match arguments.subcommand() {
        Some(("start", _)) => super::start(ctx),
        Some(("stop", _)) => super::stop(ctx),
        Some(("status", _)) => {
            super::status(ctx)?;
            let enabled = [
                "http_proxy",
                "https_proxy",
                "all_proxy",
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
            ]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()));
            ctx.message("notice.clashctl_proxy", if enabled { "on" } else { "off" });
            Ok(())
        }
        Some(("ui", _)) => super::panel::status(ctx),
        Some(("sub", arguments)) => subscription(ctx, arguments),
        Some(("node", arguments)) => node(ctx, arguments),
        Some(("tun", arguments)) => {
            match arguments.get_one::<String>("state").map(String::as_str) {
                Some("on") => super::tun::set_enabled(ctx, true),
                Some("off") => super::tun::set_enabled(ctx, false),
                _ => super::tun::status(ctx),
            }
        }
        Some(("mixin", arguments)) => {
            let path = if arguments.get_flag("runtime") {
                &ctx.paths.clash_runtime
            } else if arguments.get_flag("base-config") {
                &ctx.paths.clash_config
            } else {
                &ctx.paths.clash_mixin
            };
            storage::check_path(path, true)?;
            let text = fs::read_to_string(path)?;
            ctx.log(super::logs::redact(ctx, &text)?);
            Ok(())
        }
        Some(("secret", arguments)) => {
            if let Some(path) = arguments.get_one::<String>("file") {
                super::controller::secret_set(ctx, path)
            } else if arguments.get_flag("generate") {
                super::controller::secret_generate(ctx)
            } else if arguments.get_flag("clear") {
                super::controller::secret_clear(ctx)
            } else {
                super::controller::status(ctx)
            }
        }
        Some(("log", arguments)) => super::logs(
            ctx,
            required(arguments, "lines"),
            arguments.get_flag("follow"),
        ),
        Some(("upgrade", _)) => super::install(ctx, ""),
        _ => Err(unsupported("unsupported clashctl command")),
    }
}

fn subscription(ctx: &Context, arguments: &ArgMatches) -> Result<()> {
    match arguments.subcommand() {
        None | Some(("list", _)) => super::subscriptions(ctx),
        Some(("add", arguments)) => {
            let url = required(arguments, "url");
            let name = match arguments.get_one::<String>("name") {
                Some(name) => name.clone(),
                None => default_name(ctx, url)?,
            };
            super::subscription_add(ctx, &name, url, arguments.get_flag("use"))?;
            if arguments.get_flag("use") {
                ctx.log(ctx.i18n.text("notice.clashctl_restart"));
            }
            Ok(())
        }
        Some(("use", arguments)) => {
            super::subscription_use(ctx, required(arguments, "name"))?;
            ctx.log(ctx.i18n.text("notice.clashctl_restart"));
            Ok(())
        }
        Some(("update", arguments)) => {
            let profiles = super::read_profiles(ctx)?;
            let names = if arguments.get_flag("all") {
                profiles
                    .profiles
                    .into_iter()
                    .map(|profile| profile.name)
                    .collect::<Vec<_>>()
            } else {
                vec![
                    arguments
                        .get_one::<String>("name")
                        .cloned()
                        .or(profiles.current.clone())
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| {
                            Error::new(
                                "error.input",
                                "no current subscription; specify a name or --all",
                            )
                        })?,
                ]
            };
            if names.is_empty() {
                return Err(Error::new("error.input", "no subscriptions configured"));
            }
            let mut failures = 0;
            for name in names {
                if let Err(error) = super::subscription_update(ctx, &name) {
                    if error.key == "error.cancelled" {
                        return Err(error);
                    }
                    eprintln!("{}", ctx.i18n.error(&error));
                    failures += 1;
                }
            }
            if failures > 0 {
                return Err(Error::new(
                    "error.command",
                    format!("{failures} subscription update(s) failed"),
                ));
            }
            ctx.log(ctx.i18n.text("notice.clashctl_restart"));
            Ok(())
        }
        Some(("del", arguments)) => super::subscription_remove(ctx, required(arguments, "name")),
        Some(("rename", arguments)) => {
            rename_subscription(ctx, required(arguments, "old"), required(arguments, "new"))
        }
        _ => Err(unsupported("unsupported subscription command")),
    }
}

fn rename_subscription(ctx: &Context, old: &str, new: &str) -> Result<()> {
    super::validate_profile_name(old)?;
    super::validate_profile_name(new)?;
    let _lock = storage::Lock::acquire(&ctx.paths.clash_lock)?;
    super::recover_interrupted_files(ctx)?;
    let mut profiles = super::read_profiles(ctx)?;
    let index = profiles
        .profiles
        .iter()
        .position(|profile| profile.name == old)
        .ok_or_else(|| Error::new("error.input", "unknown subscription"))?;
    if old == new {
        return Ok(());
    }
    if profiles.profiles.iter().any(|profile| profile.name == new) {
        return Err(Error::new(
            "error.input",
            "subscription name already exists",
        ));
    }
    profiles.profiles[index].name = new.into();
    if profiles.current.as_deref() == Some(old) {
        profiles.current = Some(new.into());
    }
    super::commit_files(
        ctx,
        &[(
            ctx.paths.clash_profiles.clone(),
            Some(storage::Saved::new(
                &super::profiles_bytes(&profiles)?,
                0o600,
            )),
        )],
    )?;
    ctx.message(
        "notice.clash_subscription",
        format!("renamed {old} -> {new}"),
    );
    Ok(())
}

fn default_name(ctx: &Context, value: &str) -> Result<String> {
    super::validate_subscription_url(value)?;
    let url =
        url::Url::parse(value).map_err(|_| Error::new("error.url", "invalid subscription URL"))?;
    let base = url
        .host_str()
        .ok_or_else(|| Error::new("error.url", "subscription host is missing"))?;
    let profiles = super::read_profiles(ctx)?;
    let mut name = base.to_owned();
    let mut suffix = 2;
    while profiles.profiles.iter().any(|profile| profile.name == name) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    super::validate_profile_name(&name)?;
    Ok(name)
}

fn node(ctx: &Context, arguments: &ArgMatches) -> Result<()> {
    match arguments.subcommand() {
        Some(("use", arguments)) => super::controller::select(
            ctx,
            required(arguments, "group"),
            required(arguments, "node"),
        ),
        Some(("list", arguments)) => super::controller::list_proxies(
            ctx,
            arguments.get_one::<String>("group").map(String::as_str),
        ),
        None => {
            let group = arguments.get_one::<String>("group").map(String::as_str);
            if let (Some(group), Some(node)) = (group, arguments.get_one::<String>("node")) {
                super::controller::select(ctx, group, node)
            } else {
                super::controller::list_proxies(ctx, group)
            }
        }
        _ => Err(unsupported("unsupported node command")),
    }
}

fn version(ctx: &Context) -> Result<()> {
    println!(
        "clashctl compatibility / CqShell {}",
        env!("CARGO_PKG_VERSION")
    );
    storage::check_path(&ctx.paths.clash_binary, true)?;
    if ctx.paths.clash_binary.exists() {
        super::require_binary(ctx)?;
        ctx.log(ctx.output(&super::path_string(&ctx.paths.clash_binary)?, &["-v"])?);
    } else {
        ctx.log("Mihomo: not installed");
    }
    Ok(())
}

fn shell_init(ctx: &Context, shell: &str) -> Result<String> {
    let executable = std::env::current_exe()?;
    let quoted = super::shell_quote(shell, &super::path_string(&executable)?);
    let template = match shell {
        "bash" | "zsh" => include_str!("shell-init.sh"),
        "fish" => include_str!("shell-init.fish"),
        _ => return Err(Error::new("error.input", "unsupported shell")),
    };
    let clear = super::proxy_env(ctx, false, shell)?;
    let mut script = template
        .replace("__CQS_EXECUTABLE__", &quoted)
        .replace("__CQS_SHELL__", shell)
        .replace("__CQS_CLEAR_PROXY__", clear.trim_end());
    for (alias, operation) in ALIASES {
        match shell {
            "fish" => script.push_str(&format!(
                "function {alias}\n    clashctl {operation} $argv\nend\n"
            )),
            _ => script.push_str(&format!("{alias}() {{ clashctl {operation} \"$@\"; }}\n")),
        }
    }
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_preserves_profile_paths_metadata_and_current_selection() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        super::super::ensure_layout(&ctx).unwrap();
        fs::write(&ctx.paths.clash_profiles, "use: old\ncustom: keep\nprofiles:\n  - name: old\n    url: https://example.invalid/sub\n    path: original.yaml\n    provider: keep\n").unwrap();
        rename_subscription(&ctx, "old", "new").unwrap();
        let value = super::super::read_profiles(&ctx).unwrap();
        assert_eq!(value.current.as_deref(), Some("new"));
        assert_eq!(value.profiles[0].path, "original.yaml");
        assert_eq!(value.profiles[0].extra["provider"], "keep");
        assert_eq!(value.extra["custom"], "keep");
        let before = fs::read(&ctx.paths.clash_profiles).unwrap();
        assert!(rename_subscription(&ctx, "missing", "new").is_err());
        assert_eq!(fs::read(&ctx.paths.clash_profiles).unwrap(), before);
        assert!(!ctx.paths.clash_data.join("pending.json").exists());
    }

    #[test]
    fn reusing_a_renamed_subscription_name_cannot_overwrite_its_profile() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = crate::tests::context(temp.path());
        super::super::ensure_layout(&ctx).unwrap();
        let directory = ctx.paths.clash_data.join("profiles");
        let previous = super::super::profile_file_name("old");
        fs::write(directory.join(&previous), b"original bytes").unwrap();
        fs::write(&ctx.paths.clash_profiles, format!("profiles:\n  - name: renamed\n    url: https://example.invalid/sub\n    path: {previous}\n")).unwrap();
        let profiles = super::super::read_profiles(&ctx).unwrap();
        let next = super::super::unused_profile_file_name(&directory, &profiles, "old").unwrap();
        assert_ne!(previous, next);
        assert!(!directory.join(&next).exists());
        assert_eq!(
            fs::read(directory.join(previous)).unwrap(),
            b"original bytes"
        );
    }
}

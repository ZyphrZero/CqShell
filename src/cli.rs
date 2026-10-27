use crate::{
    actions::{self, Parameters},
    config::{Paths, Preferences},
    error::{Error, Result},
    i18n::I18n,
    runtime::Context,
};
use clap::{Arg, ArgAction, Command};
use std::{env, io::IsTerminal, path::PathBuf};

fn bootstrap(args: &[String], name: &str) -> Option<String> {
    args.iter().enumerate().find_map(|(i, a)| {
        if a == name {
            args.get(i + 1).cloned()
        } else {
            a.strip_prefix(&format!("{name}=")).map(str::to_owned)
        }
    })
}
pub fn command(i: &I18n) -> Command {
    let mut cmd = Command::new("cqs")
        .version(env!("CARGO_PKG_VERSION"))
        .about(i.text("app.description").to_owned())
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .subcommand_help_heading(i.text("cli.commands").to_owned())
        .next_help_heading(i.text("cli.options").to_owned())
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(i.text("cli.help").to_owned())
                .global(true),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .action(ArgAction::Version)
                .help(i.text("cli.version").to_owned()),
        )
        .arg(
            Arg::new("lang")
                .long("lang")
                .global(true)
                .help(i.text("cli.lang").to_owned()),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .global(true)
                .value_parser(clap::value_parser!(PathBuf))
                .help(i.text("cli.config").to_owned()),
        )
        .arg(
            Arg::new("locale-dir")
                .long("locale-dir")
                .global(true)
                .value_parser(clap::value_parser!(PathBuf))
                .help(i.text("cli.locale_dir").to_owned()),
        )
        .arg(
            Arg::new("yes")
                .short('y')
                .long("yes")
                .global(true)
                .action(ArgAction::SetTrue)
                .help(i.text("cli.yes").to_owned()),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help(i.text("cli.json").to_owned()),
        )
        .arg(
            Arg::new("install")
                .long("install")
                .action(ArgAction::SetTrue)
                .conflicts_with("uninstall")
                .help(i.text("cli.install").to_owned()),
        )
        .arg(
            Arg::new("uninstall")
                .long("uninstall")
                .action(ArgAction::SetTrue)
                .help(i.text("cli.uninstall").to_owned()),
        );
    for name in ["list", "languages"] {
        cmd = cmd.subcommand(Command::new(name).about(i.text(&format!("cli.{name}")).to_owned()));
    }
    cmd = cmd.subcommand(
        Command::new("proxy-env")
            .about(i.text("cli.proxy_env").to_owned())
            .arg(
                Arg::new("state")
                    .required(true)
                    .value_parser(["on", "off"])
                    .help(i.text("cli.proxy_env_state").to_owned()),
            )
            .arg(
                Arg::new("shell")
                    .long("shell")
                    .default_value("bash")
                    .value_parser(["bash", "zsh", "fish"])
                    .help(i.text("cli.proxy_env_shell").to_owned()),
            ),
    );
    for name in ["run", "batch"] {
        cmd = cmd.subcommand(
            Command::new(name)
                .about(i.text(&format!("cli.{name}")).to_owned())
                .help_template(format!(
                    "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
                    i.text("cli.usage")
                ))
                .next_help_heading(i.text("cli.options").to_owned())
                .arg(
                    Arg::new("action")
                        .required(true)
                        .num_args(if name == "batch" {
                            clap::builder::ValueRange::new(1..)
                        } else {
                            clap::builder::ValueRange::new(1)
                        })
                        .help(i.text("cli.action").to_owned()),
                )
                .arg(
                    Arg::new("set")
                        .long("set")
                        .action(ArgAction::Append)
                        .help(i.text("cli.set").to_owned()),
                ),
        );
    }
    cmd.help_template(format!(
        "{{name}} {{version}}\n{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
        i.text("cli.usage")
    ))
}

pub fn parse_parameters(values: impl Iterator<Item = String>) -> Result<Parameters> {
    let mut parameters = Parameters::new();
    for value in values {
        let (key, value) = value
            .split_once('=')
            .ok_or_else(|| Error::new("error.input", &value))?;
        if key.is_empty() || parameters.insert(key.into(), value.into()).is_some() {
            return Err(Error::new("error.input", key));
        }
    }
    Ok(parameters)
}

pub fn main_entry() -> i32 {
    let args: Vec<String> = env::args().collect();
    let result = (|| {
        let paths = Paths::from_env()?;
        let config = bootstrap(&args, "--config").map(PathBuf::from);
        let preferences = Preferences::read(
            config.as_deref().unwrap_or(&paths.preferences),
            config.is_some(),
        )?;
        let language = bootstrap(&args, "--lang")
            .or_else(|| env::var("CQS_LANG").ok())
            .or(preferences.language);
        let directory = bootstrap(&args, "--locale-dir")
            .map(PathBuf::from)
            .or(preferences.locale_dir);
        let i18n = I18n::load(language.as_deref(), directory.as_deref())?;
        Ok::<_, Error>((paths, i18n))
    })();
    let (paths, i18n) = match result {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let matches = match command(&i18n).try_get_matches_from(args) {
        Ok(m) => m,
        Err(e)
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            print!("{e}");
            return 0;
        }
        Err(e) => {
            eprintln!("{}", i18n.error(&Error::new("error.cli", e)));
            return 2;
        }
    };
    let ctx = Context::new(paths, i18n, matches.get_flag("yes"));
    #[cfg(unix)]
    {
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            if let Err(e) = signal_hook::flag::register(signal, ctx.cancel.clone()) {
                eprintln!("{}", ctx.i18n.error(&e.into()));
                return 1;
            }
        }
    }
    let result = dispatch(&ctx, &matches);
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}", ctx.i18n.error(&e));
            if e.key == "error.cancelled" {
                130
            } else if matches!(
                e.key,
                "error.input" | "error.action" | "error.required" | "error.cli"
            ) {
                2
            } else {
                1
            }
        }
    }
}

fn dispatch(ctx: &Context, args: &clap::ArgMatches) -> Result<()> {
    if args.get_flag("install") || args.get_flag("uninstall") {
        if args.subcommand().is_some() {
            return Err(Error::new(
                "error.cli",
                "--install/--uninstall with subcommand",
            ));
        }
        let spec = actions::find(if args.get_flag("install") {
            "app.install"
        } else {
            "app.uninstall"
        })?;
        return actions::execute(ctx, &spec, &Parameters::new());
    }
    if args.get_flag("json") && !matches!(args.subcommand_name(), Some("list" | "languages")) {
        return Err(Error::new("error.cli", "--json requires list/languages"));
    }
    match args.subcommand() {
        Some(("list", _)) => {
            let specs = actions::specs();
            if args.get_flag("json") {
                let rows = specs.iter().map(|s| serde_json::json!({"id":s.id, "group":s.group, "label":ctx.i18n.text(&s.label()), "description":ctx.i18n.text(&s.description()), "root":s.root, "mutates":s.changes, "interactive":s.external, "available":true, "unavailable_reason":null, "parameters":s.fields.iter().map(|f| {
                    let mut parameter = serde_json::json!({"name":f.name,"default":f.default,"choices":f.choices,"required":f.required});
                    if f.allow_custom {
                        parameter["allow_custom"] = true.into();
                    }
                    parameter
                }).collect::<Vec<_>>() })).collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                for spec in specs {
                    println!(
                        "{:<22} {}\n  {}",
                        spec.id,
                        ctx.i18n.text(&spec.label()),
                        ctx.i18n.text(&spec.description())
                    );
                    for field in spec.fields {
                        println!("  {}={} {:?}", field.name, field.default, field.choices);
                    }
                }
            }
            Ok(())
        }
        Some(("languages", _)) => {
            let languages = ctx.i18n.languages();
            if args.get_flag("json") {
                println!("{}", serde_json::to_string_pretty(&languages)?);
            } else {
                for (id, name) in languages {
                    println!("{id}\t{name}");
                }
            }
            Ok(())
        }
        Some(("proxy-env", matches)) => {
            let enabled = matches.get_one::<String>("state").unwrap() == "on";
            let shell = matches.get_one::<String>("shell").unwrap();
            print!("{}", crate::ops::clash::proxy_env(ctx, enabled, shell)?);
            Ok(())
        }
        Some(("run", matches)) => {
            let id = matches.get_one::<String>("action").unwrap();
            let parameters = parse_parameters(
                matches
                    .get_many::<String>("set")
                    .into_iter()
                    .flatten()
                    .cloned(),
            )?;
            actions::execute(ctx, &actions::find(id)?, &parameters)
        }
        Some(("batch", matches)) => {
            let parameters = parse_parameters(
                matches
                    .get_many::<String>("set")
                    .into_iter()
                    .flatten()
                    .cloned(),
            )?;
            let ids = matches
                .get_many::<String>("action")
                .unwrap()
                .collect::<Vec<_>>();
            let mut remaining = parameters.clone();
            let mut jobs = Vec::new();
            for id in ids {
                let spec = actions::find(id)?;
                let mut supplied = Parameters::new();
                let prefix = format!("{id}.");
                for (key, value) in &parameters {
                    if let Some(key_name) = key.strip_prefix(&prefix) {
                        supplied.insert(key_name.into(), value.clone());
                        remaining.remove(key);
                    }
                }
                jobs.push((spec.clone(), spec.parameters(&supplied)?));
            }
            if !remaining.is_empty() {
                return Err(Error::new("error.input", format!("{remaining:?}")));
            }
            actions::batch(ctx, &jobs)
        }
        None => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(Error::new("error.tty", "cqs list / cqs run"));
            }
            crate::tui::run(ctx.clone())
        }
        Some((name, _)) => Err(Error::new("error.cli", name)),
    }
}

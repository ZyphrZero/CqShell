use crate::{
    actions::{self, Parameters, Spec},
    config::{Paths, Preferences},
    error::{Error, Result},
    i18n::I18n,
    runtime::Context,
};
use clap::{Arg, ArgAction, Command};
use std::{env, fs, io::IsTerminal, path::PathBuf};

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
            Arg::new("theme")
                .long("theme")
                .global(true)
                .help(i.text("cli.theme").to_owned()),
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
    for name in ["list", "languages", "themes"] {
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
    cmd = cmd.subcommand(crate::ops::clash::compat::command(i));
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

/// The jobs of `cqs batch <ids> --set <sets>`. Each `--set <id>.<name>=<value>` goes to every
/// listed ID whose `<id>.` prefixes it, and [`Spec::parameters`] fills in the defaults and
/// validates the values. Errors keep the order `cqs batch` reports them in: malformed or
/// repeated `--set` values first, then the IDs in order (unknown ID or invalid parameters),
/// and last the `--set` values no ID claimed, as `error.input`.
pub fn batch_jobs(ids: &[&str], sets: &[&str]) -> Result<Vec<(Spec, Parameters)>> {
    let parameters = parse_parameters(sets.iter().map(|set| (*set).to_owned()))?;
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
        let parameters = spec.parameters(&supplied)?;
        jobs.push((spec, parameters));
    }
    if !remaining.is_empty() {
        return Err(Error::new("error.input", format!("{remaining:?}")));
    }
    Ok(jobs)
}

pub fn main_entry() -> i32 {
    let mut args: Vec<String> = env::args().collect();
    crate::ops::clash::compat::normalize_arguments(&mut args);
    let result = (|| {
        let mut paths = Paths::from_env()?;
        let config = bootstrap(&args, "--config").map(PathBuf::from);
        let preferences_source = config.as_deref().unwrap_or(&paths.preferences);
        let preferences = Preferences::read(preferences_source, config.is_some())?;
        let preferences_path = if let Some(path) = config.as_ref() {
            fs::canonicalize(path)
                .map_err(|e| Error::new("error.config", format!("{}: {e}", path.display())))?
        } else {
            paths.preferences.clone()
        };
        paths.preferences = preferences_path;
        let language = bootstrap(&args, "--lang")
            .or_else(|| env::var("CQS_LANG").ok())
            .or_else(|| preferences.language.clone());
        let directory = bootstrap(&args, "--locale-dir")
            .map(PathBuf::from)
            .or_else(|| preferences.locale_dir.clone());
        let i18n = I18n::load(language.as_deref(), directory.as_deref())?;
        Ok::<_, Error>((paths, i18n, preferences))
    })();
    let (paths, i18n, preferences) = match result {
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
    let result = dispatch(&ctx, &matches, &preferences);
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

fn dispatch(ctx: &Context, args: &clap::ArgMatches, preferences: &Preferences) -> Result<()> {
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
    if args.get_flag("json")
        && !matches!(
            args.subcommand_name(),
            Some("list" | "languages" | "themes")
        )
    {
        return Err(Error::new(
            "error.cli",
            "--json requires list/languages/themes",
        ));
    }
    match args.subcommand() {
        Some(("themes", _)) => themes(ctx, args.get_flag("json")),
        Some(("clashctl", matches)) => crate::ops::clash::compat::dispatch(ctx, matches),
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
            let ids = matches
                .get_many::<String>("action")
                .unwrap()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let sets = matches
                .get_many::<String>("set")
                .into_iter()
                .flatten()
                .map(String::as_str)
                .collect::<Vec<_>>();
            actions::batch(ctx, &batch_jobs(&ids, &sets)?)
        }
        None => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(Error::new("error.tty", "cqs list / cqs run"));
            }
            crate::tui::run(
                ctx.clone(),
                args.get_one::<String>("theme").map(String::as_str),
                preferences,
            )
        }
        Some((name, _)) => Err(Error::new("error.cli", name)),
    }
}

fn themes(ctx: &Context, json: bool) -> Result<()> {
    use crate::theme::{ThemeName, builtin, file};
    let name = |definition: &crate::theme::ThemeDefinition| match &definition.name {
        ThemeName::Builtin(key) => ctx.i18n.text(key).to_owned(),
        ThemeName::Custom(name) => name.clone(),
    };
    let mut rows = builtin::all().into_iter().map(|definition| serde_json::json!({"id": definition.id, "name": name(&definition), "builtin": true, "path": null, "valid": true, "error": null})).collect::<Vec<_>>();
    for custom in file::load_dir(&ctx.paths.themes) {
        let error = custom.result.as_ref().err().map(|error| {
            ctx.i18n
                .format("ui.theme_error", &[("detail", &error.to_string())])
        });
        rows.push(serde_json::json!({"id": custom.id, "name": custom.result.as_ref().map(&name).unwrap_or_else(|_| custom.id.clone()), "builtin": false, "path": custom.path, "valid": error.is_none(), "error": error}));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in rows {
            if row["valid"] == true {
                println!(
                    "{}\t{}",
                    row["id"].as_str().unwrap(),
                    row["name"].as_str().unwrap()
                );
            } else {
                eprintln!(
                    "{}",
                    row["error"].as_str().unwrap().replace(['\n', '\r'], " ")
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cqs batch <ids> --set <sets>…` through [`dispatch`]. Without `--yes` and without a
    /// terminal the batch confirmation fails, so no job runs even when every argument is valid.
    fn cqs_batch(ids: &[&str], sets: &[&str]) -> Result<()> {
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = crate::tests::context(temp.path());
        ctx.yes = false;
        ctx.interactive = false;
        let args = ["cqs", "batch"]
            .into_iter()
            .chain(ids.iter().copied())
            .chain(sets.iter().flat_map(|set| ["--set", *set]));
        let matches = command(&ctx.i18n).try_get_matches_from(args).unwrap();
        dispatch(&ctx, &matches, &Preferences::default())
    }

    #[test]
    fn batch_jobs_assigns_prefixed_values_and_fills_in_defaults() {
        let ids = ["bbr.enable", "tcp.tune", "terminal.status"];
        let sets = ["tcp.tune.rtt=80", "bbr.enable.qdisc=fq_codel"];
        let jobs = batch_jobs(&ids, &sets)
            .unwrap()
            .into_iter()
            .map(|(spec, parameters)| (spec.id, parameters))
            .collect::<Vec<_>>();
        // Jobs follow the ID order, not the --set order; an action without fields gets none.
        assert_eq!(
            jobs,
            [
                (
                    "bbr.enable",
                    Parameters::from([("qdisc".into(), "fq_codel".into())])
                ),
                (
                    "tcp.tune",
                    Parameters::from([
                        ("bandwidth".into(), "1000".into()),
                        ("rtt".into(), "80".into()),
                    ])
                ),
                ("terminal.status", Parameters::new()),
            ]
        );
        let defaults = batch_jobs(&["tcp.tune"], &[]).unwrap();
        assert_eq!(defaults[0].1, defaults[0].0.defaults());
        // `cqs batch` accepts the same arguments and stops only at the batch confirmation.
        assert_eq!(cqs_batch(&ids, &sets).unwrap_err().key, "error.confirm");
    }

    #[test]
    fn batch_jobs_reports_the_errors_cqs_batch_reported_before_the_extraction() {
        // Keys and details recorded from `cqs batch` before batch_jobs left dispatch.
        let cases: &[(&[&str], &[&str], &str, &str)] = &[
            // Malformed and repeated --set values come first, before any ID is looked up.
            (
                &["tcp.tune"],
                &["tcp.tune.rtt"],
                "error.input",
                "tcp.tune.rtt",
            ),
            (&["tcp.tune"], &["=80"], "error.input", ""),
            (
                &["tcp.tune"],
                &["tcp.tune.rtt=80", "tcp.tune.rtt=90"],
                "error.input",
                "tcp.tune.rtt",
            ),
            (&["nope"], &["x"], "error.input", "x"),
            // Then each ID in order: unknown IDs and invalid parameters.
            (&["tcp.tune", "nope"], &[], "error.action", "nope"),
            (
                &["tcp.tune"],
                &["tcp.tune.nope=1"],
                "error.input",
                "tcp.tune: nope",
            ),
            (&["tcp.tune"], &["tcp.tune.=1"], "error.input", "tcp.tune: "),
            (
                &["bbr.enable"],
                &["bbr.enable.qdisc=invalid"],
                "error.input",
                "qdisc=invalid",
            ),
            (&["tcp.tune"], &["tcp.tune.rtt=abc"], "error.input", "abc"),
            (&["ssh.key"], &[], "error.required", "public_key"),
            (
                &["bbr.enable", "nope"],
                &["bbr.enable.qdisc=invalid"],
                "error.input",
                "qdisc=invalid",
            ),
            (
                &["nope", "bbr.enable"],
                &["bbr.enable.qdisc=invalid"],
                "error.action",
                "nope",
            ),
            // Last, the values no ID claimed.
            (
                &["tcp.tune"],
                &["bbr.enable.qdisc=fq"],
                "error.input",
                r#"{"bbr.enable.qdisc": "fq"}"#,
            ),
            (
                &["tcp.tune"],
                &["bbr.enable.qdisc=fq", "x=1"],
                "error.input",
                r#"{"bbr.enable.qdisc": "fq", "x": "1"}"#,
            ),
            (&["nope"], &["other.a=1"], "error.action", "nope"),
            (
                &["bbr.enable"],
                &["bbr.enable.qdisc=invalid", "other.a=1"],
                "error.input",
                "qdisc=invalid",
            ),
        ];
        for (ids, sets, key, detail) in cases {
            for (via, error) in [
                ("batch_jobs", batch_jobs(ids, sets).unwrap_err()),
                ("cqs batch", cqs_batch(ids, sets).unwrap_err()),
            ] {
                assert_eq!(
                    (error.key, error.detail.as_str()),
                    (*key, *detail),
                    "{via} {ids:?} --set {sets:?}"
                );
            }
        }
    }
}

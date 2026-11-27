use crate::{
    actions::{self, Parameters, Spec},
    api::{self, ExecutionPolicy},
    config::{Paths, Preferences},
    error::{Error, Result},
    i18n::I18n,
    mcp::McpServer,
    runtime::Context,
};
use clap::{Arg, ArgAction, Command};
use std::{
    env, fs,
    io::{IsTerminal, Read},
    path::PathBuf,
};

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
            Arg::new("style")
                .long("style")
                .global(true)
                .help(i.text("cli.style").to_owned()),
        )
        .arg(
            Arg::new("gh-proxy")
                .long("gh-proxy")
                .global(true)
                .value_name("URL|off")
                .help(i.text("cli.gh_proxy").to_owned()),
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
    for name in ["list", "languages", "themes", "styles"] {
        cmd = cmd.subcommand(Command::new(name).about(i.text(&format!("cli.{name}")).to_owned()));
    }
    cmd = cmd.subcommand(
        Command::new("update")
            .alias("self-update")
            .about(i.text("cli.update").to_owned())
            .help_template(format!(
                "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
                i.text("cli.usage")
            ))
            .next_help_heading(i.text("cli.options").to_owned())
            .arg(
                Arg::new("check")
                    .long("check")
                    .action(ArgAction::SetTrue)
                    .conflicts_with("force")
                    .help(i.text("cli.update_check").to_owned()),
            )
            .arg(
                Arg::new("target-version")
                    .long("version")
                    .value_name("TAG")
                    .help(i.text("cli.update_version").to_owned()),
            )
            .arg(
                Arg::new("force")
                    .long("force")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.update_force").to_owned()),
            ),
    );
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
    cmd = cmd.subcommand(
        Command::new("docker")
            .about(i.text("cli.docker").to_owned())
            .help_template(format!(
                "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
                i.text("cli.usage")
            ))
            .next_help_heading(i.text("cli.options").to_owned())
            .arg(
                Arg::new("tab")
                    .long("tab")
                    .default_value("containers")
                    .value_parser([
                        "containers",
                        "compose",
                        "images",
                        "volumes",
                        "networks",
                        "daemon",
                    ])
                    .help(i.text("cli.docker_tab").to_owned()),
            )
            .arg(
                Arg::new("embedded")
                    .long("embedded")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.docker_embedded").to_owned()),
            ),
    );
    cmd = cmd.subcommand(
        Command::new("mihomo")
            .about(i.text("cli.mihomo").to_owned())
            .help_template(format!(
                "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
                i.text("cli.usage")
            ))
            .next_help_heading(i.text("cli.options").to_owned())
            .arg(
                Arg::new("tab")
                    .long("tab")
                    .default_value("overview")
                    .value_parser([
                        "overview",
                        "proxies",
                        "profiles",
                        "connections",
                        "rules",
                        "tools",
                    ])
                    .help(i.text("cli.mihomo_tab").to_owned()),
            )
            .arg(
                Arg::new("embedded")
                    .long("embedded")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.mihomo_embedded").to_owned()),
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
                        .conflicts_with("params-json")
                        .help(i.text("cli.set").to_owned()),
                )
                .arg(
                    Arg::new("params-json")
                        .long("params-json")
                        .value_name("JSON|-")
                        .help(i.text("cli.params_json").to_owned()),
                )
                .arg(
                    Arg::new("dry-run")
                        .long("dry-run")
                        .action(ArgAction::SetTrue)
                        .help(i.text("cli.dry_run").to_owned()),
                ),
        );
    }
    cmd = cmd.subcommand(
        Command::new("status")
            .about(i.text("cli.status").to_owned())
            .help_template(format!(
                "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
                i.text("cli.usage")
            ))
            .next_help_heading(i.text("cli.options").to_owned())
            .arg(
                Arg::new("section")
                    .long("section")
                    .default_value("system")
                    .value_parser(["system", "docker", "mihomo", "all"])
                    .help(i.text("cli.status_section").to_owned()),
            )
            .arg(
                Arg::new("public-ip")
                    .long("public-ip")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.status_public_ip").to_owned()),
            )
            .arg(
                Arg::new("sample-cpu")
                    .long("sample-cpu")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.status_sample_cpu").to_owned()),
            )
            .arg(
                Arg::new("include-stats")
                    .long("include-stats")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.status_include_stats").to_owned()),
            )
            .arg(
                Arg::new("include-rules")
                    .long("include-rules")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.status_include_rules").to_owned()),
            )
            .arg(
                Arg::new("include-providers")
                    .long("include-providers")
                    .action(ArgAction::SetTrue)
                    .help(i.text("cli.status_include_providers").to_owned()),
            ),
    );
    for (name, about_key) in [("describe", "cli.describe"), ("choices", "cli.choices")] {
        cmd = cmd.subcommand(
            Command::new(name)
                .about(i.text(about_key).to_owned())
                .help_template(format!(
                    "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
                    i.text("cli.usage")
                ))
                .next_help_heading(i.text("cli.options").to_owned())
                .arg(
                    Arg::new("action")
                        .required(true)
                        .help(i.text("cli.action").to_owned()),
                ),
        );
    }
    cmd = cmd.subcommand(mcp_command(i));
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
    let github_proxy = match crate::github_proxy::resolve(
        matches.get_one::<String>("gh-proxy").map(String::as_str),
        env::var("CQS_GH_PROXY").ok().as_deref(),
        env::var("GH_PROXY").ok().as_deref(),
        &preferences,
    ) {
        Ok(proxy) => proxy,
        Err(e) => {
            eprintln!("{}", i18n.error(&e));
            return 2;
        }
    };
    let mut ctx = Context::new(paths, i18n, matches.get_flag("yes"));
    ctx.github_proxy = github_proxy;
    #[cfg(unix)]
    {
        for signal in [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
            signal_hook::consts::SIGQUIT,
        ] {
            if let Err(e) = signal_hook::flag::register(signal, ctx.cancel.clone()) {
                eprintln!("{}", ctx.i18n.error(&e.into()));
                return 1;
            }
        }
    }
    let structured_json_subcommand = matches.get_flag("json")
        && matches!(
            matches.subcommand(),
            Some((
                "status" | "describe" | "choices" | "run" | "batch" | "mcp",
                _
            ))
        );
    let result = dispatch(&ctx, &matches, &preferences);
    match result {
        Ok(code) => code,
        Err(e) => {
            if structured_json_subcommand {
                let envelope = api::error_envelope(&ctx, &e);
                if let Ok(text) = serde_json::to_string_pretty(&envelope) {
                    println!("{text}");
                } else {
                    eprintln!("{}", ctx.i18n.error(&e));
                }
            } else {
                eprintln!("{}", ctx.i18n.error(&e));
            }
            api::exit_code(&e)
        }
    }
}

fn read_params_json(raw: &str) -> Result<serde_json::Value> {
    const MAX_JSON_BYTES: u64 = 1024 * 1024;
    let text = if raw == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .take(MAX_JSON_BYTES + 1)
            .read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_JSON_BYTES {
            return Err(Error::new(
                "error.input",
                "params-json stdin exceeds 1 MiB limit",
            ));
        }
        String::from_utf8(buf).map_err(|e| Error::new("error.input", e))?
    } else {
        raw.to_owned()
    };
    serde_json::from_str(&text).map_err(|e| Error::new("error.input", format!("params-json: {e}")))
}

fn dispatch(ctx: &Context, args: &clap::ArgMatches, preferences: &Preferences) -> Result<i32> {
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
        actions::execute(ctx, &spec, &Parameters::new())?;
        return Ok(0);
    }
    if args.get_flag("json") {
        let json_allowed = match args.subcommand() {
            Some((
                "list" | "languages" | "themes" | "styles" | "status" | "describe" | "choices"
                | "run" | "batch",
                _,
            )) => true,
            Some(("update", matches)) => matches.get_flag("check"),
            Some(("mcp", matches)) => matches!(
                matches.subcommand_name(),
                Some("config" | "install" | "uninstall" | "token")
            ),
            _ => false,
        };
        if !json_allowed {
            return Err(Error::new(
                "error.cli",
                "--json requires list/languages/themes/styles/status/describe/choices/run/batch, update --check, or mcp config/install/uninstall/token",
            ));
        }
    }
    match args.subcommand() {
        Some(("themes", _)) => {
            themes(ctx, args.get_flag("json"))?;
            Ok(0)
        }
        Some(("styles", _)) => {
            styles(ctx, args.get_flag("json"))?;
            Ok(0)
        }
        Some(("update", matches)) => {
            let check_only = matches.get_flag("check");
            let version = matches.get_one::<String>("target-version").cloned();
            let force = matches.get_flag("force");
            if check_only {
                crate::ops::self_update::check_and_report(
                    ctx,
                    version.as_deref(),
                    args.get_flag("json"),
                )?;
            } else {
                crate::ops::self_update::update(
                    ctx,
                    &crate::ops::self_update::UpdateOptions {
                        version,
                        force,
                        target_path: None,
                    },
                )?;
            }
            Ok(0)
        }
        Some(("clashctl", matches)) => {
            crate::ops::clash::compat::dispatch(ctx, matches)?;
            Ok(0)
        }
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
            Ok(0)
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
            Ok(0)
        }
        Some(("proxy-env", matches)) => {
            let enabled = matches.get_one::<String>("state").unwrap() == "on";
            let shell = matches.get_one::<String>("shell").unwrap();
            print!("{}", crate::ops::clash::proxy_env(ctx, enabled, shell)?);
            Ok(0)
        }
        Some(("status", matches)) => {
            let section = matches.get_one::<String>("section").unwrap().as_str();
            let public_ip = matches.get_flag("public-ip");
            let sample_cpu = matches.get_flag("sample-cpu");
            let include_stats = matches.get_flag("include-stats");
            let include_rules = matches.get_flag("include-rules");
            let include_providers = matches.get_flag("include-providers");
            let json_mode = args.get_flag("json");
            if json_mode || section != "system" {
                let payload = match section {
                    "system" => api::snapshots::system_status_json(ctx, public_ip, sample_cpu),
                    "docker" => api::snapshots::docker_snapshot_json(ctx, "all", include_stats)?,
                    "mihomo" => api::snapshots::mihomo_snapshot_json(
                        ctx,
                        include_rules,
                        include_providers,
                        None,
                        None,
                    ),
                    "all" => serde_json::json!({
                        "schema_version": api::SCHEMA_VERSION,
                        "ok": true,
                        "system": api::snapshots::system_status_json(ctx, public_ip, sample_cpu),
                        "docker": api::snapshots::docker_snapshot_json(ctx, "all", include_stats)?,
                        "mihomo": api::snapshots::mihomo_snapshot_json(
                            ctx,
                            include_rules,
                            include_providers,
                            None,
                            None,
                        ),
                    }),
                    other => return Err(Error::new("error.input", other)),
                };
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!(
                    "{}",
                    api::snapshots::system_status_text(ctx, public_ip, sample_cpu)
                );
            }
            Ok(0)
        }
        Some(("describe", matches)) => {
            let id = matches.get_one::<String>("action").unwrap();
            let spec = actions::find_ref(id)?;
            let detail = api::action_detail_json(ctx, spec);
            if args.get_flag("json") {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                let a = &detail["action"];
                println!(
                    "{} ({})  {}",
                    a["id"].as_str().unwrap_or(""),
                    a["group"].as_str().unwrap_or(""),
                    a["label"].as_str().unwrap_or("")
                );
                println!("  {}", a["description"].as_str().unwrap_or(""));
                println!(
                    "  risk={} mutates={} root={} reversible={} non_interactive={}",
                    a["risk"].as_str().unwrap_or(""),
                    a["mutates"].as_bool().unwrap_or(false),
                    a["root"].as_bool().unwrap_or(false),
                    a["reversible"].as_bool().unwrap_or(false),
                    a["non_interactive_supported"].as_bool().unwrap_or(false),
                );
                if let Some(params) = a["parameters"].as_array() {
                    for p in params {
                        println!(
                            "  param {} (required={}, default={:?}, choices={})",
                            p["name"].as_str().unwrap_or(""),
                            p["required"].as_bool().unwrap_or(false),
                            p["default"].as_str().unwrap_or(""),
                            p["choices"]
                        );
                    }
                }
            }
            Ok(0)
        }
        Some(("choices", matches)) => {
            let id = matches.get_one::<String>("action").unwrap();
            let payload = api::action_choices_json(ctx, id)?;
            if args.get_flag("json") {
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if let Some(fields) = payload["fields"].as_array() {
                if fields.is_empty() {
                    println!("{id}: no enumerated parameter choices");
                }
                for f in fields {
                    println!("{}:", f["field"].as_str().unwrap_or(""));
                    if let Some(err) = f
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        println!("  error: {err}");
                        continue;
                    }
                    if let Some(items) = f["choices"].as_array() {
                        for item in items {
                            let mark = if item["current"].as_bool().unwrap_or(false) {
                                "*"
                            } else {
                                "-"
                            };
                            let val = item["value"].as_str().unwrap_or("");
                            let detail = item["detail"].as_str().unwrap_or("");
                            if detail.is_empty() {
                                println!("  {mark} {val}");
                            } else {
                                println!("  {mark} {val} ({detail})");
                            }
                            if let Some(children) = item["children"].as_array() {
                                for child in children {
                                    let cmark = if child["current"].as_bool().unwrap_or(false) {
                                        "*"
                                    } else {
                                        "-"
                                    };
                                    println!(
                                        "      {cmark} {}",
                                        child["value"].as_str().unwrap_or("")
                                    );
                                }
                            }
                        }
                    } else if let Some(note) = f["note"].as_str() {
                        println!("  {note}");
                    }
                }
            }
            Ok(0)
        }
        Some(("mcp", matches)) => dispatch_mcp(ctx, matches, args.get_flag("json")),
        Some(("run", matches)) => {
            let id = matches.get_one::<String>("action").unwrap();
            let dry_run = matches.get_flag("dry-run");
            let json_mode = args.get_flag("json");
            let params_json = matches.get_one::<String>("params-json");
            let parameters = if let Some(raw) = params_json {
                let spec = actions::find(id)?;
                let value = read_params_json(raw)?;
                api::parameters_from_json(&spec, &value)?
            } else {
                parse_parameters(
                    matches
                        .get_many::<String>("set")
                        .into_iter()
                        .flatten()
                        .cloned(),
                )?
            };
            if json_mode {
                let (payload, code) = api::execute_action_json(
                    ctx,
                    &ExecutionPolicy::permissive(),
                    id,
                    &parameters,
                    ctx.yes,
                    dry_run,
                );
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(code);
            }
            if dry_run {
                let (payload, code) = api::execute_action_json(
                    ctx,
                    &ExecutionPolicy::permissive(),
                    id,
                    &parameters,
                    ctx.yes,
                    true,
                );
                println!("{}", api::format_dry_run_text(ctx, &payload));
                return Ok(code);
            }
            actions::execute(ctx, &actions::find(id)?, &parameters)?;
            Ok(0)
        }
        Some(("batch", matches)) => {
            let ids = matches
                .get_many::<String>("action")
                .unwrap()
                .map(String::as_str)
                .collect::<Vec<_>>();
            let dry_run = matches.get_flag("dry-run");
            let json_mode = args.get_flag("json");
            let jobs = if let Some(raw) = matches.get_one::<String>("params-json") {
                let value = read_params_json(raw)?;
                api::batch_jobs_from_json(&ids, &value)?
            } else {
                let sets = matches
                    .get_many::<String>("set")
                    .into_iter()
                    .flatten()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                batch_jobs(&ids, &sets)?
            };
            if json_mode {
                let (payload, code) = api::execute_batch_json(
                    ctx,
                    &ExecutionPolicy::permissive(),
                    &jobs,
                    ctx.yes,
                    dry_run,
                );
                println!("{}", serde_json::to_string_pretty(&payload)?);
                return Ok(code);
            }
            if dry_run {
                let (payload, code) = api::execute_batch_json(
                    ctx,
                    &ExecutionPolicy::permissive(),
                    &jobs,
                    ctx.yes,
                    true,
                );
                println!("{}", api::format_dry_run_text(ctx, &payload));
                return Ok(code);
            }
            actions::batch(ctx, &jobs)?;
            Ok(0)
        }
        Some(("docker", matches)) => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(Error::new("error.tty", "cqs list / cqs run"));
            }
            let tab = matches
                .get_one::<String>("tab")
                .and_then(|s| crate::ops::docker::model::DockerTab::from_cli(s))
                .unwrap_or(crate::ops::docker::model::DockerTab::Containers);
            let fullscreen = !matches.get_flag("embedded");
            crate::tui::run_with_target(
                ctx.clone(),
                args.get_one::<String>("theme").map(String::as_str),
                args.get_one::<String>("style").map(String::as_str),
                preferences,
                crate::tui::LaunchTarget::Docker { tab, fullscreen },
            )?;
            Ok(0)
        }
        Some(("mihomo", matches)) => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(Error::new("error.tty", "cqs list / cqs run"));
            }
            let tab = matches
                .get_one::<String>("tab")
                .and_then(|s| crate::ops::clash::model::ClashTab::from_cli(s))
                .unwrap_or(crate::ops::clash::model::ClashTab::Overview);
            let fullscreen = !matches.get_flag("embedded");
            crate::tui::run_with_target(
                ctx.clone(),
                args.get_one::<String>("theme").map(String::as_str),
                args.get_one::<String>("style").map(String::as_str),
                preferences,
                crate::tui::LaunchTarget::Clash { tab, fullscreen },
            )?;
            Ok(0)
        }
        None => {
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                return Err(Error::new("error.tty", "cqs list / cqs run"));
            }
            crate::tui::run(
                ctx.clone(),
                args.get_one::<String>("theme").map(String::as_str),
                args.get_one::<String>("style").map(String::as_str),
                preferences,
            )?;
            Ok(0)
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
    let mut rows = builtin::all()
        .into_iter()
        .map(|definition| {
            serde_json::json!({
                "id": definition.id,
                "name": name(&definition),
                "default_style": definition.default_style.id(),
                "builtin": true,
                "path": null,
                "valid": true,
                "error": null
            })
        })
        .collect::<Vec<_>>();
    for custom in file::load_dir(&ctx.paths.themes) {
        let error = custom.result.as_ref().err().map(|error| {
            ctx.i18n
                .format("ui.theme_error", &[("detail", &error.to_string())])
        });
        let default_style = custom
            .result
            .as_ref()
            .ok()
            .map(|def| def.default_style.id());
        rows.push(serde_json::json!({
            "id": custom.id,
            "name": custom.result.as_ref().map(name).unwrap_or_else(|_| custom.id.clone()),
            "default_style": default_style,
            "builtin": false,
            "path": custom.path,
            "valid": error.is_none(),
            "error": error
        }));
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

fn styles(ctx: &Context, json: bool) -> Result<()> {
    use crate::theme::UiStyle;
    let mut rows = vec![serde_json::json!({
        "id": "auto",
        "name": ctx.i18n.text("style.auto.name"),
        "description": ctx.i18n.text("style.auto.description"),
        "builtin": true
    })];
    for style in UiStyle::ALL {
        rows.push(serde_json::json!({
            "id": style.id(),
            "name": ctx.i18n.text(style.name_key()),
            "description": ctx.i18n.text(style.description_key()),
            "builtin": true
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in rows {
            println!(
                "{}\t{}",
                row["id"].as_str().unwrap(),
                row["name"].as_str().unwrap()
            );
        }
    }
    Ok(())
}

fn mcp_command(i: &I18n) -> Command {
    let tmpl = format!(
        "{{about}}\n\n{}: {{usage}}\n\n{{all-args}}",
        i.text("cli.usage")
    );
    Command::new("mcp")
        .about(i.text("cli.mcp").to_owned())
        .help_template(tmpl.clone())
        .subcommand_help_heading(i.text("cli.commands").to_owned())
        .next_help_heading(i.text("cli.options").to_owned())
        .arg(
            Arg::new("read-only")
                .long("read-only")
                .action(ArgAction::SetTrue)
                .conflicts_with("allow-danger")
                .help(i.text("cli.mcp_read_only").to_owned()),
        )
        .arg(
            Arg::new("allow-danger")
                .long("allow-danger")
                .action(ArgAction::SetTrue)
                .help(i.text("cli.mcp_allow_danger").to_owned()),
        )
        .arg(
            Arg::new("allow-group")
                .long("allow-group")
                .value_name("GROUP")
                .value_delimiter(',')
                .action(ArgAction::Append)
                .help(i.text("cli.mcp_allow_group").to_owned()),
        )
        .arg(
            Arg::new("deny-action")
                .long("deny-action")
                .value_name("ACTION_ID")
                .value_delimiter(',')
                .action(ArgAction::Append)
                .help(i.text("cli.mcp_deny_action").to_owned()),
        )
        .subcommand(
            Command::new("serve")
                .about(i.text("cli.mcp_serve").to_owned())
                .help_template(tmpl.clone())
                .next_help_heading(i.text("cli.options").to_owned())
                .arg(
                    Arg::new("listen")
                        .long("listen")
                        .value_name("IP")
                        .help(i.text("cli.mcp_listen").to_owned()),
                )
                .arg(
                    Arg::new("port")
                        .long("port")
                        .value_name("PORT")
                        .help(i.text("cli.mcp_port").to_owned()),
                )
                .arg(
                    Arg::new("path")
                        .long("path")
                        .value_name("PATH")
                        .help(i.text("cli.mcp_path").to_owned()),
                )
                .arg(
                    Arg::new("no-auth")
                        .long("no-auth")
                        .action(ArgAction::SetTrue)
                        .help(i.text("cli.mcp_no_auth").to_owned()),
                )
                .arg(
                    Arg::new("read-only")
                        .long("read-only")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["standard", "allow-danger"])
                        .help(i.text("cli.mcp_read_only").to_owned()),
                )
                .arg(
                    Arg::new("standard")
                        .long("standard")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["read-only", "allow-danger"])
                        .help(i.text("cli.mcp_standard").to_owned()),
                )
                .arg(
                    Arg::new("allow-danger")
                        .long("allow-danger")
                        .action(ArgAction::SetTrue)
                        .conflicts_with_all(["read-only", "standard"])
                        .help(i.text("cli.mcp_allow_danger").to_owned()),
                )
                .arg(
                    Arg::new("allow-group")
                        .long("allow-group")
                        .value_name("GROUP")
                        .value_delimiter(',')
                        .action(ArgAction::Append)
                        .help(i.text("cli.mcp_allow_group").to_owned()),
                )
                .arg(
                    Arg::new("deny-action")
                        .long("deny-action")
                        .value_name("ACTION_ID")
                        .value_delimiter(',')
                        .action(ArgAction::Append)
                        .help(i.text("cli.mcp_deny_action").to_owned()),
                ),
        )
        .subcommand(
            Command::new("config")
                .about(i.text("cli.mcp_config").to_owned())
                .help_template(tmpl.clone())
                .next_help_heading(i.text("cli.options").to_owned())
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .default_value("http")
                        .value_parser(["http", "ssh", "ssh_stdio", "local", "local_stdio"])
                        .help(i.text("cli.mcp_transport").to_owned()),
                )
                .arg(
                    Arg::new("client")
                        .long("client")
                        .default_value("json")
                        .value_parser(["json", "claude", "codex", "cursor"])
                        .help(i.text("cli.mcp_client").to_owned()),
                )
                .arg(
                    Arg::new("policy")
                        .long("policy")
                        .default_value("current")
                        .value_parser([
                            "current",
                            "read_only",
                            "read-only",
                            "standard",
                            "allow_danger",
                            "allow-danger",
                        ])
                        .help(i.text("cli.mcp_policy").to_owned()),
                )
                .arg(
                    Arg::new("host")
                        .long("host")
                        .value_name("HOST")
                        .help(i.text("cli.mcp_host").to_owned()),
                )
                .arg(
                    Arg::new("ssh-target")
                        .long("ssh-target")
                        .value_name("USER@HOST")
                        .help(i.text("cli.mcp_ssh_target").to_owned()),
                )
                .arg(
                    Arg::new("public-url")
                        .long("public-url")
                        .value_name("URL")
                        .help(i.text("cli.mcp_public_url").to_owned()),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .value_name("NAME")
                        .help(i.text("cli.mcp_name").to_owned()),
                )
                .arg(
                    Arg::new("reveal")
                        .long("reveal")
                        .action(ArgAction::SetTrue)
                        .help(i.text("cli.mcp_reveal").to_owned()),
                ),
        )
        .subcommand(
            Command::new("install")
                .about(i.text("cli.mcp_install").to_owned())
                .help_template(tmpl.clone())
                .next_help_heading(i.text("cli.options").to_owned())
                .arg(
                    Arg::new("client")
                        .long("client")
                        .default_value("claude")
                        .value_parser(["claude", "codex", "cursor", "file"])
                        .help(i.text("cli.mcp_client").to_owned()),
                )
                .arg(
                    Arg::new("file")
                        .long("file")
                        .value_name("PATH")
                        .help(i.text("cli.mcp_file").to_owned()),
                )
                .arg(
                    Arg::new("transport")
                        .long("transport")
                        .default_value("local_stdio")
                        .value_parser(["local", "local_stdio", "http", "ssh", "ssh_stdio"])
                        .help(i.text("cli.mcp_transport").to_owned()),
                )
                .arg(
                    Arg::new("policy")
                        .long("policy")
                        .default_value("read_only")
                        .value_parser([
                            "read_only",
                            "read-only",
                            "standard",
                            "allow_danger",
                            "allow-danger",
                        ])
                        .help(i.text("cli.mcp_policy").to_owned()),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .default_value(crate::mcp::config::DEFAULT_SERVER_NAME)
                        .value_name("NAME")
                        .help(i.text("cli.mcp_name").to_owned()),
                )
                .arg(
                    Arg::new("ssh-target")
                        .long("ssh-target")
                        .value_name("USER@HOST")
                        .help(i.text("cli.mcp_ssh_target").to_owned()),
                )
                .arg(
                    Arg::new("public-url")
                        .long("public-url")
                        .value_name("URL")
                        .help(i.text("cli.mcp_public_url").to_owned()),
                ),
        )
        .subcommand(
            Command::new("uninstall")
                .about(i.text("cli.mcp_uninstall").to_owned())
                .help_template(tmpl.clone())
                .next_help_heading(i.text("cli.options").to_owned())
                .arg(
                    Arg::new("client")
                        .long("client")
                        .default_value("claude")
                        .value_parser(["claude", "codex", "cursor", "file"])
                        .help(i.text("cli.mcp_client").to_owned()),
                )
                .arg(
                    Arg::new("file")
                        .long("file")
                        .value_name("PATH")
                        .help(i.text("cli.mcp_file").to_owned()),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .default_value(crate::mcp::config::DEFAULT_SERVER_NAME)
                        .value_name("NAME")
                        .help(i.text("cli.mcp_name").to_owned()),
                ),
        )
        .subcommand(
            Command::new("token")
                .about(i.text("cli.mcp_token").to_owned())
                .help_template(tmpl.clone())
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("status")
                        .about(i.text("cli.mcp_token_status").to_owned())
                        .help_template(tmpl.clone()),
                )
                .subcommand(
                    Command::new("init")
                        .about(i.text("cli.mcp_token_init").to_owned())
                        .help_template(tmpl.clone())
                        .arg(
                            Arg::new("reveal")
                                .long("reveal")
                                .action(ArgAction::SetTrue)
                                .help(i.text("cli.mcp_reveal").to_owned()),
                        ),
                )
                .subcommand(
                    Command::new("rotate")
                        .about(i.text("cli.mcp_token_rotate").to_owned())
                        .help_template(tmpl.clone())
                        .arg(
                            Arg::new("reveal")
                                .long("reveal")
                                .action(ArgAction::SetTrue)
                                .help(i.text("cli.mcp_reveal").to_owned()),
                        ),
                )
                .subcommand(
                    Command::new("set")
                        .about(i.text("cli.mcp_token_set").to_owned())
                        .help_template(tmpl.clone())
                        .arg(
                            Arg::new("file")
                                .long("file")
                                .value_name("PATH")
                                .conflicts_with("stdin")
                                .help(i.text("cli.mcp_token_file").to_owned()),
                        )
                        .arg(
                            Arg::new("stdin")
                                .long("stdin")
                                .action(ArgAction::SetTrue)
                                .conflicts_with("file")
                                .help(i.text("cli.mcp_token_stdin").to_owned()),
                        ),
                ),
        )
        .subcommand(
            Command::new("service")
                .about(i.text("cli.mcp_service").to_owned())
                .help_template(tmpl.clone())
                .subcommand_required(true)
                .arg_required_else_help(true)
                .arg(
                    Arg::new("scope")
                        .long("scope")
                        .global(true)
                        .default_value("user")
                        .value_parser(["user", "system"])
                        .help(i.text("field.service_scope").to_owned()),
                )
                .subcommand(
                    Command::new("status").about(i.text("action.ai.mcp.service.status").to_owned()),
                )
                .subcommand(
                    Command::new("install")
                        .about(i.text("action.ai.mcp.service.install").to_owned()),
                )
                .subcommand(Command::new("start").about(i.text("cli.mcp_service_start").to_owned()))
                .subcommand(Command::new("stop").about(i.text("cli.mcp_service_stop").to_owned()))
                .subcommand(
                    Command::new("restart").about(i.text("cli.mcp_service_restart").to_owned()),
                )
                .subcommand(
                    Command::new("remove").about(i.text("action.ai.mcp.service.remove").to_owned()),
                ),
        )
}

fn dispatch_mcp(ctx: &Context, matches: &clap::ArgMatches, json_mode: bool) -> Result<i32> {
    use crate::mcp::{
        config as mcp_cfg, http as mcp_http, install as mcp_inst, service as mcp_svc,
    };

    match matches.subcommand() {
        None => {
            let mut policy = ExecutionPolicy::mcp_default();
            policy.read_only = matches.get_flag("read-only");
            policy.allow_danger = matches.get_flag("allow-danger");
            policy.allow_groups = matches
                .get_many::<String>("allow-group")
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            policy.deny_actions = matches
                .get_many::<String>("deny-action")
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            McpServer::serve_stdio(ctx, policy)?;
            Ok(0)
        }
        Some(("serve", sub)) => {
            let mut cfg = mcp_cfg::load_config(ctx)?;
            if let Some(listen) = sub.get_one::<String>("listen") {
                let (norm, _) = mcp_cfg::normalize_listen(listen)?;
                cfg.listen = norm;
            }
            if let Some(port) = sub.get_one::<String>("port") {
                cfg.port = mcp_cfg::validate_port(port)?;
            }
            if let Some(path) = sub.get_one::<String>("path") {
                cfg.endpoint_path = mcp_cfg::validate_endpoint_path(path)?;
            }
            if sub.get_flag("no-auth") {
                cfg.auth_enabled = false;
            }
            if sub.get_flag("read-only") {
                cfg.policy_mode = mcp_cfg::PolicyMode::ReadOnly;
            } else if sub.get_flag("standard") {
                cfg.policy_mode = mcp_cfg::PolicyMode::Standard;
            } else if sub.get_flag("allow-danger") {
                cfg.policy_mode = mcp_cfg::PolicyMode::AllowDanger;
            }
            let cli_groups: Vec<String> = sub
                .get_many::<String>("allow-group")
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if !cli_groups.is_empty() {
                cfg.allow_groups = cli_groups;
            }
            let cli_deny: Vec<String> = sub
                .get_many::<String>("deny-action")
                .into_iter()
                .flatten()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
            if !cli_deny.is_empty() {
                cfg.deny_actions = cli_deny;
            }
            cfg.validate()?;
            if cfg.auth_enabled {
                let (_, created) = mcp_cfg::ensure_token(ctx)?;
                if created {
                    eprintln!(
                        "Initialized 0600 MCP Bearer token: {}",
                        mcp_cfg::token_path(ctx).display()
                    );
                }
            }
            let policy = cfg.execution_policy();
            mcp_http::serve_http(ctx, cfg, policy)?;
            Ok(0)
        }
        Some(("config", sub)) => {
            let mut cfg = mcp_cfg::load_config(ctx)?;
            if let Some(name) = sub.get_one::<String>("name") {
                cfg.server_name = mcp_cfg::validate_server_name(name)?;
            }
            if let Some(ssh) = sub.get_one::<String>("ssh-target") {
                cfg.ssh_target = mcp_cfg::validate_ssh_target(ssh)?;
            }
            if let Some(url) = sub.get_one::<String>("public-url") {
                cfg.public_url = mcp_cfg::validate_public_url(url)?;
            }
            cfg.validate()?;
            let transport =
                mcp_cfg::TransportMode::parse(sub.get_one::<String>("transport").unwrap())?;
            let client = mcp_cfg::ClientFormat::parse(sub.get_one::<String>("client").unwrap())?;
            let policy_raw = sub.get_one::<String>("policy").unwrap().as_str();
            let policy_override = if policy_raw == "current" {
                None
            } else {
                Some(mcp_cfg::PolicyMode::parse(policy_raw)?)
            };
            let host_override = sub.get_one::<String>("host").map(String::as_str);
            let reveal = sub.get_flag("reveal");
            let token_value = if transport == mcp_cfg::TransportMode::Http && cfg.auth_enabled {
                if reveal {
                    let (tok, _) = mcp_cfg::ensure_token(ctx)?;
                    Some(tok)
                } else {
                    Some(mcp_cfg::REDACTED_TOKEN.to_owned())
                }
            } else {
                None
            };
            let cqs_cmd = mcp_cfg::preferred_cqs_command(ctx);
            let snippet = mcp_cfg::render_snippet(
                &cfg,
                transport,
                client,
                &cqs_cmd,
                host_override,
                policy_override,
                token_value.as_deref(),
            );
            if json_mode {
                let tok_status = mcp_cfg::token_status(ctx)?;
                let payload = serde_json::json!({
                    "schema_version": api::SCHEMA_VERSION,
                    "ok": true,
                    "transport": transport.as_str(),
                    "client": client.as_str(),
                    "server_name": cfg.server_name,
                    "policy_mode": policy_override.unwrap_or(cfg.policy_mode).as_str(),
                    "endpoint_url": cfg.effective_http_url(host_override),
                    "ssh_target": cfg.effective_ssh_target(host_override),
                    "ssh_tunnel_command": cfg.ssh_tunnel_command(host_override),
                    "auth_enabled": cfg.auth_enabled,
                    "revealed": reveal,
                    "token_file": tok_status.path.display().to_string(),
                    "token_exists": tok_status.exists,
                    "token_fingerprint": tok_status.fingerprint,
                    "snippet": snippet,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                println!("{snippet}");
            }
            Ok(0)
        }
        Some(("install", sub)) => {
            let target = mcp_inst::InstallTarget::parse(
                sub.get_one::<String>("client").unwrap(),
                sub.get_one::<String>("file").map(String::as_str),
            )?;
            let transport =
                mcp_cfg::TransportMode::parse(sub.get_one::<String>("transport").unwrap())?;
            let policy = mcp_cfg::PolicyMode::parse(sub.get_one::<String>("policy").unwrap())?;
            let server_name = sub.get_one::<String>("name").unwrap().clone();
            let outcome = mcp_inst::install(
                ctx,
                &mcp_inst::InstallOptions {
                    target,
                    transport,
                    policy_override: Some(policy),
                    server_name,
                    ssh_target_override: sub.get_one::<String>("ssh-target").cloned(),
                    public_url_override: sub.get_one::<String>("public-url").cloned(),
                },
            )?;
            if json_mode {
                let payload = serde_json::json!({
                    "schema_version": api::SCHEMA_VERSION,
                    "ok": true,
                    "client": outcome.client,
                    "path": outcome.path.display().to_string(),
                    "server_name": outcome.server_name,
                    "transport": outcome.transport.as_str(),
                    "backup_path": outcome.backup_path.as_ref().map(|p| p.display().to_string()),
                    "created_token": outcome.created_token,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else {
                ctx.message(
                    "notice.mcp_installed",
                    format!(
                        "{} ({}) -> {} [{}]",
                        outcome.client,
                        outcome.transport.as_str(),
                        outcome.path.display(),
                        outcome.server_name
                    ),
                );
                if let Some(backup) = outcome.backup_path {
                    ctx.message("notice.backup", backup.display());
                }
            }
            Ok(0)
        }
        Some(("uninstall", sub)) => {
            let target = mcp_inst::InstallTarget::parse(
                sub.get_one::<String>("client").unwrap(),
                sub.get_one::<String>("file").map(String::as_str),
            )?;
            let server_name = sub.get_one::<String>("name").unwrap();
            let outcome = mcp_inst::uninstall(ctx, &target, server_name)?;
            if json_mode {
                let payload = serde_json::json!({
                    "schema_version": api::SCHEMA_VERSION,
                    "ok": true,
                    "client": outcome.client,
                    "path": outcome.path.display().to_string(),
                    "server_name": outcome.server_name,
                    "removed": outcome.removed,
                });
                println!("{}", serde_json::to_string_pretty(&payload)?);
            } else if outcome.removed {
                ctx.message(
                    "notice.mcp_uninstalled",
                    format!(
                        "{} [{}] <- {}",
                        outcome.client,
                        outcome.server_name,
                        outcome.path.display()
                    ),
                );
            } else {
                ctx.message(
                    "notice.mcp_not_registered",
                    format!(
                        "{} [{}] ({})",
                        outcome.client,
                        outcome.server_name,
                        outcome.path.display()
                    ),
                );
            }
            Ok(0)
        }
        Some(("token", sub)) => match sub.subcommand() {
            Some(("status", _)) => {
                let st = mcp_cfg::token_status(ctx)?;
                if json_mode {
                    let payload = serde_json::json!({
                        "schema_version": api::SCHEMA_VERSION,
                        "ok": true,
                        "exists": st.exists,
                        "path": st.path.display().to_string(),
                        "mode": st.mode_octal,
                        "fingerprint": st.fingerprint,
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    println!(
                        "token_file={} exists={} mode={} fingerprint={}",
                        st.path.display(),
                        st.exists,
                        st.mode_octal.as_deref().unwrap_or("-"),
                        st.fingerprint.as_deref().unwrap_or("none")
                    );
                }
                Ok(0)
            }
            Some(("init", tsub)) => {
                let (token, created) = mcp_cfg::ensure_token(ctx)?;
                let st = mcp_cfg::token_status(ctx)?;
                let reveal = tsub.get_flag("reveal");
                if json_mode {
                    let mut payload = serde_json::json!({
                        "schema_version": api::SCHEMA_VERSION,
                        "ok": true,
                        "created": created,
                        "path": st.path.display().to_string(),
                        "fingerprint": st.fingerprint,
                        "revealed": reveal,
                    });
                    if reveal {
                        payload["token"] = serde_json::json!(token);
                    }
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else if reveal {
                    println!("{token}");
                } else {
                    ctx.message(
                        "notice.mcp_token_ready",
                        format!(
                            "{} ({})",
                            st.path.display(),
                            st.fingerprint.as_deref().unwrap_or("ready")
                        ),
                    );
                }
                Ok(0)
            }
            Some(("rotate", tsub)) => {
                let token = mcp_cfg::rotate_token(ctx)?;
                let st = mcp_cfg::token_status(ctx)?;
                let reveal = tsub.get_flag("reveal");
                if json_mode {
                    let mut payload = serde_json::json!({
                        "schema_version": api::SCHEMA_VERSION,
                        "ok": true,
                        "rotated": true,
                        "path": st.path.display().to_string(),
                        "fingerprint": st.fingerprint,
                        "revealed": reveal,
                    });
                    if reveal {
                        payload["token"] = serde_json::json!(token);
                    }
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else if reveal {
                    println!("{token}");
                } else {
                    ctx.message(
                        "notice.mcp_token_rotated",
                        format!(
                            "{} ({})",
                            st.path.display(),
                            st.fingerprint.as_deref().unwrap_or("rotated")
                        ),
                    );
                }
                Ok(0)
            }
            Some(("set", tsub)) => {
                let path = if let Some(file) = tsub.get_one::<String>("file") {
                    mcp_cfg::set_token_from_file(ctx, std::path::Path::new(file))?
                } else if tsub.get_flag("stdin") {
                    mcp_cfg::set_token_from_reader(ctx, std::io::stdin())?
                } else {
                    return Err(Error::new(
                        "error.input",
                        "specify --file <0600-path> or --stdin (plaintext CLI token arguments are forbidden)",
                    ));
                };
                let st = mcp_cfg::token_status(ctx)?;
                if json_mode {
                    let payload = serde_json::json!({
                        "schema_version": api::SCHEMA_VERSION,
                        "ok": true,
                        "path": path.display().to_string(),
                        "fingerprint": st.fingerprint,
                    });
                    println!("{}", serde_json::to_string_pretty(&payload)?);
                } else {
                    ctx.message(
                        "notice.mcp_token_ready",
                        format!(
                            "{} ({})",
                            path.display(),
                            st.fingerprint.as_deref().unwrap_or("saved")
                        ),
                    );
                }
                Ok(0)
            }
            _ => Err(Error::new("error.cli", "mcp token")),
        },
        Some(("service", sub)) => {
            let scope = sub.get_one::<String>("scope").unwrap().as_str();
            match sub.subcommand() {
                Some(("status", _)) => mcp_svc::status(ctx, scope)?,
                Some(("install", _)) => mcp_svc::install(ctx, scope)?,
                Some(("start", _)) => mcp_svc::control(ctx, scope, "start")?,
                Some(("stop", _)) => mcp_svc::control(ctx, scope, "stop")?,
                Some(("restart", _)) => mcp_svc::control(ctx, scope, "restart")?,
                Some(("remove", _)) => mcp_svc::remove(ctx, scope)?,
                _ => return Err(Error::new("error.cli", "mcp service")),
            }
            Ok(0)
        }
        Some((other, _)) => Err(Error::new("error.cli", format!("mcp {other}"))),
    }
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
        dispatch(&ctx, &matches, &Preferences::default()).map(|_| ())
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

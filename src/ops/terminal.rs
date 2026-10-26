use crate::{
    error::{Error, Result},
    runtime::Context,
    storage::{self, Lock, Saved},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub shell: String,
    pub style: String,
    pub color: String,
    pub time: String,
    pub git: String,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            shell: "bash".into(),
            style: "full".into(),
            color: "cyan".into(),
            time: "off".into(),
            git: "on".into(),
        }
    }
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        for (value, choices) in [
            (&self.shell, &["bash", "zsh", "both"][..]),
            (&self.style, &["compact", "full"]),
            (&self.color, &["cyan", "green", "magenta"]),
            (&self.time, &["on", "off"]),
            (&self.git, &["on", "off"]),
        ] {
            if !choices.contains(&value.as_str()) {
                return Err(Error::new("error.input", value));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    paths: BTreeMap<String, PathBuf>,
    files: BTreeMap<String, Option<Saved>>,
}

fn mapping(ctx: &Context) -> BTreeMap<String, PathBuf> {
    BTreeMap::from([
        ("bash".into(), ctx.paths.bashrc.clone()),
        ("zsh".into(), ctx.paths.zshrc.clone()),
        ("config".into(), ctx.paths.terminal.clone()),
    ])
}
fn paths(ctx: &Context) -> Result<(PathBuf, BTreeMap<String, PathBuf>)> {
    let state = ctx.paths.state.join("terminal");
    let mapping = mapping(ctx);
    let all: Vec<_> = mapping
        .values()
        .cloned()
        .chain(
            [
                "original.json",
                "options.json",
                "pending.json",
                "config.lock",
            ]
            .map(|p| state.join(p)),
        )
        .collect();
    if all.iter().collect::<BTreeSet<_>>().len() != all.len() {
        return Err(Error::new("error.path", "duplicate terminal targets"));
    }
    for p in all {
        storage::check_path(&p, true)?;
    }
    if state.join("pending.json").exists() {
        return Err(Error::new(
            "error.pending",
            state.join("pending.json").display(),
        ));
    }
    Ok((state, mapping))
}
fn load_baseline(
    state: &std::path::Path,
    mapping: &BTreeMap<String, PathBuf>,
) -> Result<Option<Baseline>> {
    match fs::read(state.join("original.json")) {
        Ok(bytes) => {
            let baseline: Baseline = serde_json::from_slice(&bytes)?;
            if &baseline.paths != mapping || baseline.files.keys().ne(mapping.keys()) {
                return Err(Error::new("error.snapshot", state.display()));
            }
            for saved in baseline.files.values().flatten() {
                saved.bytes()?;
            }
            Ok(Some(baseline))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn options(ctx: &Context) -> Result<Options> {
    let (state, _) = paths(ctx)?;
    let options = match fs::read(state.join("options.json")) {
        Ok(data) => serde_json::from_slice(&data)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Options::default(),
        Err(e) => return Err(e.into()),
    };
    options.validate()?;
    Ok(options)
}
pub fn edit_rc(raw: &[u8], shell: &str, enabled: bool, config: &str) -> Result<Vec<u8>> {
    let begin = b"# >>> cqs terminal >>>";
    let end = b"# <<< cqs terminal <<<";
    let legacy = format!("eval \"$(starship init {shell})\"");
    let mut output = Vec::new();
    let mut inside = false;
    for line in raw.split_inclusive(|b| *b == b'\n') {
        let stripped = line.trim_ascii_end();
        if stripped == begin {
            if inside {
                return Err(Error::new("error.terminal_marker", "nested"));
            }
            inside = true;
        } else if stripped == end {
            if !inside {
                return Err(Error::new("error.terminal_marker", "missing start"));
            }
            inside = false;
        } else if !inside && stripped != legacy.as_bytes() {
            output.extend_from_slice(line);
        }
    }
    if inside {
        return Err(Error::new("error.terminal_marker", "missing end"));
    }
    if enabled {
        if !output.is_empty() && output.last() != Some(&b'\n') {
            output.push(b'\n');
        }
        // Quote the generated shell literal; paths are never evaluated as shell code.
        let quoted = format!("'{}'", config.replace('\'', "'\"'\"'"));
        output.extend_from_slice(format!("# >>> cqs terminal >>>\nif command -v starship >/dev/null 2>&1; then\n    export STARSHIP_CONFIG={quoted}\n    eval \"$(starship init {shell})\"\nfi\n# <<< cqs terminal <<<\n").as_bytes());
    }
    Ok(output)
}

pub fn render(options: &Options) -> Result<String> {
    options.validate()?;
    let color = &options.color;
    let full = options.style == "full";
    let git_disabled = options.git == "off";
    let time_disabled = options.time == "off";
    Ok(format!(
        r#"# Managed by CqShell.
add_newline = {full}
format = '$username$hostname$directory$git_branch$git_status$cmd_duration$time$line_break$character'
[username]
show_always = true
style_user = 'bold {color}'
style_root = 'bold red'
format = '[$user]($style)'
[hostname]
ssh_only = false
style = 'bold {color}'
format = '[@$hostname]($style) '
[directory]
style = 'bold {color}'
truncation_length = 3
truncation_symbol = '.../'
read_only = ' ro'
format = '[$path]($style)[$read_only](red) '
[git_branch]
disabled = {git_disabled}
symbol = 'git:'
style = 'bold {color}'
format = '[$symbol$branch]($style) '
[git_status]
disabled = {git_disabled}
style = 'yellow'
format = '([$all_status$ahead_behind]($style) )'
ahead = 'up${{count}}'
behind = 'down${{count}}'
diverged = 'up${{ahead_count}}/down${{behind_count}}'
[cmd_duration]
disabled = {compact}
min_time = 2000
format = '[took $duration](yellow) '
[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
format = '[$time](dimmed white) '
[line_break]
disabled = {compact}
[character]
success_symbol = '[>](bold {color})'
error_symbol = '[>](bold red)'
vimcmd_symbol = '[<](bold {color})'
"#,
        compact = !full
    ))
}

pub fn apply(ctx: &Context, options: &Options) -> Result<()> {
    options.validate()?;
    ctx.require(&["starship"])?;
    if options.shell != "bash" {
        ctx.require(&["zsh"])?;
    }
    let (state, _) = paths(ctx)?;
    storage::private_dir(&state)?;
    let _lock = Lock::acquire(&state.join("config.lock"))?;
    let (state, mapping) = paths(ctx)?;
    let current = mapping
        .iter()
        .map(|(key, path)| Ok((key.clone(), Saved::capture(path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let original = load_baseline(&state, &mapping)?.unwrap_or(Baseline {
        paths: mapping.clone(),
        files: current.clone(),
    });
    let mut updates = vec![(state.join("original.json"), storage::json_saved(&original)?)];
    for shell in ["bash", "zsh"] {
        let saved = &current[shell];
        let raw = saved
            .as_ref()
            .map(Saved::bytes)
            .transpose()?
            .unwrap_or_default();
        let data = edit_rc(
            &raw,
            shell,
            options.shell == shell || options.shell == "both",
            crate::runtime::path(&ctx.paths.terminal)?,
        )?;
        let desired = if data.is_empty() && saved.is_none() {
            None
        } else {
            Some(Saved::new(&data, saved.as_ref().map_or(0o600, |s| s.mode)))
        };
        updates.push((mapping[shell].clone(), desired));
    }
    updates.push((
        mapping["config"].clone(),
        Some(Saved::new(render(options)?.as_bytes(), 0o600)),
    ));
    updates.push((state.join("options.json"), storage::json_saved(options)?));
    storage::write_transaction(&updates, &state.join("pending.json"), || ctx.check())?;
    ctx.log(ctx.i18n.text("notice.terminal"));
    Ok(())
}
pub fn restore(ctx: &Context) -> Result<()> {
    let (state, _) = paths(ctx)?;
    let _lock = Lock::acquire(&state.join("config.lock"))?;
    let (state, mapping) = paths(ctx)?;
    let original = load_baseline(&state, &mapping)?
        .ok_or_else(|| Error::new("error.snapshot", state.display()))?;
    let mut updates = original
        .files
        .into_iter()
        .map(|(key, saved)| (mapping[&key].clone(), saved))
        .collect::<Vec<_>>();
    updates.extend([
        (state.join("options.json"), None),
        (state.join("original.json"), None),
    ]);
    storage::write_transaction(&updates, &state.join("pending.json"), || ctx.check())?;
    ctx.message("notice.restored", state.display());
    Ok(())
}
pub fn status(ctx: &Context) -> Result<()> {
    let (state, _) = paths(ctx)?;
    let _lock = Lock::acquire(&state.join("config.lock"))?;
    let (state, mapping) = paths(ctx)?;
    let enabled = load_baseline(&state, &mapping)?.is_some();
    ctx.log(ctx.i18n.text(if enabled {
        "status.enabled"
    } else {
        "status.disabled"
    }));
    ctx.log(toml::to_string_pretty(&options(ctx)?)?);
    ctx.message("notice.backup", state.join("original.json").display());
    Ok(())
}

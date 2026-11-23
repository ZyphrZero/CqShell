use crate::{
    error::{Error, Result},
    runtime::Context,
    storage::{self, Lock, Saved},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::IsTerminal,
    path::PathBuf,
};
use unicode_width::UnicodeWidthStr;

pub const SHELLS: &[&str] = &["bash", "zsh", "both"];
pub const STYLES: &[&str] = &[
    "full",
    "compact",
    "box",
    "ribbon",
    "powerline",
    "minimal",
    "brackets",
];
pub const COLORS: &[&str] = &[
    "cyan",
    "green",
    "magenta",
    "tokyo-night",
    "catppuccin",
    "rose-pine",
    "nord",
    "gruvbox",
    "dracula",
];
pub const PREVIEW_STYLES: &[&str] = &[
    "full",
    "compact",
    "box",
    "ribbon",
    "powerline",
    "minimal",
    "brackets",
    "all",
];
pub const PREVIEW_COLORS: &[&str] = &[
    "cyan",
    "green",
    "magenta",
    "tokyo-night",
    "catppuccin",
    "rose-pine",
    "nord",
    "gruvbox",
    "dracula",
    "all",
];

pub type Rgb = (u8, u8, u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleMeta {
    pub id: &'static str,
    pub multiline: bool,
    pub nerd_font: bool,
}

const STYLE_METAS: &[StyleMeta] = &[
    StyleMeta {
        id: "full",
        multiline: true,
        nerd_font: false,
    },
    StyleMeta {
        id: "compact",
        multiline: false,
        nerd_font: false,
    },
    StyleMeta {
        id: "box",
        multiline: true,
        nerd_font: false,
    },
    StyleMeta {
        id: "ribbon",
        multiline: true,
        nerd_font: false,
    },
    StyleMeta {
        id: "powerline",
        multiline: true,
        nerd_font: true,
    },
    StyleMeta {
        id: "minimal",
        multiline: true,
        nerd_font: false,
    },
    StyleMeta {
        id: "brackets",
        multiline: true,
        nerd_font: false,
    },
];

pub fn style_meta(id: &str) -> &'static StyleMeta {
    STYLE_METAS
        .iter()
        .find(|m| m.id == id)
        .unwrap_or(&STYLE_METAS[0])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptPalette {
    pub id: &'static str,
    pub user: Rgb,
    pub root: Rgb,
    pub host: Rgb,
    pub dir: Rgb,
    pub git: Rgb,
    pub git_status: Rgb,
    pub duration: Rgb,
    pub time: Rgb,
    pub ok: Rgb,
    pub err: Rgb,
    pub muted: Rgb,
    pub surface: Rgb,
    pub surface_alt: Rgb,
    pub on_accent: Rgb,
    pub text: Rgb,
}

impl PromptPalette {
    /// Five representative colors used for palette swatch bars in the TUI.
    pub fn swatches(&self) -> [Rgb; 5] {
        [self.user, self.dir, self.git, self.git_status, self.err]
    }
}

const PALETTES: &[PromptPalette] = &[
    PromptPalette {
        id: "cyan",
        user: (0x22, 0xd3, 0xee),
        root: (0xfb, 0x71, 0x85),
        host: (0x67, 0xe8, 0xf9),
        dir: (0x38, 0xbd, 0xf8),
        git: (0xa7, 0x8b, 0xfa),
        git_status: (0xfb, 0xbf, 0x24),
        duration: (0xf5, 0x9e, 0x0b),
        time: (0x94, 0xa3, 0xb8),
        ok: (0x22, 0xd3, 0xee),
        err: (0xf8, 0x71, 0x71),
        muted: (0x64, 0x74, 0x8b),
        surface: (0x16, 0x22, 0x31),
        surface_alt: (0x1e, 0x30, 0x44),
        on_accent: (0x0b, 0x12, 0x20),
        text: (0xe2, 0xe8, 0xf0),
    },
    PromptPalette {
        id: "green",
        user: (0x34, 0xd3, 0x99),
        root: (0xfb, 0x71, 0x85),
        host: (0x6e, 0xe7, 0xb7),
        dir: (0x2d, 0xd4, 0xbf),
        git: (0xa3, 0xe6, 0x35),
        git_status: (0xfb, 0xbf, 0x24),
        duration: (0xf5, 0x9e, 0x0b),
        time: (0x94, 0xa3, 0xb8),
        ok: (0x34, 0xd3, 0x99),
        err: (0xf8, 0x71, 0x71),
        muted: (0x64, 0x74, 0x8b),
        surface: (0x13, 0x26, 0x20),
        surface_alt: (0x1b, 0x35, 0x2d),
        on_accent: (0x08, 0x19, 0x12),
        text: (0xe2, 0xf5, 0xec),
    },
    PromptPalette {
        id: "magenta",
        user: (0xe8, 0x79, 0xf9),
        root: (0xf4, 0x3f, 0x5e),
        host: (0xf0, 0xab, 0xfc),
        dir: (0xc0, 0x84, 0xfc),
        git: (0x38, 0xbd, 0xf8),
        git_status: (0xfb, 0xbf, 0x24),
        duration: (0xfb, 0x92, 0x3c),
        time: (0xa1, 0xa1, 0xaa),
        ok: (0xe8, 0x79, 0xf9),
        err: (0xfb, 0x71, 0x85),
        muted: (0x71, 0x71, 0x7a),
        surface: (0x26, 0x18, 0x2c),
        surface_alt: (0x35, 0x21, 0x3e),
        on_accent: (0x19, 0x0b, 0x1f),
        text: (0xf4, 0xe8, 0xf9),
    },
    PromptPalette {
        id: "tokyo-night",
        user: (0x7a, 0xa2, 0xf7),
        root: (0xf7, 0x76, 0x8e),
        host: (0x7d, 0xcf, 0xff),
        dir: (0xbb, 0x9a, 0xf7),
        git: (0x9e, 0xce, 0x6a),
        git_status: (0xe0, 0xaf, 0x68),
        duration: (0xff, 0x9e, 0x64),
        time: (0x78, 0x7c, 0x99),
        ok: (0x9e, 0xce, 0x6a),
        err: (0xf7, 0x76, 0x8e),
        muted: (0x56, 0x5f, 0x89),
        surface: (0x24, 0x28, 0x3b),
        surface_alt: (0x2f, 0x35, 0x49),
        on_accent: (0x1a, 0x1b, 0x26),
        text: (0xc0, 0xca, 0xf5),
    },
    PromptPalette {
        id: "catppuccin",
        user: (0x89, 0xb4, 0xfa),
        root: (0xf3, 0x8b, 0xa8),
        host: (0x94, 0xe2, 0xd5),
        dir: (0xcb, 0xa6, 0xf7),
        git: (0xa6, 0xe3, 0xa1),
        git_status: (0xf9, 0xe2, 0xaf),
        duration: (0xfa, 0xb3, 0x87),
        time: (0x93, 0x99, 0xb2),
        ok: (0xa6, 0xe3, 0xa1),
        err: (0xf3, 0x8b, 0xa8),
        muted: (0x6c, 0x70, 0x86),
        surface: (0x31, 0x32, 0x44),
        surface_alt: (0x45, 0x47, 0x5a),
        on_accent: (0x1e, 0x1e, 0x2e),
        text: (0xcd, 0xd6, 0xf4),
    },
    PromptPalette {
        id: "rose-pine",
        user: (0xeb, 0xbc, 0xba),
        root: (0xeb, 0x6f, 0x92),
        host: (0xf6, 0xc1, 0x77),
        dir: (0x9c, 0xcf, 0xd8),
        git: (0xc4, 0xa7, 0xe7),
        git_status: (0xf6, 0xc1, 0x77),
        duration: (0xea, 0x9a, 0x97),
        time: (0x90, 0x8c, 0xaa),
        ok: (0x9c, 0xcf, 0xd8),
        err: (0xeb, 0x6f, 0x92),
        muted: (0x6e, 0x6a, 0x86),
        surface: (0x26, 0x23, 0x3a),
        surface_alt: (0x39, 0x35, 0x52),
        on_accent: (0x19, 0x17, 0x24),
        text: (0xe0, 0xde, 0xf4),
    },
    PromptPalette {
        id: "nord",
        user: (0x88, 0xc0, 0xd0),
        root: (0xbf, 0x61, 0x6a),
        host: (0x81, 0xa1, 0xc1),
        dir: (0x8f, 0xbc, 0xbb),
        git: (0xb4, 0x8e, 0xad),
        git_status: (0xeb, 0xcb, 0x8b),
        duration: (0xd0, 0x87, 0x70),
        time: (0x7b, 0x88, 0xa1),
        ok: (0xa3, 0xbe, 0x8c),
        err: (0xbf, 0x61, 0x6a),
        muted: (0x61, 0x6e, 0x88),
        surface: (0x3b, 0x42, 0x52),
        surface_alt: (0x43, 0x4c, 0x5e),
        on_accent: (0x2e, 0x34, 0x40),
        text: (0xec, 0xef, 0xf4),
    },
    PromptPalette {
        id: "gruvbox",
        user: (0xfa, 0xbd, 0x2f),
        root: (0xfb, 0x49, 0x34),
        host: (0xfe, 0x80, 0x19),
        dir: (0x83, 0xa5, 0x98),
        git: (0xb8, 0xbb, 0x26),
        git_status: (0xd3, 0x86, 0x9b),
        duration: (0xfe, 0x80, 0x19),
        time: (0x92, 0x83, 0x74),
        ok: (0xb8, 0xbb, 0x26),
        err: (0xfb, 0x49, 0x34),
        muted: (0x7c, 0x6f, 0x64),
        surface: (0x3c, 0x38, 0x36),
        surface_alt: (0x50, 0x49, 0x45),
        on_accent: (0x28, 0x28, 0x28),
        text: (0xeb, 0xdb, 0xb2),
    },
    PromptPalette {
        id: "dracula",
        user: (0xbd, 0x93, 0xf9),
        root: (0xff, 0x55, 0x55),
        host: (0xff, 0x79, 0xc6),
        dir: (0x8b, 0xe9, 0xfd),
        git: (0x50, 0xfa, 0x7b),
        git_status: (0xf1, 0xfa, 0x8c),
        duration: (0xff, 0xb8, 0x6c),
        time: (0x8b, 0x95, 0xc9),
        ok: (0x50, 0xfa, 0x7b),
        err: (0xff, 0x55, 0x55),
        muted: (0x62, 0x72, 0xa4),
        surface: (0x38, 0x3a, 0x59),
        surface_alt: (0x44, 0x47, 0x5a),
        on_accent: (0x28, 0x2a, 0x36),
        text: (0xf8, 0xf8, 0xf2),
    },
];

pub fn palette(id: &str) -> &'static PromptPalette {
    PALETTES.iter().find(|p| p.id == id).unwrap_or(&PALETTES[0])
}

fn hex(rgb: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

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
            (&self.shell, SHELLS),
            (&self.style, STYLES),
            (&self.color, COLORS),
            (&self.time, &["on", "off"][..]),
            (&self.git, &["on", "off"][..]),
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

/// Generate a complete, valid Starship TOML configuration for the given options.
pub fn render(options: &Options) -> Result<String> {
    options.validate()?;
    let p = palette(&options.color);
    let user = hex(p.user);
    let root = hex(p.root);
    let host = hex(p.host);
    let dir = hex(p.dir);
    let git = hex(p.git);
    let git_status = hex(p.git_status);
    let duration = hex(p.duration);
    let time = hex(p.time);
    let ok = hex(p.ok);
    let err = hex(p.err);
    let muted = hex(p.muted);
    let surface = hex(p.surface);
    let surface_alt = hex(p.surface_alt);
    let on_accent = hex(p.on_accent);
    let text = hex(p.text);

    let git_disabled = options.git == "off";
    let time_disabled = options.time == "off";

    let body = match options.style.as_str() {
        "compact" => format!(
            r#"# Managed by CqShell (style: compact, palette: {palette_id}).
add_newline = false
format = '$username$hostname$directory$git_branch$git_status$status$jobs$time$character'

[username]
show_always = true
style_user = 'bold {user}'
style_root = 'bold {root}'
format = '[$user]($style)'

[hostname]
ssh_only = false
style = '{host}'
format = '[@$hostname]($style) '

[directory]
style = 'bold {dir}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = '{err}'
format = '[$path]($style)[$read_only]($read_only_style) '

[git_branch]
disabled = {git_disabled}
symbol = 'git:'
style = 'bold {git}'
format = '[$symbol$branch]($style) '

[git_status]
disabled = {git_disabled}
style = '{git_status}'
format = '([$all_status$ahead_behind]($style) )'
modified = '!'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[status]
disabled = false
symbol = '✘'
style = 'bold {err}'
format = '[$symbol$status]($style) '

[jobs]
symbol = '✦'
style = '{duration}'
format = '[$symbol$number]($style) '

[cmd_duration]
disabled = true

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = '{time}'
format = '[$time]($style) '

[line_break]
disabled = true

[character]
success_symbol = '[❯](bold {ok})'
error_symbol = '[❯](bold {err})'
vimcmd_symbol = '[❮](bold {git})'
"#,
            palette_id = p.id
        ),
        "box" => format!(
            r#"# Managed by CqShell (style: box, palette: {palette_id}).
add_newline = true
format = '[╭─ ]({muted})$username$hostname[ · ]({muted})$directory$git_branch$git_status$python$container$status$jobs$fill$cmd_duration$time$line_break[╰─]({muted})$character'

[fill]
symbol = '─'
style = '{muted}'

[username]
show_always = true
style_user = 'bold {user}'
style_root = 'bold {root}'
format = '[$user]($style)'

[hostname]
ssh_only = false
style = '{host}'
format = '[@$hostname]($style)'

[directory]
style = 'bold {dir}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = '{err}'
format = '[$path]($style)[$read_only]($read_only_style) '

[git_branch]
disabled = {git_disabled}
symbol = 'git:'
style = 'bold {git}'
format = '[· ]({muted})[$symbol$branch]($style) '

[git_status]
disabled = {git_disabled}
style = '{git_status}'
format = '([$all_status$ahead_behind]($style) )'
modified = '!'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[python]
detect_extensions = []
detect_files = []
detect_folders = []
style = '{git}'
format = '[· ]({muted})[venv:$virtualenv]($style) '

[container]
style = '{duration}'
format = '[· ]({muted})[$name]($style) '

[status]
disabled = false
symbol = '✘'
style = 'bold {err}'
format = '[$symbol $status]($style) '

[jobs]
symbol = '✦'
style = '{duration}'
format = '[$symbol$number]($style) '

[cmd_duration]
disabled = false
min_time = 2000
style = '{duration}'
format = '[ $duration]($style)'

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = '{time}'
format = '[ $time]($style)'

[line_break]
disabled = false

[character]
success_symbol = '[❯](bold {ok})'
error_symbol = '[❯](bold {err})'
vimcmd_symbol = '[❮](bold {git})'
"#,
            palette_id = p.id
        ),
        "ribbon" => format!(
            r#"# Managed by CqShell (style: ribbon, palette: {palette_id}).
add_newline = true
format = '$username$hostname$directory$git_branch$git_status$python$container$status$jobs$cmd_duration$time$line_break$character'

[username]
show_always = true
style_user = 'bold fg:{on_accent} bg:{user}'
style_root = 'bold fg:{on_accent} bg:{root}'
format = '[ $user ]($style)'

[hostname]
ssh_only = false
style = 'fg:{host} bg:{surface_alt}'
format = '[@$hostname ]($style) '

[directory]
style = 'bold fg:{dir} bg:{surface}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = 'bold fg:{err} bg:{surface}'
format = '[ $path ]($style)[$read_only]($read_only_style) '

[git_branch]
disabled = {git_disabled}
symbol = 'git:'
style = 'bold fg:{git} bg:{surface_alt}'
format = '[ $symbol$branch ]($style) '

[git_status]
disabled = {git_disabled}
style = 'fg:{git_status} bg:{surface}'
format = '([ $all_status$ahead_behind ]($style) )'
modified = '!'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[python]
detect_extensions = []
detect_files = []
detect_folders = []
style = 'fg:{text} bg:{surface}'
format = '[ venv:$virtualenv ]($style) '

[container]
style = 'fg:{duration} bg:{surface}'
format = '[ $name ]($style) '

[status]
disabled = false
symbol = '✘'
style = 'bold fg:{on_accent} bg:{err}'
format = '[ $symbol $status ]($style) '

[jobs]
symbol = '✦'
style = 'fg:{duration} bg:{surface}'
format = '[ $symbol$number ]($style) '

[cmd_duration]
disabled = false
min_time = 2000
style = 'fg:{duration} bg:{surface}'
format = '[ $duration ]($style) '

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = 'fg:{time} bg:{surface}'
format = '[ $time ]($style)'

[line_break]
disabled = false

[character]
success_symbol = '[❯](bold {ok})'
error_symbol = '[❯](bold {err})'
vimcmd_symbol = '[❮](bold {git})'
"#,
            palette_id = p.id
        ),
        "powerline" => format!(
            r#"# Managed by CqShell (style: powerline, palette: {palette_id}, requires Nerd Font).
add_newline = true
format = '[\uE0B6](fg:{surface_alt})$username$hostname[\uE0B0](fg:{surface_alt} bg:{surface})$directory[\uE0B4](fg:{surface}) $git_branch$git_status$python$container$status$jobs$cmd_duration$time$line_break[╰─]({muted})$character'

[username]
show_always = true
style_user = 'bold fg:{user} bg:{surface_alt}'
style_root = 'bold fg:{root} bg:{surface_alt}'
format = '[ $user]($style)'

[hostname]
ssh_only = false
style = 'fg:{host} bg:{surface_alt}'
format = '[@$hostname ]($style)'

[directory]
style = 'bold fg:{dir} bg:{surface}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = 'bold fg:{err} bg:{surface}'
format = '[ $path ]($style)[$read_only]($read_only_style)'

[git_branch]
disabled = {git_disabled}
symbol = '\uE0A0 '
style = 'bold fg:{git} bg:{surface_alt}'
format = '[\uE0B6](fg:{surface_alt})[ $symbol$branch ]($style)[\uE0B4](fg:{surface_alt}) '

[git_status]
disabled = {git_disabled}
style = 'fg:{git_status} bg:{surface}'
format = '([\uE0B6](fg:{surface})[ $all_status$ahead_behind ]($style)[\uE0B4](fg:{surface}) )'
modified = '!'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[python]
detect_extensions = []
detect_files = []
detect_folders = []
style = 'fg:{text} bg:{surface}'
format = '[\uE0B6](fg:{surface})[ venv:$virtualenv ]($style)[\uE0B4](fg:{surface}) '

[container]
style = 'fg:{duration} bg:{surface}'
format = '[\uE0B6](fg:{surface})[ $name ]($style)[\uE0B4](fg:{surface}) '

[status]
disabled = false
symbol = '✘'
style = 'bold fg:{on_accent} bg:{err}'
format = '[\uE0B6](fg:{err})[ $symbol $status ]($style)[\uE0B4](fg:{err}) '

[jobs]
symbol = '✦'
style = 'fg:{duration} bg:{surface}'
format = '[\uE0B6](fg:{surface})[ $symbol$number ]($style)[\uE0B4](fg:{surface}) '

[cmd_duration]
disabled = false
min_time = 2000
style = 'fg:{duration} bg:{surface}'
format = '[\uE0B6](fg:{surface})[ $duration ]($style)[\uE0B4](fg:{surface}) '

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = 'fg:{time} bg:{surface}'
format = '[\uE0B6](fg:{surface})[ $time ]($style)[\uE0B4](fg:{surface})'

[line_break]
disabled = false

[character]
success_symbol = '[❯](bold {ok})'
error_symbol = '[❯](bold {err})'
vimcmd_symbol = '[❮](bold {git})'
"#,
            palette_id = p.id
        )
        .replace("\\uE0B6", "\u{e0b6}")
        .replace("\\uE0B0", "\u{e0b0}")
        .replace("\\uE0B4", "\u{e0b4}")
        .replace("\\uE0A0", "\u{e0a0}"),
        "minimal" => format!(
            r#"# Managed by CqShell (style: minimal, palette: {palette_id}).
add_newline = true
format = '$directory$git_branch$git_status$cmd_duration$status$jobs$username$hostname$time$line_break$character'

[username]
show_always = true
style_user = '{muted}'
style_root = 'bold {root}'
format = '[$user]($style)'

[hostname]
ssh_only = false
style = '{muted}'
format = '[@$hostname]($style) '

[directory]
style = 'bold {dir}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = '{err}'
format = '[$path]($style)[$read_only]($read_only_style) '

[git_branch]
disabled = {git_disabled}
symbol = ''
style = '{git}'
format = '[$branch]($style) '

[git_status]
disabled = {git_disabled}
style = '{git_status}'
format = '([$all_status$ahead_behind]($style) )'
modified = '*'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[status]
disabled = false
symbol = '✘'
style = '{err}'
format = '[$symbol$status]($style) '

[jobs]
symbol = '✦'
style = '{duration}'
format = '[$symbol$number]($style) '

[cmd_duration]
disabled = false
min_time = 2000
style = '{duration}'
format = '[$duration]($style) '

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = '{time}'
format = '[$time]($style)'

[line_break]
disabled = false

[character]
success_symbol = '[❯](bold {ok})'
error_symbol = '[❯](bold {err})'
vimcmd_symbol = '[❮](bold {git})'
"#,
            palette_id = p.id
        ),
        "brackets" => format!(
            r#"# Managed by CqShell (style: brackets, palette: {palette_id}).
add_newline = true
format = '[\[]({muted})$username$hostname[\]]({muted}) $directory$git_branch$git_status$python$container$status$jobs$cmd_duration$time$line_break$character'

[username]
show_always = true
style_user = 'bold {user}'
style_root = 'bold {root}'
format = '[$user]($style)'

[hostname]
ssh_only = false
style = '{host}'
format = '[@$hostname]($style)'

[directory]
style = 'bold {dir}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = '{err}'
format = '[\[]({muted})[$path]($style)[$read_only]($read_only_style)[\]]({muted}) '

[git_branch]
disabled = {git_disabled}
symbol = 'git:'
style = 'bold {git}'
format = '[\[]({muted})[$symbol$branch]($style)[\]]({muted}) '

[git_status]
disabled = {git_disabled}
style = '{git_status}'
format = '([\[]({muted})[$all_status$ahead_behind]($style)[\]]({muted}) )'
modified = '!'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[python]
detect_extensions = []
detect_files = []
detect_folders = []
style = '{git}'
format = '[\[]({muted})[venv:$virtualenv]($style)[\]]({muted}) '

[container]
style = '{duration}'
format = '[\[]({muted})[$name]($style)[\]]({muted}) '

[status]
disabled = false
symbol = '✘'
style = 'bold {err}'
format = '[\[]({muted})[$symbol $status]($style)[\]]({muted}) '

[jobs]
symbol = '✦'
style = '{duration}'
format = '[\[]({muted})[$symbol$number]($style)[\]]({muted}) '

[cmd_duration]
disabled = false
min_time = 2000
style = '{duration}'
format = '[\[]({muted})[$duration]($style)[\]]({muted}) '

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = '{time}'
format = '[\[]({muted})[$time]($style)[\]]({muted})'

[line_break]
disabled = false

[character]
success_symbol = '[›](bold {ok})'
error_symbol = '[›](bold {err})'
vimcmd_symbol = '[‹](bold {git})'
"#,
            palette_id = p.id
        ),
        _ => format!(
            r#"# Managed by CqShell (style: full, palette: {palette_id}).
add_newline = true
format = '$username$hostname$directory$git_branch$git_status$python$container$status$jobs$cmd_duration$time$line_break$character'

[username]
show_always = true
style_user = 'bold {user}'
style_root = 'bold {root}'
format = '[$user]($style)'

[hostname]
ssh_only = false
style = '{host}'
format = '[@$hostname]($style) '

[directory]
style = 'bold {dir}'
truncation_length = 3
truncation_symbol = '…/'
read_only = ' ro'
read_only_style = '{err}'
format = '[$path]($style)[$read_only]($read_only_style) '

[git_branch]
disabled = {git_disabled}
symbol = 'git:'
style = 'bold {git}'
format = '[$symbol$branch]($style) '

[git_status]
disabled = {git_disabled}
style = '{git_status}'
format = '([$all_status$ahead_behind]($style) )'
modified = '!'
staged = '+'
untracked = '?'
deleted = 'x'
ahead = '⇡${{count}}'
behind = '⇣${{count}}'
diverged = '⇡${{ahead_count}}⇣${{behind_count}}'

[python]
detect_extensions = []
detect_files = []
detect_folders = []
style = '{git}'
format = '[venv:$virtualenv]($style) '

[container]
style = '{duration}'
format = '[$name]($style) '

[status]
disabled = false
symbol = '✘'
style = 'bold {err}'
format = '[$symbol $status]($style) '

[jobs]
symbol = '✦'
style = '{duration}'
format = '[$symbol$number]($style) '

[cmd_duration]
disabled = false
min_time = 2000
style = '{duration}'
format = '[took $duration]($style) '

[time]
disabled = {time_disabled}
time_format = '%H:%M:%S'
style = '{time}'
format = '[$time]($style) '

[line_break]
disabled = false

[character]
success_symbol = '[❯](bold {ok})'
error_symbol = '[❯](bold {err})'
vimcmd_symbol = '[❮](bold {git})'
"#,
            palette_id = p.id
        ),
    };
    Ok(body)
}

/// Styled span for rendering terminal prompt previews in both TUI (Ratatui) and CLI (ANSI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSpan {
    pub text: String,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub dim: bool,
}

impl PreviewSpan {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: None,
            bg: None,
            bold: false,
            dim: false,
        }
    }

    pub fn fg(text: impl Into<String>, fg: Rgb, bold: bool) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bg: None,
            bold,
            dim: false,
        }
    }

    pub fn pill(text: impl Into<String>, fg: Rgb, bg: Rgb, bold: bool) -> Self {
        Self {
            text: text.into(),
            fg: Some(fg),
            bg: Some(bg),
            bold,
            dim: false,
        }
    }

    pub fn dim(text: impl Into<String>, fg: Option<Rgb>) -> Self {
        Self {
            text: text.into(),
            fg,
            bg: None,
            bold: false,
            dim: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewLine {
    pub spans: Vec<PreviewSpan>,
}

impl PreviewLine {
    pub fn new(spans: Vec<PreviewSpan>) -> Self {
        Self { spans }
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| s.text.width()).sum()
    }

    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewScenario {
    /// Regular user in a dirty Git repository with a recent command duration.
    Normal,
    /// Previous command exited with non-zero code (127).
    Error,
    /// Elevated root session in `/etc/nginx`.
    Root,
}

/// Build realistic prompt preview lines matching [`render`] for the given options and scenario.
pub fn preview_lines(
    options: &Options,
    host: &str,
    scenario: PreviewScenario,
    width: usize,
    ascii_only: bool,
) -> Vec<PreviewLine> {
    let p = palette(&options.color);
    let host = if host.trim().is_empty() {
        "vps-sg-01"
    } else {
        host.trim()
    };
    let show_git = options.git != "off" && scenario != PreviewScenario::Root;
    let show_time = options.time == "on";
    let is_root = scenario == PreviewScenario::Root;
    let is_error = scenario == PreviewScenario::Error;

    let user = if is_root { "root" } else { "dev" };
    let dir = if is_root { "/etc/nginx" } else { "~/app" };
    let command = match scenario {
        PreviewScenario::Normal => "git status",
        PreviewScenario::Error => "nginx -t",
        PreviewScenario::Root => "systemctl reload nginx",
    };

    let user_color = if is_root { p.root } else { p.user };
    let prompt_sym = if ascii_only {
        ">"
    } else if options.style == "brackets" {
        "›"
    } else {
        "❯"
    };
    let err_sym = if ascii_only { "x" } else { "✘" };
    let arrow_up = if ascii_only { "^1" } else { "⇡1" };
    let git_status_txt = if options.style == "minimal" {
        format!("*+2 {arrow_up}")
    } else {
        format!("!1+2 {arrow_up}")
    };
    let dot_sep = if ascii_only { " | " } else { " · " };
    let box_top = if ascii_only { "+- " } else { "╭─ " };
    let box_bot = if ascii_only { "+-" } else { "╰─" };
    let fill_ch = if ascii_only { "-" } else { "─" };
    let pl_l = if ascii_only { "[" } else { "\u{e0b6}" };
    let pl_sep = if ascii_only { ">" } else { "\u{e0b0}" };
    let pl_r = if ascii_only { "]" } else { "\u{e0b4}" };
    let pl_git = if ascii_only { "git:" } else { "\u{e0a0} " };

    let mut prompt_char = PreviewSpan::fg(
        format!("{prompt_sym} "),
        if is_error { p.err } else { p.ok },
        true,
    );
    if options.style == "compact" {
        prompt_char.text = format!("{prompt_sym} ");
    }
    let cmd_span = PreviewSpan::fg(command, p.text, false);

    match options.style.as_str() {
        "compact" => {
            let mut spans = vec![
                PreviewSpan::fg(user, user_color, true),
                PreviewSpan::fg(format!("@{host} "), p.host, false),
                PreviewSpan::fg(format!("{dir} "), p.dir, true),
            ];
            if show_git {
                spans.push(PreviewSpan::fg("git:main ", p.git, true));
                spans.push(PreviewSpan::fg(
                    format!("{git_status_txt} "),
                    p.git_status,
                    false,
                ));
            }
            if is_error {
                spans.push(PreviewSpan::fg(format!("{err_sym}127 "), p.err, true));
            }
            if show_time {
                spans.push(PreviewSpan::fg("16:20:05 ", p.time, false));
            }
            spans.push(prompt_char);
            spans.push(cmd_span);
            vec![PreviewLine::new(spans)]
        }
        "box" => {
            let mut left = vec![
                PreviewSpan::fg(box_top, p.muted, false),
                PreviewSpan::fg(user, user_color, true),
                PreviewSpan::fg(format!("@{host}"), p.host, false),
                PreviewSpan::fg(dot_sep, p.muted, false),
                PreviewSpan::fg(format!("{dir} "), p.dir, true),
            ];
            if show_git {
                left.push(PreviewSpan::fg(
                    if ascii_only { "| " } else { "· " },
                    p.muted,
                    false,
                ));
                left.push(PreviewSpan::fg("git:main ", p.git, true));
                left.push(PreviewSpan::fg(
                    format!("{git_status_txt} "),
                    p.git_status,
                    false,
                ));
            }
            if is_error {
                left.push(PreviewSpan::fg(format!("{err_sym} 127 "), p.err, true));
            }

            let mut right = Vec::new();
            if scenario == PreviewScenario::Normal {
                right.push(PreviewSpan::fg(" 1.4s", p.duration, false));
            }
            if show_time {
                right.push(PreviewSpan::fg(" 16:20:05", p.time, false));
            }

            let left_w: usize = left.iter().map(|s| s.text.width()).sum();
            let right_w: usize = right.iter().map(|s| s.text.width()).sum();
            let fill_w = width.saturating_sub(left_w + right_w).max(2);
            left.push(PreviewSpan::fg(fill_ch.repeat(fill_w), p.muted, false));
            left.extend(right);

            let line2 = PreviewLine::new(vec![
                PreviewSpan::fg(box_bot, p.muted, false),
                prompt_char,
                cmd_span,
            ]);
            vec![PreviewLine::new(left), line2]
        }
        "ribbon" => {
            let mut line1 = vec![
                PreviewSpan::pill(format!(" {user} "), p.on_accent, user_color, true),
                PreviewSpan::pill(format!("@{host} "), p.host, p.surface_alt, false),
                PreviewSpan::plain(" "),
                PreviewSpan::pill(format!(" {dir} "), p.dir, p.surface, true),
            ];
            if show_git {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::pill(" git:main ", p.git, p.surface_alt, true));
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::pill(
                    format!(" {git_status_txt} "),
                    p.git_status,
                    p.surface,
                    false,
                ));
            }
            if is_error {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::pill(
                    format!(" {err_sym} 127 "),
                    p.on_accent,
                    p.err,
                    true,
                ));
            }
            if scenario == PreviewScenario::Normal {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::pill(" 1.4s ", p.duration, p.surface, false));
            }
            if show_time {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::pill(" 16:20:05 ", p.time, p.surface, false));
            }
            let line2 = PreviewLine::new(vec![prompt_char, cmd_span]);
            vec![PreviewLine::new(line1), line2]
        }
        "powerline" => {
            let mut line1 = vec![
                PreviewSpan::fg(pl_l, p.surface_alt, false),
                PreviewSpan::pill(format!(" {user}"), user_color, p.surface_alt, true),
                PreviewSpan::pill(format!("@{host} "), p.host, p.surface_alt, false),
                PreviewSpan::pill(pl_sep, p.surface_alt, p.surface, false),
                PreviewSpan::pill(format!(" {dir} "), p.dir, p.surface, true),
                PreviewSpan::fg(pl_r, p.surface, false),
            ];
            if show_git {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::fg(pl_l, p.surface_alt, false));
                line1.push(PreviewSpan::pill(
                    format!(" {pl_git}main "),
                    p.git,
                    p.surface_alt,
                    true,
                ));
                line1.push(PreviewSpan::fg(pl_r, p.surface_alt, false));
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::fg(pl_l, p.surface, false));
                line1.push(PreviewSpan::pill(
                    format!(" {git_status_txt} "),
                    p.git_status,
                    p.surface,
                    false,
                ));
                line1.push(PreviewSpan::fg(pl_r, p.surface, false));
            }
            if is_error {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::fg(pl_l, p.err, false));
                line1.push(PreviewSpan::pill(
                    format!(" {err_sym} 127 "),
                    p.on_accent,
                    p.err,
                    true,
                ));
                line1.push(PreviewSpan::fg(pl_r, p.err, false));
            }
            if scenario == PreviewScenario::Normal {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::fg(pl_l, p.surface, false));
                line1.push(PreviewSpan::pill(" 1.4s ", p.duration, p.surface, false));
                line1.push(PreviewSpan::fg(pl_r, p.surface, false));
            }
            if show_time {
                line1.push(PreviewSpan::plain(" "));
                line1.push(PreviewSpan::fg(pl_l, p.surface, false));
                line1.push(PreviewSpan::pill(" 16:20:05 ", p.time, p.surface, false));
                line1.push(PreviewSpan::fg(pl_r, p.surface, false));
            }
            let line2 = PreviewLine::new(vec![
                PreviewSpan::fg(box_bot, p.muted, false),
                prompt_char,
                cmd_span,
            ]);
            vec![PreviewLine::new(line1), line2]
        }
        "minimal" => {
            let mut line1 = vec![PreviewSpan::fg(format!("{dir} "), p.dir, true)];
            if show_git {
                line1.push(PreviewSpan::fg("main ", p.git, false));
                line1.push(PreviewSpan::fg(
                    format!("{git_status_txt} "),
                    p.git_status,
                    false,
                ));
            }
            if scenario == PreviewScenario::Normal {
                line1.push(PreviewSpan::fg("1.4s ", p.duration, false));
            }
            if is_error {
                line1.push(PreviewSpan::fg(format!("{err_sym}127 "), p.err, false));
            }
            line1.push(PreviewSpan::fg(
                user,
                if is_root { p.root } else { p.muted },
                is_root,
            ));
            line1.push(PreviewSpan::fg(format!("@{host} "), p.muted, false));
            if show_time {
                line1.push(PreviewSpan::fg("16:20:05", p.time, false));
            }
            let line2 = PreviewLine::new(vec![prompt_char, cmd_span]);
            vec![PreviewLine::new(line1), line2]
        }
        "brackets" => {
            let mut line1 = vec![
                PreviewSpan::fg("[", p.muted, false),
                PreviewSpan::fg(user, user_color, true),
                PreviewSpan::fg(format!("@{host}"), p.host, false),
                PreviewSpan::fg("] [", p.muted, false),
                PreviewSpan::fg(dir, p.dir, true),
                PreviewSpan::fg("] ", p.muted, false),
            ];
            if show_git {
                line1.push(PreviewSpan::fg("[", p.muted, false));
                line1.push(PreviewSpan::fg("git:main", p.git, true));
                line1.push(PreviewSpan::fg("] [", p.muted, false));
                line1.push(PreviewSpan::fg(git_status_txt, p.git_status, false));
                line1.push(PreviewSpan::fg("] ", p.muted, false));
            }
            if is_error {
                line1.push(PreviewSpan::fg("[", p.muted, false));
                line1.push(PreviewSpan::fg(format!("{err_sym} 127"), p.err, true));
                line1.push(PreviewSpan::fg("] ", p.muted, false));
            }
            if scenario == PreviewScenario::Normal {
                line1.push(PreviewSpan::fg("[", p.muted, false));
                line1.push(PreviewSpan::fg("1.4s", p.duration, false));
                line1.push(PreviewSpan::fg("] ", p.muted, false));
            }
            if show_time {
                line1.push(PreviewSpan::fg("[", p.muted, false));
                line1.push(PreviewSpan::fg("16:20:05", p.time, false));
                line1.push(PreviewSpan::fg("]", p.muted, false));
            }
            let line2 = PreviewLine::new(vec![prompt_char, cmd_span]);
            vec![PreviewLine::new(line1), line2]
        }
        _ => {
            let mut line1 = vec![
                PreviewSpan::fg(user, user_color, true),
                PreviewSpan::fg(format!("@{host} "), p.host, false),
                PreviewSpan::fg(format!("{dir} "), p.dir, true),
            ];
            if show_git {
                line1.push(PreviewSpan::fg("git:main ", p.git, true));
                line1.push(PreviewSpan::fg(
                    format!("{git_status_txt} "),
                    p.git_status,
                    false,
                ));
            }
            if is_error {
                line1.push(PreviewSpan::fg(format!("{err_sym} 127 "), p.err, true));
            }
            if scenario == PreviewScenario::Normal {
                line1.push(PreviewSpan::fg("took 1.4s ", p.duration, false));
            }
            if show_time {
                line1.push(PreviewSpan::fg("16:20:05 ", p.time, false));
            }
            let line2 = PreviewLine::new(vec![prompt_char, cmd_span]);
            vec![PreviewLine::new(line1), line2]
        }
    }
}

/// Format preview lines with truecolor ANSI SGR escapes (`color = true`) or plain text (`color = false`).
pub fn format_preview_lines(lines: &[PreviewLine], color: bool) -> String {
    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        for span in &line.spans {
            if !color {
                out.push_str(&span.text);
                continue;
            }
            let mut codes = Vec::new();
            if span.bold {
                codes.push("1".to_owned());
            }
            if span.dim {
                codes.push("2".to_owned());
            }
            if let Some((r, g, b)) = span.fg {
                codes.push(format!("38;2;{r};{g};{b}"));
            }
            if let Some((r, g, b)) = span.bg {
                codes.push(format!("48;2;{r};{g};{b}"));
            }
            if codes.is_empty() {
                out.push_str(&span.text);
            } else {
                out.push_str("\x1b[");
                out.push_str(&codes.join(";"));
                out.push('m');
                out.push_str(&span.text);
                out.push_str("\x1b[0m");
            }
        }
    }
    out
}

fn emit_preview(ctx: &Context, ansi_text: &str, plain_text: &str, use_ansi: bool) {
    if use_ansi && ctx.events.is_none() {
        println!("{ansi_text}");
    } else {
        ctx.log(plain_text);
    }
}

/// Read-only CLI preview of terminal prompt styles and color palettes. Works on all platforms
/// without modifying files or requiring Starship.
pub fn preview(
    ctx: &Context,
    style: &str,
    color: &str,
    time: &str,
    git: &str,
    shell: &str,
) -> Result<()> {
    if !PREVIEW_STYLES.contains(&style) {
        return Err(Error::new("error.input", style));
    }
    if !PREVIEW_COLORS.contains(&color) {
        return Err(Error::new("error.input", color));
    }
    if !["on", "off"].contains(&time) {
        return Err(Error::new("error.input", time));
    }
    if !["on", "off"].contains(&git) {
        return Err(Error::new("error.input", git));
    }
    if !SHELLS.contains(&shell) {
        return Err(Error::new("error.input", shell));
    }

    let use_ansi = ctx.events.is_none()
        && std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none();
    let host = sysinfo::System::host_name().unwrap_or_else(|| "vps-sg-01".into());
    let i = &ctx.i18n;

    if style == "all" {
        let palette_id = if color == "all" { "tokyo-night" } else { color };
        let header = format!(
            "{} · {} = {palette_id}",
            i.text("ui.prompt_gallery_styles"),
            i.text("field.color")
        );
        emit_preview(ctx, &format!("\x1b[1m{header}\x1b[0m"), &header, use_ansi);
        for &st in STYLES {
            let opts = Options {
                shell: shell.into(),
                style: st.into(),
                color: palette_id.into(),
                time: time.into(),
                git: git.into(),
            };
            let meta = style_meta(st);
            let font_note = if meta.nerd_font {
                i.text("ui.prompt_nerd_font")
            } else {
                i.text("ui.prompt_unicode_ok")
            };
            let label = format!(
                "\n[{st}] {} — {font_note}",
                i.text(&format!("prompt.style.{st}.name"))
            );
            let lines = preview_lines(&opts, &host, PreviewScenario::Normal, 72, false);
            emit_preview(
                ctx,
                &format!(
                    "\x1b[2m{label}\x1b[0m\n{}",
                    format_preview_lines(&lines, true)
                ),
                &format!("{label}\n{}", format_preview_lines(&lines, false)),
                use_ansi,
            );
        }
        return Ok(());
    }

    if color == "all" {
        let header = format!(
            "{} · {} = {style}",
            i.text("ui.prompt_gallery_colors"),
            i.text("field.style")
        );
        emit_preview(ctx, &format!("\x1b[1m{header}\x1b[0m"), &header, use_ansi);
        for &col in COLORS {
            let opts = Options {
                shell: shell.into(),
                style: style.into(),
                color: col.into(),
                time: time.into(),
                git: git.into(),
            };
            let label = format!("\n[{col}] {}", i.text(&format!("prompt.color.{col}.name")));
            let lines = preview_lines(&opts, &host, PreviewScenario::Normal, 72, false);
            emit_preview(
                ctx,
                &format!(
                    "\x1b[2m{label}\x1b[0m\n{}",
                    format_preview_lines(&lines, true)
                ),
                &format!("{label}\n{}", format_preview_lines(&lines, false)),
                use_ansi,
            );
        }
        return Ok(());
    }

    let opts = Options {
        shell: shell.into(),
        style: style.into(),
        color: color.into(),
        time: time.into(),
        git: git.into(),
    };
    opts.validate()?;
    let meta = style_meta(style);
    let font_note = if meta.nerd_font {
        i.text("ui.prompt_nerd_font")
    } else {
        i.text("ui.prompt_unicode_ok")
    };
    let title = format!(
        "{} · {style} ({}) · {color} ({}) · {font_note}",
        i.text("ui.prompt_preview_title"),
        i.text(&format!("prompt.style.{style}.name")),
        i.text(&format!("prompt.color.{color}.name")),
    );
    emit_preview(ctx, &format!("\x1b[1m{title}\x1b[0m"), &title, use_ansi);

    for (scenario, key) in [
        (PreviewScenario::Normal, "ui.prompt_scene_normal"),
        (PreviewScenario::Error, "ui.prompt_scene_error"),
        (PreviewScenario::Root, "ui.prompt_scene_root"),
    ] {
        let heading = format!("\n# {}", i.text(key));
        let lines = preview_lines(&opts, &host, scenario, 72, false);
        emit_preview(
            ctx,
            &format!(
                "\x1b[2m{heading}\x1b[0m\n{}",
                format_preview_lines(&lines, true)
            ),
            &format!("{heading}\n{}", format_preview_lines(&lines, false)),
            use_ansi,
        );
    }

    let apply_cmd = format!(
        "\ncqs run terminal.apply --set shell={shell} --set style={style} --set color={color} --set time={time} --set git={git}"
    );
    emit_preview(
        ctx,
        &format!("\x1b[2m{apply_cmd}\x1b[0m"),
        &apply_cmd,
        use_ansi,
    );
    Ok(())
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

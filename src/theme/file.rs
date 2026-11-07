use super::*;
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};
use toml::{Value, map::Map};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeError {
    Id {
        path: PathBuf,
        id: String,
    },
    Io {
        path: PathBuf,
        detail: String,
    },
    Toml {
        path: PathBuf,
        detail: String,
    },
    Missing {
        path: PathBuf,
        key: String,
    },
    Unknown {
        path: PathBuf,
        key: String,
    },
    Color {
        path: PathBuf,
        key: String,
        value: String,
    },
    Enum {
        path: PathBuf,
        key: String,
        value: String,
    },
}

impl fmt::Display for ThemeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id { path, id } => write!(
                formatter,
                "{}: invalid or reserved theme ID {id:?}",
                path.display()
            ),
            Self::Io { path, detail } | Self::Toml { path, detail } => {
                write!(formatter, "{}: {detail}", path.display())
            }
            Self::Missing { path, key } => write!(formatter, "{}: missing {key}", path.display()),
            Self::Unknown { path, key } => write!(formatter, "{}: unknown {key}", path.display()),
            Self::Color { path, key, value } => write!(
                formatter,
                "{}: invalid color {key} = {value}",
                path.display()
            ),
            Self::Enum { path, key, value } => write!(
                formatter,
                "{}: invalid value {key} = {value}",
                path.display()
            ),
        }
    }
}
impl std::error::Error for ThemeError {}

#[derive(Debug, Clone)]
pub struct CustomTheme {
    pub id: String,
    pub path: PathBuf,
    pub result: Result<ThemeDefinition, ThemeError>,
}

fn invalid(path: &Path, key: &str, value: &Value) -> ThemeError {
    ThemeError::Enum {
        path: path.into(),
        key: key.into(),
        value: value.to_string(),
    }
}

fn table<'a>(
    value: &'a Value,
    path: &Path,
    prefix: &str,
    names: &[&str],
) -> Result<&'a Map<String, Value>, ThemeError> {
    let table = value
        .as_table()
        .ok_or_else(|| invalid(path, prefix, value))?;
    let key = |name: &str| {
        if prefix.is_empty() {
            name.into()
        } else {
            format!("{prefix}.{name}")
        }
    };
    for name in names {
        if !table.contains_key(*name) {
            return Err(ThemeError::Missing {
                path: path.into(),
                key: key(name),
            });
        }
    }
    if let Some(name) = table.keys().find(|name| !names.contains(&name.as_str())) {
        return Err(ThemeError::Unknown {
            path: path.into(),
            key: key(name),
        });
    }
    Ok(table)
}

fn enumeration<T>(
    table: &Map<String, Value>,
    name: &str,
    path: &Path,
    parse: fn(&str) -> Option<T>,
) -> Result<T, ThemeError> {
    let value = &table[name];
    value
        .as_str()
        .and_then(parse)
        .ok_or_else(|| invalid(path, &format!("patterns.{name}"), value))
}

pub fn parse(id: &str, text: &str) -> Result<ThemeDefinition, ThemeError> {
    parse_at(id, text, &PathBuf::from(format!("{id}.toml")))
}

fn parse_at(id: &str, text: &str, path: &Path) -> Result<ThemeDefinition, ThemeError> {
    if id.is_empty()
        || id.len() > 32
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || builtin::all().iter().any(|theme| theme.id == id)
    {
        return Err(ThemeError::Id {
            path: path.into(),
            id: id.into(),
        });
    }
    let value: Value = toml::from_str(text).map_err(|error: toml::de::Error| ThemeError::Toml {
        path: path.into(),
        detail: error.to_string(),
    })?;
    let root = table(
        &value,
        path,
        "",
        &["name", "background", "colors", "patterns"],
    )?;
    let name = root["name"]
        .as_str()
        .filter(|name| !name.chars().any(char::is_control))
        .ok_or_else(|| invalid(path, "name", &root["name"]))?;
    let background = root["background"]
        .as_str()
        .and_then(BackgroundKind::parse)
        .filter(|kind| *kind != BackgroundKind::Follow)
        .ok_or_else(|| invalid(path, "background", &root["background"]))?;
    let colors = table(&root["colors"], path, "colors", &COLOR_NAMES)?;
    let mut palette = [Color::Default; 24];
    for (index, name) in COLOR_NAMES.iter().enumerate() {
        palette[index] = colors[*name]
            .as_str()
            .and_then(Color::parse)
            .ok_or_else(|| ThemeError::Color {
                path: path.into(),
                key: format!("colors.{name}"),
                value: colors[*name].to_string(),
            })?;
    }
    let patterns = table(&root["patterns"], path, "patterns", &PATTERN_NAMES)?;
    let patterns = Patterns {
        border: enumeration(patterns, "border", path, BorderStyle::parse)?,
        title: enumeration(patterns, "title", path, TitleStyle::parse)?,
        bar: enumeration(patterns, "bar", path, BarStyle::parse)?,
        sparkline: enumeration(patterns, "sparkline", path, SparklineStyle::parse)?,
        selection: enumeration(patterns, "selection", path, SelectionStyle::parse)?,
        checkbox: enumeration(patterns, "checkbox", path, CheckboxStyle::parse)?,
        dot: enumeration(patterns, "dot", path, DotStyle::parse)?,
        rule: enumeration(patterns, "rule", path, RuleStyle::parse)?,
        keys: enumeration(patterns, "keys", path, KeyStyle::parse)?,
        background: enumeration(patterns, "background", path, BackgroundStyle::parse)?,
        gradient: patterns["gradient"]
            .as_bool()
            .ok_or_else(|| invalid(path, "patterns.gradient", &patterns["gradient"]))?,
        tertiary: enumeration(patterns, "tertiary", path, TertiaryStyle::parse)?,
        glyphs: enumeration(patterns, "glyphs", path, GlyphSet::parse)?,
    };
    Ok(ThemeDefinition {
        id: id.into(),
        name: ThemeName::Custom(name.into()),
        palette: PaletteSource::Fixed(Palette::from_colors(palette)),
        patterns,
        background,
    })
}

pub fn load_dir(directory: &Path) -> Vec<CustomTheme> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            return vec![CustomTheme {
                id: String::new(),
                path: directory.into(),
                result: Err(ThemeError::Io {
                    path: directory.into(),
                    detail: error.to_string(),
                }),
            }];
        }
    };
    let mut themes = entries
        .filter_map(|entry| {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    return Some(CustomTheme {
                        id: String::new(),
                        path: directory.into(),
                        result: Err(ThemeError::Io {
                            path: directory.into(),
                            detail: error.to_string(),
                        }),
                    });
                }
            };
            if path.extension().and_then(|value| value.to_str()) != Some("toml") {
                return None;
            }
            let id = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let result = fs::read_to_string(&path)
                .map_err(|error| ThemeError::Io {
                    path: path.clone(),
                    detail: error.to_string(),
                })
                .and_then(|text| parse_at(&id, &text, &path));
            Some(CustomTheme { id, path, result })
        })
        .collect::<Vec<_>>();
    themes.sort_by(|left, right| left.path.cmp(&right.path));
    themes
}

pub fn print(definition: &ThemeDefinition) -> String {
    let PaletteSource::Fixed(palette) = definition.palette else {
        panic!("only fixed palettes can be written");
    };
    let ThemeName::Custom(name) = &definition.name else {
        panic!("a custom theme name is required");
    };
    assert!(matches!(
        definition.background,
        BackgroundKind::Dark | BackgroundKind::Light
    ));
    let mut output = format!(
        "name = {}\nbackground = \"{}\"\n\n[colors]\n",
        Value::String(name.clone()),
        definition.background.name()
    );
    for (name, role) in COLOR_NAMES.iter().zip(Role::ALL) {
        output.push_str(&format!("{name} = \"{}\"\n", palette.get(role).value()));
    }
    output.push_str("\n[patterns]\n");
    for (name, value) in PATTERN_NAMES.iter().zip(definition.patterns.values()) {
        if *name == "gradient" {
            output.push_str(&format!("{name} = {value}\n"));
        } else {
            output.push_str(&format!("{name} = \"{value}\"\n"));
        }
    }
    output
}

#[cfg(test)]
mod tests;

//! Custom theme file loading, strict validation, and TOML printing.

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
    Value {
        path: PathBuf,
        key: String,
        value: String,
    },
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id { path, id } => {
                write!(f, "{}: invalid or reserved theme ID {id:?}", path.display())
            }
            Self::Io { path, detail } | Self::Toml { path, detail } => {
                write!(f, "{}: {detail}", path.display())
            }
            Self::Missing { path, key } => write!(f, "{}: missing {key}", path.display()),
            Self::Unknown { path, key } => write!(f, "{}: unknown {key}", path.display()),
            Self::Color { path, key, value } => {
                write!(f, "{}: invalid color {key} = {value}", path.display())
            }
            Self::Value { path, key, value } => {
                write!(f, "{}: invalid value {key} = {value}", path.display())
            }
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

fn invalid_value(path: &Path, key: &str, value: &Value) -> ThemeError {
    ThemeError::Value {
        path: path.into(),
        key: key.into(),
        value: value.to_string(),
    }
}

fn require_table<'a>(
    value: &'a Value,
    path: &Path,
    prefix: &str,
    required: &[&str],
    optional: &[&str],
) -> Result<&'a Map<String, Value>, ThemeError> {
    let table = value
        .as_table()
        .ok_or_else(|| invalid_value(path, prefix, value))?;
    let key = |name: &str| {
        if prefix.is_empty() {
            name.into()
        } else {
            format!("{prefix}.{name}")
        }
    };
    for name in required {
        if !table.contains_key(*name) {
            return Err(ThemeError::Missing {
                path: path.into(),
                key: key(name),
            });
        }
    }
    if let Some(name) = table
        .keys()
        .find(|name| !required.contains(&name.as_str()) && !optional.contains(&name.as_str()))
    {
        return Err(ThemeError::Unknown {
            path: path.into(),
            key: key(name),
        });
    }
    Ok(table)
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
    let value: Value = toml::from_str(text).map_err(|error| ThemeError::Toml {
        path: path.into(),
        detail: error.to_string(),
    })?;
    let root = require_table(
        &value,
        path,
        "",
        &["name", "appearance", "colors"],
        &["style"],
    )?;
    let name = root["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty() && !name.chars().any(char::is_control))
        .ok_or_else(|| invalid_value(path, "name", &root["name"]))?;
    let appearance = root["appearance"]
        .as_str()
        .and_then(Appearance::parse)
        .ok_or_else(|| invalid_value(path, "appearance", &root["appearance"]))?;
    let default_style = match root.get("style") {
        Some(raw) => raw
            .as_str()
            .and_then(UiStyle::parse)
            .ok_or_else(|| invalid_value(path, "style", raw))?,
        None => UiStyle::Rounded,
    };
    let colors = require_table(&root["colors"], path, "colors", &COLOR_NAMES, &[])?;
    let mut palette = [Color::Default; 20];
    for (index, key) in COLOR_NAMES.iter().enumerate() {
        palette[index] = colors[*key]
            .as_str()
            .and_then(Color::parse)
            .ok_or_else(|| ThemeError::Color {
                path: path.into(),
                key: format!("colors.{key}"),
                value: colors[*key].to_string(),
            })?;
    }
    Ok(ThemeDefinition {
        id: id.into(),
        name: ThemeName::Custom(name.into()),
        appearance,
        palette: PaletteSource::Fixed(Palette::from_colors(palette)),
        default_style,
        paint_bg: true,
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
        panic!("only fixed palettes can be printed");
    };
    let ThemeName::Custom(name) = &definition.name else {
        panic!("a custom theme name is required");
    };
    assert!(matches!(
        definition.appearance,
        Appearance::Dark | Appearance::Light
    ));
    let mut output = format!(
        "name = {}\nappearance = \"{}\"\n",
        Value::String(name.clone()),
        definition.appearance.name()
    );
    if definition.default_style != UiStyle::Rounded {
        output.push_str(&format!("style = \"{}\"\n", definition.default_style.id()));
    }
    output.push_str("\n[colors]\n");
    for (key, role) in COLOR_NAMES.iter().zip(Role::ALL) {
        output.push_str(&format!("{key} = \"{}\"\n", palette.get(role).value()));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_theme_round_trips_and_rejects_invalid_inputs() {
        let mut def = builtin::obsidian();
        def.id = "my-slate".into();
        def.name = ThemeName::Custom("My Slate".into());
        let printed = print(&def);
        let parsed = parse("my-slate", &printed).expect("valid custom theme");
        assert_eq!(parsed, def);

        // Built-in ID is rejected
        assert!(matches!(
            parse("obsidian", &printed),
            Err(ThemeError::Id { .. })
        ));
        // Unknown key is rejected
        assert!(matches!(
            parse("my-slate", &format!("{printed}\nextra = 1\n")),
            Err(ThemeError::Unknown { .. })
        ));
        // Missing key is rejected
        let missing = printed.replacen("danger = \"#fb7185\"\n", "", 1);
        assert!(matches!(
            parse("my-slate", &missing),
            Err(ThemeError::Missing { .. })
        ));
        // Invalid color is rejected
        let bad_color = printed.replacen("#fb7185", "#xyz", 1);
        assert!(matches!(
            parse("my-slate", &bad_color),
            Err(ThemeError::Color { .. })
        ));
        // Optional style defaults to Rounded when omitted
        let without_style = printed.replacen("style = \"bento\"\n", "", 1);
        let parsed_default = parse("my-slate", &without_style).expect("style is optional");
        assert_eq!(parsed_default.default_style, UiStyle::Rounded);
        // Valid explicit style parses cleanly
        let with_hud = printed.replacen("style = \"bento\"", "style = \"hud\"", 1);
        let parsed_hud = parse("my-slate", &with_hud).expect("valid explicit style");
        assert_eq!(parsed_hud.default_style, UiStyle::Hud);
        // Invalid explicit style is rejected with Value error
        let bad_style = printed.replacen("style = \"bento\"", "style = \"neon-x\"", 1);
        assert!(matches!(
            parse("my-slate", &bad_style),
            Err(ThemeError::Value { key, .. }) if key == "style"
        ));
    }
}

use crate::error::{Error, Result};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::Path,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub locale: String,
    pub name: String,
    pub messages: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct I18n {
    selected: String,
    catalogs: BTreeMap<String, Catalog>,
}

pub fn placeholders(value: &str) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    let mut remaining = value;
    while let Some(start) = remaining.find('{') {
        if remaining[..start].contains('}') {
            return Err(Error::new("error.locale", value));
        }
        let end = remaining[start + 1..]
            .find('}')
            .ok_or_else(|| Error::new("error.locale", value))?
            + start
            + 1;
        let key = &remaining[start + 1..end];
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return Err(Error::new("error.locale", value));
        }
        found.insert(key.to_owned());
        remaining = &remaining[end + 1..];
    }
    if remaining.contains('}') {
        return Err(Error::new("error.locale", value));
    }
    Ok(found)
}

pub fn validate(reference: &Catalog, candidate: &Catalog) -> Result<()> {
    if reference.messages.keys().ne(candidate.messages.keys()) {
        let missing: Vec<_> = reference
            .messages
            .keys()
            .filter(|k| !candidate.messages.contains_key(*k))
            .collect();
        let extra: Vec<_> = candidate
            .messages
            .keys()
            .filter(|k| !reference.messages.contains_key(*k))
            .collect();
        return Err(Error::new(
            "error.locale",
            format!("{}: missing={missing:?}; extra={extra:?}", candidate.locale),
        ));
    }
    for (key, text) in &candidate.messages {
        if text.trim().is_empty() || placeholders(text)? != placeholders(&reference.messages[key])?
        {
            return Err(Error::new(
                "error.locale",
                format!("{}: {key}", candidate.locale),
            ));
        }
    }
    Ok(())
}

pub fn normalize(locale: &str) -> String {
    let value = locale
        .split('.')
        .next()
        .unwrap_or(locale)
        .split('@')
        .next()
        .unwrap_or(locale)
        .replace('_', "-");
    match value.to_ascii_lowercase().as_str() {
        "zh" | "zh-cn" | "zh-hans" | "zh-sg" => "zh-CN".into(),
        "en" | "en-us" | "c" | "posix" => "en-US".into(),
        _ => value,
    }
}

impl I18n {
    pub fn load(explicit: Option<&str>, directory: Option<&Path>) -> Result<Self> {
        let english: Catalog = toml::from_str(include_str!("../locales/en-US.toml"))?;
        let chinese: Catalog = toml::from_str(include_str!("../locales/zh-CN.toml"))?;
        validate(&english, &english)?;
        validate(&english, &chinese)?;
        let mut catalogs = BTreeMap::from([
            (english.locale.clone(), english.clone()),
            (chinese.locale.clone(), chinese),
        ]);
        if let Some(directory) = directory {
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                if path.extension().is_none_or(|e| e != "toml") {
                    continue;
                }
                let catalog: Catalog = toml::from_str(&fs::read_to_string(&path)?)?;
                if path.file_stem().and_then(|p| p.to_str()) != Some(&catalog.locale)
                    || catalog.name.trim().is_empty()
                {
                    return Err(Error::new("error.locale", path.display()));
                }
                validate(&english, &catalog)?;
                catalogs.insert(catalog.locale.clone(), catalog);
            }
        }
        let requested = explicit
            .map(str::to_owned)
            .or_else(|| env::var("CQS_LANG").ok());
        let selected = if let Some(requested) = requested {
            let value = normalize(&requested);
            if !catalogs.contains_key(&value) {
                return Err(Error::new("error.locale", requested));
            }
            value
        } else {
            let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .find_map(|k| env::var(k).ok().filter(|v| !v.is_empty()));
            let locale = normalize(locale.as_deref().unwrap_or("en-US"));
            // The OS locale is a preference, not an explicitly requested translation.
            if catalogs.contains_key(&locale) {
                locale
            } else {
                "en-US".into()
            }
        };
        Ok(Self { selected, catalogs })
    }

    pub fn locale(&self) -> &str {
        &self.selected
    }
    pub fn languages(&self) -> Vec<(&str, &str)> {
        self.catalogs
            .values()
            .map(|c| (c.locale.as_str(), c.name.as_str()))
            .collect()
    }
    pub fn select(&mut self, locale: &str) -> Result<()> {
        if !self.catalogs.contains_key(locale) {
            return Err(Error::new("error.locale", locale));
        }
        self.selected = locale.into();
        Ok(())
    }
    pub fn text(&self, key: &str) -> &str {
        self.catalogs[&self.selected]
            .messages
            .get(key)
            .unwrap_or_else(|| panic!("missing translation key: {key}"))
    }
    pub fn format(&self, key: &str, values: &[(&str, &str)]) -> String {
        // Substitute once: parameter values are never interpreted as templates.
        let template = self.text(key);
        let mut out = String::new();
        let mut rest = template;
        while let Some(start) = rest.find('{') {
            let end = rest[start..].find('}').expect("validated catalog") + start;
            out.push_str(&rest[..start]);
            let name = &rest[start + 1..end];
            out.push_str(
                values
                    .iter()
                    .find(|(k, _)| *k == name)
                    .unwrap_or_else(|| panic!("missing parameter {name} for {key}"))
                    .1,
            );
            rest = &rest[end + 1..];
        }
        out.push_str(rest);
        out
    }
    pub fn error(&self, error: &Error) -> String {
        self.format(error.key, &[("detail", &error.detail)])
    }
    pub fn has(&self, key: &str) -> bool {
        self.catalogs[&self.selected].messages.contains_key(key)
    }
}

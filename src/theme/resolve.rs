//! Theme & UI style selection precedence and graceful fallback.

use super::{StyleSelection, ThemeDefinition, ThemeSource, builtin, file::CustomTheme};

pub fn resolve_id(
    flag: Option<&str>,
    environment: Option<&str>,
    config: Option<&str>,
) -> (String, ThemeSource) {
    [
        (flag, ThemeSource::Flag),
        (environment, ThemeSource::Env),
        (config, ThemeSource::Config),
    ]
    .into_iter()
    .find_map(|(value, source)| {
        value
            .filter(|value| !value.is_empty())
            .map(|value| (value.to_owned(), source))
    })
    .unwrap_or_else(|| ("auto".into(), ThemeSource::Default))
}

pub fn select_style(
    flag: Option<&str>,
    environment: Option<&str>,
    config: Option<&str>,
) -> (StyleSelection, ThemeSource, Option<String>) {
    let (id, source) = resolve_id(flag, environment, config);
    match StyleSelection::parse(&id) {
        Some(selection) => (selection, source, None),
        None => (
            StyleSelection::Auto,
            source,
            Some(format!("unknown style ID {id:?}")),
        ),
    }
}

pub fn select(
    id: &str,
    source: ThemeSource,
    builtins: &[ThemeDefinition],
    customs: &[CustomTheme],
) -> (ThemeDefinition, Option<String>) {
    if let Some(definition) = builtins.iter().find(|definition| definition.id == id) {
        return (definition.clone(), None);
    }
    if let Some(custom) = customs.iter().find(|custom| custom.id == id) {
        return match &custom.result {
            Ok(definition) => (definition.clone(), None),
            Err(error) => (builtin::auto(), Some(error.to_string())),
        };
    }
    // Smoothly migrate retired theme IDs saved in config.toml from earlier versions.
    if source == ThemeSource::Config
        && let Some(alias) = builtin::legacy_alias(id)
        && let Some(definition) = builtins.iter().find(|definition| definition.id == alias)
    {
        return (definition.clone(), None);
    }
    if let Some(error) = customs
        .iter()
        .filter(|custom| custom.id.is_empty())
        .find_map(|custom| custom.result.as_ref().err())
    {
        return (builtin::auto(), Some(error.to_string()));
    }
    (builtin::auto(), Some(format!("unknown theme ID {id:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_skips_only_empty_values() {
        for (flag, env, config, expected, source) in [
            (
                Some("obsidian"),
                Some("daylight"),
                Some("rose-pine"),
                "obsidian",
                ThemeSource::Flag,
            ),
            (
                Some(""),
                Some("daylight"),
                Some("rose-pine"),
                "daylight",
                ThemeSource::Env,
            ),
            (
                None,
                Some(""),
                Some("rose-pine"),
                "rose-pine",
                ThemeSource::Config,
            ),
            (None, None, None, "auto", ThemeSource::Default),
        ] {
            assert_eq!(resolve_id(flag, env, config), (expected.into(), source));
        }
    }

    #[test]
    fn unknown_and_invalid_themes_fallback_to_auto() {
        let builtins = builtin::all();
        let (definition, error) = select("missing", ThemeSource::Flag, &builtins, &[]);
        assert_eq!(definition.id, "auto");
        assert!(error.unwrap().contains("missing"));

        let invalid = CustomTheme {
            id: "bad".into(),
            path: "bad.toml".into(),
            result: super::super::file::parse("bad", "bad"),
        };
        let (_, error) = select("bad", ThemeSource::Env, &builtins, &[invalid]);
        assert!(error.unwrap().contains("bad.toml"));
    }

    #[test]
    fn retired_config_theme_ids_migrate_silently() {
        let builtins = builtin::all();
        let (def, err) = select("aurora", ThemeSource::Config, &builtins, &[]);
        assert_eq!(def.id, "auto");
        assert_eq!(err, None);
        let (def, err) = select("classic", ThemeSource::Config, &builtins, &[]);
        assert_eq!(def.id, "obsidian");
        assert_eq!(err, None);
        // Explicit CLI flag with retired ID still reports unknown
        let (def, err) = select("aurora", ThemeSource::Flag, &builtins, &[]);
        assert_eq!(def.id, "auto");
        assert!(err.is_some());
    }

    #[test]
    fn style_selection_precedence_and_unknown_fallback() {
        use crate::theme::UiStyle;
        let (sel, src, err) = select_style(Some("hud"), Some("bento"), Some("atelier"));
        assert_eq!(sel, StyleSelection::Fixed(UiStyle::Hud));
        assert_eq!(src, ThemeSource::Flag);
        assert_eq!(err, None);

        let (sel, src, err) = select_style(Some(""), Some("studio"), Some("atelier"));
        assert_eq!(sel, StyleSelection::Fixed(UiStyle::Studio));
        assert_eq!(src, ThemeSource::Env);
        assert_eq!(err, None);

        let (sel, src, err) = select_style(None, None, Some("editorial"));
        assert_eq!(sel, StyleSelection::Fixed(UiStyle::Editorial));
        assert_eq!(src, ThemeSource::Config);
        assert_eq!(err, None);

        let (sel, src, err) = select_style(None, None, None);
        assert_eq!(sel, StyleSelection::Auto);
        assert_eq!(src, ThemeSource::Default);
        assert_eq!(err, None);

        let (sel, src, err) = select_style(Some("bogus"), None, None);
        assert_eq!(sel, StyleSelection::Auto);
        assert_eq!(src, ThemeSource::Flag);
        assert!(err.unwrap().contains("bogus"));
    }
}

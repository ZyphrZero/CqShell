use super::{ThemeDefinition, ThemeSource, builtin, file::CustomTheme};

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
    .unwrap_or_else(|| ("aurora".into(), ThemeSource::Default))
}

pub fn select(
    id: &str,
    _source: ThemeSource,
    builtins: &[ThemeDefinition],
    customs: &[CustomTheme],
) -> (ThemeDefinition, Option<String>) {
    if let Some(definition) = builtins.iter().find(|definition| definition.id == id) {
        return (definition.clone(), None);
    }
    if let Some(custom) = customs.iter().find(|custom| custom.id == id) {
        return match &custom.result {
            Ok(definition) => (definition.clone(), None),
            Err(error) => (builtin::aurora(), Some(error.to_string())),
        };
    }
    if let Some(error) = customs
        .iter()
        .filter(|custom| custom.id.is_empty())
        .find_map(|custom| custom.result.as_ref().err())
    {
        return (builtin::aurora(), Some(error.to_string()));
    }
    (builtin::aurora(), Some(format!("unknown theme ID {id:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn precedence_skips_only_empty_values() {
        for (flag, env, config, expected, source) in [
            (
                Some("neon"),
                Some("nord"),
                Some("sand"),
                "neon",
                ThemeSource::Flag,
            ),
            (
                Some(""),
                Some("nord"),
                Some("sand"),
                "nord",
                ThemeSource::Env,
            ),
            (None, Some(""), Some("sand"), "sand", ThemeSource::Config),
            (None, None, None, "aurora", ThemeSource::Default),
        ] {
            assert_eq!(resolve_id(flag, env, config), (expected.into(), source));
        }
    }
    #[test]
    fn unknown_and_invalid_themes_explain_the_fallback() {
        let builtins = builtin::all();
        let (definition, error) = select("missing", ThemeSource::Flag, &builtins, &[]);
        assert_eq!(definition.id, "aurora");
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
    fn directory_errors_are_preserved_when_a_custom_theme_cannot_be_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("themes");
        std::fs::write(&directory, "not a directory").unwrap();
        let customs = super::super::file::load_dir(&directory);
        let expected = customs[0].result.as_ref().unwrap_err().to_string();
        let builtins = builtin::all();

        let (definition, error) = select("custom", ThemeSource::Config, &builtins, &customs);
        assert_eq!(definition.id, "aurora");
        assert_eq!(error.as_deref(), Some(expected.as_str()));

        let (definition, error) = select("classic", ThemeSource::Flag, &builtins, &customs);
        assert_eq!(definition.id, "classic");
        assert_eq!(error, None);
    }

    #[test]
    fn missing_directories_and_unrelated_invalid_files_keep_unknown_id_errors() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("themes");
        let builtins = builtin::all();
        for create_directory in [false, true] {
            if create_directory {
                std::fs::create_dir(&directory).unwrap();
                std::fs::write(directory.join("bad.toml"), "name = 4").unwrap();
            }
            let customs = super::super::file::load_dir(&directory);
            let (_, error) = select("missing", ThemeSource::Config, &builtins, &customs);
            assert_eq!(error.as_deref(), Some("unknown theme ID \"missing\""));
        }
    }
}

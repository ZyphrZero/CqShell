use super::*;
use proptest::prelude::*;

fn definitions() -> impl Strategy<Value = ThemeDefinition> {
    let color = prop_oneof![
        Just(Color::Default),
        prop::sample::select(Ansi16::ALL.to_vec()).prop_map(Color::Ansi),
        any::<(u8, u8, u8)>().prop_map(|(red, green, blue)| Color::Rgb(red, green, blue))
    ];
    (
        "custom-[a-z0-9_-]{1,20}",
        prop::collection::vec(
            prop::sample::select(vec!['a', '中', ' ', '\'', '"', '\\', 'é']),
            0..40,
        ),
        prop::collection::vec(color, 24),
        any::<[u8; 12]>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(id, name, colors, indexes, gradient, light)| {
            let pick = |index: usize, count: usize| indexes[index] as usize % count;
            ThemeDefinition {
                id,
                name: ThemeName::Custom(name.into_iter().collect()),
                palette: PaletteSource::Fixed(Palette::from_colors(colors.try_into().unwrap())),
                background: if light {
                    BackgroundKind::Light
                } else {
                    BackgroundKind::Dark
                },
                patterns: Patterns {
                    border: BorderStyle::ALL[pick(0, BorderStyle::ALL.len())],
                    title: TitleStyle::ALL[pick(1, TitleStyle::ALL.len())],
                    bar: BarStyle::ALL[pick(2, BarStyle::ALL.len())],
                    sparkline: SparklineStyle::ALL[pick(3, SparklineStyle::ALL.len())],
                    selection: SelectionStyle::ALL[pick(4, SelectionStyle::ALL.len())],
                    checkbox: CheckboxStyle::ALL[pick(5, CheckboxStyle::ALL.len())],
                    dot: DotStyle::ALL[pick(6, DotStyle::ALL.len())],
                    rule: RuleStyle::ALL[pick(7, RuleStyle::ALL.len())],
                    keys: KeyStyle::ALL[pick(8, KeyStyle::ALL.len())],
                    background: BackgroundStyle::ALL[pick(9, BackgroundStyle::ALL.len())],
                    tertiary: TertiaryStyle::ALL[pick(10, TertiaryStyle::ALL.len())],
                    glyphs: GlyphSet::ALL[pick(11, GlyphSet::ALL.len())],
                    gradient,
                },
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    // Feature: tui-redesign, Property 4: 主题文件往返
    #[test]
    fn theme_files_round_trip(definition in definitions()) {
        prop_assert_eq!(parse(&definition.id, &print(&definition)), Ok(definition));
    }
    // Feature: tui-redesign, Property 5: 主题文件单点变异
    #[test]
    fn one_mutation_reports_its_exact_key(definition in definitions(), mutation in 0u8..4, level in 0u8..3, index in any::<usize>()) {
        let mut raw: Value = toml::from_str(&print(&definition)).unwrap();
        let (prefix, keys): (&str, &[&str]) = match level {
            0 => ("", &["name", "background", "colors", "patterns"]), 1 => ("colors", &COLOR_NAMES), _ => ("patterns", &PATTERN_NAMES),
        };
        let key;
        match mutation {
            0 | 1 => {
                let target = if prefix.is_empty() { raw.as_table_mut().unwrap() } else { raw[prefix].as_table_mut().unwrap() };
                let name = if mutation == 0 { keys[index % keys.len()] } else { "unexpected" };
                key = if prefix.is_empty() { name.to_owned() } else { format!("{prefix}.{name}") };
                if mutation == 0 { target.remove(name); } else { target.insert(name.into(), Value::Integer(1)); }
            }
            2 => {
                let name = COLOR_NAMES[index % COLOR_NAMES.len()];
                raw["colors"][name] = Value::String("#gg0000".into());
                key = format!("colors.{name}");
            }
            _ => {
                let names = PATTERN_NAMES.iter().filter(|name| **name != "gradient").collect::<Vec<_>>();
                if level == 0 { raw["background"] = Value::String("not-a-kind".into()); key = "background".into(); }
                else { let name = *names[index % names.len()]; raw["patterns"][name] = Value::String("not-a-style".into()); key = format!("patterns.{name}"); }
            }
        }
        let error = parse(&definition.id, &toml::to_string(&raw).unwrap()).unwrap_err();
        match (mutation, error) {
            (0, ThemeError::Missing { key: actual, .. }) | (1, ThemeError::Unknown { key: actual, .. }) |
            (2, ThemeError::Color { key: actual, .. }) | (3, ThemeError::Enum { key: actual, .. }) => prop_assert_eq!(actual, key),
            (_, error) => prop_assert!(false, "unexpected error: {error:?}"),
        }
    }
}

#[test]
fn invalid_ids_types_files_and_directories_are_reported() {
    let mut definition = builtin::nord();
    definition.id = "my-nord".into();
    definition.name = ThemeName::Custom("My Nord".into());
    let text = print(&definition);
    for id in [
        "",
        "nord",
        "bad.name",
        "中文",
        "abcdefghijklmnopqrstuvwxyz123456789",
    ] {
        assert!(matches!(parse(id, &text), Err(ThemeError::Id { .. })));
    }
    assert!(matches!(
        parse("good", "name = ["),
        Err(ThemeError::Toml { .. })
    ));
    assert!(
        matches!(parse("good", &text.replace("gradient = false", "gradient = \"false\"")), Err(ThemeError::Enum { key, .. }) if key == "patterns.gradient")
    );
    let temp = tempfile::tempdir().unwrap();
    assert!(load_dir(&temp.path().join("missing")).is_empty());
    fs::write(temp.path().join("z-valid.toml"), &text).unwrap();
    fs::write(temp.path().join("a-broken.toml"), "bad").unwrap();
    fs::create_dir(temp.path().join("unreadable.toml")).unwrap();
    let themes = load_dir(temp.path());
    assert_eq!(
        themes
            .iter()
            .map(|theme| theme.id.as_str())
            .collect::<Vec<_>>(),
        ["a-broken", "unreadable", "z-valid"]
    );
    assert!(matches!(themes[1].result, Err(ThemeError::Io { .. })));
    assert!(themes[2].result.is_ok());
    assert!(
        themes[0]
            .result
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains(&temp.path().display().to_string())
    );
}

#[test]
fn documented_nord_example_is_the_printer_output() {
    let document =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/THEMES.md")).unwrap();
    let document = document.replace("\r\n", "\n");
    let example = document
        .split("```toml\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let mut definition = builtin::nord();
    definition.id = "my-nord".into();
    definition.name = ThemeName::Custom("My Nord".into());
    assert_eq!(example, print(&definition));
}

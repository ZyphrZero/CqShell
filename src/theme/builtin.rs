//! Built-in themes. Palettes and pattern sets follow the role and pattern tables of the
//! theme design.

use super::{
    Ansi16, BackgroundKind, BackgroundStyle, BarStyle, BorderStyle, CheckboxStyle, Color, DotStyle,
    GlyphSet, KeyStyle, Palette, PaletteSource, Patterns, RuleStyle, SelectionStyle,
    SparklineStyle, TertiaryStyle, ThemeDefinition, ThemeName, TitleStyle,
};

const DEFAULT: Color = Color::Default;
const BLACK: Color = Color::Ansi(Ansi16::Black);

/// `0xrrggbb` as an RGB color, so values read like the design tables.
const fn hex(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// classic on a dark or undetected background: exactly the colors the TUI drew before
/// themes existed, so the first delivery step renders cell for cell as before.
pub const CLASSIC_DARK: Palette = Palette {
    background: DEFAULT,
    surface: DEFAULT,
    surface_alt: DEFAULT,
    selection: hex(0x45cdb6),
    selection_text: BLACK,
    selection_mark: BLACK,
    dialog: DEFAULT,
    dialog_selection: hex(0x45cdb6),
    border: hex(0x3d5265),
    border_focus: hex(0x45cdb6),
    rule: hex(0x3d5265),
    shadow: DEFAULT,
    dim: hex(0x3d5265),
    text1: DEFAULT,
    text2: hex(0x91a2b0),
    text3: hex(0x91a2b0),
    accent: hex(0x45cdb6),
    accent2: hex(0x45cdb6),
    on_accent: BLACK,
    meter: hex(0x5dd888),
    info: hex(0x70aaff),
    success: hex(0x5dd888),
    warning: hex(0xffbe5e),
    danger: hex(0xff7585),
};

/// classic on a detected light background, where the dark variant's muted text and accent
/// fall to about 2.6:1 and 2.0:1 against white.
pub const CLASSIC_LIGHT: Palette = Palette {
    background: DEFAULT,
    surface: DEFAULT,
    surface_alt: DEFAULT,
    selection: hex(0x0a7d70),
    selection_text: hex(0xffffff),
    selection_mark: hex(0xffffff),
    dialog: DEFAULT,
    dialog_selection: hex(0x0a7d70),
    border: hex(0x8c959f),
    border_focus: hex(0x0a7d70),
    rule: hex(0xd0d7de),
    shadow: DEFAULT,
    dim: hex(0xc4ccd4),
    text1: DEFAULT,
    text2: hex(0x57606a),
    text3: hex(0x57606a),
    accent: hex(0x0a7d70),
    accent2: hex(0x0a7d70),
    on_accent: hex(0xffffff),
    meter: hex(0x1a7f37),
    info: hex(0x0969da),
    success: hex(0x1a7f37),
    warning: hex(0x9a6700),
    danger: hex(0xcf222e),
};

pub const CLASSIC_PATTERNS: Patterns = Patterns {
    border: BorderStyle::Rounded,
    title: TitleStyle::Inline,
    bar: BarStyle::Line,
    sparkline: SparklineStyle::Blocks,
    selection: SelectionStyle::Fill,
    checkbox: CheckboxStyle::Box,
    dot: DotStyle::Circle,
    rule: RuleStyle::Solid,
    keys: KeyStyle::Plain,
    background: BackgroundStyle::Terminal,
    gradient: false,
    tertiary: TertiaryStyle::Color,
    glyphs: GlyphSet::Unicode,
};

pub fn classic() -> ThemeDefinition {
    ThemeDefinition {
        id: "classic".into(),
        name: ThemeName::Builtin("theme.classic.name"),
        palette: PaletteSource::Adaptive {
            dark: CLASSIC_DARK,
            light: CLASSIC_LIGHT,
        },
        patterns: CLASSIC_PATTERNS,
        background: BackgroundKind::Follow,
    }
}

/// Every built-in theme, in the order the Style_Panel and `cqs themes` list them.
pub fn all() -> Vec<ThemeDefinition> {
    vec![
        aurora(),
        classic(),
        phosphor(),
        neon(),
        nord(),
        sand(),
        high_contrast(),
        plain(),
    ]
}

pub fn reference_colors(theme: &super::Theme, background: &super::Background) -> (Color, Color) {
    let light =
        matches!(background, super::Background::Detected(rgb) if super::derive::is_light(*rgb));
    if theme.id == "plain" {
        return if light {
            (hex(0xffffff), hex(0x000000))
        } else {
            (hex(0x000000), hex(0xe5e5e5))
        };
    }
    if theme.palette.background != Color::Default {
        return (theme.palette.background, theme.palette.text1);
    }
    if theme.id == "classic" && light {
        (hex(0xffffff), hex(0x1f2328))
    } else {
        (hex(0x090e16), hex(0xe5edf3))
    }
}

pub fn aurora() -> ThemeDefinition {
    ThemeDefinition {
        id: "aurora".into(),
        name: ThemeName::Builtin("theme.aurora.name"),
        palette: PaletteSource::Derived,
        background: BackgroundKind::Follow,
        patterns: Patterns {
            border: BorderStyle::Card,
            title: TitleStyle::Label,
            selection: SelectionStyle::Bar,
            keys: KeyStyle::Keycap,
            ..CLASSIC_PATTERNS
        },
    }
}

fn fixed(id: &str, name: &'static str, colors: [u32; 24], patterns: Patterns) -> ThemeDefinition {
    ThemeDefinition {
        id: id.into(),
        name: ThemeName::Builtin(name),
        background: BackgroundKind::Dark,
        palette: PaletteSource::Fixed(Palette::from_colors(colors.map(hex))),
        patterns,
    }
}

pub fn phosphor() -> ThemeDefinition {
    fixed(
        "phosphor",
        "theme.phosphor.name",
        [
            0x030a05, 0x06150a, 0x0c2814, 0x35cc60, 0x03120a, 0x03120a, 0x0c2814, 0x35cc60,
            0x17602d, 0x4dff7a, 0x17602d, 0x000000, 0x0f3a1c, 0x52ff82, 0x35cc60, 0x1f8a3e,
            0xb4ffcb, 0x52ff82, 0x03120a, 0x52ff82, 0xb4ffcb, 0x52ff82, 0xffcf3f, 0xff6a45,
        ],
        Patterns {
            border: BorderStyle::Double,
            title: TitleStyle::Bracket,
            bar: BarStyle::Block,
            selection: SelectionStyle::Inverse,
            checkbox: CheckboxStyle::Filled,
            dot: DotStyle::Square,
            rule: RuleStyle::Dotted,
            keys: KeyStyle::Bracket,
            background: BackgroundStyle::Paint,
            ..CLASSIC_PATTERNS
        },
    )
}

pub fn neon() -> ThemeDefinition {
    fixed(
        "neon",
        "theme.neon.name",
        [
            0x0c0720, 0x140c2c, 0x21144a, 0x21144a, 0x00e5ff, 0xff3df2, 0x1a1038, 0x21144a,
            0x3a2a6b, 0xff3df2, 0x3a2a6b, 0x05030d, 0x2a2050, 0xf3ecff, 0xb7a6e4, 0x7663ad,
            0x00e5ff, 0xff3df2, 0x0c0720, 0x00e5ff, 0x7aa2ff, 0x3dffb8, 0xffe24d, 0xff4f7b,
        ],
        Patterns {
            border: BorderStyle::Heavy,
            title: TitleStyle::Arrow,
            sparkline: SparklineStyle::Braille,
            selection: SelectionStyle::Bar,
            checkbox: CheckboxStyle::Diamond,
            dot: DotStyle::Diamond,
            rule: RuleStyle::Dashed,
            keys: KeyStyle::Keycap,
            background: BackgroundStyle::Paint,
            gradient: true,
            ..CLASSIC_PATTERNS
        },
    )
}

pub fn nord() -> ThemeDefinition {
    fixed(
        "nord",
        "theme.nord.name",
        [
            0x2e3440, 0x3b4252, 0x434c5e, 0x434c5e, 0xeceff4, 0x8fc4d3, 0x3b4252, 0x434c5e,
            0x4c566a, 0x8fc4d3, 0x4c566a, 0x242933, 0x4c566a, 0xeceff4, 0xd8dee9, 0x8f9bb3,
            0x8fc4d3, 0x81a1c1, 0x2e3440, 0x8fc4d3, 0xa7bed4, 0xabc496, 0xebcb8b, 0xdfb0b4,
        ],
        Patterns {
            border: BorderStyle::Square,
            bar: BarStyle::Segment,
            selection: SelectionStyle::Bar,
            checkbox: CheckboxStyle::Radio,
            keys: KeyStyle::Keycap,
            background: BackgroundStyle::Paint,
            ..CLASSIC_PATTERNS
        },
    )
}

pub fn sand() -> ThemeDefinition {
    fixed(
        "sand",
        "theme.sand.name",
        [
            0x282828, 0x32302f, 0x3c3836, 0x32302f, 0xfe8019, 0xfe8019, 0x3c3836, 0x3c3836,
            0x504945, 0xfe8019, 0x504945, 0x1d2021, 0x504945, 0xebdbb2, 0xd5c4a1, 0x928374,
            0xfe8019, 0xfabd2f, 0x282828, 0xfabd2f, 0x88a99c, 0xb8bb26, 0xfabd2f, 0xfc7b6c,
        ],
        Patterns {
            border: BorderStyle::Titlebar,
            title: TitleStyle::Bar,
            bar: BarStyle::Square,
            selection: SelectionStyle::Arrow,
            checkbox: CheckboxStyle::Square,
            rule: RuleStyle::None,
            background: BackgroundStyle::Paint,
            ..CLASSIC_PATTERNS
        },
    )
}

pub fn high_contrast() -> ThemeDefinition {
    fixed(
        "high-contrast",
        "theme.high-contrast.name",
        [
            0x000000, 0x000000, 0x000000, 0xffd400, 0x000000, 0x000000, 0x000000, 0xffd400,
            0xffffff, 0xffd400, 0xffffff, 0x000000, 0x5a5a5a, 0xffffff, 0xe6e6e6, 0xbdbdbd,
            0xffd400, 0xffd400, 0x000000, 0xffd400, 0x6cc7ff, 0x00ff66, 0xff9f1a, 0xff5c5c,
        ],
        Patterns {
            border: BorderStyle::Heavy,
            bar: BarStyle::Block,
            keys: KeyStyle::Bracket,
            background: BackgroundStyle::Paint,
            ..CLASSIC_PATTERNS
        },
    )
}

pub fn plain() -> ThemeDefinition {
    let mut palette = Palette::from_colors([Color::Default; 24]);
    palette.info = Color::Ansi(Ansi16::Cyan);
    palette.success = Color::Ansi(Ansi16::Green);
    palette.warning = Color::Ansi(Ansi16::Yellow);
    palette.danger = Color::Ansi(Ansi16::Red);
    ThemeDefinition {
        id: "plain".into(),
        name: ThemeName::Builtin("theme.plain.name"),
        background: BackgroundKind::Follow,
        palette: PaletteSource::Fixed(palette),
        patterns: Patterns {
            border: BorderStyle::Ascii,
            bar: BarStyle::Hash,
            sparkline: SparklineStyle::Ascii,
            selection: SelectionStyle::Arrow,
            checkbox: CheckboxStyle::Ascii,
            dot: DotStyle::Ascii,
            rule: RuleStyle::Ascii,
            keys: KeyStyle::Bracket,
            tertiary: TertiaryStyle::Dim,
            glyphs: GlyphSet::Ascii,
            ..CLASSIC_PATTERNS
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::I18n;

    #[test]
    fn classic_dark_keeps_the_colors_drawn_before_themes() {
        // The constants of the pre-theme `tui/view/theme.rs`, by the role that replaces them.
        let (text, muted) = (Color::Default, Color::Rgb(145, 162, 176));
        let border = Color::Rgb(61, 82, 101);
        let accent = Color::Rgb(69, 205, 182);
        let on_tone = Color::Ansi(Ansi16::Black);
        let p = CLASSIC_DARK;
        assert_eq!([p.text1, p.text2, p.text3], [text, muted, muted]);
        assert_eq!([p.border, p.rule, p.dim], [border; 3]);
        assert_eq!(
            [p.accent, p.border_focus, p.selection, p.dialog_selection],
            [accent; 4]
        );
        assert_eq!([p.on_accent, p.selection_text], [on_tone; 2]);
        assert_eq!(p.info, Color::Rgb(112, 170, 255));
        assert_eq!([p.success, p.meter], [Color::Rgb(93, 216, 136); 2]);
        assert_eq!(p.warning, Color::Rgb(255, 190, 94));
        assert_eq!(p.danger, Color::Rgb(255, 117, 133));
    }

    #[test]
    fn builtin_names_exist_in_every_locale() {
        let mut i18n = I18n::load(Some("en-US"), None).unwrap();
        for locale in ["en-US", "zh-CN"] {
            i18n.select(locale).unwrap();
            for def in all() {
                let ThemeName::Builtin(key) = def.name else {
                    panic!("{} has no locale key", def.id);
                };
                assert!(i18n.has(key), "{locale}: {key}");
            }
        }
    }
}

#[cfg(test)]
mod contrast;

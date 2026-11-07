//! Theme data: colors, palette roles and pattern sets, resolved against the terminal
//! background. Free of ratatui types so `cqs themes` works without starting the TUI.

pub mod builtin;
pub mod derive;
pub mod file;
pub mod resolve;
mod vocabulary;
pub use vocabulary::{COLOR_NAMES, PATTERN_NAMES};

/// A theme color. `Default` is the terminal's own foreground or background color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Color {
    Default,
    Ansi(Ansi16),
    Rgb(u8, u8, u8),
}

/// The 16 ANSI colors in palette index order: 0–7 normal, 8–15 bright.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ansi16 {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

impl Ansi16 {
    /// Every color in index order, so `ALL[i]` is ANSI color `i`.
    pub const ALL: [Self; 16] = [
        Self::Black,
        Self::Red,
        Self::Green,
        Self::Yellow,
        Self::Blue,
        Self::Magenta,
        Self::Cyan,
        Self::White,
        Self::BrightBlack,
        Self::BrightRed,
        Self::BrightGreen,
        Self::BrightYellow,
        Self::BrightBlue,
        Self::BrightMagenta,
        Self::BrightCyan,
        Self::BrightWhite,
    ];

    /// The name written in theme files and snapshots.
    pub fn name(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Red => "red",
            Self::Green => "green",
            Self::Yellow => "yellow",
            Self::Blue => "blue",
            Self::Magenta => "magenta",
            Self::Cyan => "cyan",
            Self::White => "white",
            Self::BrightBlack => "bright-black",
            Self::BrightRed => "bright-red",
            Self::BrightGreen => "bright-green",
            Self::BrightYellow => "bright-yellow",
            Self::BrightBlue => "bright-blue",
            Self::BrightMagenta => "bright-magenta",
            Self::BrightCyan => "bright-cyan",
            Self::BrightWhite => "bright-white",
        }
    }
}

/// Semantic color roles. Fields follow the role order of theme files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Whole-screen fill when `patterns.background` is `Paint`.
    pub background: Color,
    /// Cards and panels.
    pub surface: Color,
    /// Title bars and keycaps.
    pub surface_alt: Color,
    /// Selected-row fill.
    pub selection: Color,
    /// Text on the selected row.
    pub selection_text: Color,
    /// One-cell bar or marker on the selected row.
    pub selection_mark: Color,
    /// Dialog fill.
    pub dialog: Color,
    /// Selected-row fill inside dialogs.
    pub dialog_selection: Color,
    pub border: Color,
    /// Border of the focused area.
    pub border_focus: Color,
    /// Separators and section rules.
    pub rule: Color,
    pub shadow: Color,
    /// Foreground of the scrim behind dialogs.
    pub dim: Color,
    /// Primary, secondary and tertiary Text_Level.
    pub text1: Color,
    pub text2: Color,
    pub text3: Color,
    pub accent: Color,
    /// Gradient end and secondary markers.
    pub accent2: Color,
    /// Text on an accent background.
    pub on_accent: Color,
    /// Gauge fill below the warning threshold.
    pub meter: Color,
    pub info: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

/// A palette role, named after the `Palette` field it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Background,
    Surface,
    SurfaceAlt,
    Selection,
    SelectionText,
    SelectionMark,
    Dialog,
    DialogSelection,
    Border,
    BorderFocus,
    Rule,
    Shadow,
    Dim,
    Text1,
    Text2,
    Text3,
    Accent,
    Accent2,
    OnAccent,
    Meter,
    Info,
    Success,
    Warning,
    Danger,
}

impl Role {
    /// Every role in the field order of `Palette` and of theme files.
    pub const ALL: [Self; 24] = [
        Self::Background,
        Self::Surface,
        Self::SurfaceAlt,
        Self::Selection,
        Self::SelectionText,
        Self::SelectionMark,
        Self::Dialog,
        Self::DialogSelection,
        Self::Border,
        Self::BorderFocus,
        Self::Rule,
        Self::Shadow,
        Self::Dim,
        Self::Text1,
        Self::Text2,
        Self::Text3,
        Self::Accent,
        Self::Accent2,
        Self::OnAccent,
        Self::Meter,
        Self::Info,
        Self::Success,
        Self::Warning,
        Self::Danger,
    ];
}

impl Palette {
    pub fn from_colors(colors: [Color; 24]) -> Self {
        let [
            background,
            surface,
            surface_alt,
            selection,
            selection_text,
            selection_mark,
            dialog,
            dialog_selection,
            border,
            border_focus,
            rule,
            shadow,
            dim,
            text1,
            text2,
            text3,
            accent,
            accent2,
            on_accent,
            meter,
            info,
            success,
            warning,
            danger,
        ] = colors;
        Self {
            background,
            surface,
            surface_alt,
            selection,
            selection_text,
            selection_mark,
            dialog,
            dialog_selection,
            border,
            border_focus,
            rule,
            shadow,
            dim,
            text1,
            text2,
            text3,
            accent,
            accent2,
            on_accent,
            meter,
            info,
            success,
            warning,
            danger,
        }
    }
    /// The color this palette gives `role`.
    pub fn get(&self, role: Role) -> Color {
        match role {
            Role::Background => self.background,
            Role::Surface => self.surface,
            Role::SurfaceAlt => self.surface_alt,
            Role::Selection => self.selection,
            Role::SelectionText => self.selection_text,
            Role::SelectionMark => self.selection_mark,
            Role::Dialog => self.dialog,
            Role::DialogSelection => self.dialog_selection,
            Role::Border => self.border,
            Role::BorderFocus => self.border_focus,
            Role::Rule => self.rule,
            Role::Shadow => self.shadow,
            Role::Dim => self.dim,
            Role::Text1 => self.text1,
            Role::Text2 => self.text2,
            Role::Text3 => self.text3,
            Role::Accent => self.accent,
            Role::Accent2 => self.accent2,
            Role::OnAccent => self.on_accent,
            Role::Meter => self.meter,
            Role::Info => self.info,
            Role::Success => self.success,
            Role::Warning => self.warning,
            Role::Danger => self.danger,
        }
    }
}

/// Text_Level: how prominent a piece of text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Primary,
    Secondary,
    Tertiary,
}

impl Level {
    /// The palette role text at this level is drawn in.
    pub fn role(self) -> Role {
        match self {
            Self::Primary => Role::Text1,
            Self::Secondary => Role::Text2,
            Self::Tertiary => Role::Text3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    Rounded,
    Square,
    Double,
    Heavy,
    Ascii,
    /// Surface-filled cards without borders; columns are split by a rule-colored line.
    Card,
    /// Surface-filled cards with a `surface_alt` title bar.
    Titlebar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleStyle {
    Inline,
    Bracket,
    Arrow,
    /// Written into the title bar of `BorderStyle::Titlebar`.
    Bar,
    /// A tertiary label inside the card.
    Label,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarStyle {
    Line,
    Block,
    Segment,
    Square,
    Hash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SparklineStyle {
    Blocks,
    Braille,
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionStyle {
    Fill,
    Bar,
    Inverse,
    Arrow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckboxStyle {
    Box,
    Filled,
    Diamond,
    Radio,
    Square,
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DotStyle {
    Circle,
    Diamond,
    Square,
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleStyle {
    Solid,
    Dotted,
    Dashed,
    None,
    Ascii,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStyle {
    Keycap,
    Plain,
    Bracket,
}

/// How cells outside every Surface are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundStyle {
    /// Keep the terminal background.
    Terminal,
    /// Fill every cell with `palette.background`.
    Paint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TertiaryStyle {
    /// Draw `text3` in its own color.
    Color,
    /// Draw `text3` with the DIM modifier.
    Dim,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphSet {
    Unicode,
    Ascii,
}

/// Pattern_Set: every field is a closed set of styles. Fields follow the order of theme files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Patterns {
    pub border: BorderStyle,
    pub title: TitleStyle,
    pub bar: BarStyle,
    pub sparkline: SparklineStyle,
    pub selection: SelectionStyle,
    pub checkbox: CheckboxStyle,
    pub dot: DotStyle,
    pub rule: RuleStyle,
    pub keys: KeyStyle,
    pub background: BackgroundStyle,
    /// Bar fills blend from `accent` to `accent2`; RGB colors only.
    pub gradient: bool,
    pub tertiary: TertiaryStyle,
    pub glyphs: GlyphSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteSource {
    Fixed(Palette),
    /// `light` on a detected light background, `dark` otherwise.
    Adaptive {
        dark: Palette,
        light: Palette,
    },
    /// Derived from the detected background color.
    Derived,
}

/// The background a theme is designed for, shown in the Style_Panel summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackgroundKind {
    /// Follows the terminal background.
    Follow,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeName {
    /// A key into the locale files.
    Builtin(&'static str),
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDefinition {
    pub id: String,
    pub name: ThemeName,
    pub palette: PaletteSource,
    pub patterns: Patterns,
    pub background: BackgroundKind,
}

/// The terminal background as far as detection could tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Background {
    Detected((u8, u8, u8)),
    /// Detection ran but produced no color; the text says why.
    Unavailable(String),
    /// Detection is turned off in config.toml.
    Disabled,
}

/// A theme resolved against the background, ready for the Renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: String,
    pub palette: Palette,
    pub patterns: Patterns,
}

/// Where the theme ID in effect came from, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    /// `--theme`.
    Flag,
    /// `CQS_THEME`.
    Env,
    /// `theme` in config.toml.
    Config,
    /// None of the above: the built-in default.
    Default,
}

/// The TUI's theme state. The Renderer draws with `active`; changing it takes effect from
/// the next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeState {
    /// Built-in themes, then custom ones.
    pub definitions: Vec<ThemeDefinition>,
    pub active: Theme,
    /// The ID saved in config.toml, if any.
    pub saved_id: Option<String>,
    pub source: ThemeSource,
    /// Why the requested theme could not be used.
    pub error: Option<String>,
    pub background: Background,
}

pub fn resolve(def: &ThemeDefinition, bg: &Background) -> Theme {
    let (palette, patterns) = match &def.palette {
        PaletteSource::Fixed(palette) => (*palette, def.patterns),
        PaletteSource::Adaptive { dark, light } => {
            let on_light = matches!(bg, Background::Detected(rgb) if derive::is_light(*rgb));
            (if on_light { *light } else { *dark }, def.patterns)
        }
        PaletteSource::Derived => match bg {
            Background::Detected(rgb) => (derive::aurora(*rgb), def.patterns),
            _ => (builtin::CLASSIC_DARK, builtin::CLASSIC_PATTERNS),
        },
    };
    Theme {
        id: def.id.clone(),
        palette,
        patterns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIGHT: Background = Background::Detected((0xff, 0xff, 0xff));
    const DARK: Background = Background::Detected((0x0b, 0x11, 0x19));

    #[test]
    fn ansi_names_follow_index_order() {
        let names: Vec<_> = Ansi16::ALL.iter().map(|color| color.name()).collect();
        assert_eq!(
            names,
            [
                "black",
                "red",
                "green",
                "yellow",
                "blue",
                "magenta",
                "cyan",
                "white",
                "bright-black",
                "bright-red",
                "bright-green",
                "bright-yellow",
                "bright-blue",
                "bright-magenta",
                "bright-cyan",
                "bright-white",
            ]
        );
    }

    #[test]
    fn each_role_reads_its_own_palette_field() {
        // A distinct color per field, in field order.
        let c = |n: u8| Color::Rgb(n, 0, 0);
        let palette = Palette {
            background: c(0),
            surface: c(1),
            surface_alt: c(2),
            selection: c(3),
            selection_text: c(4),
            selection_mark: c(5),
            dialog: c(6),
            dialog_selection: c(7),
            border: c(8),
            border_focus: c(9),
            rule: c(10),
            shadow: c(11),
            dim: c(12),
            text1: c(13),
            text2: c(14),
            text3: c(15),
            accent: c(16),
            accent2: c(17),
            on_accent: c(18),
            meter: c(19),
            info: c(20),
            success: c(21),
            warning: c(22),
            danger: c(23),
        };
        for (n, role) in Role::ALL.into_iter().enumerate() {
            assert_eq!(palette.get(role), c(n as u8), "{role:?}");
        }
        let levels = [Level::Primary, Level::Secondary, Level::Tertiary].map(Level::role);
        assert_eq!(levels, [Role::Text1, Role::Text2, Role::Text3]);
    }

    #[test]
    fn adaptive_palettes_switch_only_on_a_detected_light_background() {
        let classic = builtin::classic();
        let palette = |bg: &Background| resolve(&classic, bg).palette;
        assert_eq!(palette(&LIGHT), builtin::CLASSIC_LIGHT);
        assert_eq!(palette(&DARK), builtin::CLASSIC_DARK);
        assert_eq!(
            palette(&Background::Unavailable("timeout".into())),
            builtin::CLASSIC_DARK
        );
        assert_eq!(palette(&Background::Disabled), builtin::CLASSIC_DARK);
        let theme = resolve(&classic, &LIGHT);
        assert_eq!(theme.id, "classic");
        assert_eq!(theme.patterns, builtin::CLASSIC_PATTERNS);
    }

    #[test]
    fn fixed_palettes_ignore_the_background() {
        let mut palette = builtin::CLASSIC_DARK;
        palette.background = Color::Rgb(1, 2, 3);
        let def = ThemeDefinition {
            id: "mine".into(),
            name: ThemeName::Custom("Mine".into()),
            palette: PaletteSource::Fixed(palette),
            patterns: Patterns {
                border: BorderStyle::Ascii,
                ..builtin::CLASSIC_PATTERNS
            },
            background: BackgroundKind::Dark,
        };
        for bg in [LIGHT, DARK, Background::Disabled] {
            let theme = resolve(&def, &bg);
            assert_eq!(theme.palette, palette);
            assert_eq!(theme.patterns.border, BorderStyle::Ascii);
        }
    }

    #[test]
    fn derived_palettes_fall_back_to_classic_dark_without_a_detected_background() {
        let def = ThemeDefinition {
            id: "aurora".into(),
            name: ThemeName::Builtin("theme.aurora.name"),
            palette: PaletteSource::Derived,
            patterns: Patterns {
                border: BorderStyle::Card,
                ..builtin::CLASSIC_PATTERNS
            },
            background: BackgroundKind::Follow,
        };
        for bg in [
            Background::Unavailable("timeout".into()),
            Background::Disabled,
        ] {
            assert_eq!(
                resolve(&def, &bg),
                Theme {
                    id: "aurora".into(),
                    palette: builtin::CLASSIC_DARK,
                    patterns: builtin::CLASSIC_PATTERNS,
                }
            );
        }
    }
}

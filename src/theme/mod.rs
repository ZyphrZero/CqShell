//! Layered-surface theme system. Free of ratatui types so `cqs themes` works without starting the
//! TUI.

pub mod builtin;
pub mod derive;
pub mod file;
pub mod resolve;

/// A theme color. `Default` inherits the terminal's own foreground or background color.
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

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.name() == value)
    }
}

impl Color {
    pub fn parse(value: &str) -> Option<Self> {
        if value == "default" {
            return Some(Self::Default);
        }
        if let Some(hex) = value.strip_prefix('#') {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Self::Rgb(r, g, b));
        }
        Ansi16::parse(value).map(Self::Ansi)
    }

    pub fn value(self) -> String {
        match self {
            Self::Default => "default".into(),
            Self::Ansi(ansi) => ansi.name().into(),
            Self::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        }
    }

    pub fn rgb(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::Rgb(r, g, b) => Some((r, g, b)),
            _ => None,
        }
    }
}

/// Ordered color role names matching `[colors]` in custom theme TOML files.
pub const COLOR_NAMES: [&str; 20] = [
    "canvas",
    "sidebar",
    "surface",
    "surface_alt",
    "elevated",
    "selection",
    "border",
    "border_focus",
    "rule",
    "shadow",
    "text1",
    "text2",
    "text3",
    "accent",
    "accent2",
    "on_accent",
    "info",
    "success",
    "warning",
    "danger",
];

/// Semantic color palette built around layered surface elevation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub canvas: Color,
    pub sidebar: Color,
    pub surface: Color,
    pub surface_alt: Color,
    pub elevated: Color,
    pub selection: Color,
    pub border: Color,
    pub border_focus: Color,
    pub rule: Color,
    pub shadow: Color,
    pub text1: Color,
    pub text2: Color,
    pub text3: Color,
    pub accent: Color,
    pub accent2: Color,
    pub on_accent: Color,
    pub info: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Canvas,
    Sidebar,
    Surface,
    SurfaceAlt,
    Elevated,
    Selection,
    Border,
    BorderFocus,
    Rule,
    Shadow,
    Text1,
    Text2,
    Text3,
    Accent,
    Accent2,
    OnAccent,
    Info,
    Success,
    Warning,
    Danger,
}

impl Role {
    pub const ALL: [Self; 20] = [
        Self::Canvas,
        Self::Sidebar,
        Self::Surface,
        Self::SurfaceAlt,
        Self::Elevated,
        Self::Selection,
        Self::Border,
        Self::BorderFocus,
        Self::Rule,
        Self::Shadow,
        Self::Text1,
        Self::Text2,
        Self::Text3,
        Self::Accent,
        Self::Accent2,
        Self::OnAccent,
        Self::Info,
        Self::Success,
        Self::Warning,
        Self::Danger,
    ];
}

impl Palette {
    pub const fn from_colors(c: [Color; 20]) -> Self {
        Self {
            canvas: c[0],
            sidebar: c[1],
            surface: c[2],
            surface_alt: c[3],
            elevated: c[4],
            selection: c[5],
            border: c[6],
            border_focus: c[7],
            rule: c[8],
            shadow: c[9],
            text1: c[10],
            text2: c[11],
            text3: c[12],
            accent: c[13],
            accent2: c[14],
            on_accent: c[15],
            info: c[16],
            success: c[17],
            warning: c[18],
            danger: c[19],
        }
    }

    pub const fn get(self, role: Role) -> Color {
        match role {
            Role::Canvas => self.canvas,
            Role::Sidebar => self.sidebar,
            Role::Surface => self.surface,
            Role::SurfaceAlt => self.surface_alt,
            Role::Elevated => self.elevated,
            Role::Selection => self.selection,
            Role::Border => self.border,
            Role::BorderFocus => self.border_focus,
            Role::Rule => self.rule,
            Role::Shadow => self.shadow,
            Role::Text1 => self.text1,
            Role::Text2 => self.text2,
            Role::Text3 => self.text3,
            Role::Accent => self.accent,
            Role::Accent2 => self.accent2,
            Role::OnAccent => self.on_accent,
            Role::Info => self.info,
            Role::Success => self.success,
            Role::Warning => self.warning,
            Role::Danger => self.danger,
        }
    }
}

/// Text hierarchy level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Primary,
    Secondary,
    Tertiary,
}

impl Level {
    pub const fn role(self) -> Role {
        match self {
            Self::Primary => Role::Text1,
            Self::Secondary => Role::Text2,
            Self::Tertiary => Role::Text3,
        }
    }
}

/// Overall light/dark appearance of a theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Follow,
    Dark,
    Light,
}

impl Appearance {
    pub fn name(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }
}

/// Built-in UI design language (chrome, borders, headers, selection, keycaps, gauges, motion).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiStyle {
    /// Modern rounded frames (`╭─╮`), inline border titles, pill keycaps, smooth precision gauges.
    Rounded,
    /// Borderless layered Bento cards with tonal elevation and subtle shadows.
    Bento,
    /// Airy Swiss editorial layout: bold header + crisp top rule, no side/bottom box clutter.
    Editorial,
    /// Cyber HUD precision corners (`┏ ┓ / ┗ ┛`) with hairline walls, diamond glyphs, segmented gauges.
    Hud,
    /// Soft studio cards with full-width tinted `SurfaceAlt` header strips and `▸` cursor.
    Studio,
    /// Sharp technical frames (`┌─┐`), `[ Title ]` headers, dotted rules, segmented gauges.
    Atelier,
    /// High-contrast heavy borders (`┏━┓`), solid inverse selection, `[KEY]` bracket keycaps.
    Heavy,
    /// Pure ASCII borders (`+--+`) and glyphs, zero background paint, reduced motion for serial/IPMI.
    Ascii,
}

impl UiStyle {
    pub const ALL: [Self; 8] = [
        Self::Rounded,
        Self::Bento,
        Self::Editorial,
        Self::Hud,
        Self::Studio,
        Self::Atelier,
        Self::Heavy,
        Self::Ascii,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Rounded => "rounded",
            Self::Bento => "bento",
            Self::Editorial => "editorial",
            Self::Hud => "hud",
            Self::Studio => "studio",
            Self::Atelier => "atelier",
            Self::Heavy => "heavy",
            Self::Ascii => "ascii",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.id() == value)
    }

    pub const fn name_key(self) -> &'static str {
        match self {
            Self::Rounded => "style.rounded.name",
            Self::Bento => "style.bento.name",
            Self::Editorial => "style.editorial.name",
            Self::Hud => "style.hud.name",
            Self::Studio => "style.studio.name",
            Self::Atelier => "style.atelier.name",
            Self::Heavy => "style.heavy.name",
            Self::Ascii => "style.ascii.name",
        }
    }

    pub const fn description_key(self) -> &'static str {
        match self {
            Self::Rounded => "style.rounded.description",
            Self::Bento => "style.bento.description",
            Self::Editorial => "style.editorial.description",
            Self::Hud => "style.hud.description",
            Self::Studio => "style.studio.description",
            Self::Atelier => "style.atelier.description",
            Self::Heavy => "style.heavy.description",
            Self::Ascii => "style.ascii.description",
        }
    }

    /// Compact emblem shown beside each style in the Appearance Drawer.
    pub const fn preview_glyph(self, ascii_mode: bool) -> &'static str {
        if ascii_mode {
            return match self {
                Self::Rounded => "(---)",
                Self::Bento => "[===]",
                Self::Editorial => "-----",
                Self::Hud => "<--->",
                Self::Studio => "[###]",
                Self::Atelier => "[---]",
                Self::Heavy => "#===#",
                Self::Ascii => "+---+",
            };
        }
        match self {
            Self::Rounded => "╭───╮",
            Self::Bento => "▀▄▄▄▀",
            Self::Editorial => "─────",
            Self::Hud => "┏━━━┓",
            Self::Studio => "█▀▀▀█",
            Self::Atelier => "┌───┐",
            Self::Heavy => "┏═══┓",
            Self::Ascii => "+---+",
        }
    }

    pub const fn is_borderless(self) -> bool {
        matches!(self, Self::Bento | Self::Editorial | Self::Studio)
    }

    pub const fn uses_segmented_gauge(self) -> bool {
        matches!(self, Self::Hud | Self::Atelier)
    }
}

/// User's selected UI style mode: follow the palette's default style or lock a specific [`UiStyle`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StyleSelection {
    #[default]
    Auto,
    Fixed(UiStyle),
}

impl StyleSelection {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed(style) => style.id(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value == "auto" {
            Some(Self::Auto)
        } else {
            UiStyle::parse(value).map(Self::Fixed)
        }
    }

    pub const fn resolve(self, palette_default: UiStyle) -> UiStyle {
        match self {
            Self::Auto => palette_default,
            Self::Fixed(style) => style,
        }
    }
}

/// Compact rendering flags derived from the resolved [`UiStyle`], palette, and transparency toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StyleFlags {
    /// Use pure ASCII borders and glyphs without color-only state cues (`ascii` style).
    pub ascii: bool,
    /// Draw full outer borders around cards and panels.
    pub framed: bool,
    /// Paint canvas/surface fill colors (`false` for `plain` palette, `ascii` style, or `transparent_bg`).
    pub paint_bg: bool,
    /// Selection row uses full inverse accent fill (`heavy` style).
    pub solid_selection: bool,
}

impl StyleFlags {
    pub const fn for_style(style: UiStyle, palette_paints_bg: bool) -> Self {
        let ascii = matches!(style, UiStyle::Ascii);
        let paint_bg = palette_paints_bg && !ascii;
        let framed = !style.is_borderless() || !paint_bg;
        Self {
            ascii,
            framed,
            paint_bg,
            solid_selection: matches!(style, UiStyle::Heavy),
        }
    }
}

/// Result of querying the terminal background color before entering raw mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Background {
    Detected((u8, u8, u8)),
    Disabled,
    Unavailable(String),
}

/// How a theme produces its concrete [`Palette`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteSource {
    /// Derived from detected terminal background; falls back to Obsidian when unavailable.
    Derived,
    Fixed(Palette),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeName {
    Builtin(&'static str),
    Custom(String),
}

/// Declarative definition of a built-in or custom color palette (with its default UI style).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDefinition {
    pub id: String,
    pub name: ThemeName,
    pub appearance: Appearance,
    pub palette: PaletteSource,
    pub default_style: UiStyle,
    pub paint_bg: bool,
}

/// Concrete resolved theme (palette + UI style + derived rendering flags) ready for rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub id: String,
    pub name: ThemeName,
    pub appearance: Appearance,
    pub palette: Palette,
    pub style: UiStyle,
    pub default_style: UiStyle,
    pub flags: StyleFlags,
    pub light: bool,
}

/// Where a startup theme or style ID came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    Flag,
    Env,
    Config,
    Default,
}

/// Live theme & style state owned by the TUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeState {
    pub active: Theme,
    pub selected_id: String,
    pub style_selection: StyleSelection,
    pub source: ThemeSource,
    pub style_source: ThemeSource,
    pub background: Background,
    pub transparent_bg: bool,
    pub reduced_motion: bool,
    pub error: Option<String>,
}

/// Resolve a [`ThemeDefinition`] and [`StyleSelection`] against the terminal background and
/// transparency toggle.
pub fn resolve(
    definition: &ThemeDefinition,
    style_selection: StyleSelection,
    background: &Background,
    transparent_bg: bool,
) -> Theme {
    let (mut palette, light) = match definition.palette {
        PaletteSource::Derived => match background {
            Background::Detected(rgb) => (derive::adaptive(*rgb), derive::is_light(*rgb)),
            Background::Disabled | Background::Unavailable(_) => (builtin::OBSIDIAN, false),
        },
        PaletteSource::Fixed(palette) => {
            let light = match definition.appearance {
                Appearance::Light => true,
                Appearance::Dark => false,
                Appearance::Follow => {
                    matches!(background, Background::Detected(rgb) if derive::is_light(*rgb))
                }
            };
            (palette, light)
        }
    };

    let style = style_selection.resolve(definition.default_style);
    let mut flags = StyleFlags::for_style(style, definition.paint_bg);
    if transparent_bg && !flags.ascii {
        palette.canvas = Color::Default;
        palette.sidebar = Color::Default;
        palette.surface = Color::Default;
        flags.paint_bg = false;
        flags.framed = true;
    }

    Theme {
        id: definition.id.clone(),
        name: definition.name.clone(),
        appearance: definition.appearance,
        palette,
        style,
        default_style: definition.default_style,
        flags,
        light,
    }
}

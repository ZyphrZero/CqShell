//! Curated built-in themes designed around layered surface elevation.

use super::{
    Ansi16, Appearance, Background, Color, Palette, PaletteSource, Theme, ThemeDefinition,
    ThemeName, UiStyle, derive,
};

const fn hex(value: u32) -> Color {
    Color::Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Obsidian: flagship modern dark workbench (Linear / OpenCode inspired).
pub const OBSIDIAN: Palette = Palette::from_colors([
    hex(0x0d1117), // canvas
    hex(0x121821), // sidebar
    hex(0x161e2a), // surface
    hex(0x1e293b), // surface_alt
    hex(0x1b2433), // elevated
    hex(0x18313a), // selection
    hex(0x2b394d), // border
    hex(0x5eead4), // border_focus
    hex(0x212d3d), // rule
    hex(0x05080d), // shadow
    hex(0xedf2f7), // text1
    hex(0xa3b3c6), // text2
    hex(0x73869c), // text3
    hex(0x5eead4), // accent
    hex(0x7dd3fc), // accent2
    hex(0x091318), // on_accent
    hex(0x7dd3fc), // info
    hex(0x4ade80), // success
    hex(0xfbbf24), // warning
    hex(0xfb7185), // danger
]);

/// Daylight: crisp modern light workbench.
pub const DAYLIGHT: Palette = Palette::from_colors([
    hex(0xf1f5f9), // canvas
    hex(0xe8eef5), // sidebar
    hex(0xffffff), // surface
    hex(0xe6ecf3), // surface_alt
    hex(0xffffff), // elevated
    hex(0xd6f3ee), // selection
    hex(0xcbd5e1), // border
    hex(0x0b665f), // border_focus
    hex(0xe2e8f0), // rule
    hex(0xcbd5e1), // shadow
    hex(0x0f172a), // text1
    hex(0x334155), // text2
    hex(0x52637a), // text3
    hex(0x0b665f), // accent
    hex(0x035c8e), // accent2
    hex(0xffffff), // on_accent
    hex(0x035c8e), // info
    hex(0x146c35), // success
    hex(0x8f5200), // warning
    hex(0xb81038), // danger
]);

/// Tokyo Night: deep indigo surfaces with luminous periwinkle & lavender accents.
pub const TOKYO_NIGHT: Palette = Palette::from_colors([
    hex(0x16161e), // canvas
    hex(0x1a1b26), // sidebar
    hex(0x1f2335), // surface
    hex(0x292e42), // surface_alt
    hex(0x24283b), // elevated
    hex(0x283457), // selection
    hex(0x3b4261), // border
    hex(0x7aa2f7), // border_focus
    hex(0x2a2f45), // rule
    hex(0x0c0c12), // shadow
    hex(0xe4eafd), // text1
    hex(0xa9b1d6), // text2
    hex(0x7c86b0), // text3
    hex(0x7aa2f7), // accent
    hex(0xbb9af7), // accent2
    hex(0x121521), // on_accent
    hex(0x7dcfff), // info
    hex(0x9ece6a), // success
    hex(0xe0af68), // warning
    hex(0xf7768e), // danger
]);

/// Catppuccin Mocha: soft warm slate surfaces with teal & lavender accents.
pub const CATPPUCCIN: Palette = Palette::from_colors([
    hex(0x181825), // canvas
    hex(0x1e1e2e), // sidebar
    hex(0x24273a), // surface
    hex(0x313244), // surface_alt
    hex(0x2a2d40), // elevated
    hex(0x2a3d49), // selection
    hex(0x45475a), // border
    hex(0x94e2d5), // border_focus
    hex(0x313244), // rule
    hex(0x0f0f17), // shadow
    hex(0xcdd6f4), // text1
    hex(0xbac2de), // text2
    hex(0x8890ae), // text3
    hex(0x94e2d5), // accent
    hex(0xcba6f7), // accent2
    hex(0x11111b), // on_accent
    hex(0x89dceb), // info
    hex(0xa6e3a1), // success
    hex(0xf9e2af), // warning
    hex(0xf38ba8), // danger
]);

/// Rosé Pine: deep plum surfaces with rose gold & foam accents.
pub const ROSE_PINE: Palette = Palette::from_colors([
    hex(0x191724), // canvas
    hex(0x1f1d2e), // sidebar
    hex(0x26233a), // surface
    hex(0x312d4a), // surface_alt
    hex(0x2b2740), // elevated
    hex(0x3d3048), // selection
    hex(0x44415a), // border
    hex(0xebbcba), // border_focus
    hex(0x332f48), // rule
    hex(0x0e0c16), // shadow
    hex(0xe0def4), // text1
    hex(0xb5b0cc), // text2
    hex(0x908caa), // text3
    hex(0xebbcba), // accent
    hex(0x9ccfd8), // accent2
    hex(0x191724), // on_accent
    hex(0x9ccfd8), // info
    hex(0x95d5b2), // success
    hex(0xf6c177), // warning
    hex(0xf27c9d), // danger
]);

/// High Contrast: WCAG AAA pure-black theme with crisp borders and vivid yellow focus.
pub const HIGH_CONTRAST: Palette = Palette::from_colors([
    hex(0x000000), // canvas
    hex(0x080808), // sidebar
    hex(0x0f0f0f), // surface
    hex(0x1e1e1e), // surface_alt
    hex(0x080808), // elevated
    hex(0xffd400), // selection
    hex(0xd8d8d8), // border
    hex(0xffd400), // border_focus
    hex(0x707070), // rule
    hex(0x000000), // shadow
    hex(0xffffff), // text1
    hex(0xe5e5e5), // text2
    hex(0xbdbdbd), // text3
    hex(0xffd400), // accent
    hex(0x6cc7ff), // accent2
    hex(0x000000), // on_accent
    hex(0x6cc7ff), // info
    hex(0x00ff66), // success
    hex(0xffb020), // warning
    hex(0xff6b6b), // danger
]);

pub fn auto() -> ThemeDefinition {
    ThemeDefinition {
        id: "auto".into(),
        name: ThemeName::Builtin("theme.auto.name"),
        appearance: Appearance::Follow,
        palette: PaletteSource::Derived,
        default_style: UiStyle::Rounded,
        paint_bg: true,
    }
}

pub fn obsidian() -> ThemeDefinition {
    ThemeDefinition {
        id: "obsidian".into(),
        name: ThemeName::Builtin("theme.obsidian.name"),
        appearance: Appearance::Dark,
        palette: PaletteSource::Fixed(OBSIDIAN),
        default_style: UiStyle::Bento,
        paint_bg: true,
    }
}

pub fn daylight() -> ThemeDefinition {
    ThemeDefinition {
        id: "daylight".into(),
        name: ThemeName::Builtin("theme.daylight.name"),
        appearance: Appearance::Light,
        palette: PaletteSource::Fixed(DAYLIGHT),
        default_style: UiStyle::Editorial,
        paint_bg: true,
    }
}

pub fn tokyo_night() -> ThemeDefinition {
    ThemeDefinition {
        id: "tokyo-night".into(),
        name: ThemeName::Builtin("theme.tokyo-night.name"),
        appearance: Appearance::Dark,
        palette: PaletteSource::Fixed(TOKYO_NIGHT),
        default_style: UiStyle::Hud,
        paint_bg: true,
    }
}

pub fn catppuccin() -> ThemeDefinition {
    ThemeDefinition {
        id: "catppuccin".into(),
        name: ThemeName::Builtin("theme.catppuccin.name"),
        appearance: Appearance::Dark,
        palette: PaletteSource::Fixed(CATPPUCCIN),
        default_style: UiStyle::Studio,
        paint_bg: true,
    }
}

pub fn rose_pine() -> ThemeDefinition {
    ThemeDefinition {
        id: "rose-pine".into(),
        name: ThemeName::Builtin("theme.rose-pine.name"),
        appearance: Appearance::Dark,
        palette: PaletteSource::Fixed(ROSE_PINE),
        default_style: UiStyle::Atelier,
        paint_bg: true,
    }
}

pub fn high_contrast() -> ThemeDefinition {
    ThemeDefinition {
        id: "high-contrast".into(),
        name: ThemeName::Builtin("theme.high-contrast.name"),
        appearance: Appearance::Dark,
        palette: PaletteSource::Fixed(HIGH_CONTRAST),
        default_style: UiStyle::Heavy,
        paint_bg: true,
    }
}

pub fn plain() -> ThemeDefinition {
    let mut palette = Palette::from_colors([Color::Default; 20]);
    palette.info = Color::Ansi(Ansi16::Cyan);
    palette.success = Color::Ansi(Ansi16::Green);
    palette.warning = Color::Ansi(Ansi16::Yellow);
    palette.danger = Color::Ansi(Ansi16::Red);
    ThemeDefinition {
        id: "plain".into(),
        name: ThemeName::Builtin("theme.plain.name"),
        appearance: Appearance::Follow,
        palette: PaletteSource::Fixed(palette),
        default_style: UiStyle::Ascii,
        paint_bg: false,
    }
}

/// All built-in themes in display order (theme drawer & `cqs themes`).
pub fn all() -> Vec<ThemeDefinition> {
    vec![
        auto(),
        obsidian(),
        daylight(),
        tokyo_night(),
        catppuccin(),
        rose_pine(),
        high_contrast(),
        plain(),
    ]
}

/// Map retired theme IDs from older `config.toml` files to their closest modern equivalent.
pub fn legacy_alias(id: &str) -> Option<&'static str> {
    match id {
        "aurora" => Some("auto"),
        "classic" | "phosphor" => Some("obsidian"),
        "nord" | "neon" => Some("tokyo-night"),
        "sand" => Some("rose-pine"),
        _ => None,
    }
}

/// Reference terminal background & foreground colors used by HTML visual exports.
pub fn reference_colors(theme: &Theme, background: &Background) -> (Color, Color) {
    let light = matches!(background, Background::Detected(rgb) if derive::is_light(*rgb));
    if theme.id == "plain" {
        return if light {
            (hex(0xffffff), hex(0x000000))
        } else {
            (hex(0x000000), hex(0xe5e5e5))
        };
    }
    if theme.palette.canvas != Color::Default {
        return (theme.palette.canvas, theme.palette.text1);
    }
    if light {
        (hex(0xf8fafc), hex(0x0f172a))
    } else {
        (hex(0x0d1117), hex(0xedf2f7))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Role, StyleSelection, derive::contrast, resolve};

    #[test]
    fn every_builtin_palette_maps_to_a_distinct_ui_style() {
        let builtins = all();
        assert_eq!(builtins.len(), UiStyle::ALL.len());
        let mut seen = std::collections::HashSet::new();
        for def in builtins {
            assert!(
                seen.insert(def.default_style),
                "duplicate default_style {:?} on {}",
                def.default_style,
                def.id
            );
        }
    }

    #[test]
    fn every_builtin_rgb_theme_meets_wcag_contrast_targets() {
        let backgrounds = [
            Background::Unavailable("none".into()),
            Background::Detected((0x0d, 0x11, 0x17)),
            Background::Detected((0xf8, 0xfa, 0xfc)),
        ];
        for def in all() {
            if !def.paint_bg {
                continue;
            }
            for bg in &backgrounds {
                let theme = resolve(&def, StyleSelection::Auto, bg, false);
                let p = theme.palette;
                let surfaces = [
                    ("canvas", p.canvas.rgb().unwrap()),
                    ("sidebar", p.sidebar.rgb().unwrap()),
                    ("surface", p.surface.rgb().unwrap()),
                    ("surface_alt", p.surface_alt.rgb().unwrap()),
                    ("elevated", p.elevated.rgb().unwrap()),
                ];
                let text_roles = [
                    (Role::Text1, 7.0),
                    (Role::Text2, 4.5),
                    (Role::Accent, 4.5),
                    (Role::Accent2, 4.5),
                    (Role::Info, 4.5),
                    (Role::Success, 4.5),
                    (Role::Warning, 4.5),
                    (Role::Danger, 4.5),
                ];
                for (role, min_ratio) in text_roles {
                    let fg = p.get(role).rgb().unwrap();
                    for (surface_name, surface_rgb) in surfaces {
                        let ratio = contrast(fg, surface_rgb);
                        assert!(
                            ratio >= min_ratio,
                            "theme {} ({bg:?}): {role:?} on {surface_name} contrast {ratio:.2} < {min_ratio}",
                            theme.id
                        );
                    }
                }
                // Check text on selection
                let sel_bg = p.selection.rgb().unwrap();
                let sel_fg = if theme.flags.solid_selection {
                    p.on_accent.rgb().unwrap()
                } else {
                    p.text1.rgb().unwrap()
                };
                assert!(
                    contrast(sel_fg, sel_bg) >= 6.0,
                    "theme {} selection contrast {:.2} < 6.0",
                    theme.id,
                    contrast(sel_fg, sel_bg)
                );
                // Check on_accent contrast on accent
                assert!(
                    contrast(p.on_accent.rgb().unwrap(), p.accent.rgb().unwrap()) >= 4.5,
                    "theme {} on_accent contrast {:.2} < 4.5",
                    theme.id,
                    contrast(p.on_accent.rgb().unwrap(), p.accent.rgb().unwrap())
                );
            }
        }
    }
}

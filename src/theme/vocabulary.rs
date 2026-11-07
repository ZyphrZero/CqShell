use super::*;

macro_rules! vocabulary {
    ($kind:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        impl $kind {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub fn name(self) -> &'static str { match self { $(Self::$variant => $name),+ } }
            pub fn parse(name: &str) -> Option<Self> { match name { $($name => Some(Self::$variant)),+, _ => None } }
        }
    };
}

vocabulary!(BorderStyle { Rounded => "rounded", Square => "square", Double => "double", Heavy => "heavy", Ascii => "ascii", Card => "card", Titlebar => "titlebar" });
vocabulary!(TitleStyle { Inline => "inline", Bracket => "bracket", Arrow => "arrow", Bar => "bar", Label => "label" });
vocabulary!(BarStyle { Line => "line", Block => "block", Segment => "segment", Square => "square", Hash => "hash" });
vocabulary!(SparklineStyle { Blocks => "blocks", Braille => "braille", Ascii => "ascii" });
vocabulary!(SelectionStyle { Fill => "fill", Bar => "bar", Inverse => "inverse", Arrow => "arrow" });
vocabulary!(CheckboxStyle { Box => "box", Filled => "filled", Diamond => "diamond", Radio => "radio", Square => "square", Ascii => "ascii" });
vocabulary!(DotStyle { Circle => "circle", Diamond => "diamond", Square => "square", Ascii => "ascii" });
vocabulary!(RuleStyle { Solid => "solid", Dotted => "dotted", Dashed => "dashed", None => "none", Ascii => "ascii" });
vocabulary!(KeyStyle { Keycap => "keycap", Plain => "plain", Bracket => "bracket" });
vocabulary!(BackgroundStyle { Terminal => "terminal", Paint => "paint" });
vocabulary!(TertiaryStyle { Color => "color", Dim => "dim" });
vocabulary!(GlyphSet { Unicode => "unicode", Ascii => "ascii" });
vocabulary!(BackgroundKind { Follow => "follow", Dark => "dark", Light => "light" });

pub const COLOR_NAMES: [&str; 24] = [
    "background",
    "surface",
    "surface_alt",
    "selection",
    "selection_text",
    "selection_mark",
    "dialog",
    "dialog_selection",
    "border",
    "border_focus",
    "rule",
    "shadow",
    "dim",
    "text1",
    "text2",
    "text3",
    "accent",
    "accent2",
    "on_accent",
    "meter",
    "info",
    "success",
    "warning",
    "danger",
];
pub const PATTERN_NAMES: [&str; 13] = [
    "border",
    "title",
    "bar",
    "sparkline",
    "selection",
    "checkbox",
    "dot",
    "rule",
    "keys",
    "background",
    "gradient",
    "tertiary",
    "glyphs",
];

impl Color {
    pub fn parse(value: &str) -> Option<Self> {
        if value == "default" {
            return Some(Self::Default);
        }
        if let Some(color) = Ansi16::ALL.iter().find(|color| color.name() == value) {
            return Some(Self::Ansi(*color));
        }
        if value.len() != 7
            || !value.starts_with('#')
            || !value.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
        {
            return None;
        }
        let number = u32::from_str_radix(&value[1..], 16).ok()?;
        Some(Self::Rgb(
            (number >> 16) as u8,
            (number >> 8) as u8,
            number as u8,
        ))
    }

    pub fn value(self) -> String {
        match self {
            Self::Default => "default".into(),
            Self::Ansi(color) => color.name().into(),
            Self::Rgb(red, green, blue) => format!("#{red:02x}{green:02x}{blue:02x}"),
        }
    }
}

impl Patterns {
    pub fn values(self) -> [&'static str; 13] {
        [
            self.border.name(),
            self.title.name(),
            self.bar.name(),
            self.sparkline.name(),
            self.selection.name(),
            self.checkbox.name(),
            self.dot.name(),
            self.rule.name(),
            self.keys.name(),
            self.background.name(),
            if self.gradient { "true" } else { "false" },
            self.tertiary.name(),
            self.glyphs.name(),
        ]
    }
}

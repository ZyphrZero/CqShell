//! Contrast_Test for classic: recomputes the classic rows of the theme design's contrast
//! table from the palette data, on the table's reference terminal colors, and checks the
//! readability thresholds of requirement 28.2.

use super::{CLASSIC_DARK, CLASSIC_LIGHT, CLASSIC_PATTERNS};
use crate::theme::derive::{BLACK, WHITE, contrast};
use crate::theme::{Ansi16, Color, Palette, SelectionStyle, TertiaryStyle};

type Rgb = (u8, u8, u8);

/// Terminal colors that stand in for `Color::Default`: foreground roles take `fg`,
/// Surfaces take `bg`.
struct Terminal {
    bg: Rgb,
    fg: Rgb,
}

const CLASSIC_DARK_TERMINAL: Terminal = Terminal {
    bg: (0x09, 0x0e, 0x16),
    fg: (0xe5, 0xed, 0xf3),
};

const CLASSIC_LIGHT_TERMINAL: Terminal = Terminal {
    bg: WHITE,
    fg: (0x1f, 0x23, 0x28),
};

/// Columns of the contrast table, in order; the last one is the selected-row text.
const COLUMNS: [&str; 9] = [
    "text1",
    "text2",
    "text3",
    "accent",
    "info",
    "success",
    "warning",
    "danger",
    "selection_text",
];

/// The classic rows as printed in the design.
const CLASSIC_DARK_ROW: [f64; 9] = [16.33, 7.37, 7.37, 9.82, 8.18, 10.72, 11.77, 7.49, 10.66];
const CLASSIC_LIGHT_ROW: [f64; 9] = [15.80, 6.39, 6.39, 5.02, 5.19, 5.08, 4.87, 5.36, 5.02];

/// The table prints two decimals.
const TOLERANCE: f64 = 0.01;

/// Minimum ratio per column. Requirement 28.2 asks 4.5 of text1 and text2 and 3.0 of
/// text3; 4.5 for accent and the status colors is the design's own target. The
/// selected-row text has no threshold.
const THRESHOLDS: [Option<f64>; 9] = [
    Some(4.5),
    Some(4.5),
    Some(3.0),
    Some(4.5),
    Some(4.5),
    Some(4.5),
    Some(4.5),
    Some(4.5),
    None,
];

/// RGB of a palette color on the reference terminal, reading `Default` as `default`.
///
/// The design gives no RGB values for ANSI colors. Black is taken as #000000, the only
/// value for which the table's selected-row figure for classic dark (black on #45cdb6,
/// 10.66) holds. The palettes checked here use no other ANSI color, so any other one
/// fails the test rather than guessing a terminal's palette.
fn rgb(color: Color, default: Rgb) -> Rgb {
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Ansi(Ansi16::Black) => BLACK,
        Color::Ansi(other) => panic!("no reference RGB for ANSI color {}", other.name()),
    }
}

/// Recomputes one table row. Every foreground takes its minimum ratio over the Surfaces
/// that carry text: background, surface and dialog. classic fills the selected row and
/// draws all of its text in selection_text, so selection only enters the last column.
fn row(p: &Palette, t: &Terminal) -> [f64; 9] {
    // The row's premises: with a `bar` selection, selection would be one more Surface for
    // every foreground; with `dim` tertiary text, text3 would need the DIM model.
    assert_eq!(CLASSIC_PATTERNS.selection, SelectionStyle::Fill);
    assert_eq!(CLASSIC_PATTERNS.tertiary, TertiaryStyle::Color);

    let surfaces = [p.background, p.surface, p.dialog].map(|s| rgb(s, t.bg));
    let worst = |fg: Color| {
        let fg = rgb(fg, t.fg);
        surfaces
            .iter()
            .map(|&s| contrast(fg, s))
            .fold(f64::INFINITY, f64::min)
    };
    [
        worst(p.text1),
        worst(p.text2),
        worst(p.text3),
        worst(p.accent),
        worst(p.info),
        worst(p.success),
        worst(p.warning),
        worst(p.danger),
        contrast(rgb(p.selection_text, t.fg), rgb(p.selection, t.bg)),
    ]
}

fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Every column of `variant` whose recomputed ratio is off the table by more than the
/// tolerance, with both values and the reference background.
fn table_mismatches(variant: &str, p: &Palette, t: &Terminal, table: &[f64; 9]) -> Vec<String> {
    let actual = row(p, t);
    COLUMNS
        .iter()
        .zip(actual.iter().zip(table))
        .filter(|(_, (actual, table))| (*actual - *table).abs() > TOLERANCE)
        .map(|(column, (actual, table))| {
            format!(
                "{variant} {column}: computed {actual:.4}, table {table:.2} (bg {})",
                hex(t.bg)
            )
        })
        .collect()
}

/// Every column of `variant` whose recomputed ratio falls below its threshold.
fn threshold_failures(variant: &str, p: &Palette, t: &Terminal) -> Vec<String> {
    let actual = row(p, t);
    COLUMNS
        .iter()
        .zip(actual.iter().zip(THRESHOLDS))
        .filter_map(|(column, (&actual, min))| {
            let min = min?;
            (actual < min)
                .then(|| format!("{variant} {column}: {actual:.4} < {min} (bg {})", hex(t.bg)))
        })
        .collect()
}

#[test]
fn classic_dark_matches_the_contrast_table() {
    let failures = table_mismatches(
        "classic dark",
        &CLASSIC_DARK,
        &CLASSIC_DARK_TERMINAL,
        &CLASSIC_DARK_ROW,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn classic_light_matches_the_contrast_table() {
    let failures = table_mismatches(
        "classic light",
        &CLASSIC_LIGHT,
        &CLASSIC_LIGHT_TERMINAL,
        &CLASSIC_LIGHT_ROW,
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn classic_meets_the_contrast_thresholds() {
    let mut failures = threshold_failures("classic dark", &CLASSIC_DARK, &CLASSIC_DARK_TERMINAL);
    failures.extend(threshold_failures(
        "classic light",
        &CLASSIC_LIGHT,
        &CLASSIC_LIGHT_TERMINAL,
    ));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fixed_themes_match_the_contrast_table() {
    for (definition, expected) in [
        (
            super::phosphor(),
            [12.03, 7.49, 3.58, 13.62, 13.62, 12.03, 10.72, 5.57],
        ),
        (
            super::neon(),
            [14.45, 7.59, 3.29, 10.80, 6.68, 12.82, 12.85, 5.27],
        ),
        (
            super::nord(),
            [7.49, 6.39, 3.08, 4.52, 4.50, 4.55, 5.52, 4.52],
        ),
        (
            super::sand(),
            [8.45, 6.76, 3.16, 4.59, 4.53, 5.62, 6.84, 4.52],
        ),
        (
            super::high_contrast(),
            [21.00, 16.83, 11.18, 14.67, 11.25, 15.50, 10.23, 6.94],
        ),
    ] {
        let crate::theme::PaletteSource::Fixed(palette) = definition.palette else {
            panic!("fixed theme");
        };
        let mut surfaces = vec![palette.background, palette.surface, palette.dialog];
        if definition.patterns.selection == SelectionStyle::Bar {
            surfaces.push(palette.selection);
        }
        for (index, (color, expected)) in [
            palette.text1,
            palette.text2,
            palette.text3,
            palette.accent,
            palette.info,
            palette.success,
            palette.warning,
            palette.danger,
        ]
        .into_iter()
        .zip(expected)
        .enumerate()
        {
            let actual = surfaces
                .iter()
                .map(|surface| contrast(rgb(color, WHITE), rgb(*surface, BLACK)))
                .fold(f64::INFINITY, f64::min);
            assert!(
                (actual - expected).abs() < TOLERANCE,
                "{} {} {actual} != {expected}",
                definition.id,
                COLUMNS[index]
            );
            assert!(actual >= if index == 2 { 3.0 } else { 4.5 });
        }
        let selected = match definition.id.as_str() {
            "phosphor" => Some(9.10),
            "neon" => Some(10.80),
            "nord" => Some(7.49),
            "high-contrast" => Some(14.67),
            _ => None,
        };
        if let Some(expected) = selected {
            let actual = contrast(
                rgb(palette.selection_text, WHITE),
                rgb(palette.selection, BLACK),
            );
            assert!(
                (actual - expected).abs() < TOLERANCE,
                "{} selected: {actual} != {expected}",
                definition.id
            );
        }
    }
}

#[test]
fn plain_text_meets_both_xterm_reference_backgrounds() {
    use crate::theme::{Background, derive::mix, resolve};
    let theme = resolve(&super::plain(), &Background::Disabled);
    for (background, foreground, expected) in
        [(BLACK, (0xe5, 0xe5, 0xe5), 16.67), (WHITE, BLACK, 21.00)]
    {
        for color in [theme.palette.text1, theme.palette.text2] {
            assert!((contrast(rgb(color, foreground), background) - expected).abs() < TOLERANCE);
        }
        let dim = mix(foreground, background, 0.5);
        assert!(contrast(dim, background) >= 3.0);
    }
}

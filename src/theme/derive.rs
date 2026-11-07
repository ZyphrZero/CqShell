//! Color arithmetic on 8-bit sRGB: WCAG 2.x relative luminance and contrast ratio, and
//! linear blending.

pub const WHITE: (u8, u8, u8) = (0xff, 0xff, 0xff);
pub const BLACK: (u8, u8, u8) = (0, 0, 0);

/// WCAG 2.x relative luminance, from 0 (black) to 1 (white).
pub fn relative_luminance((r, g, b): (u8, u8, u8)) -> f64 {
    fn linear(channel: u8) -> f64 {
        let c = f64::from(channel) / 255.0;
        // WCAG 2.x prints the cut-off as 0.03928, sRGB as 0.04045; no 8-bit level lies
        // between the two, so both give the same result.
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

/// WCAG 2.x contrast ratio, from 1 (identical) to 21 (black and white); symmetric.
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Per-channel linear blend from `a` at `t = 0` to `b` at `t = 1`, rounded to the nearest
/// level; `t` is clamped to that range.
pub fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let channel =
        |from: u8, to: u8| (f64::from(from) + (f64::from(to) - f64::from(from)) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// A background is light when black text contrasts with it more than white text, which
/// holds for relative luminance above √0.0525 − 0.05 ≈ 0.179.
pub fn is_light(bg: (u8, u8, u8)) -> bool {
    contrast(bg, BLACK) > contrast(bg, WHITE)
}

pub fn aurora(background: (u8, u8, u8)) -> super::Palette {
    use super::{Color, Palette};
    let light = is_light(background);
    let endpoint = if light { BLACK } else { WHITE };
    let scale = ((contrast(background, endpoint) - 4.6) / 3.4).clamp(0.0, 1.0);
    let surface = mix(background, endpoint, 0.04 * scale);
    let selection = mix(background, endpoint, 0.08 * scale);
    let dialog = mix(background, endpoint, 0.07 * scale);
    let dialog_selection = mix(background, endpoint, 0.13 * scale);
    let surfaces = [background, surface, selection, dialog, dialog_selection];
    let worst = |color| {
        surfaces
            .iter()
            .map(|surface| contrast(color, *surface))
            .fold(f64::INFINITY, f64::min)
    };
    let ceiling = worst(endpoint);
    let adjusted = |base, goal: f64| {
        let goal = goal.min(ceiling);
        if worst(base) >= goal {
            return base;
        }
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..14 {
            let middle = (low + high) / 2.0;
            if worst(mix(base, endpoint, middle)) >= goal {
                high = middle;
            } else {
                low = middle;
            }
        }
        mix(base, endpoint, high)
    };
    let rgb = |(red, green, blue)| Color::Rgb(red, green, blue);
    let bases = if light {
        [
            (0x0b, 0x7a, 0x6c),
            (0x2a, 0x62, 0xc4),
            (0x1a, 0x7f, 0x45),
            (0x8a, 0x5a, 0),
            (0xb8, 0x32, 0x48),
        ]
    } else {
        [
            (0x4f, 0xd1, 0xbd),
            (0x86, 0xb4, 0xff),
            (0x6f, 0xd3, 0x9a),
            (0xf2, 0xc4, 0x6d),
            (0xf2, 0x87, 0x9a),
        ]
    };
    let [accent, info, success, warning, danger] = bases.map(|base| adjusted(base, 4.5));
    let text1 = rgb(adjusted(background, 12.0));
    Palette {
        background: rgb(background),
        surface: rgb(surface),
        surface_alt: rgb(selection),
        selection: rgb(selection),
        selection_text: text1,
        selection_mark: rgb(accent),
        dialog: rgb(dialog),
        dialog_selection: rgb(dialog_selection),
        border: rgb(mix(background, endpoint, 0.20)),
        border_focus: rgb(accent),
        rule: rgb(mix(background, endpoint, 0.10)),
        shadow: rgb(mix(background, BLACK, if light { 0.10 } else { 0.45 })),
        dim: rgb(mix(background, endpoint, 0.24)),
        text1,
        text2: rgb(adjusted(background, 7.0)),
        text3: rgb(adjusted(background, 4.0)),
        accent: rgb(accent),
        accent2: rgb(accent),
        on_accent: rgb(if contrast(accent, BLACK) > contrast(accent, WHITE) {
            BLACK
        } else {
            WHITE
        }),
        meter: rgb(accent),
        info: rgb(info),
        success: rgb(success),
        warning: rgb(warning),
        danger: rgb(danger),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Color, Role};
    use proptest::strategy::Strategy;

    fn rgb(color: Color) -> (u8, u8, u8) {
        match color {
            Color::Rgb(red, green, blue) => (red, green, blue),
            _ => panic!("aurora must use RGB"),
        }
    }

    fn check_aurora(background: (u8, u8, u8)) {
        let palette = aurora(background);
        for surface in [
            palette.background,
            palette.surface,
            palette.selection,
            palette.dialog,
            palette.dialog_selection,
        ] {
            for role in [
                Role::Text1,
                Role::Text2,
                Role::Text3,
                Role::Accent,
                Role::Info,
                Role::Success,
                Role::Warning,
                Role::Danger,
            ] {
                let minimum = if role == Role::Text3 { 3.0 } else { 4.5 };
                assert!(
                    contrast(rgb(palette.get(role)), rgb(surface)) >= minimum,
                    "{background:?} {role:?} against {surface:?}"
                );
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]
        // Feature: tui-redesign, Property 11: aurora 对比度
        #[test]
        fn aurora_meets_contrast_for_any_background(background in proptest::prop_oneof![proptest::prelude::any::<(u8,u8,u8)>(), (112u8..123).prop_map(|gray| (gray,gray,gray))]) {
            check_aurora(background);
        }
    }

    #[test]
    fn aurora_matches_the_reference_palettes_and_all_grays() {
        for gray in 0..=255 {
            check_aurora((gray, gray, gray));
        }
        for (background, expected) in [
            (
                (0x0b, 0x11, 0x19),
                [
                    0x151b22, 0x1f242b, 0x1c2229, 0x2b3037, 0x232930, 0x3c4147, 0x464a50, 0xf3f4f4,
                    0xbbbdbf, 0x8b8e91, 0x4fd1bd,
                ],
            ),
            (
                (0xf6, 0xf7, 0xf9),
                [
                    0xecedef, 0xe2e3e5, 0xe5e6e8, 0xd6d7d9, 0xdddee0, 0xc5c6c7, 0xbbbcbd, 0x1a1a1b,
                    0x414242, 0x656566, 0x0a6a5e,
                ],
            ),
        ] {
            let palette = aurora(background);
            for (role, number) in [
                Role::Surface,
                Role::Selection,
                Role::Dialog,
                Role::DialogSelection,
                Role::Rule,
                Role::Border,
                Role::Dim,
                Role::Text1,
                Role::Text2,
                Role::Text3,
                Role::Accent,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    palette.get(role),
                    Color::Rgb((number >> 16) as u8, (number >> 8) as u8, number as u8),
                    "{role:?}"
                );
            }
        }
        let light = aurora((0xf6, 0xf7, 0xf9));
        for (role, expected) in [
            (Role::Info, 0x275ab5u32),
            (Role::Success, 0x166c3a),
            (Role::Warning, 0x825400),
            (Role::Danger, 0xab2f43),
        ] {
            assert_eq!(
                light.get(role),
                Color::Rgb(
                    (expected >> 16) as u8,
                    (expected >> 8) as u8,
                    expected as u8
                )
            );
        }
        for (background, expected) in [
            ((11, 17, 25), [12.06, 7.05, 4.03, 7.08, 5.51]),
            ((246, 247, 249), [12.08, 7.00, 4.04, 4.51, 4.53]),
        ] {
            let palette = aurora(background);
            for (role, expected) in [
                Role::Text1,
                Role::Text2,
                Role::Text3,
                Role::Accent,
                Role::Danger,
            ]
            .into_iter()
            .zip(expected)
            {
                let actual = [
                    palette.background,
                    palette.surface,
                    palette.selection,
                    palette.dialog,
                    palette.dialog_selection,
                ]
                .into_iter()
                .map(|surface| contrast(rgb(palette.get(role)), rgb(surface)))
                .fold(f64::INFINITY, f64::min);
                assert!(
                    (actual - expected).abs() < 0.01,
                    "{role:?}: {actual} != {expected}"
                );
            }
        }
    }

    fn close(actual: f64, expected: f64, tolerance: f64) -> bool {
        (actual - expected).abs() <= tolerance
    }

    #[test]
    fn relative_luminance_of_known_colors() {
        assert!(close(relative_luminance(BLACK), 0.0, 1e-12));
        assert!(close(relative_luminance(WHITE), 1.0, 1e-12));
        assert!(close(relative_luminance((0xff, 0, 0)), 0.2126, 1e-12));
        assert!(close(relative_luminance((0, 0xff, 0)), 0.7152, 1e-12));
        assert!(close(relative_luminance((0, 0, 0xff)), 0.0722, 1e-12));
        assert!(close(
            relative_luminance((0x80, 0x80, 0x80)),
            0.2158605,
            1e-6
        ));
    }

    #[test]
    fn contrast_of_known_pairs() {
        assert!(close(contrast(WHITE, BLACK), 21.0, 1e-9));
        assert!(close(contrast(BLACK, WHITE), 21.0, 1e-9));
        assert!(close(
            contrast((0x45, 0xcd, 0xb6), (0x45, 0xcd, 0xb6)),
            1.0,
            1e-12
        ));
        // The dark classic colors that motivate its light variant: muted text and accent
        // on white.
        assert!(close(contrast((0x91, 0xa2, 0xb0), WHITE), 2.6, 0.05));
        assert!(close(contrast((0x45, 0xcd, 0xb6), WHITE), 2.0, 0.05));
    }

    #[test]
    fn mix_returns_the_endpoints_and_blends_between_them() {
        let (a, b) = ((0, 0, 0), (100, 200, 50));
        assert_eq!(mix(a, b, 0.0), a);
        assert_eq!(mix(a, b, 1.0), b);
        assert_eq!(mix(a, b, 0.5), (50, 100, 25));
        assert_eq!(mix(a, b, -1.0), a);
        assert_eq!(mix(a, b, 2.0), b);
        // Rounds to the nearest level: 11 + 244 × 0.04 = 20.76.
        assert_eq!(mix((11, 17, 25), WHITE, 0.04), (21, 27, 34));
    }

    #[test]
    fn is_light_switches_at_the_luminance_threshold() {
        let threshold = 0.0525_f64.sqrt() - 0.05;
        let (below, above) = ((0x75, 0x75, 0x75), (0x76, 0x76, 0x76));
        assert!(relative_luminance(below) < threshold);
        assert!(relative_luminance(above) > threshold);
        assert!(!is_light(below));
        assert!(is_light(above));
        assert!(is_light(WHITE));
        assert!(!is_light(BLACK));
    }
}

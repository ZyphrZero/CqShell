//! WCAG 2.x color math (sRGB relative luminance, contrast ratio, RGB blending) and adaptive
//! layered-surface palette derivation.

use super::{Color, Palette};

pub const WHITE: (u8, u8, u8) = (0xff, 0xff, 0xff);
pub const BLACK: (u8, u8, u8) = (0, 0, 0);

/// WCAG 2.x relative luminance, from 0 (black) to 1 (white).
pub fn relative_luminance((r, g, b): (u8, u8, u8)) -> f64 {
    fn linear(channel: u8) -> f64 {
        let c = f64::from(channel) / 255.0;
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

/// Per-channel linear blend from `a` at `t = 0` to `b` at `t = 1`.
pub fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let channel =
        |from: u8, to: u8| (f64::from(from) + (f64::from(to) - f64::from(from)) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

/// A background is light when black text contrasts with it more than white text.
pub fn is_light(bg: (u8, u8, u8)) -> bool {
    contrast(bg, BLACK) > contrast(bg, WHITE)
}

/// Derive a layered-surface palette rooted in the user's real terminal background while ensuring
/// WCAG contrast across every surface layer.
pub fn adaptive(background: (u8, u8, u8)) -> Palette {
    let light = is_light(background);
    let endpoint = if light { BLACK } else { WHITE };
    let scale = ((contrast(background, endpoint) - 4.6) / 3.4).clamp(0.0, 1.0);

    let (sidebar, surface, surface_alt, elevated) = if light {
        (
            mix(background, endpoint, 0.035 * scale),
            mix(background, WHITE, 0.72),
            mix(background, endpoint, 0.075 * scale),
            WHITE,
        )
    } else {
        (
            mix(background, endpoint, 0.032 * scale),
            mix(background, endpoint, 0.058 * scale),
            mix(background, endpoint, 0.095 * scale),
            mix(background, endpoint, 0.080 * scale),
        )
    };

    let bases = if light {
        [
            (0x0d, 0x94, 0x88), // accent (teal)
            (0x02, 0x84, 0xc7), // accent2 (sky)
            (0x02, 0x84, 0xc7), // info
            (0x16, 0xa3, 0x4a), // success
            (0xb4, 0x53, 0x09), // warning
            (0xdc, 0x26, 0x26), // danger
        ]
    } else {
        [
            (0x5e, 0xea, 0xd4), // accent (mint teal)
            (0x7d, 0xd3, 0xfc), // accent2 (sky)
            (0x7d, 0xd3, 0xfc), // info
            (0x4a, 0xde, 0x80), // success
            (0xfb, 0xbf, 0x24), // warning
            (0xfb, 0x71, 0x85), // danger
        ]
    };

    let selection = mix(surface, bases[0], if light { 0.14 } else { 0.16 } * scale);
    let surfaces = [
        background,
        sidebar,
        surface,
        surface_alt,
        elevated,
        selection,
    ];
    let worst = |color| {
        surfaces
            .iter()
            .map(|s| contrast(color, *s))
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

    let rgb = |(r, g, b)| Color::Rgb(r, g, b);
    let [accent, accent2, info, success, warning, danger] = bases.map(|b| adjusted(b, 4.5));
    let text1 = adjusted(background, 12.0);
    let text2 = adjusted(background, 6.5);
    let text3 = adjusted(background, 4.0);
    let on_accent = if contrast(accent, BLACK) > contrast(accent, WHITE) {
        BLACK
    } else {
        WHITE
    };

    Palette {
        canvas: rgb(background),
        sidebar: rgb(sidebar),
        surface: rgb(surface),
        surface_alt: rgb(surface_alt),
        elevated: rgb(elevated),
        selection: rgb(selection),
        border: rgb(mix(background, endpoint, if light { 0.18 } else { 0.16 })),
        border_focus: rgb(accent),
        rule: rgb(mix(background, endpoint, if light { 0.11 } else { 0.10 })),
        shadow: rgb(mix(background, BLACK, if light { 0.12 } else { 0.50 })),
        text1: rgb(text1),
        text2: rgb(text2),
        text3: rgb(text3),
        accent: rgb(accent),
        accent2: rgb(accent2),
        on_accent: rgb(on_accent),
        info: rgb(info),
        success: rgb(success),
        warning: rgb(warning),
        danger: rgb(danger),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn black_and_white_contrast_is_twenty_one() {
        assert!((contrast(BLACK, WHITE) - 21.0).abs() < 1e-9);
        assert!((contrast(WHITE, BLACK) - 21.0).abs() < 1e-9);
        assert!(!is_light((0x0d, 0x11, 0x17)));
        assert!(is_light((0xf8, 0xfa, 0xfc)));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn adaptive_palette_maintains_readable_contrast_for_any_terminal_background(
            r in 0u8..=255,
            g in 0u8..=255,
            b in 0u8..=255,
        ) {
            let bg = (r, g, b);
            let palette = adaptive(bg);
            let surfaces = [
                palette.canvas.rgb().unwrap(),
                palette.sidebar.rgb().unwrap(),
                palette.surface.rgb().unwrap(),
                palette.surface_alt.rgb().unwrap(),
                palette.elevated.rgb().unwrap(),
                palette.selection.rgb().unwrap(),
            ];
            let endpoint = if is_light(bg) { BLACK } else { WHITE };
            let ceiling = surfaces
                .iter()
                .map(|s| contrast(endpoint, *s))
                .fold(f64::INFINITY, f64::min);

            let text1 = palette.text1.rgb().unwrap();
            let text2 = palette.text2.rgb().unwrap();
            let accent = palette.accent.rgb().unwrap();

            let min_text1 = surfaces.iter().map(|s| contrast(text1, *s)).fold(f64::INFINITY, f64::min);
            let min_text2 = surfaces.iter().map(|s| contrast(text2, *s)).fold(f64::INFINITY, f64::min);
            let min_accent = surfaces.iter().map(|s| contrast(accent, *s)).fold(f64::INFINITY, f64::min);

            prop_assert!(min_text1 + 0.05 >= ceiling.min(12.0), "text1 {min_text1} < {}", ceiling.min(12.0));
            prop_assert!(min_text2 + 0.05 >= ceiling.min(6.5), "text2 {min_text2} < {}", ceiling.min(6.5));
            prop_assert!(min_accent + 0.05 >= ceiling.min(4.5), "accent {min_accent} < {}", ceiling.min(4.5));
        }
    }
}

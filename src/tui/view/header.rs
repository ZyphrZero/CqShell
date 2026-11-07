//! The header row (Requirement 33): brand, version and breadcrumb on the left; the mode
//! banner, hostname, mode marker and language name right-aligned. Every part is a Segment.
//! A header wider than the screen gives up the banner, then the hostname, then the
//! breadcrumb's last step; the mode marker and the language name always stay.
use super::{
    Segment,
    theme::{self, Glyph},
};
use crate::{
    dashboard::Reading,
    theme::{Level, Role},
    tui::{App, GROUPS, Row},
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

/// Blank cells at each end of the row.
const MARGIN: usize = 1;
/// The fewest blank cells between the two sides.
const GAP: usize = 2;

/// What a header Segment shows. Variants follow the drawing order: the left side, then the
/// right side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Brand,
    Version,
    Category,
    /// The breadcrumb's last step: the selected action, or the Checklist size in the
    /// Optimization_Category.
    Step,
    /// `ui.root_banner`, `ui.user_banner` or `ui.preview_banner`.
    Banner,
    Host,
    Mode,
    Language,
}
impl Part {
    fn left(self) -> bool {
        matches!(
            self,
            Self::Brand | Self::Version | Self::Category | Self::Step
        )
    }
}
/// Parts given up, in this order, while the header is wider than the screen (Requirement
/// 33.6). The mode marker and the language name are never among them.
const DROP_ORDER: [Part; 3] = [Part::Banner, Part::Host, Part::Step];

/// A header Segment and the joint drawn before it when another Segment precedes it on the
/// same side.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    part: Part,
    joint: Segment,
    segment: Segment,
}

/// The mode marker's text key, the banner key and the marker style. An unsupported platform
/// previews even as root, since no system action runs there.
fn mode(app: &App) -> (&'static str, &'static str, Style) {
    let t = &app.theme.active;
    if app.platform_error.is_some() {
        (
            "ui.mode_preview",
            "ui.preview_banner",
            theme::fg(t, Role::Warning),
        )
    } else if app.root {
        ("ui.mode_root", "ui.root_banner", theme::fg(t, Role::Danger))
    } else {
        (
            "ui.mode_user",
            "ui.user_banner",
            theme::text(t, Level::Tertiary),
        )
    }
}

/// Every part the header shows when there is room for all of them, in drawing order.
fn items(app: &App, selected: &Row) -> Vec<Entry> {
    let i = &app.ctx.i18n;
    let t = &app.theme.active;
    let blank = |cells: usize| Segment::new(" ".repeat(cells), Style::default());
    let joint = |glyph| {
        Segment::new(
            format!(" {} ", theme::glyph(t, glyph)),
            theme::text(t, Level::Tertiary),
        )
    };
    let item = |part, joint, segment| Entry {
        part,
        joint,
        segment,
    };
    let mut items = vec![
        item(
            Part::Brand,
            blank(0),
            Segment::new(
                "CqShell",
                theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
            ),
        ),
        item(
            Part::Version,
            blank(1),
            Segment::new(format!("v{}", env!("CARGO_PKG_VERSION")), theme::muted(t)),
        ),
        item(
            Part::Category,
            blank(2),
            Segment::new(
                i.text(&format!("group.{}", GROUPS[app.group])),
                theme::muted(t),
            ),
        ),
    ];
    // The Optimization_Category counts its Checklist whichever row is selected, the
    // Execute_Row included (Requirement 33.2).
    if app.optimization() {
        items.push(item(
            Part::Step,
            joint(Glyph::Separator),
            Segment::new(
                app.marked.len().to_string(),
                theme::fg(t, Role::Accent).add_modifier(Modifier::BOLD),
            ),
        ));
    } else if let Row::Action(spec) = selected {
        items.push(item(
            Part::Step,
            joint(Glyph::Mark),
            Segment::new(i.text(&spec.label()), theme::text(t, Level::Primary)),
        ));
    }
    let (mode_key, banner_key, mode_style) = mode(app);
    items.push(item(
        Part::Banner,
        joint(Glyph::Separator),
        Segment::new(i.text(banner_key), theme::text(t, Level::Tertiary)),
    ));
    if let Reading::Value(host) = &app.snapshot.local.host
        && !host.is_empty()
    {
        items.push(item(
            Part::Host,
            joint(Glyph::Separator),
            Segment::new(host.clone(), theme::text(t, Level::Primary)),
        ));
    }
    items.push(item(
        Part::Mode,
        joint(Glyph::Separator),
        Segment::new(
            format!("{} {}", theme::glyph(t, Glyph::On), i.text(mode_key)),
            mode_style,
        ),
    ));
    let language = i
        .languages()
        .into_iter()
        .find(|(id, _)| *id == i.locale())
        .map_or(i.locale(), |(_, name)| name);
    items.push(item(
        Part::Language,
        joint(Glyph::Separator),
        Segment::new(language, theme::muted(t)),
    ));
    items
}

/// Cells one side's Segments take, with the joints between them.
fn side_width<'a>(items: impl Iterator<Item = &'a Entry>) -> usize {
    items
        .enumerate()
        .map(|(index, item)| item.segment.width() + if index == 0 { 0 } else { item.joint.width() })
        .sum()
}
/// Cells the row needs for `items`: both margins, both sides and the gap between them.
fn cells(items: &[Entry]) -> usize {
    2 * MARGIN
        + side_width(items.iter().filter(|item| item.part.left()))
        + GAP
        + side_width(items.iter().filter(|item| !item.part.left()))
}
/// `items` without the parts given up, in `DROP_ORDER`, until the row fits in `width`
/// cells. Dropping stops as soon as it fits; a row that still does not fit is clipped when
/// drawn.
fn fit(mut items: Vec<Entry>, width: usize) -> Vec<Entry> {
    for part in DROP_ORDER {
        if cells(&items) <= width {
            break;
        }
        items.retain(|item| item.part != part);
    }
    items
}

/// One side within `room` cells: whole Segments while they fit, then the next one clipped
/// with an ellipsis, then nothing (Compact_Mode's last step). Returns the spans and the
/// cells they take.
fn side(theme: &crate::theme::Theme, items: &[&Entry], room: usize) -> (Vec<Span<'static>>, usize) {
    let mut spans = Vec::new();
    let mut used = 0;
    for (index, item) in items.iter().enumerate() {
        let joint = (index > 0).then_some(&item.joint);
        let lead = used + joint.map_or(0, Segment::width);
        let whole = lead + item.segment.width() <= room;
        let text = if whole {
            item.segment.text.clone()
        } else {
            super::clip_with(
                &item.segment.text,
                room.saturating_sub(lead),
                super::ellipsis(theme),
            )
        };
        if text.is_empty() {
            break;
        }
        if let Some(joint) = joint {
            spans.push(Span::styled(joint.text.clone(), joint.style));
        }
        used = lead + text.width();
        spans.push(Span::styled(text, item.segment.style));
        if !whole {
            break;
        }
    }
    (spans, used)
}

pub(super) fn draw(app: &App, f: &mut Frame, area: Rect, selected: &Row) {
    let width = usize::from(area.width);
    let items = fit(items(app, selected), width);
    let (left, right): (Vec<&Entry>, Vec<&Entry>) = items.iter().partition(|item| item.part.left());
    let inner = width.saturating_sub(2 * MARGIN);
    // The right side holds the parts that always stay, so it is laid out first and the
    // left side takes the cells that remain.
    let (right, right_used) = side(&app.theme.active, &right, inner);
    let (left, left_used) = side(
        &app.theme.active,
        &left,
        inner.saturating_sub(right_used + GAP),
    );
    let mut spans = vec![Span::raw(" ".repeat(MARGIN))];
    spans.extend(left);
    spans.push(Span::raw(
        " ".repeat(inner.saturating_sub(left_used + right_used)),
    ));
    spans.extend(right);
    spans.push(Span::raw(" ".repeat(MARGIN)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::Part::*;
    use super::*;
    use crate::{
        actions,
        error::Error,
        tui::{OPTIMIZATION_GROUP, snapshot::fixture_app},
    };
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Color};

    fn parts(items: &[Entry]) -> Vec<Part> {
        items.iter().map(|item| item.part).collect()
    }
    fn find(items: &[Entry], part: Part) -> Option<&Entry> {
        items.iter().find(|item| item.part == part)
    }
    /// The parts `fit` gives up at `width`, in the order they went.
    fn dropped(app: &App, width: usize) -> Vec<Part> {
        let kept = parts(&fit(items(app, &app.selected_row()), width));
        DROP_ORDER
            .into_iter()
            .filter(|part| !kept.contains(part))
            .collect()
    }
    /// The header drawn alone in a `width`×1 buffer, and its text with the continuation
    /// cells of wide glyphs left out.
    fn row(app: &App, width: u16) -> (String, Buffer) {
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|f| draw(app, f, f.area(), &app.selected_row()))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        let mut hidden = 0;
        for x in 0..width {
            let symbol = buffer[(x, 0)].symbol();
            if hidden > 0 {
                hidden -= 1;
                continue;
            }
            text.push_str(symbol);
            hidden = symbol.width().saturating_sub(1);
        }
        (text, buffer)
    }

    #[test]
    fn the_header_lists_brand_version_breadcrumb_then_banner_host_mode_and_language() {
        for locale in ["en-US", "zh-CN"] {
            let app = fixture_app(locale);
            let i = &app.ctx.i18n;
            let Row::Action(spec) = app.selected_row() else {
                panic!("the fixture selects an action");
            };
            let items = items(&app, &app.selected_row());
            assert_eq!(
                parts(&items),
                [Brand, Version, Category, Step, Banner, Host, Mode, Language]
            );
            let texts = items
                .iter()
                .map(|item| item.segment.text.as_str())
                .collect::<Vec<_>>();
            let language = if locale == "en-US" {
                "English"
            } else {
                "简体中文"
            };
            let version = format!("v{}", env!("CARGO_PKG_VERSION"));
            let mode = format!("● {}", i.text("ui.mode_user"));
            assert_eq!(
                texts,
                [
                    "CqShell",
                    version.as_str(),
                    i.text("group.system"),
                    i.text(&spec.label()),
                    i.text("ui.user_banner"),
                    "vps-sg-01",
                    mode.as_str(),
                    language,
                ]
            );
            assert_eq!(find(&items, Step).unwrap().joint.text, " › ");
            let (text, _) = row(&app, 200);
            let breadcrumb = format!("{} › {}", i.text("group.system"), i.text(&spec.label()));
            assert!(
                text.starts_with(&format!(
                    " CqShell v{}  {breadcrumb} ",
                    env!("CARGO_PKG_VERSION")
                )),
                "{text:?}"
            );
            let right = format!(
                " {} · vps-sg-01 · {mode} · {language} ",
                i.text("ui.user_banner")
            );
            assert!(text.ends_with(&right), "{text:?}");
            assert_eq!(text.width(), 200);
        }
    }

    #[test]
    fn narrow_headers_drop_the_banner_then_the_host_then_the_action() {
        let app = fixture_app("en-US");
        let version = env!("CARGO_PKG_VERSION");
        // Left "CqShell v<version>  System › System overview" takes 34 cells besides the
        // version and right "<banner> · vps-sg-01 · ● USER · English" 62; with the margins
        // and the gap the full row needs 100 cells besides the version (106 for v0.1.4).
        let v = format!("v{version}").width();
        let full = 100 + v;
        // Without the 34-cell banner and its joint, then the host's 12, then the action's 18.
        let (no_banner, no_host, least) = (full - 34, full - 34 - 12, full - 34 - 12 - 18);
        for (width, expected) in [
            (200, &[][..]),
            (120, &[]),
            (full, &[]),
            (full - 1, &[Banner]),
            (80, &[Banner]),
            (no_banner, &[Banner]),
            (no_banner - 1, &[Banner, Host]),
            (no_host, &[Banner, Host]),
            (no_host - 1, &[Banner, Host, Step]),
            (least, &[Banner, Host, Step]),
            (20, &[Banner, Host, Step]),
        ] {
            assert_eq!(dropped(&app, width), expected, "width {width}");
        }
        for width in 1..=200 {
            let all = items(&app, &app.selected_row());
            let kept = fit(all.clone(), width);
            let gone = dropped(&app, width);
            // Parts go strictly in DROP_ORDER, and only while the row does not fit.
            assert_eq!(gone, DROP_ORDER[..gone.len()], "width {width}");
            if let Some((last, before)) = gone.split_last() {
                let mut wider = all;
                wider.retain(|item| !before.contains(&item.part));
                assert!(cells(&wider) > width, "{last:?} went at width {width}");
            }
            assert!(cells(&kept) <= width || gone.len() == DROP_ORDER.len());
            assert!(find(&kept, Mode).is_some() && find(&kept, Language).is_some());
        }
        for (width, expected) in [
            (
                80,
                format!(
                    " CqShell v{version}  System › System overview{}vps-sg-01 · ● USER · English ",
                    " ".repeat(16 - v)
                ),
            ),
            (
                no_host,
                format!(" CqShell v{version}  System › System overview  ● USER · English "),
            ),
            (
                least,
                format!(" CqShell v{version}  System  ● USER · English "),
            ),
            // Past the last drop the left side is clipped; the mode and language stay.
            (30, " CqShell v…  ● USER · English ".into()),
            (20, "   ● USER · English ".into()),
        ] {
            let (text, _) = row(&app, width as u16);
            assert_eq!(text, expected, "width {width}");
        }
    }

    #[test]
    fn root_preview_and_user_modes_color_their_marker() {
        let mut app = fixture_app("en-US");
        let check = |app: &App, mode: &str, banner: &str, role: Role| {
            let i = &app.ctx.i18n;
            let t = &app.theme.active;
            let items = items(app, &app.selected_row());
            let marker = find(&items, Mode).unwrap();
            assert_eq!(marker.segment.text, format!("● {}", i.text(mode)));
            let style = match role {
                Role::Text3 => theme::text(t, Level::Tertiary),
                other => theme::fg(t, other),
            };
            assert_eq!(marker.segment.style, style, "{mode}");
            assert_eq!(find(&items, Banner).unwrap().segment.text, i.text(banner));
            // The dot and the text share the color in the drawn row.
            let (text, buffer) = row(app, 80);
            let at = text[..text.find('●').unwrap()].width() as u16;
            for x in at..at + marker.segment.width() as u16 {
                assert_eq!(buffer[(x, 0)].fg, theme::color(t, role), "{mode} at {x}");
            }
        };
        check(&app, "ui.mode_user", "ui.user_banner", Role::Text3);
        app.root = true;
        check(&app, "ui.mode_root", "ui.root_banner", Role::Danger);
        // An unsupported platform previews, as root too.
        app.platform_error = Some(Error::new("error.platform", "windows"));
        check(&app, "ui.mode_preview", "ui.preview_banner", Role::Warning);
        app.root = false;
        check(&app, "ui.mode_preview", "ui.preview_banner", Role::Warning);
        // classic draws them in its palette colors.
        let t = &app.theme.active;
        assert_eq!(theme::color(t, Role::Danger), Color::Rgb(0xff, 0x75, 0x85));
        assert_eq!(theme::color(t, Role::Warning), Color::Rgb(0xff, 0xbe, 0x5e));
        assert_eq!(theme::color(t, Role::Text3), Color::Rgb(0x91, 0xa2, 0xb0));
    }

    #[test]
    fn the_optimization_breadcrumb_counts_the_checklist() {
        let mut app = fixture_app("zh-CN");
        app.group = OPTIMIZATION_GROUP;
        app.marked
            .extend(["bbr.enable", "tcp.tune"].map(String::from));
        // An action row and the Execute_Row show the same breadcrumb.
        for position in [0, actions::OPTIMIZATION.len()] {
            app.positions[OPTIMIZATION_GROUP] = position;
            let items = items(&app, &app.selected_row());
            assert_eq!(find(&items, Category).unwrap().segment.text, "一键优化");
            let step = find(&items, Step).unwrap();
            assert_eq!(
                (step.joint.text.as_str(), step.segment.text.as_str()),
                (" · ", "2")
            );
            let (text, _) = row(&app, 120);
            assert!(text.contains("  一键优化 · 2  "), "{text:?}");
        }
        app.marked.clear();
        assert_eq!(
            find(&items(&app, &app.selected_row()), Step)
                .unwrap()
                .segment
                .text,
            "0"
        );
    }

    #[test]
    fn the_host_shows_only_for_a_value() {
        let mut app = fixture_app("en-US");
        for reading in [
            Reading::Loading,
            Reading::NotQueried,
            Reading::Unavailable("no hostname".into()),
            Reading::Value(String::new()),
        ] {
            app.snapshot.local.host = reading;
            let items = items(&app, &app.selected_row());
            assert!(
                find(&items, Host).is_none(),
                "{:?}",
                app.snapshot.local.host
            );
            assert_eq!(parts(&items).len(), 7);
        }
    }
}

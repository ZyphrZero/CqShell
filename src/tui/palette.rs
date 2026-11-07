//! Command_Palette (design component 5, Requirement 8): its state, [`search`], which lists
//! the Actions whose localized name or Action ID contains the query, grouped by category,
//! and [`Results`], the rows the palette lists for its query. Nothing here draws or handles
//! keys; the view marks the matched parts with the ranges [`Hit`] carries.
use super::{GROUPS, OPTIMIZATION_GROUP, input::InputBuffer, sections};
use crate::{actions::Spec, i18n::I18n};
use std::ops::Range;

/// The open Command_Palette: the query being typed and the highlighted result, counted
/// across all result rows. It opens with an empty query and the first result highlighted
/// (Requirement 8.1).
#[derive(Debug, Default)]
pub(super) struct CommandPalette {
    pub query: InputBuffer,
    pub cursor: usize,
}

/// The hits of one Reorderable_Category, in Action_List order.
#[derive(Debug)]
pub(super) struct HitGroup {
    /// Index into `GROUPS`; never `OPTIMIZATION_GROUP`.
    pub group: usize,
    /// Never empty: a category without hits has no group.
    pub hits: Vec<Hit>,
}

/// One matching Action. Ranges are byte ranges that start and end on character boundaries,
/// so `&text[range]` is always valid, CJK text included.
#[derive(Debug)]
pub(super) struct Hit {
    pub spec: Spec,
    /// The parts of the localized name that match, in order; empty when only the ID matches
    /// or the query is empty.
    pub label_ranges: Vec<Range<usize>>,
    /// `Some` only when the ID matches and the name does not: the parts of `spec.id` that
    /// match, for the row that also shows the ID in text3.
    pub id_ranges: Option<Vec<Range<usize>>>,
}

/// The Actions of `specs` whose localized name or ID contains `query`, both compared after
/// `to_lowercase()` (Requirement 8.2). Groups follow `order`, the Reorderable_Categories in
/// Category_List order; hits within a group follow the Action_List, read from the same
/// `sections::blocks` as `App::rows` (Requirement 8.3). The Optimization_Category has no
/// Actions of its own, so it never forms a group even when `order` names it. An empty query
/// matches every Action, with nothing highlighted.
pub(super) fn search(query: &str, specs: &[Spec], i18n: &I18n, order: &[usize]) -> Vec<HitGroup> {
    let query = query.to_lowercase();
    order
        .iter()
        .copied()
        .filter(|group| *group != OPTIMIZATION_GROUP)
        .filter_map(|group| {
            let hits = sections::blocks(GROUPS[group], specs)
                .into_iter()
                .flat_map(|(_, ids)| ids)
                // Only the given specs are searched.
                .filter_map(|id| specs.iter().find(|spec| spec.id == id))
                .filter_map(|spec| hit(spec, &query, i18n))
                .collect::<Vec<_>>();
            (!hits.is_empty()).then_some(HitGroup { group, hits })
        })
        .collect()
}

/// `spec` as a hit for the lowercase `query`: by its name when the name matches, otherwise
/// by its ID.
fn hit(spec: &Spec, query: &str, i18n: &I18n) -> Option<Hit> {
    if let Some(label_ranges) = matches(i18n.text(&spec.label()), query) {
        return Some(Hit {
            spec: spec.clone(),
            label_ranges,
            id_ranges: None,
        });
    }
    matches(spec.id, query).map(|id_ranges| Hit {
        spec: spec.clone(),
        label_ranges: Vec::new(),
        id_ranges: Some(id_ranges),
    })
}

/// `None` unless `text.to_lowercase()` contains the lowercase `query`; otherwise the parts of
/// `text` that form its non-overlapping occurrences there, in order, with parts that touch
/// or share a character merged.
///
/// Lowercasing may change a character's length ("İ", two bytes, becomes "i̇", three), so each
/// occurrence in the lowercase text maps back to the characters of `text` whose lowercase
/// forms it overlaps. `str::to_lowercase` lowercases each character on its own except for
/// Σ, whose two lowercase forms are both two bytes long, so the per-character lengths below
/// add up to the lowercase text.
fn matches(text: &str, query: &str) -> Option<Vec<Range<usize>>> {
    let lower = text.to_lowercase();
    if !lower.contains(query) {
        return None;
    }
    if query.is_empty() {
        return Some(Vec::new());
    }
    // For each character of `text`: where its lowercase form ends in `lower`, and its bytes.
    let mut end = 0;
    let spans = text
        .char_indices()
        .map(|(at, c)| {
            end += c.to_lowercase().map(char::len_utf8).sum::<usize>();
            (end, at..at + c.len_utf8())
        })
        .collect::<Vec<_>>();
    debug_assert_eq!(end, lower.len());
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (start, found) in lower.match_indices(query) {
        // The first character whose lowercase form ends after the occurrence starts, and the
        // first whose form reaches its end; every lowercase form is at least one byte long.
        let first = spans.partition_point(|(stop, _)| *stop <= start);
        let last = spans.partition_point(|(stop, _)| *stop < start + found.len());
        let range = spans[first].1.start..spans[last].1.end;
        match ranges.last_mut() {
            Some(previous) if range.start <= previous.end => previous.end = range.end,
            _ => ranges.push(range),
        }
    }
    Some(ranges)
}

/// One row of the Command_Palette's result list.
#[derive(Debug)]
pub(super) enum ResultRow {
    /// The name of the category whose hits follow; never highlighted.
    Group(usize),
    Hit(Hit),
}

/// The rows the Command_Palette lists for its query. A query lists the hits of [`search`],
/// each group after its category's name (Requirement 8.3); an empty query lists Recent_Use,
/// most recently started first and without category names (Requirement 8.6). Empty when
/// nothing matches or, for an empty query, nothing has run yet; the view then shows
/// `ui.no_match` or `ui.palette_hint` (Requirement 8.7). Only hits are highlighted, and
/// `CommandPalette::cursor` counts them across all rows.
#[derive(Debug)]
pub(super) struct Results {
    pub rows: Vec<ResultRow>,
}
impl Results {
    pub fn new<'a>(
        query: &str,
        recent: impl IntoIterator<Item = &'a str>,
        specs: &[Spec],
        i18n: &I18n,
        order: &[usize],
    ) -> Self {
        let rows = if query.is_empty() {
            recent
                .into_iter()
                .filter_map(|id| specs.iter().find(|spec| spec.id == id))
                .map(|spec| {
                    ResultRow::Hit(Hit {
                        spec: spec.clone(),
                        label_ranges: Vec::new(),
                        id_ranges: None,
                    })
                })
                .collect()
        } else {
            search(query, specs, i18n, order)
                .into_iter()
                .flat_map(|group| {
                    std::iter::once(ResultRow::Group(group.group))
                        .chain(group.hits.into_iter().map(ResultRow::Hit))
                })
                .collect()
        };
        Self { rows }
    }
    fn hits(&self) -> impl Iterator<Item = (usize, &Hit)> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| match row {
                ResultRow::Hit(hit) => Some((index, hit)),
                ResultRow::Group(_) => None,
            })
    }
    /// The number of hits: the matches the palette's title counts (Requirement 8.5).
    pub fn count(&self) -> usize {
        self.hits().count()
    }
    /// The `n`-th hit, counted across all rows.
    pub fn hit(&self, n: usize) -> Option<&Hit> {
        self.hits().nth(n).map(|(_, hit)| hit)
    }
    /// Index into `rows` of the `n`-th hit.
    pub fn row_of(&self, n: usize) -> Option<usize> {
        self.hits().nth(n).map(|(index, _)| index)
    }
    /// Which hit `rows[index]` is; `None` for a category name or a row past the end.
    pub fn position(&self, index: usize) -> Option<usize> {
        self.hits().position(|(at, _)| at == index)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{App, Row, default_category_order};
    use super::*;
    use crate::{actions, config::Paths, runtime::Context};
    use proptest::{prelude::*, sample::Index};
    use std::collections::BTreeSet;

    const LOCALES: [&str; 2] = ["zh-CN", "en-US"];

    fn load(locale: &str) -> I18n {
        I18n::load(Some(locale), None).unwrap()
    }

    /// The parts of `text` that `ranges` mark; slicing panics unless every range lies on
    /// character boundaries.
    fn parts<'a>(text: &'a str, ranges: &[Range<usize>]) -> Vec<&'a str> {
        ranges.iter().map(|range| &text[range.clone()]).collect()
    }

    fn marked<'a>(text: &'a str, query: &str) -> Option<Vec<&'a str>> {
        matches(text, query).map(|ranges| parts(text, &ranges))
    }

    #[test]
    fn matched_ranges_map_back_to_character_boundaries() {
        // Byte ranges: "安装 " is seven bytes long.
        let bytes = matches("安装 Docker", "docker")
            .unwrap()
            .iter()
            .map(|range| (range.start, range.end))
            .collect::<Vec<_>>();
        assert_eq!(bytes, [(7, 13)]);
        assert_eq!(marked("安装 Docker", "docker"), Some(vec!["Docker"]));
        assert_eq!(marked("aXa", "a"), Some(vec!["a", "a"]));
        assert_eq!(marked("aa", "a"), Some(vec!["aa"]), "touching parts merge");
        // "İx" lowercases to "i\u{307}x"; an occurrence that starts inside the lowercase
        // form of "İ" covers the whole character.
        assert_eq!(marked("İx", "\u{307}x"), Some(vec!["İx"]));
        assert_eq!(marked("Clean system", "zz"), None);
        assert_eq!(marked("Clean system", ""), Some(vec![]));
    }

    #[test]
    fn id_only_hits_carry_the_id_ranges_and_optimization_never_forms_a_group() {
        let specs = actions::specs();
        let order = default_category_order();
        let groups = search("BBR.Enable", &specs, &load("en-US"), &order);
        assert_eq!(groups.len(), 1);
        let [hit] = &groups[0].hits[..] else {
            panic!("{groups:?}")
        };
        assert_eq!(hit.spec.id, "bbr.enable");
        assert!(hit.label_ranges.is_empty());
        let id_ranges = hit.id_ranges.as_deref().unwrap();
        assert_eq!(parts(hit.spec.id, id_ranges), ["bbr.enable"]);

        // "安装 Docker" matches by name, so the ID is not shown.
        let zh = load("zh-CN");
        let groups = search("DOCKER", &specs, &zh, &order);
        let hit = groups
            .iter()
            .flat_map(|group| &group.hits)
            .find(|hit| hit.spec.id == "docker.install")
            .unwrap();
        let name = zh.text(&hit.spec.label());
        assert_eq!(parts(name, &hit.label_ranges), ["Docker"]);
        assert_eq!(hit.id_ranges, None);

        let display = std::iter::once(OPTIMIZATION_GROUP)
            .chain(order)
            .collect::<Vec<_>>();
        let groups = search("", &specs, &load("en-US"), &display);
        assert!(groups.iter().all(|group| group.group != OPTIMIZATION_GROUP));
        let hits = groups.iter().flat_map(|group| &group.hits);
        assert!(hits.clone().all(|hit| hit.label_ranges.is_empty()));
        assert_eq!(
            hits.count(),
            specs.len(),
            "an empty query matches every Action"
        );
    }

    fn row_ids(results: &Results) -> Vec<String> {
        results
            .rows
            .iter()
            .map(|row| match row {
                ResultRow::Group(group) => format!("<{}>", GROUPS[*group]),
                ResultRow::Hit(hit) => hit.spec.id.to_owned(),
            })
            .collect()
    }

    #[test]
    fn an_empty_query_lists_recent_use_and_a_query_names_each_group() {
        let specs = actions::specs();
        let order = default_category_order();
        let i18n = load("en-US");
        // Nothing has run: an empty list, for which the view shows `ui.palette_hint`.
        let none = Results::new("", [], &specs, &i18n, &order);
        assert!(none.rows.is_empty());
        assert_eq!(
            (none.count(), none.hit(0).map(|hit| hit.spec.id)),
            (0, None)
        );

        // Recent_Use as recorded, newest first, without category names or marks.
        let recent = Results::new("", ["tcp.tune", "system.info"], &specs, &i18n, &order);
        assert_eq!(row_ids(&recent), ["tcp.tune", "system.info"]);
        assert_eq!(recent.count(), 2);
        assert_eq!(recent.row_of(1), Some(1));
        let ResultRow::Hit(hit) = &recent.rows[0] else {
            panic!("{recent:?}")
        };
        assert!(hit.label_ranges.is_empty() && hit.id_ranges.is_none());

        // A query: each group after its category's name, in Category_List order, with hits
        // counted across groups.
        let bbr = Results::new("bbr", ["tcp.tune"], &specs, &i18n, &order);
        assert_eq!(
            row_ids(&bbr),
            [
                "<kernel>",
                "kernel.install",
                "kernel.remove",
                "<network>",
                "bbr.status",
                "bbr.enable",
                "bbr.restore",
            ]
        );
        assert_eq!(bbr.count(), 5);
        assert_eq!(bbr.hit(2).map(|hit| hit.spec.id), Some("bbr.status"));
        assert_eq!(
            (bbr.row_of(0), bbr.row_of(2), bbr.row_of(5)),
            (Some(1), Some(4), None)
        );
        assert_eq!(bbr.position(4), Some(2));
        assert_eq!(bbr.position(3), None, "a category name is not a hit");
        assert_eq!(bbr.position(7), None);
        assert!(
            Results::new("zzz", [], &specs, &i18n, &order)
                .rows
                .is_empty()
        );
    }

    /// Every localized name in both locales and every Action ID: the texts queries are cut
    /// from.
    fn texts() -> Vec<String> {
        let specs = actions::specs();
        let mut texts = specs
            .iter()
            .map(|spec| spec.id.to_owned())
            .collect::<Vec<_>>();
        for locale in LOCALES {
            let i18n = load(locale);
            texts.extend(specs.iter().map(|spec| i18n.text(&spec.label()).to_owned()));
        }
        texts
    }

    /// A non-empty query: mostly a random substring of a real name or ID, otherwise one to
    /// four arbitrary printable characters; then each character kept, uppercased or
    /// lowercased at random.
    fn query() -> impl Strategy<Value = String> {
        let cut = (
            proptest::sample::select(texts()),
            any::<Index>(),
            any::<Index>(),
        )
            .prop_map(|(text, start, len)| {
                let chars = text.chars().collect::<Vec<_>>();
                let start = start.index(chars.len());
                let len = 1 + len.index(chars.len() - start);
                chars[start..start + len].iter().collect::<String>()
            });
        (
            prop_oneof![4 => cut, 1 => "\\PC{1,4}"],
            proptest::collection::vec(0..3u8, 0..64),
        )
            .prop_map(|(text, cases)| {
                let cases = cases.into_iter().chain(std::iter::repeat(0));
                text.chars()
                    .zip(cases)
                    .map(|(c, case)| match case {
                        1 => c.to_uppercase().collect::<String>(),
                        2 => c.to_lowercase().collect(),
                        _ => c.to_string(),
                    })
                    .collect()
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        // Feature: tui-redesign, Property 8: 搜索完整且无误报
        /// **Validates: Requirements 8.2, 8.3**
        #[test]
        fn search_is_complete_and_has_no_false_positives(
            query in query(),
            locale in proptest::sample::select(LOCALES.to_vec()),
            order in Just((0..OPTIMIZATION_GROUP).collect::<Vec<_>>()).prop_shuffle(),
        ) {
            let specs = actions::specs();
            let i18n = load(locale);
            let wanted = query.to_lowercase();
            let matching = |spec: &Spec| {
                i18n.text(&spec.label()).to_lowercase().contains(&wanted)
                    || spec.id.to_lowercase().contains(&wanted)
            };
            let groups = search(&query, &specs, &i18n, &order);
            let found = groups
                .iter()
                .flat_map(|group| group.hits.iter().map(|hit| (group.group, hit.spec.id)))
                .collect::<Vec<_>>();

            // An Action is listed if and only if its name or ID contains the query, once.
            let listed = found.iter().map(|(_, id)| *id).collect::<BTreeSet<_>>();
            prop_assert_eq!(listed.len(), found.len(), "an Action is listed twice");
            for spec in &specs {
                prop_assert_eq!(listed.contains(spec.id), matching(spec), "{}", spec.id);
            }

            // Groups follow `order`, each non-empty; hits follow the Action_List.
            let app = App::new(Context::new(Paths::from_env().unwrap(), load("en-US"), false));
            let expected = order
                .iter()
                .flat_map(|&group| {
                    app.rows(group).into_iter().filter_map(move |row| match row {
                        Row::Action(spec) => Some((group, spec)),
                        _ => None,
                    })
                })
                .filter(|(_, spec)| matching(spec))
                .map(|(group, spec)| (group, spec.id))
                .collect::<Vec<_>>();
            prop_assert_eq!(&found, &expected);
            let mut expected_groups = expected.iter().map(|(group, _)| *group).collect::<Vec<_>>();
            expected_groups.dedup();
            let group_order = groups.iter().map(|group| group.group).collect::<Vec<_>>();
            prop_assert_eq!(group_order, expected_groups);
        }
    }
}

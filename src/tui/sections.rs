//! Section table of the Action_List (design, component 4): (category, [(Section ID,
//! [Action ID])]). A Section title's text key is `section.<category>.<Section ID>`. Categories
//! that are not listed show their actions ungrouped, in the order of `actions::specs()`.
use crate::actions::Spec;

/// A Section: its ID and its Action IDs in display order.
type Section = (&'static str, &'static [&'static str]);
/// A run of an Action_List: its Section ID (`None` in an ungrouped category) and its Action
/// IDs in display order.
pub(super) type Block = (Option<&'static str>, Vec<&'static str>);

const SECTIONS: &[(&str, &[Section])] = &[
    (
        "system",
        &[
            ("overview", &["system.info", "system.public_ip"]),
            (
                "packages",
                &[
                    "system.update",
                    "system.mirror.test",
                    "system.mirror.apply",
                    "system.mirror.restore",
                    "system.clean",
                ],
            ),
            ("settings", &["system.timezone", "system.fail2ban"]),
            ("swap", &["swap.create", "swap.auto", "swap.remove"]),
        ],
    ),
    (
        "clash",
        &[
            (
                "core",
                &["clash.install", "clash.status", "clash.start", "clash.stop"],
            ),
            (
                "subscriptions",
                &[
                    "clash.sub.list",
                    "clash.sub.add",
                    "clash.sub.use",
                    "clash.sub.update",
                    "clash.sub.remove",
                ],
            ),
            (
                "controller",
                &[
                    "clash.controller.status",
                    "clash.controller.configure",
                    "clash.proxies",
                    "clash.proxy.select",
                    "clash.secret.generate",
                    "clash.secret.set",
                    "clash.secret.clear",
                ],
            ),
            ("panel", &["clash.panel.status", "clash.panel.install"]),
            ("tun", &["clash.tun.status", "clash.tun.configure"]),
            (
                "service",
                &[
                    "clash.logs",
                    "clash.service.status",
                    "clash.service.install",
                    "clash.service.remove",
                ],
            ),
        ],
    ),
    (
        "network",
        &[
            ("bbr", &["bbr.status", "bbr.enable", "bbr.restore"]),
            ("tcp", &["tcp.tune", "tcp.restore"]),
        ],
    ),
    (
        "docker",
        &[
            ("setup", &["docker.install", "docker.status"]),
            ("config", &["docker.mirror", "docker.ipv6"]),
            ("cleanup", &["docker.clean", "docker.uninstall"]),
        ],
    ),
    (
        "tools",
        &[
            (
                "single",
                &[
                    "tools.curl",
                    "tools.wget",
                    "tools.nano",
                    "tools.unzip",
                    "tools.tar",
                    "tools.tmux",
                    "tools.iftop",
                    "tools.btop",
                    "tools.gdu",
                    "tools.fzf",
                ],
            ),
            ("all", &["tools.install"]),
        ],
    ),
];

/// The Sections of `group` in display order, or `None` for an ungrouped category.
pub(super) fn of(group: &str) -> Option<&'static [Section]> {
    SECTIONS
        .iter()
        .find(|(category, _)| *category == group)
        .map(|(_, sections)| *sections)
}

/// The Action_List order of `group` (design, component 4): one block per Section in table
/// order for a grouped category, otherwise a single untitled block with the category's actions
/// in the order of `specs`, which callers pass as `actions::specs()`. `App::rows` and the
/// Command_Palette's `search` both read this, so search results within a category follow
/// the Action_List exactly (Requirement 8.3).
pub(super) fn blocks(group: &str, specs: &[Spec]) -> Vec<Block> {
    match of(group) {
        Some(sections) => sections
            .iter()
            .map(|(section, ids)| (Some(*section), ids.to_vec()))
            .collect(),
        None => vec![(
            None,
            specs
                .iter()
                .filter(|spec| spec.group == group)
                .map(|spec| spec.id)
                .collect(),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, i18n::I18n};
    use std::collections::BTreeSet;

    #[test]
    fn every_grouped_category_lists_each_of_its_actions_exactly_once() {
        let listed = SECTIONS
            .iter()
            .map(|(category, _)| *category)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            listed,
            BTreeSet::from(["system", "clash", "network", "docker", "tools"]),
            "the other categories stay ungrouped"
        );
        let i18n = I18n::load(Some("en-US"), None).unwrap();
        for (category, sections) in SECTIONS {
            let ids = sections
                .iter()
                .flat_map(|(_, ids)| ids.iter().copied())
                .collect::<Vec<_>>();
            let unique = ids.iter().copied().collect::<BTreeSet<_>>();
            assert_eq!(unique.len(), ids.len(), "{category}: an ID appears twice");
            let expected = actions::specs()
                .into_iter()
                .filter(|spec| spec.group == *category)
                .map(|spec| spec.id)
                .collect::<BTreeSet<_>>();
            assert_eq!(unique, expected, "{category}");
            // Both bundled locales carry the same keys, so en-US stands for both.
            for (section, _) in *sections {
                assert!(i18n.has(&format!("section.{category}.{section}")));
            }
        }
    }
}

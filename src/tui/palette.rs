//! Enhanced Command Palette (`/`, `Ctrl+K`): recent actions, workbench quick commands, and
//! multi-token fuzzy + substring matching across localized names, IDs, categories, and
//! descriptions.

use super::{
    input::InputBuffer,
    sections,
    session::SessionRecords,
    state::{GROUPS, OPTIMIZATION_GROUP},
};
use crate::{actions::Spec, i18n::I18n};
use std::ops::Range;

#[derive(Debug, Default)]
pub(super) struct CommandPalette {
    pub query: InputBuffer,
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkbenchCommand {
    Overview,
    Optimization,
    Docker,
    DockerFocus,
    Mihomo,
    MihomoFocus,
    GithubProxy,
    SocksSecret,
    Theme,
    TransparentBg,
    ReducedMotion,
    Sidebar,
    Language,
    Refresh,
    PublicIp,
    Logs,
    FullLogs,
    CopyCommand,
    CopyLogs,
    PlainView,
}

impl WorkbenchCommand {
    pub const ALL: [Self; 20] = [
        Self::Overview,
        Self::Optimization,
        Self::Docker,
        Self::DockerFocus,
        Self::Mihomo,
        Self::MihomoFocus,
        Self::GithubProxy,
        Self::SocksSecret,
        Self::Theme,
        Self::TransparentBg,
        Self::ReducedMotion,
        Self::Sidebar,
        Self::Language,
        Self::Refresh,
        Self::PublicIp,
        Self::Logs,
        Self::FullLogs,
        Self::CopyCommand,
        Self::CopyLogs,
        Self::PlainView,
    ];

    pub fn label_key(self) -> &'static str {
        match self {
            Self::Overview => "cmd.overview",
            Self::Optimization => "cmd.optimization",
            Self::Docker => "cmd.docker",
            Self::DockerFocus => "cmd.docker_focus",
            Self::Mihomo => "cmd.mihomo",
            Self::MihomoFocus => "cmd.mihomo_focus",
            Self::GithubProxy => "cmd.github_proxy",
            Self::SocksSecret => "cmd.socks_secret",
            Self::Theme => "cmd.theme",
            Self::TransparentBg => "cmd.transparent",
            Self::ReducedMotion => "cmd.motion",
            Self::Sidebar => "cmd.sidebar",
            Self::Language => "cmd.language",
            Self::Refresh => "cmd.refresh",
            Self::PublicIp => "cmd.public_ip",
            Self::Logs => "cmd.logs",
            Self::FullLogs => "cmd.full_logs",
            Self::CopyCommand => "cmd.copy_cmd",
            Self::CopyLogs => "cmd.copy_logs",
            Self::PlainView => "cmd.plain_view",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Self::Overview => "H",
            Self::Optimization => "O",
            Self::Docker => "5",
            Self::DockerFocus => "z",
            Self::Mihomo => "6",
            Self::MihomoFocus => "z",
            Self::GithubProxy => "P",
            Self::SocksSecret => "Shift+S",
            Self::Theme => "T",
            Self::TransparentBg => "T·B",
            Self::ReducedMotion => "T·M",
            Self::Sidebar => "[",
            Self::Language => "L",
            Self::Refresh => "R",
            Self::PublicIp => "N",
            Self::Logs => "G",
            Self::FullLogs => "F2",
            Self::CopyCommand => "C",
            Self::CopyLogs => "Y",
            Self::PlainView => "V",
        }
    }

    pub fn keywords(self) -> &'static str {
        match self {
            Self::Overview => "overview home dashboard 概览 首页 仪表盘",
            Self::Optimization => "optimize batch checklist 一键优化 批量",
            Self::Docker => {
                "docker container compose image volume network daemon 容器 编排 镜像 卷 网络"
            }
            Self::DockerFocus => "docker focus fullscreen zoom 全屏 聚焦 容器",
            Self::Mihomo => {
                "mihomo clash proxy node profile subscription rule delay speed 代理 节点 策略组 订阅 测速 规则 连接"
            }
            Self::MihomoFocus => "mihomo clash focus fullscreen zoom 全屏 聚焦 代理 面板",
            Self::GithubProxy => "github gh proxy mirror china acceleration 代理 加速 镜像 国内",
            Self::SocksSecret => {
                "socks socks5 socks5h secret password auth credential uri mihomo 密钥 密码 凭据 代理 连接串"
            }
            Self::Theme => "theme style appearance color 主题 风格 外观 配色",
            Self::TransparentBg => "transparent background glass 透明 背景",
            Self::ReducedMotion => "motion animation static ssh 动效 动画 静态",
            Self::Sidebar => "sidebar nav collapse 侧边栏 导航 折叠",
            Self::Language => "language locale english chinese 语言 中英",
            Self::Refresh => "refresh reload metrics 刷新 状态",
            Self::PublicIp => "public ip isp ipv4 ipv6 公网",
            Self::Logs => "logs output drawer 日志 面板",
            Self::FullLogs => "full logs modal 完整日志 全屏",
            Self::CopyCommand => "copy command cli clipboard osc52 复制 命令 剪贴板",
            Self::CopyLogs => "copy logs output clipboard yank 复制 日志 剪贴板",
            Self::PlainView => "plain text borderless select drag copy 纯文本 无边框 拖选 复制",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) enum PaletteItem {
    Action {
        spec: Spec,
        label_ranges: Vec<Range<usize>>,
        id_ranges: Option<Vec<Range<usize>>>,
    },
    Command {
        command: WorkbenchCommand,
        label_ranges: Vec<Range<usize>>,
    },
}

#[derive(Debug, Clone)]
pub(super) enum PaletteRow {
    Header(String),
    Item(PaletteItem),
}

pub(super) struct Results {
    pub rows: Vec<PaletteRow>,
    pub item_indices: Vec<usize>,
    pub total_items: usize,
}

impl Results {
    pub fn count(&self) -> usize {
        self.item_indices.len()
    }

    pub fn item_at(&self, cursor: usize) -> Option<&PaletteItem> {
        let row_idx = *self.item_indices.get(cursor)?;
        match &self.rows[row_idx] {
            PaletteRow::Item(item) => Some(item),
            PaletteRow::Header(_) => None,
        }
    }

    pub fn row_of(&self, cursor: usize) -> usize {
        self.item_indices.get(cursor).copied().unwrap_or(0)
    }
}

pub(super) fn search(
    query: &str,
    specs: &[Spec],
    i18n: &I18n,
    order: &[usize],
    records: &SessionRecords,
) -> Results {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|part| part.to_lowercase())
        .filter(|part| !part.is_empty())
        .collect();

    let total_items = specs.len() + WorkbenchCommand::ALL.len();
    let mut rows = Vec::new();
    let mut item_indices = Vec::new();

    if tokens.is_empty() {
        // 1. Recent actions from this session (up to 5)
        let recent_specs: Vec<Spec> = records
            .recent
            .iter()
            .take(5)
            .filter_map(|id| specs.iter().find(|s| s.id == *id).cloned())
            .collect();
        if !recent_specs.is_empty() {
            rows.push(PaletteRow::Header(i18n.text("palette.recent").into()));
            for spec in recent_specs {
                item_indices.push(rows.len());
                rows.push(PaletteRow::Item(PaletteItem::Action {
                    spec,
                    label_ranges: Vec::new(),
                    id_ranges: None,
                }));
            }
        }

        // 2. Workbench quick commands
        rows.push(PaletteRow::Header(i18n.text("palette.commands").into()));
        for command in WorkbenchCommand::ALL {
            item_indices.push(rows.len());
            rows.push(PaletteRow::Item(PaletteItem::Command {
                command,
                label_ranges: Vec::new(),
            }));
        }

        // 3. All actions grouped in category order
        for &group in order.iter().filter(|g| **g != OPTIMIZATION_GROUP) {
            let group_specs: Vec<Spec> = sections::blocks(GROUPS[group], specs)
                .into_iter()
                .flat_map(|(_, ids)| ids)
                .filter_map(|id| specs.iter().find(|s| s.id == id).cloned())
                .collect();
            if group_specs.is_empty() {
                continue;
            }
            rows.push(PaletteRow::Header(
                i18n.text(&format!("group.{}", GROUPS[group])).into(),
            ));
            for spec in group_specs {
                item_indices.push(rows.len());
                rows.push(PaletteRow::Item(PaletteItem::Action {
                    spec,
                    label_ranges: Vec::new(),
                    id_ranges: None,
                }));
            }
        }
        return Results {
            rows,
            item_indices,
            total_items,
        };
    }

    // Matching workbench commands (scored highest when prefix/substring matches)
    let mut matched_cmds = Vec::new();
    for command in WorkbenchCommand::ALL {
        let label = i18n.text(command.label_key());
        let haystack_extra = command.keywords();
        if let Some((score, ranges)) = match_tokens(label, haystack_extra, "", &tokens) {
            matched_cmds.push((score, command, ranges));
        }
    }
    matched_cmds.sort_by_key(|a| std::cmp::Reverse(a.0));

    // Matching actions grouped by category, scored so crisp matches appear cleanly
    let mut action_groups: Vec<(usize, Vec<(i32, PaletteItem)>)> = Vec::new();
    for &group in order.iter().filter(|g| **g != OPTIMIZATION_GROUP) {
        let group_label = i18n.text(&format!("group.{}", GROUPS[group]));
        let mut hits = Vec::new();
        for (_, ids) in sections::blocks(GROUPS[group], specs) {
            for id in ids {
                let Some(spec) = specs.iter().find(|s| s.id == id) else {
                    continue;
                };
                let label = i18n.text(&spec.label());
                let desc = i18n.text(&spec.description());
                let extra = format!("{group_label} {desc}");
                if let Some((score, label_ranges, id_ranges)) =
                    match_action(label, spec.id, &extra, &tokens)
                {
                    hits.push((
                        score,
                        PaletteItem::Action {
                            spec: spec.clone(),
                            label_ranges,
                            id_ranges,
                        },
                    ));
                }
            }
        }
        if !hits.is_empty() {
            hits.sort_by_key(|a| std::cmp::Reverse(a.0));
            action_groups.push((group, hits));
        }
    }
    // Sort categories by their highest-scoring hit so direct label/ID matches rise above
    // description-only mentions (e.g. searching "bbr" ranks Network/Kernel before System Info).
    action_groups.sort_by(|a, b| {
        let top_a = a.1.first().map_or(0, |(s, _)| *s);
        let top_b = b.1.first().map_or(0, |(s, _)| *s);
        top_b.cmp(&top_a)
    });

    // Actions first if query looks like an action or matched actions strongly, commands right after
    // (or commands first if query starts with `>`).
    let commands_first = query.trim_start().starts_with('>');
    if commands_first && !matched_cmds.is_empty() {
        rows.push(PaletteRow::Header(i18n.text("palette.commands").into()));
        for (_, command, label_ranges) in matched_cmds.drain(..) {
            item_indices.push(rows.len());
            rows.push(PaletteRow::Item(PaletteItem::Command {
                command,
                label_ranges,
            }));
        }
    }

    for (group, hits) in action_groups {
        rows.push(PaletteRow::Header(
            i18n.text(&format!("group.{}", GROUPS[group])).into(),
        ));
        for (_, item) in hits {
            item_indices.push(rows.len());
            rows.push(PaletteRow::Item(item));
        }
    }

    if !matched_cmds.is_empty() {
        rows.push(PaletteRow::Header(i18n.text("palette.commands").into()));
        for (_, command, label_ranges) in matched_cmds {
            item_indices.push(rows.len());
            rows.push(PaletteRow::Item(PaletteItem::Command {
                command,
                label_ranges,
            }));
        }
    }

    Results {
        rows,
        item_indices,
        total_items,
    }
}

type ByteRanges = Vec<Range<usize>>;
type ActionMatch = (i32, ByteRanges, Option<ByteRanges>);

fn match_action(label: &str, id: &str, extra: &str, tokens: &[String]) -> Option<ActionMatch> {
    let mut score = 0i32;
    let mut label_ranges = Vec::new();
    let mut id_ranges = Vec::new();
    let mut any_id_match = false;

    for token in tokens {
        let token = token.trim_start_matches('>');
        if token.is_empty() {
            continue;
        }
        if let Some((s, ranges)) = match_field(label, token) {
            score += s + 40;
            label_ranges.extend(ranges);
        } else if let Some((s, ranges)) = match_field(id, token) {
            score += s + 25;
            id_ranges.extend(ranges);
            any_id_match = true;
        } else if let Some((s, _)) = match_field(extra, token) {
            score += s;
        } else {
            return None;
        }
    }
    merge_ranges(&mut label_ranges);
    merge_ranges(&mut id_ranges);
    Some((score, label_ranges, any_id_match.then_some(id_ranges)))
}

fn match_tokens(
    label: &str,
    extra: &str,
    id: &str,
    tokens: &[String],
) -> Option<(i32, Vec<Range<usize>>)> {
    let mut score = 0i32;
    let mut label_ranges = Vec::new();
    for token in tokens {
        let token = token.trim_start_matches('>');
        if token.is_empty() {
            continue;
        }
        if let Some((s, ranges)) = match_field(label, token) {
            score += s + 30;
            label_ranges.extend(ranges);
        } else if !id.is_empty()
            && let Some((s, _)) = match_field(id, token)
        {
            score += s + 15;
        } else if let Some((s, _)) = match_field(extra, token) {
            score += s;
        } else {
            return None;
        }
    }
    merge_ranges(&mut label_ranges);
    Some((score, label_ranges))
}

/// Match `token` (already lowercased) against `text`: exact substring first (highest score),
/// then fuzzy subsequence for ASCII/Latin tokens >= 2 chars. Returns byte ranges on char
/// boundaries of `text`.
fn match_field(text: &str, token: &str) -> Option<(i32, Vec<Range<usize>>)> {
    if token.is_empty() {
        return Some((0, Vec::new()));
    }
    let mut lower = String::with_capacity(text.len());
    let mut spans: Vec<(usize, Range<usize>)> = Vec::new();
    for (byte_at, ch) in text.char_indices() {
        let low: String = ch.to_lowercase().collect();
        lower.push_str(&low);
        spans.push((lower.len(), byte_at..byte_at + ch.len_utf8()));
    }

    // 1. Substring match
    if let Some(pos) = lower.find(token) {
        let end = pos + token.len();
        let first = spans.partition_point(|(stop, _)| *stop <= pos);
        let last = spans.partition_point(|(stop, _)| *stop < end);
        let mut ranges = Vec::new();
        if first < spans.len() && last < spans.len() {
            ranges.push(spans[first].1.start..spans[last].1.end);
        }
        let bonus = if pos == 0 { 60 } else { 35 };
        return Some((bonus, ranges));
    }

    // 2. Fuzzy subsequence (only when token has >= 2 chars and every char is matched in order)
    if token.chars().count() < 2 {
        return None;
    }
    let mut ranges = Vec::new();
    let mut search_from = 0usize;
    let mut gaps = 0i32;
    for target in token.chars() {
        let found = lower[search_from..].find(target)?;
        let abs = search_from + found;
        if search_from > 0 && found > 0 {
            gaps += 1;
        }
        let idx = spans.partition_point(|(stop, _)| *stop <= abs);
        if idx < spans.len() {
            ranges.push(spans[idx].1.clone());
        }
        search_from = abs + target.len_utf8();
    }
    merge_ranges(&mut ranges);
    Some(((20 - gaps * 3).max(1), ranges))
}

fn merge_ranges(ranges: &mut Vec<Range<usize>>) {
    if ranges.len() <= 1 {
        return;
    }
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges.drain(..) {
        match merged.last_mut() {
            Some(prev) if range.start <= prev.end => prev.end = prev.end.max(range.end),
            _ => merged.push(range),
        }
    }
    *ranges = merged;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{actions, tui::state::default_category_order};

    #[test]
    fn empty_query_includes_recent_commands_and_actions() {
        let i18n = I18n::load(Some("zh-CN"), None).unwrap();
        let specs = actions::specs();
        let order = default_category_order();
        let mut records = SessionRecords::default();
        records.started("bbr.enable");
        let results = search("", &specs, &i18n, &order, &records);
        assert!(results.count() >= specs.len() + WorkbenchCommand::ALL.len());
        assert!(matches!(
            results.item_at(0),
            Some(PaletteItem::Action { spec, .. }) if spec.id == "bbr.enable"
        ));
    }

    #[test]
    fn multi_token_and_fuzzy_queries_find_actions_and_commands() {
        let i18n = I18n::load(Some("zh-CN"), None).unwrap();
        let specs = actions::specs();
        let order = default_category_order();
        let records = SessionRecords::default();

        let bbr = search("bbr 启用", &specs, &i18n, &order, &records);
        assert!(bbr.count() >= 1);
        assert!(matches!(
            bbr.item_at(0),
            Some(PaletteItem::Action { spec, .. }) if spec.id == "bbr.enable"
        ));

        let theme_cmd = search("> 主题", &specs, &i18n, &order, &records);
        assert!(matches!(
            theme_cmd.item_at(0),
            Some(PaletteItem::Command {
                command: WorkbenchCommand::Theme,
                ..
            })
        ));
    }
}

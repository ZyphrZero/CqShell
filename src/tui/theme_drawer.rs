//! Live Appearance & Theme Drawer (`T`): dual-tab live preview for color palettes (`[配色]`)
//! and UI design styles (`[样式]`), plus live toggles for transparent canvas (`B`) and reduced
//! motion (`M`). `Enter` persists all changes to `config.toml`; `Esc` restores prior state.

use crate::{
    i18n::I18n,
    theme::{
        self, Background, StyleSelection, Theme, ThemeDefinition, ThemeName, UiStyle, builtin, file,
    },
};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrawerTab {
    Palette,
    Style,
}

impl DrawerTab {
    pub const fn toggle(self) -> Self {
        match self {
            Self::Palette => Self::Style,
            Self::Style => Self::Palette,
        }
    }
}

pub(super) const STYLE_CHOICES: [StyleSelection; 9] = [
    StyleSelection::Auto,
    StyleSelection::Fixed(UiStyle::Rounded),
    StyleSelection::Fixed(UiStyle::Bento),
    StyleSelection::Fixed(UiStyle::Editorial),
    StyleSelection::Fixed(UiStyle::Hud),
    StyleSelection::Fixed(UiStyle::Studio),
    StyleSelection::Fixed(UiStyle::Atelier),
    StyleSelection::Fixed(UiStyle::Heavy),
    StyleSelection::Fixed(UiStyle::Ascii),
];

#[derive(Debug, Clone)]
pub(super) enum ThemeEntry {
    Valid {
        definition: ThemeDefinition,
        custom_path: Option<PathBuf>,
    },
    Invalid {
        id: String,
        error: String,
    },
}

impl ThemeEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Valid { definition, .. } => &definition.id,
            Self::Invalid { id, .. } => id,
        }
    }

    pub fn display_name(&self, i18n: &I18n) -> String {
        match self {
            Self::Valid { definition, .. } => match &definition.name {
                ThemeName::Builtin(key) => i18n.text(key).into(),
                ThemeName::Custom(name) => name.clone(),
            },
            Self::Invalid { id, .. } => id.clone(),
        }
    }

    pub fn valid(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }
}

#[derive(Debug, Clone)]
pub(super) struct ThemeDrawer {
    pub tab: DrawerTab,
    pub entries: Vec<ThemeEntry>,
    pub cursor: usize,
    pub style_cursor: usize,
    pub previous_theme: Theme,
    pub previous_style_selection: StyleSelection,
    pub previous_transparent: bool,
    pub previous_motion: bool,
    pub saved_id: Option<String>,
    pub saved_style: Option<String>,
    pub opened: Instant,
}

impl ThemeDrawer {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        themes_dir: &Path,
        current: &Theme,
        style_selection: StyleSelection,
        saved_id: Option<String>,
        saved_style: Option<String>,
        transparent_bg: bool,
        reduced_motion: bool,
        now: Instant,
    ) -> Self {
        let mut entries: Vec<ThemeEntry> = builtin::all()
            .into_iter()
            .map(|definition| ThemeEntry::Valid {
                definition,
                custom_path: None,
            })
            .collect();
        for custom in file::load_dir(themes_dir) {
            if custom.id.is_empty() {
                continue;
            }
            match custom.result {
                Ok(definition) => entries.push(ThemeEntry::Valid {
                    definition,
                    custom_path: Some(custom.path),
                }),
                Err(err) => entries.push(ThemeEntry::Invalid {
                    id: custom.id,
                    error: err.to_string(),
                }),
            }
        }
        let cursor = entries
            .iter()
            .position(|entry| entry.id() == current.id && entry.valid())
            .unwrap_or(0);
        let style_cursor = STYLE_CHOICES
            .iter()
            .position(|choice| *choice == style_selection)
            .unwrap_or(0);
        Self {
            tab: DrawerTab::Palette,
            entries,
            cursor,
            style_cursor,
            previous_theme: current.clone(),
            previous_style_selection: style_selection,
            previous_transparent: transparent_bg,
            previous_motion: reduced_motion,
            saved_id,
            saved_style,
            opened: now,
        }
    }

    pub fn active_count(&self) -> usize {
        match self.tab {
            DrawerTab::Palette => self.entries.len(),
            DrawerTab::Style => STYLE_CHOICES.len(),
        }
    }

    pub fn active_cursor(&self) -> usize {
        match self.tab {
            DrawerTab::Palette => self.cursor,
            DrawerTab::Style => self.style_cursor,
        }
    }

    pub fn selected(&self) -> &ThemeEntry {
        &self.entries[self.cursor]
    }

    pub fn selected_style(&self) -> StyleSelection {
        STYLE_CHOICES[self.style_cursor]
    }

    pub fn resolve_selected(&self, background: &Background, transparent_bg: bool) -> Option<Theme> {
        match self.selected() {
            ThemeEntry::Valid { definition, .. } => Some(theme::resolve(
                definition,
                self.selected_style(),
                background,
                transparent_bg,
            )),
            ThemeEntry::Invalid { .. } => None,
        }
    }
}

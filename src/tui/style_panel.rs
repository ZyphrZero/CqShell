use crate::theme::{self, Theme, ThemeDefinition, ThemeName, ThemeState, file::CustomTheme};

pub(super) struct Entry {
    pub id: String,
    pub definition: Result<usize, String>,
    pub builtin: bool,
}

impl Entry {
    pub fn definition<'a>(
        &'a self,
        definitions: &'a [ThemeDefinition],
    ) -> Result<&'a ThemeDefinition, &'a str> {
        self.definition
            .as_ref()
            .map(|index| &definitions[*index])
            .map_err(String::as_str)
    }
}

pub(super) struct StylePanel {
    pub cursor: usize,
    pub entries: Vec<Entry>,
    pub previous: Theme,
}

impl StylePanel {
    pub fn new(
        previous: Theme,
        definitions: &mut Vec<ThemeDefinition>,
        mut customs: Vec<CustomTheme>,
    ) -> Self {
        definitions.retain(|definition| matches!(definition.name, ThemeName::Builtin(_)));
        let mut entries = definitions
            .iter()
            .enumerate()
            .map(|(index, definition)| Entry {
                id: definition.id.clone(),
                definition: Ok(index),
                builtin: true,
            })
            .collect::<Vec<_>>();
        customs.sort_by(|left, right| left.id.cmp(&right.id));
        entries.extend(customs.into_iter().map(|custom| {
            Entry {
                id: custom.id,
                definition: custom
                    .result
                    .map(|definition| {
                        let index = definitions.len();
                        definitions.push(definition);
                        index
                    })
                    .map_err(|error| error.to_string()),
                builtin: false,
            }
        }));
        let cursor = entries
            .iter()
            .position(|entry| entry.id == previous.id)
            .unwrap_or(0);
        Self {
            cursor,
            entries,
            previous,
        }
    }
    pub fn rows(&self) -> Vec<Option<usize>> {
        let mut rows = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if !entry.builtin && (index == 0 || self.entries[index - 1].builtin) {
                rows.push(None);
            }
            rows.push(Some(index));
        }
        rows
    }
    pub fn preview(&self, state: &ThemeState) -> Option<Theme> {
        self.entries[self.cursor]
            .definition(&state.definitions)
            .ok()
            .map(|definition| theme::resolve(definition, &state.background))
    }
}

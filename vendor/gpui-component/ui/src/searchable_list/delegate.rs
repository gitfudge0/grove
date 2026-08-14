use gpui::{AnyElement, App, IntoElement, SharedString, Task, Window};

use crate::IndexPath;

use super::change::SearchableListChange;

pub trait SearchableListItem: Clone {
    type Value: Clone + PartialEq;

    fn title(&self) -> SharedString;

    /// `None` falls back to `title()`.
    fn display_title(&self) -> Option<AnyElement> {
        None
    }

    /// Default renders `title()`.
    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.title()
    }

    fn value(&self) -> &Self::Value;

    /// Defaults to case-insensitive substring match on `title()`.
    fn matches(&self, query: &str) -> bool {
        self.title().to_lowercase().contains(&query.to_lowercase())
    }

    fn disabled(&self) -> bool {
        false
    }
}

pub trait SearchableListDelegate: Sized + 'static {
    type Item: SearchableListItem;

    /// Defaults to 1.
    fn sections_count(&self, _: &App) -> usize {
        1
    }

    /// Deprecated: override [`render_section_header`] instead.
    #[deprecated]
    fn section(&self, _section: usize) -> Option<AnyElement> {
        None
    }

    fn items_count(&self, section: usize) -> usize;

    fn item(&self, ix: IndexPath) -> Option<&Self::Item>;

    fn position<V>(&self, _value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq;

    /// May return an async `Task` to filter or fetch items.
    fn perform_search(&mut self, _query: &str, _window: &mut Window, _cx: &mut App) -> Task<()> {
        Task::ready(())
    }

    /// `Some(_)` suppresses the adapter's default layout (including the trailing check icon) and renders as-is; `checked` reflects `is_item_checked`.
    fn render_item(
        &self,
        _ix: IndexPath,
        _item: &Self::Item,
        _checked: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }

    /// `Some(_)` bypasses the adapter's default div wrapper; `None` falls back to `section()`.
    fn render_section_header(
        &self,
        _section: usize,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }

    fn is_item_enabled(&self, _ix: IndexPath, item: &Self::Item, _cx: &App) -> bool {
        !item.disabled()
    }

    /// Default: checks whether the item's value is present in `current_selection`.
    fn is_item_checked(
        &self,
        _ix: IndexPath,
        item: &Self::Item,
        current_selection: &[(IndexPath, Self::Item)],
        _cx: &App,
    ) -> bool {
        current_selection
            .iter()
            .any(|(_, selected_item)| selected_item.value() == item.value())
    }

    /// May freely mutate `selection`; `changes` is informational. No `cx` — runs synchronously while the list is borrowed; side effects go in `on_confirm`.
    fn on_will_change(
        &mut self,
        selection: &mut Vec<(IndexPath, Self::Item)>,
        changes: &[SearchableListChange],
    ) {
        for change in changes {
            match change {
                SearchableListChange::Select { index } => {
                    let Some(item) = self.item(*index) else {
                        continue;
                    };

                    if !selection
                        .iter()
                        .any(|(_, selected_item)| selected_item.value() == item.value())
                    {
                        selection.push((*index, item.clone()));
                    }
                }
                SearchableListChange::Deselect { index } => {
                    if let Some(item) = self.item(*index) {
                        let has_value = selection
                            .iter()
                            .any(|(_, selected_item)| selected_item.value() == item.value());

                        if has_value {
                            selection
                                .retain(|(_, selected_item)| selected_item.value() != item.value());
                            continue;
                        }
                    }

                    selection.retain(|(selected_ix, _)| selected_ix != index);
                }
            }
        }
    }

    fn on_confirm(&mut self, _final_selection: &[(IndexPath, Self::Item)]) {}
}

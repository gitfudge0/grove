use gpui::{App, ElementId, Entity, FocusHandle, Global, OwnedMenu};
use std::collections::HashSet;

use crate::text::{SelectionScope, TextViewState};

pub(crate) fn init(cx: &mut App) {
    cx.set_global(GlobalState::new());
}

impl Global for GlobalState {}

pub struct GlobalState {
    pub(crate) text_view_state_stack: Vec<Entity<TextViewState>>,
    /// Non-empty means we're inside at least one deferred context; prevents double-deferred elements panicking GPUI.
    open_deferred_popovers: HashSet<ElementId>,
    app_menus: Vec<OwnedMenu>,
    /// Set by components owning their own mouse-down interaction; reset in the capture phase of every left mouse down.
    pub(crate) suppress_text_selection: bool,
    /// Pushed/popped by `SelectionScopeMarker` around each Dialog/Sheet subtree; empty means the base window layer.
    selection_scope_stack: Vec<SelectionScope>,
}

impl GlobalState {
    pub(crate) fn new() -> Self {
        Self {
            text_view_state_stack: Vec::new(),
            open_deferred_popovers: HashSet::new(),
            app_menus: Vec::new(),
            suppress_text_selection: false,
            selection_scope_stack: Vec::new(),
        }
    }

    /// Call from a mouse-down handler (bubble phase) of a component owning its own press/drag interaction.
    pub fn suppress_text_selection(cx: &mut App) {
        Self::global_mut(cx).suppress_text_selection = true;
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }

    pub(crate) fn text_view_state(&self) -> Option<&Entity<TextViewState>> {
        self.text_view_state_stack.last()
    }

    pub(crate) fn push_selection_scope(&mut self, scope: SelectionScope) {
        self.selection_scope_stack.push(scope);
    }

    pub(crate) fn pop_selection_scope(&mut self) {
        self.selection_scope_stack.pop();
    }

    /// `Base` when not inside any Dialog/Sheet content subtree.
    pub(crate) fn current_selection_scope(&self) -> SelectionScope {
        self.selection_scope_stack
            .last()
            .copied()
            .unwrap_or(SelectionScope::Base)
    }

    pub(crate) fn is_in_deferred_context(&self) -> bool {
        !self.open_deferred_popovers.is_empty()
    }

    pub(crate) fn register_deferred_popover(&mut self, focus_handle: &FocusHandle) {
        self.open_deferred_popovers
            .insert(format!("{focus_handle:?}").into());
    }

    pub(crate) fn unregister_deferred_popover(&mut self, focus_handle: &FocusHandle) {
        let element_id: ElementId = format!("{focus_handle:?}").into();
        self.open_deferred_popovers.remove(&element_id);
    }

    pub fn app_menus(&self) -> &[OwnedMenu] {
        &self.app_menus
    }

    pub fn set_app_menus(&mut self, menus: Vec<OwnedMenu>) {
        self.app_menus = menus;
    }
}

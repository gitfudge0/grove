use crate::{
    Placement, Root,
    dialog::{AlertDialog, Dialog},
    input::InputState,
    notification::Notification,
    sheet::Sheet,
};
use gpui::{App, ElementId, Entity, Window};
use std::rc::Rc;

pub trait WindowExt: Sized {
    fn open_sheet<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static;

    fn open_sheet_at<F>(&mut self, placement: Placement, cx: &mut App, build: F)
    where
        F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static;

    fn has_active_sheet(&mut self, cx: &mut App) -> bool;

    fn close_sheet(&mut self, cx: &mut App);

    fn open_dialog<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static;

    /// Convenience method with opinionated defaults: center-aligned footer buttons and a variant-based icon.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use gpui_component::{AlertDialog, alert::AlertVariant};
    ///
    /// window.open_alert_dialog(cx, |alert, _, _| {
    ///     alert.warning()
    ///         .title("Unsaved Changes")
    ///         .description("You have unsaved changes. Are you sure you want to leave?")
    ///         .show_cancel(true)
    /// });
    /// ```
    fn open_alert_dialog<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static;

    fn has_active_dialog(&mut self, cx: &mut App) -> bool;

    fn close_dialog(&mut self, cx: &mut App);

    fn close_all_dialogs(&mut self, cx: &mut App);

    fn push_notification(&mut self, note: impl Into<Notification>, cx: &mut App);

    /// Matches ids registered via either `Notification::id` or `Notification::id1` (any key).
    fn remove_notification<T: Sized + 'static>(&mut self, cx: &mut App);

    /// Paired with `Notification::id1`.
    fn remove_notification1<T: Sized + 'static>(&mut self, key: impl Into<ElementId>, cx: &mut App);

    fn clear_notifications(&mut self, cx: &mut App);

    fn notifications(&mut self, cx: &mut App) -> Rc<Vec<Entity<Notification>>>;

    fn focused_input(&mut self, cx: &mut App) -> Option<Entity<InputState>>;
    fn has_focused_input(&mut self, cx: &mut App) -> bool;

    /// Merges all selectable TextViews' selections, top to bottom, joined with `\n`; empty if the window root isn't a [`Root`].
    fn selected_text(&mut self, cx: &mut App) -> String;

    /// True for either a window-level drag selection or a view-local one (select-all, double-click word).
    fn has_text_selection(&mut self, cx: &mut App) -> bool;

    fn clear_text_selection(&mut self, cx: &mut App);

    fn end_text_selection(&mut self, cx: &mut App);
}

impl WindowExt for Window {
    #[inline]
    fn open_sheet<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static,
    {
        self.open_sheet_at(Placement::Right, cx, build)
    }

    #[inline]
    fn open_sheet_at<F>(&mut self, placement: Placement, cx: &mut App, build: F)
    where
        F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static,
    {
        Root::update(self, cx, move |root, window, cx| {
            root.open_sheet_at(placement, build, window, cx);
        })
    }

    #[inline]
    fn has_active_sheet(&mut self, cx: &mut App) -> bool {
        Root::read(self, cx).active_sheet.is_some()
    }

    #[inline]
    fn close_sheet(&mut self, cx: &mut App) {
        Root::update(self, cx, |root, window, cx| {
            root.close_sheet(window, cx);
        })
    }

    #[inline]
    fn open_dialog<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static,
    {
        Root::update(self, cx, move |root, window, cx| {
            root.open_dialog(build, window, cx);
        })
    }

    #[inline]
    fn open_alert_dialog<F>(&mut self, cx: &mut App, build: F)
    where
        F: Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static,
    {
        self.open_dialog(cx, move |_, window, cx| {
            build(AlertDialog::new(cx), window, cx).into_dialog(window, cx)
        })
    }

    #[inline]
    fn has_active_dialog(&mut self, cx: &mut App) -> bool {
        Root::read(self, cx).active_dialogs.len() > 0
    }

    #[inline]
    fn close_dialog(&mut self, cx: &mut App) {
        Root::update(self, cx, |root, window, cx| {
            root.close_dialog(window, cx);
        })
    }

    #[inline]
    fn close_all_dialogs(&mut self, cx: &mut App) {
        Root::update(self, cx, |root, window, cx| {
            root.close_all_dialogs(window, cx);
        })
    }

    #[inline]
    fn push_notification(&mut self, note: impl Into<Notification>, cx: &mut App) {
        let note = note.into();
        Root::update(self, cx, |root, window, cx| {
            root.push_notification(note, window, cx);
        })
    }

    #[inline]
    fn remove_notification<T: Sized + 'static>(&mut self, cx: &mut App) {
        Root::update(self, cx, |root, window, cx| {
            root.remove_notification::<T>(window, cx);
        })
    }

    #[inline]
    fn remove_notification1<T: Sized + 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
    ) {
        let key = key.into();
        Root::update(self, cx, |root, window, cx| {
            root.remove_notification1::<T>(key, window, cx);
        })
    }

    #[inline]
    fn clear_notifications(&mut self, cx: &mut App) {
        Root::update(self, cx, |root, window, cx| {
            root.clear_notifications(window, cx);
        })
    }

    #[inline]
    fn notifications(&mut self, cx: &mut App) -> Rc<Vec<Entity<Notification>>> {
        Rc::new(Root::read(self, cx).notification.read(cx).notifications())
    }

    #[inline]
    fn has_focused_input(&mut self, cx: &mut App) -> bool {
        Root::read(self, cx).focused_input.is_some()
    }

    #[inline]
    fn focused_input(&mut self, cx: &mut App) -> Option<Entity<InputState>> {
        Root::read(self, cx).focused_input.clone()
    }

    #[inline]
    fn selected_text(&mut self, cx: &mut App) -> String {
        let Some(root) = self.root::<Root>().flatten() else {
            return String::new();
        };
        root.read(cx).window_selected_text(cx)
    }

    #[inline]
    fn has_text_selection(&mut self, cx: &mut App) -> bool {
        let Some(root) = self.root::<Root>().flatten() else {
            return false;
        };
        root.read(cx).has_text_selection(cx)
    }

    #[inline]
    fn clear_text_selection(&mut self, cx: &mut App) {
        let Some(root) = self.root::<Root>().flatten() else {
            return;
        };
        root.update(cx, |root, cx| root.clear_text_selection(cx));
    }

    #[inline]
    fn end_text_selection(&mut self, cx: &mut App) {
        let Some(root) = self.root::<Root>().flatten() else {
            return;
        };
        root.update(cx, |root, cx| root.end_text_selection(cx));
    }
}

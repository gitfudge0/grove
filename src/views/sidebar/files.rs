//! Files occupies the existing right-panel destination; content occupies the canvas.
use super::*;
use crate::views::file_browser::{FileBrowser, EXPLORER_WIDTH};

impl Sidebar {
    pub(super) fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.files_active && self.files_owner == self.selection;
        self.files_active = !active;
        self.files_owner = self.selection.clone();
        if self.files_active {
            self.checkout_panel.side_chat_owner = None;
            self.checkout_panel.hidden = false;
            self.dismiss_agent_peek(window, cx);
            self.dismiss_plan_peek(window, cx);
            self.dismiss_script_peek(window, cx);
        }
        cx.notify();
    }
    pub(super) fn files_control(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.files_active && self.files_owner == self.selection;
        let focus = self
            .files_toggle_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let available = !self.checkout_targets(cx).is_empty();
        if !available {
            return crate::views::components::disabled_header_control(
                "files-toggle",
                "Choose a checkout to browse files",
            )
            .child(icon("folder", ICON_LG, c::FG_DIM()))
            .into_any_element();
        }
        crate::views::components::header_control_with_label_tooltip(
            "files-toggle",
            if active { "Hide files" } else { "Show files" },
        )
        .debug_selector(|| "files-toggle".into())
        .track_focus(&focus)
        .tab_index(0)
        .aria_expanded(active)
        .focus_visible(|s| s.bg(c::BG_HOVER()))
        .when(active, |s| s.bg(c::BG_HOVER()))
        .child(icon("folder", ICON_LG, c::FG_DIM()))
        .on_click(cx.listener(|this, _, window, cx| this.toggle_files(window, cx)))
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") && !event.is_held {
                this.toggle_files(window, cx);
                window.prevent_default();
                cx.stop_propagation();
            }
        }))
        .into_any_element()
    }
    pub(super) fn render_files_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Construct/retain the native transcript first. It owns its browser and draft.
        let mut body = self.render_content_body(window, cx);
        let browser = if let Some(Selection::Session(id) | Selection::Home(id)) = self.selection {
            self.codex_views
                .get(&id)
                .map(|view| view.read(cx).file_browser.clone())
        } else {
            None
        };
        let browser = if let Some(browser) = browser {
            browser
        } else {
            let root = super::checkout_panel::selected_checkout(
                &self.checkout_targets(cx),
                self.checkout_panel.chosen_root.as_deref(),
            )
            .map(|(_, path)| path);
            let Some(root) = root else {
                return body;
            };
            let stale = self
                .standalone_files
                .as_ref()
                .is_none_or(|(owner, path, _)| owner != &self.selection || path != &root);
            if stale {
                let browser = cx.new(|cx| FileBrowser::new(root.clone().into(), window, cx));
                self.observers
                    .push(cx.observe(&browser, |_, _, cx| cx.notify()));
                self.standalone_files = Some((self.selection.clone(), root, browser));
            }
            let browser = self
                .standalone_files
                .as_ref()
                .map(|(_, _, browser)| browser.clone());
            let Some(browser) = browser else {
                return body;
            };
            if browser.read(cx).visible {
                body = div()
                    .size_full()
                    .min_h_0()
                    .child(browser.clone())
                    .into_any_element();
            }
            browser
        };
        let explorer = browser.update(cx, |browser, cx| browser.explorer(window, cx));
        div()
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .child(div().flex_1().min_w_0().min_h_0().child(body))
            .child(
                div()
                    .w(rpx(EXPLORER_WIDTH))
                    .max_w(gpui::relative(0.4))
                    .h_full()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(c::BORDER())
                    .child(explorer),
            )
            .into_any_element()
    }
}

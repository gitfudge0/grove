//! Settings shortcut reference and its navigation entry point.

use super::{c, handled_shortcuts, keymap, rpx, Page, Scope, SettingsPanel, SHORTCUT_KEY_W};
use crate::views::tokens::{
    FIELD_LABEL_COL_W, SPACE_LG, SPACE_SM, TEXT_BODY, TEXT_MICRO, TEXT_SMALL,
};
use gpui::prelude::{InteractiveElement, ParentElement, Styled};
use gpui::{div, Context, Window};

impl SettingsPanel {
    pub fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_operation_pending(cx) {
            return;
        }
        self.open(window, cx);
        self.select_category(self.category, cx);
        self.standalone_shortcuts = true;
        self.show(Page::Shortcuts, cx);
    }

    pub(super) fn shortcuts_body(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut body = div().flex().flex_col().gap(rpx(SPACE_SM));
        for (index, shortcut) in handled_shortcuts() {
            let scopes = shortcut
                .scopes
                .iter()
                .map(|scope| match scope {
                    Scope::Global => "Global".to_string(),
                    Scope::Screen(screen) => screen.label().to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let chord = if shortcut.literal {
                shortcut.display_keys.to_string()
            } else if shortcut.requires_alt {
                format!(
                    "{}{}",
                    keymap::alt_chord_prefix().replace('-', "+"),
                    shortcut.display_keys
                )
            } else {
                format!("{}+{}", keymap::platform_mod_label(), shortcut.display_keys)
            };
            body = body.child(
                div()
                    .id(format!("settings-shortcut-{index}"))
                    .flex()
                    .flex_wrap()
                    .gap(rpx(SPACE_LG))
                    .py(rpx(SPACE_SM))
                    .child(
                        div()
                            .min_w(rpx(SHORTCUT_KEY_W))
                            .font_family(crate::fonts::UI_FAMILY)
                            .text_size(rpx(TEXT_SMALL))
                            .text_color(c::FG_DIM())
                            .child(super::super::components::shortcut_hint(chord)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(rpx(FIELD_LABEL_COL_W))
                            .text_size(rpx(TEXT_BODY))
                            .text_color(c::FG())
                            .child(shortcut.description),
                    )
                    .child(
                        div()
                            .text_size(rpx(TEXT_MICRO))
                            .text_color(c::FG_DIM())
                            .child(scopes),
                    ),
            );
        }
        let _ = cx;
        body.child(Self::section("GROVE’S CONVERSATION VIEW"))
            .child(Self::setting_row(
                "Enter",
                "Send when idle; queue input while an agent turn is running.",
                div(),
            ))
            .child(Self::setting_row(
                super::super::components::shortcut_hint("Shift+Enter"),
                "Insert a new line.",
                div(),
            ))
            .child(Self::setting_row(
                "Steer current turn",
                "Codex and Claude Code, while an agent turn is running.",
                div()
                    .text_size(rpx(TEXT_SMALL))
                    .child(super::super::components::shortcut_hint(format!(
                        "{}+Enter",
                        keymap::platform_mod_label()
                    ))),
            ))
    }
}

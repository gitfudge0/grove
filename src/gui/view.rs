//! `Grove::view` and the chrome it composes (appbar, sidebar, workspace,
//! statusbar, modal layer). Pure rendering — no state mutation.

use super::icons::icon;
use super::metrics::{
    APPBAR_H, CELL_H, CELL_W, MONO_FONT, ROW_H, SESSBAR_H, SIDEBAR_DIVIDER_W, STATUS_H, UI_BOLD,
    UI_FONT,
};
use super::palette as c;
use super::pty::{rebuild_row_runs, PtyProgram};
use super::rows::{project_row, session_row, single_line, state_glyph, worktree_row};
use super::state::{FocusedPane, Grove, Msg, PtyCacheEntry, PtyCell, PtyPane, UpgradeState};
use super::update::{
    platform_mod_label, project_theme_pane_rows, theme_pane_rows, update_available_actions,
    GlobalShortcut, PaletteRow, Scope, SettingRow, ShortcutDef, UpdateAction, SHORTCUTS,
};
use super::widgets::{
    control_btn_sized, control_icon_btn, divider_h, divider_v, dot, empty_terminals_workspace,
    empty_workspace, footer_container, footer_hint, ghost_scrollable, icon_btn, keycap,
    keycap_text, launcher_row, modal_action, modal_action_sized, modal_checkbox,
    modal_footer_hints, modal_footer_row, modal_header, modal_header_row, modal_list_row,
    modal_list_row_sized, modal_panel, palette_input_style, section_header, seg_button,
    sidebar_agent_menu_overlay, skip_perms_seg, slot_badge, tool_btn, tool_btn_toggle, vline,
    ModalBtn, SegSide, PALETTE_ROW_H,
};
use crate::app::{
    AddProjectStep, ConfirmKind, GitProbe, LauncherOptions, LauncherSettings, Modal, OnboardStep,
    SettingsPane,
};
use crate::git::Worktree;
use crate::session::{Session, SessionStatus};
use iced::border::Radius;
use iced::widget::{
    button, canvas as canvas_widget, column, container, rich_text, row, scrollable, span, stack,
    text, text_input, Column, Id, Row, Space,
};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow, Vector};
use std::sync::atomic::Ordering;

/// Stable id for the add-project / add-worktree primary text input, used to
/// focus it from `update` when the modal opens.
pub fn modal_input_id() -> Id {
    Id::new("modal-input-primary")
}

/// Stable id for the add-project details-step name field, used to focus it
/// when the modal advances to step 2.
pub fn modal_name_id() -> Id {
    Id::new("modal-input-name")
}

/// Shared `text_input` styling for modal fields: strip background, themed
/// border, cyan caret/selection. Focus brightens the border.
fn input_field_style(_t: &iced::Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(c::BG_STRIP()),
        border: Border {
            color: if focused { c::MAGENTA() } else { c::BORDER() },
            width: 1.0,
            radius: Radius::from(4.0),
        },
        icon: c::FG_MUTE(),
        placeholder: c::FG_MUTE(),
        value: c::FG(),
        selection: c::CYAN(),
    }
}

/// Stable id for the theme-picker scrollable, used to scroll the active
/// selection into view from `update`.
pub fn theme_picker_scrollable_id() -> Id {
    Id::new("theme-picker-list")
}

/// Stable id for the palette Theme sub-pane's list scrollable — same idiom
/// as [`theme_picker_scrollable_id`], for the same reason: `themes_of` is
/// alphabetical, so the current theme usually sits below the pane's 280px
/// fold and must be scrolled into view from `update`.
pub fn launcher_theme_scrollable_id() -> Id {
    Id::new("launcher-theme-list")
}

/// Stable id for the palette Settings drill-in's Root list scrollable —
/// same idiom again: 8 rows plus section headers overflow the 380px cap, so
/// cursor moves (and sub-pane exits landing near the bottom) must scroll
/// the selection into view from `update`.
pub fn launcher_settings_scrollable_id() -> Id {
    Id::new("launcher-settings-list")
}

/// A mod+key hint chip: on macOS the modifier renders as the ⌘ glyph icon,
/// elsewhere as `platform_mod_label()`. Used for the palette's ⌘T action-row
/// chip (`color` = `FG_DIM`) and its ⌘1…⌘N recent-row digit chips (`color` =
/// `FG_MUTE`, a quieter shade so they recede behind the row text).
fn mod_key_chip<'a>(key: &'static str, color: Color) -> Element<'a, Msg> {
    let inner: Element<'a, Msg> = if cfg!(target_os = "macos") {
        row![
            icon("command", 10.0, color),
            text(key).font(MONO_FONT).size(11).color(color),
        ]
        .spacing(1)
        .align_y(iced::Alignment::Center)
        .into()
    } else {
        text(format!("{}+{}", platform_mod_label(), key))
            .font(MONO_FONT)
            .size(11)
            .color(color)
            .into()
    };
    keycap(inner)
}

/// Render `s` as rich text, coloring the character ranges in `ranges` cyan
/// (the typing-state fuzzy-match highlight) and everything else
/// `base_color`. `ranges` are **char** indices from
/// `launcher::fuzzy_match_indices`, not byte offsets. Falls back to a plain
/// `text` widget when there's nothing to highlight.
fn highlighted_line<'a>(
    s: &str,
    ranges: &[(usize, usize)],
    base_color: Color,
    font: iced::Font,
    size: f32,
) -> Element<'a, Msg> {
    if ranges.is_empty() {
        return text(s.to_string())
            .font(font)
            .size(size)
            .color(base_color)
            .into();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut sorted: Vec<(usize, usize)> = ranges.to_vec();
    sorted.sort_by_key(|r| r.0);
    let mut spans: Vec<iced::widget::text::Span<'static>> = Vec::new();
    let mut cursor = 0usize;
    for (start, end) in sorted {
        let start = start.min(chars.len());
        let end = end.min(chars.len()).max(start);
        if start > cursor {
            spans.push(span(chars[cursor..start].iter().collect::<String>()).color(base_color));
        }
        if end > start {
            spans.push(span(chars[start..end].iter().collect::<String>()).color(c::CYAN()));
        }
        cursor = cursor.max(end);
    }
    if cursor < chars.len() {
        spans.push(span(chars[cursor..].iter().collect::<String>()).color(base_color));
    }
    rich_text(spans).font(font).size(size).into()
}

/// The ⌘-digit key bound to root-mode recent-row `i` (0-based), if any.
/// `update.rs`'s mod+digit handler accepts any digit 1-9, but `palette_rows`
/// caps recents at 6 (`.take(6)`), so only the first 6 rows ever get a real
/// binding — this is why the palette shows at most ⌘1…⌘6, not ⌘1…⌘9.
fn digit_label(i: usize) -> Option<&'static str> {
    ["1", "2", "3", "4", "5", "6"].get(i).copied()
}

use std::sync::Arc;

fn session_context_title(s: &Session) -> Option<String> {
    let raw = s.current_title()?;
    if raw.eq_ignore_ascii_case(&s.label) || raw.eq_ignore_ascii_case(s.agent.label()) {
        return None;
    }
    // OSC titles often start with emoji or box-drawing characters that the UI
    // font (IBM Plex Sans) can't render — strip them so the sess_bar never
    // shows a tofu box. The sidebar applies the same filter.
    super::rows::sanitize_ui_text(&raw)
}

fn is_in_progress_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("in progress") || lower.contains("in-progress") || lower.contains("in_progress")
}

impl Grove {
    /// Whether any home terminal has a live process running — the signal
    /// behind the "TERMINALS" header's collapsed-state activity dot. Shared
    /// by the docked (collapsed) and inline (expanded, forced `false` — see
    /// call site) header renders so the scan isn't duplicated.
    fn home_terminals_activity(&self) -> bool {
        self.app.home_terminals.iter().any(|s| {
            matches!(
                *s.status.lock().unwrap_or_else(|e| e.into_inner()),
                SessionStatus::Running
            )
        })
    }

    pub fn view(&self) -> Element<'_, Msg> {
        // The first-run wizard owns the entire window while active: no
        // sidebar/statusbar/scrim behind it, just its own full-viewport chrome.
        // It still goes through the shared background wrapper below, but
        // skips `body`/the modal layer entirely.
        if let Modal::Onboarding {
            step,
            path,
            dir_sel,
            name,
            note,
            agent_sel,
            perms_skip,
            ..
        } = &self.app.modal
        {
            let content = self.onboarding_view(
                *step,
                path,
                *dir_sel,
                name.as_deref(),
                note.as_deref(),
                *agent_sel,
                *perms_skip,
            );
            return container(content)
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    text_color: Some(c::FG()),
                    ..Default::default()
                })
                .into();
        }
        let body = if self.app.chrome_visible {
            let workspace_row: Element<'_, Msg> = if self.grid_view {
                // Grid mode: sidebar is hidden, workspace fills the full width.
                self.workspace()
            } else {
                row![
                    self.sidebar(),
                    self.sidebar_resize_handle(),
                    self.workspace()
                ]
                .height(Length::Fill)
                .width(Length::Fill)
                .into()
            };
            column![
                self.appbar(),
                container(workspace_row)
                    .height(Length::Fill)
                    .width(Length::Fill),
                self.statusbar(),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            let waiting = self.waiting_sessions();
            let workspace: Element<'_, Msg> = if waiting.is_empty() {
                self.workspace()
            } else {
                stack![self.workspace(), self.zen_attention_pill(waiting.len())]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            };
            column![workspace].width(Length::Fill).height(Length::Fill)
        };

        let show_attention_dropdown = self.attention_open && self.app.chrome_visible;
        let content: Element<'_, Msg> = if matches!(self.app.modal, Modal::None)
            && !self.show_changelog
            && !show_attention_dropdown
        {
            body.into()
        } else {
            let mut layers = stack![body];
            if !matches!(self.app.modal, Modal::None) {
                layers = layers.push(self.modal_layer());
            }
            if self.show_changelog {
                layers = layers.push(self.changelog_modal());
            }
            if show_attention_dropdown {
                layers = layers.push(self.attention_dropdown());
            }
            layers.width(Length::Fill).height(Length::Fill).into()
        };

        container(content)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                text_color: Some(c::FG()),
                ..Default::default()
            })
            .into()
    }

    // ── appbar ────────────────────────────────────────────────────────────
    fn appbar(&self) -> Element<'_, Msg> {
        let waiting = self.waiting_sessions();
        let brand = row![text("grove").font(UI_BOLD).size(14).color(c::MAGENTA()),]
            .spacing(8)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        // App size, theme, and terminal backend now live in the Settings modal;
        // the appbar's right cluster is just the cog entry point.
        let cog = icon_btn("cog", Msg::OpenSettings);
        let cog: Element<'_, Msg> = if matches!(self.upgrade, UpgradeState::Available(_)) {
            stack![
                cog,
                container(dot(c::GREEN()))
                    .align_x(iced::alignment::Horizontal::Right)
                    .align_y(iced::alignment::Vertical::Top)
                    .width(Length::Fill)
                    .height(Length::Fill),
            ]
            .into()
        } else {
            cog
        };
        // Agent-view toggle. In agent (grid) view it grows a "+" session-launcher
        // segment on its left, forming a single segmented combo; on every other
        // screen it is a lone muted button. The combo replaces the floating "+"
        // FAB that used to hover over the grid.
        let view_control: Element<'_, Msg> = if self.grid_view {
            let plus = button(
                container(icon("plus", 13.0, c::MAGENTA()))
                    .center_x(26)
                    .center_y(22),
            )
            .on_press(Msg::OpenSessionLauncher)
            .padding(0)
            .style(|_, status| button::Style {
                background: if matches!(status, button::Status::Hovered) {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: c::MAGENTA(),
                // Round the left corners only — the right edge butts the grid seg.
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius {
                        top_left: 4.0,
                        top_right: 0.0,
                        bottom_right: 0.0,
                        bottom_left: 4.0,
                    },
                },
                shadow: Shadow::default(),
                snap: false,
            });
            let grid_seg = button(
                container(icon("grid", 13.0, c::CYAN()))
                    .center_x(26)
                    .center_y(22),
            )
            .on_press(Msg::ToggleGridView)
            .padding(0)
            .style(|_, status| button::Style {
                background: Some(Background::Color(
                    if matches!(status, button::Status::Hovered) {
                        c::BG_HOVER()
                    } else {
                        c::BG_HL()
                    },
                )),
                text_color: c::CYAN(),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius {
                        top_left: 0.0,
                        top_right: 4.0,
                        bottom_right: 4.0,
                        bottom_left: 0.0,
                    },
                },
                shadow: Shadow::default(),
                snap: false,
            });
            // A short, fixed-height hairline between the segments. Using a
            // Fill-height divider here would inherit the appbar's full height and
            // stretch the combo taller than the lone toggle button.
            let seg_divider = container(Space::new().width(1))
                .width(1)
                .height(Length::Fixed(14.0))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BORDER())),
                    ..Default::default()
                });
            container(row![plus, seg_divider, grid_seg].align_y(iced::Alignment::Center))
                .style(|_| container::Style {
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(5.0),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            let grid_color = c::FG_MUTE();
            button(
                container(icon("grid", 13.0, grid_color))
                    .center_x(22)
                    .center_y(22),
            )
            .on_press(Msg::ToggleGridView)
            .padding(0)
            .style(move |_, status| button::Style {
                background: if matches!(status, button::Status::Hovered) {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: grid_color,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
                snap: false,
            })
            .into()
        };

        // Attention-queue pill: only rendered while at least one session is
        // waiting for input. Pulses in sync with the grid tile's amber accent
        // (see `attention_pulse`) and toggles the dropdown on click.
        let attention_pill: Option<Element<'_, Msg>> = if waiting.is_empty() {
            None
        } else {
            let dot_alpha = 1.0 - 0.4 * self.attention_pulse();
            let dot_color = Color {
                a: dot_alpha,
                ..c::AMBER()
            };
            let label = if waiting.len() == 1 {
                "1 needs you".to_string()
            } else {
                format!("{} need you", waiting.len())
            };
            let content = row![
                dot(dot_color),
                text(label).font(UI_FONT).size(11).color(c::AMBER()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            Some(
                button(
                    container(content)
                        .padding(Padding::from([4, 10]))
                        .style(|_| container::Style {
                            background: Some(Background::Color(Color {
                                a: 0.08,
                                ..c::AMBER()
                            })),
                            border: Border {
                                color: c::AMBER(),
                                width: 1.0,
                                radius: Radius::from(999.0),
                            },
                            ..Default::default()
                        }),
                )
                .on_press(Msg::ToggleAttentionQueue)
                .padding(0)
                .style(|_, status| button::Style {
                    background: if matches!(status, button::Status::Hovered) {
                        Some(Background::Color(Color {
                            a: 0.14,
                            ..c::AMBER()
                        }))
                    } else {
                        None
                    },
                    text_color: c::AMBER(),
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: false,
                })
                .into(),
            )
        };

        let mut right = row![view_control];
        if let Some(pill) = attention_pill {
            right = right.push(pill);
        }
        let right = right
            .push(cog)
            .spacing(4)
            .padding(Padding::from([0, 16]))
            .align_y(iced::Alignment::Center);

        let inner = row![
            container(brand).width(self.sidebar_width),
            Space::new().width(Length::Fill),
            right,
        ]
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        let bar = container(inner)
            .height(APPBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![bar, divider_h(c::BORDER())].into()
    }

    /// Small floating badge shown top-right over the zen workspace while at
    /// least one session waits for input — chrome (and thus the appbar pill)
    /// is hidden in zen, so this is the only always-visible attention signal
    /// there. Clicking it jumps straight to the first waiting session; it is
    /// not a dropdown, so no backdrop/dismiss handling is needed.
    fn zen_attention_pill(&self, count: usize) -> Element<'_, Msg> {
        let dot_alpha = 1.0 - 0.4 * self.attention_pulse();
        let dot_color = Color {
            a: dot_alpha,
            ..c::AMBER()
        };
        let content = row![
            dot(dot_color),
            text(count.to_string())
                .font(UI_FONT)
                .size(11)
                .color(c::AMBER()),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);

        let pill = button(
            container(content)
                .padding(Padding::from([2, 8]))
                .style(|_| container::Style {
                    background: Some(Background::Color(Color {
                        a: 0.08,
                        ..c::AMBER()
                    })),
                    border: Border {
                        color: c::AMBER(),
                        width: 1.0,
                        radius: Radius::from(999.0),
                    },
                    ..Default::default()
                }),
        )
        .on_press(Msg::JumpToWaitingSession)
        .padding(0)
        .style(|_, status| button::Style {
            background: if matches!(status, button::Status::Hovered) {
                Some(Background::Color(Color {
                    a: 0.14,
                    ..c::AMBER()
                }))
            } else {
                None
            },
            text_color: c::AMBER(),
            border: Border::default(),
            shadow: Shadow::default(),
            snap: false,
        });

        column![
            Space::new().height(12.0),
            row![Space::new().width(Length::Fill), pill].padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 12.0,
            }),
            Space::new().height(Length::Fill),
        ]
        .height(Length::Fill)
        .into()
    }

    /// Anchored top-right dropdown listing every session currently waiting
    /// for input, opened via the appbar pill (`Msg::ToggleAttentionQueue`).
    /// Same backdrop-dismiss idiom as `sidebar_agent_menu_overlay`.
    fn attention_dropdown(&self) -> Element<'_, Msg> {
        let waiting = self.waiting_sessions();

        let backdrop = button(Space::new().width(Length::Fill).height(Length::Fill))
            .on_press(Msg::CloseAttentionQueue)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(0)
            .style(|_, _| button::Style {
                background: None,
                text_color: Color::TRANSPARENT,
                border: Border::default(),
                shadow: Shadow::default(),
                snap: false,
            });

        let mut rows_col = column![].spacing(0);
        for &si in &waiting {
            let s = &self.app.sessions[si];
            let state = self.activity_state(s);
            let subtitle = format!("{} / {}", s.project, crate::app::path_basename(&s.wt_path));
            let content = row![
                state_glyph(state, self.blink_tick, self.attention_pulse()),
                column![
                    text(s.agent.label()).font(UI_FONT).size(11).color(c::FG()),
                    text(subtitle).font(MONO_FONT).size(10).color(c::FG_MUTE()),
                ]
                .spacing(1),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center)
            .padding(Padding {
                top: 6.0,
                bottom: 6.0,
                left: 12.0,
                right: 10.0,
            });

            let row_btn = button(content)
                .on_press(Msg::SelectSession(si))
                .width(Length::Fill)
                .padding(0)
                .style(|_, status| button::Style {
                    background: if matches!(status, button::Status::Hovered) {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        None
                    },
                    text_color: c::FG(),
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: false,
                });

            // 3px amber left accent bar, same idiom as the waiting sidebar row.
            let bar: Element<'_, Msg> = container(
                container(Space::new().width(3.0))
                    .width(3.0)
                    .height(Length::Fill)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::AMBER())),
                        ..Default::default()
                    }),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Start)
            .into();

            rows_col = rows_col.push(stack![row_btn, bar]);
        }

        let footer_hint: Element<'_, Msg> = if cfg!(target_os = "macos") {
            row![
                icon("command", 10.0, c::FG_MUTE()),
                text("'").font(MONO_FONT).size(10).color(c::FG_MUTE()),
                text(" jump to next")
                    .font(UI_FONT)
                    .size(10)
                    .color(c::FG_MUTE()),
            ]
            .spacing(1)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            text(format!("{}+' jump to next", platform_mod_label()))
                .font(UI_FONT)
                .size(10)
                .color(c::FG_MUTE())
                .into()
        };
        let footer = container(footer_hint).width(Length::Fill).padding(Padding {
            top: 6.0,
            bottom: 6.0,
            left: 12.0,
            right: 10.0,
        });

        let panel = container(
            column![rows_col, divider_h(c::BORDER()), footer]
                .spacing(0)
                .width(Length::Fixed(280.0)),
        )
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP())),
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });

        let positioned = column![
            Space::new().height(APPBAR_H + 1.0),
            row![Space::new().width(Length::Fill), panel].padding(Padding {
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 16.0,
            }),
            Space::new().height(Length::Fill),
        ]
        .height(Length::Fill);

        stack![backdrop, positioned]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// The draggable divider between the sidebar and the workspace. A 1px line
    /// centered in a `SIDEBAR_DIVIDER_W`-wide hit zone, with a resize cursor on
    /// hover. The press starts a drag; cursor moves and the release are tracked
    /// by a global subscription (see `Grove::subscription`).
    fn sidebar_resize_handle(&self) -> Element<'_, Msg> {
        iced::widget::mouse_area(
            container(divider_v(c::BORDER()))
                .height(Length::Fill)
                .center_x(SIDEBAR_DIVIDER_W),
        )
        .on_press(Msg::SidebarDragStart)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
    }

    // ── sidebar ───────────────────────────────────────────────────────────
    fn sidebar(&self) -> Element<'_, Msg> {
        let tree_head = self.tree_head();
        let content: Element<'_, Msg> = self.tree_view();
        let tree_area = container(ghost_scrollable(content).height(Length::Fill))
            .height(Length::Fill)
            .padding(Padding {
                top: 8.0,
                bottom: 12.0,
                left: 0.0,
                right: 0.0,
            });
        let agent_menu_top = self.open_agent_menu_top();
        let tree_layer: Element<'_, Msg> = match agent_menu_top {
            Some((proj, wt, top, is_main)) => stack![
                tree_area,
                sidebar_agent_menu_overlay(proj, wt, top, is_main, &self.app.available_agents),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            None => tree_area.into(),
        };

        // When the TERMINALS section is collapsed, its header+rows no longer
        // render inside the scrollable tree — dock a standalone copy of the
        // header at the very bottom instead so it's always reachable.
        let docked_terminals: Option<Element<'_, Msg>> = if self.terminals_collapsed {
            Some(crate::gui::rows::home_terminals_header(
                false,
                self.app.home_terminals.len(),
                self.home_terminals_activity(),
            ))
        } else {
            None
        };

        let mut stack_col =
            column![tree_head, divider_h(c::BORDER_SOFT()), tree_layer,].height(Length::Fill);
        if let Some(docked) = docked_terminals {
            stack_col = stack_col.push(divider_h(c::BORDER_SOFT()));
            stack_col = stack_col.push(docked);
        }

        container(stack_col)
            .width(self.sidebar_width)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_RAIL())),
                ..Default::default()
            })
            .into()
    }

    fn tree_head(&self) -> Element<'_, Msg> {
        // Glyph shows the *next* action the cycle button will take.
        let glyph = match self.tree_expand.next() {
            crate::gui::state::TreeExpand::SessionsOnly => "expand-sessions",
            crate::gui::state::TreeExpand::All => "expand-all",
            crate::gui::state::TreeExpand::Collapsed => "collapse-all",
        };
        let toggle = button(
            container(icon(glyph, 13.0, c::FG_MUTE()))
                .center_x(22)
                .center_y(22),
        )
        .on_press(Msg::ToggleCollapseAll)
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hovered {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: if hovered { c::FG() } else { c::FG_MUTE() },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
                snap: false,
            }
        });

        let add_btn = button(
            container(icon("plus", 12.0, c::FG_MUTE()))
                .center_x(22)
                .center_y(22),
        )
        .on_press(Msg::AddProject)
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if hovered {
                    Some(Background::Color(c::BG_HOVER()))
                } else {
                    None
                },
                text_color: if hovered { c::FG() } else { c::FG_MUTE() },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(4.0),
                },
                shadow: Shadow::default(),
                snap: false,
            }
        });

        // Tree is always active now, so the collapse-all toggle is always shown.
        let right_tools: Element<'_, Msg> =
            container(row![add_btn, toggle].align_y(iced::Alignment::Center))
                .height(Length::Fill)
                .align_y(iced::Alignment::Center)
                .into();

        let section_label = section_header("PROJECTS", 0.0, 0.0);

        container(
            row![section_label, Space::new().width(Length::Fill), right_tools,]
                .align_y(iced::Alignment::Center)
                .height(Length::Fill)
                .padding(Padding {
                    top: 0.0,
                    bottom: 0.0,
                    left: 14.0,
                    right: 8.0,
                }),
        )
        .height(SESSBAR_H)
        .width(Length::Fill)
        .into()
    }

    fn tree_view(&self) -> Element<'_, Msg> {
        let mut col: Column<'_, Msg> = Column::new();
        let projects: Vec<_> = self
            .app
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.name.clone(), p.path.clone()))
            .collect();
        for (pi, pname, ppath) in projects {
            let expanded = !self.collapsed.contains(&pi);
            let is_git = crate::git::is_repo(&ppath);
            let count = self
                .app
                .sessions
                .iter()
                .filter(|s| s.project == pname)
                .count();
            // Collapsed projects surface the most urgent descendant state as
            // a trailing glyph; expanded parents show nothing extra.
            let proj_rollup = if !expanded {
                super::activity::most_urgent(
                    self.app
                        .sessions
                        .iter()
                        .filter(|s| s.project == pname)
                        .map(|s| self.activity_state(s)),
                )
            } else {
                None
            };
            col = col.push(project_row(
                pi,
                &pname,
                count,
                expanded,
                is_git,
                proj_rollup,
                self.blink_tick,
                self.attention_pulse(),
            ));

            if !expanded {
                continue;
            }
            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };
            for (wi, w) in wts.iter().enumerate() {
                let wname = if w.is_main {
                    pname.clone()
                } else {
                    crate::app::path_basename(&w.path)
                };
                let active_wt = pi == self.app.proj_idx && wi == self.app.wt_idx;
                let hovered = self.hovered_wt == Some((pi, wi));
                let wt_expanded = !self.collapsed_wt.contains(&(pi, wi));
                // Same roll-up rule as projects: only when collapsed.
                let wt_rollup = if !wt_expanded {
                    super::activity::most_urgent(
                        self.app
                            .sessions
                            .iter()
                            .filter(|s| s.wt_path == w.path)
                            .map(|s| self.activity_state(s)),
                    )
                } else {
                    None
                };
                let has_run = self.app.store.projects.get(pi).is_some_and(|p| {
                    p.scripts
                        .run
                        .as_deref()
                        .is_some_and(|s| !s.trim().is_empty())
                });
                let git_suffix = self
                    .git_state
                    .lock()
                    .ok()
                    .and_then(|g| g.get(&w.path).and_then(crate::git::git_state_suffix));
                let wt_el = worktree_row(
                    pi,
                    wi,
                    &wname,
                    &w.branch,
                    active_wt,
                    w.is_main,
                    is_git,
                    hovered,
                    wt_expanded,
                    has_run,
                    wt_rollup,
                    self.blink_tick,
                    self.attention_pulse(),
                    &self.app.available_agents,
                    git_suffix,
                );
                col = col.push(
                    iced::widget::mouse_area(wt_el)
                        .on_enter(Msg::HoverWorktree(Some((pi, wi))))
                        .on_exit(Msg::HoverWorktree(None)),
                );

                if !wt_expanded {
                    continue;
                }
                for (si, s) in self.app.sessions.iter().enumerate() {
                    if s.wt_path == w.path {
                        // The tree and the pinned terminals now render
                        // simultaneously, so a session must not show the
                        // "active" highlight while the workspace is actually
                        // showing a home terminal.
                        let active = !self.terminal_focused && self.app.active_session == Some(si);
                        let pending_kill = self.pending_kill == Some(si);
                        col = col.push(session_row(
                            si,
                            s,
                            &wname,
                            active,
                            pending_kill,
                            self.activity_state(s),
                            self.blink_tick,
                            self.attention_pulse(),
                        ));
                    }
                }
            }
        }

        if !self.terminals_collapsed {
            // Expanded: every terminal already renders its own row below, so
            // the header's activity dot (a "something's running in here" cue
            // for the *collapsed* state) would be redundant — always off.
            col = col.push(divider_h(c::BORDER_SOFT()));
            col = col.push(crate::gui::rows::home_terminals_header(
                true,
                self.app.home_terminals.len(),
                false,
            ));
            for (i, s) in self.app.home_terminals.iter().enumerate() {
                let active = self.terminal_focused && self.app.active_terminal == Some(i);
                let pending_kill = self.pending_kill_terminal == Some(i);
                col = col.push(crate::gui::rows::terminal_row(i, s, active, pending_kill));
            }
        }

        col.into()
    }

    /// Session indices in the order `tree_view` renders them, honoring
    /// collapse state. Kept as a separate method (identical to
    /// `tree_session_order`) because `mod+1..9` calls it by this name.
    pub fn visible_session_order(&self) -> Vec<usize> {
        self.tree_session_order()
    }

    /// Session indices in the top-to-bottom order `tree_view` renders them,
    /// skipping sessions hidden under a collapsed project or worktree.
    fn tree_session_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for (pi, _p) in self.app.store.projects.iter().enumerate() {
            if self.collapsed.contains(&pi) {
                continue;
            }
            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };
            for (wi, w) in wts.iter().enumerate() {
                if self.collapsed_wt.contains(&(pi, wi)) {
                    continue;
                }
                for (si, s) in self.app.sessions.iter().enumerate() {
                    if s.wt_path == w.path {
                        order.push(si);
                    }
                }
            }
        }
        order
    }

    /// Find the y-pixel offset of the open agent menu, if any, so the overlay
    /// can be positioned. Walks the tree in the same order `tree_view` does.
    fn open_agent_menu_top(&self) -> Option<(usize, usize, f32, bool)> {
        let (open_proj, open_wt) = self.open_agent_menu?;
        let mut acc_y: f32 = 0.0;

        for (pi, pname) in self
            .app
            .store
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.name.as_str()))
        {
            acc_y += ROW_H; // project row
            if self.collapsed.contains(&pi) {
                continue;
            }

            let wts: &[Worktree] = if pi == self.app.proj_idx {
                &self.app.worktrees
            } else {
                self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
            };

            for (wi, w) in wts.iter().enumerate() {
                let wname = if w.is_main {
                    pname.to_string()
                } else {
                    crate::app::path_basename(&w.path)
                };
                let show_branch = super::rows::worktree_shows_branch(w.is_main, &w.branch, &wname);
                let wt_h = super::rows::worktree_row_height(show_branch);
                if pi == open_proj && wi == open_wt {
                    return Some((pi, wi, 6.0 + acc_y + wt_h, w.is_main));
                }
                acc_y += wt_h;

                if self.collapsed_wt.contains(&(pi, wi)) {
                    continue;
                }
                for s in &self.app.sessions {
                    if s.project == pname && s.wt_path == w.path {
                        acc_y += ROW_H;
                    }
                }
            }
        }

        None
    }

    /// The draggable divider between the session view and the right-docked
    /// terminal panel. Mirrors `sidebar_resize_handle` but drives the panel's
    /// percentage split (`term_panel_portion`) instead of a pixel width.
    fn term_panel_resize_handle(&self) -> Element<'_, Msg> {
        iced::widget::mouse_area(
            container(divider_v(c::BORDER()))
                .height(Length::Fill)
                .center_x(SIDEBAR_DIVIDER_W),
        )
        .on_press(Msg::TermPanelDragStart)
        .interaction(iced::mouse::Interaction::ResizingHorizontally)
        .into()
    }

    // ── workspace ─────────────────────────────────────────────────────────
    fn grid_workspace(&self) -> Element<'_, Msg> {
        use super::metrics::grid_layout;

        let n = self.tile_order.len();
        if n == 0 {
            return empty_workspace();
        }
        let (grid_cols, grid_rows) = grid_layout(n);

        // Lay out columns-of-tiles (not rows-of-tiles): each column stacks only
        // the tiles it actually has, so a column left with a single tile (e.g.
        // the odd one out in a 2×2 grid holding 3 sessions) spans the full
        // workspace height instead of leaving an empty cell beside it.
        let mut cols_row = row![].spacing(1).height(Length::Fill);
        for col_idx in 0..grid_cols {
            let mut col_el = column![]
                .spacing(1)
                .width(Length::Fill)
                .height(Length::Fill);
            for row_idx in 0..grid_rows {
                let tile_idx = row_idx * grid_cols + col_idx;
                if tile_idx >= n {
                    continue;
                }
                let si = self.tile_order[tile_idx];
                let mut el: Element<'_, Msg> = if si < self.app.sessions.len() {
                    self.grid_tile(tile_idx, si, &self.app.sessions[si])
                } else {
                    // Stale index: render blank until KillSession prunes tile_order.
                    container(Space::new().width(Length::Fill).height(Length::Fill))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(|_| container::Style {
                            background: Some(Background::Color(c::BG())),
                            ..Default::default()
                        })
                        .into()
                };
                // Draw-only slide for a tile that just swapped places: ease
                // its rendered position back from where it came from, in
                // grid cells, to zero. Layout is untouched (see slide.rs).
                if let Some(slide) = &self.grid_slide {
                    if let Some(&(_, d_col, d_row)) =
                        slide.tiles.iter().find(|(idx, _, _)| *idx == tile_idx)
                    {
                        let t =
                            super::update::slide_progress(slide.start, std::time::Instant::now());
                        if t < 1.0 {
                            let (tile_w, tile_h) = super::metrics::grid_tile_size(
                                self.window_size.width,
                                self.window_size.height,
                                self.ui_zoom,
                                n,
                            );
                            let remaining = 1.0 - t;
                            // ponytail: uses the nominal equal-cell tile size, so a
                            // horizontal swap between columns of unequal tile
                            // heights (ragged grid) is approximate — it settles
                            // exactly at t=1. Upgrade path is a real per-tile rect
                            // calc if it ever reads wrong.
                            let offset = iced::Vector::new(
                                d_col as f32 * (tile_w + 1.0) * remaining,
                                d_row as f32 * (tile_h + 1.0) * remaining,
                            );
                            el = super::slide::slide(el, offset);
                        }
                    }
                }
                col_el = col_el.push(el);
            }
            cols_row = cols_row.push(col_el);
        }

        // Inter-tile gaps: set the container background to BORDER_SOFT;
        // 1px spacing in column/row lets that background show through.
        let grid = container(cols_row)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BORDER_SOFT())),
                ..Default::default()
            });

        // The session launcher is opened from the "+" segment of the agent-view
        // combo in the appbar (see `appbar`); the grid workspace itself is just
        // the tile grid with no floating action button.
        grid.into()
    }

    fn workspace(&self) -> Element<'_, Msg> {
        if self.grid_view {
            return self.grid_workspace();
        }
        if self.terminal_tab() {
            return self.terminal_workspace();
        }
        let left: Element<'_, Msg> = match self.app.active_session {
            Some(i) if i < self.app.sessions.len() => column![
                self.sess_bar(i, &self.app.sessions[i]),
                self.pty(PtyPane::Agent, &self.app.sessions[i]),
            ]
            .height(Length::Fill)
            .into(),
            _ => empty_workspace(),
        };

        // When the slide-over panel is open and a session is active, split the
        // workspace: session view on the left (filling remaining space), the
        // worktree terminal panel docked full-height on the right (~46%).
        let inner: Element<'_, Msg> = if self.term_panel_open && self.active_wt_path().is_some() {
            row![
                container(left)
                    .width(Length::FillPortion(100 - self.term_panel_portion))
                    .height(Length::Fill),
                self.term_panel_resize_handle(),
                container(self.term_panel())
                    .width(Length::FillPortion(self.term_panel_portion))
                    .height(Length::Fill),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            left
        };

        container(inner)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                ..Default::default()
            })
            .into()
    }

    /// The right-docked terminal slide-over for the active session's worktree:
    /// a thin tab strip (one tab per shell + a `＋` add) above the active
    /// shell's PTY. Reuses the shared `pty()` renderer.
    fn term_panel(&self) -> Element<'_, Msg> {
        let Some(wt) = self.active_wt_path() else {
            return empty_workspace();
        };
        let shells = self.app.wt_terminals_for(&wt);
        let active_idx = self.app.active_wt_terminal_idx(&wt);

        // Tab strip: a small mono tab per shell with a running/exited dot and a
        // × close, plus a ＋ to add a new shell.
        let mut tabs = row![].spacing(6).align_y(iced::Alignment::Center);
        for (i, s) in shells.iter().enumerate() {
            tabs = tabs.push(self.term_panel_tab(i, s, active_idx == Some(i)));
        }
        tabs = tabs.push(
            button(
                container(icon("plus", 13.0, c::FG_DIM()))
                    .center_x(22)
                    .center_y(22),
            )
            .on_press(Msg::NewWtTerminal)
            .padding(0)
            .style(|_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: if hovered {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        None
                    },
                    text_color: if hovered { c::FG() } else { c::FG_DIM() },
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: Radius::from(4.0),
                    },
                    shadow: Shadow::default(),
                    snap: false,
                }
            }),
        );

        // Collapse the whole slide-over (same effect as the header term
        // toggle), so the panel is always dismissable from itself. Uses a
        // distinct "collapse-right" glyph (rather than the per-tab ×) so the
        // two affordances don't read as the same action, plus a tooltip to
        // disambiguate at a glance.
        let close_panel = Self::hint(
            icon_btn("collapse-right", Msg::ToggleTermPanel),
            "collapse panel",
        );

        let strip = container(
            row![
                container(
                    scrollable(tabs).direction(scrollable::Direction::Horizontal(
                        scrollable::Scrollbar::new().width(0).scroller_width(0)
                    ))
                )
                .width(Length::Fill)
                .clip(true),
                close_panel,
            ]
            .align_y(iced::Alignment::Center)
            .height(Length::Fill)
            .padding(Padding::from([0, 10])),
        )
        .height(SESSBAR_H)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(c::BG_STRIP())),
            ..Default::default()
        });

        let surface: Element<'_, Msg> = match self.app.active_wt_terminal(&wt) {
            Some(s) => self.pty(PtyPane::Panel, s),
            None => empty_workspace(),
        };

        column![strip, divider_h(c::BORDER_SOFT()), surface]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Wrap `content` with a small hint label shown on hover. Styled to match
    /// the app's other floating surfaces (BG_STRIP background, BORDER border).
    fn hint<'a>(content: impl Into<Element<'a, Msg>>, label: &'a str) -> Element<'a, Msg> {
        iced::widget::tooltip(
            content,
            container(text(label).font(UI_FONT).size(11).color(c::FG_DIM()))
                .padding(Padding::from([4, 8]))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG_STRIP())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                }),
            iced::widget::tooltip::Position::Top,
        )
        .into()
    }

    /// A single tab in the terminal panel's tab strip.
    fn term_panel_tab<'a>(&self, idx: usize, s: &Session, active: bool) -> Element<'a, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let dot_color = if running { c::GREEN() } else { c::FG_MUTE() };
        let name_color = if active { c::CYAN() } else { c::FG_DIM() };

        let close_btn = button(
            container(icon("close", 11.0, c::FG_MUTE()))
                .center_x(16)
                .center_y(18),
        )
        .on_press(Msg::CloseWtTerminal(idx))
        .padding(0)
        .style(|_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: None,
                text_color: if hovered { c::RED() } else { c::FG_MUTE() },
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: Radius::from(3.0),
                },
                shadow: Shadow::default(),
                snap: false,
            }
        });
        let close = container(Self::hint(close_btn, "close shell")).padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 2.0,
        });

        // Tabs are identified by a terminal icon (status conveyed by the dot
        // and the active highlight), not a textual name — cleaner when several
        // shells share a worktree. Spacing widened so the dot / icon / × read
        // as distinct controls rather than one blob.
        let label = row![dot(dot_color), icon("term", 13.0, name_color), close,]
            .spacing(6)
            .align_y(iced::Alignment::Center);

        button(container(label).padding(Padding::from([0, 8])).center_y(24))
            .on_press(Msg::SelectWtTerminal(idx))
            .padding(0)
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: if active {
                        Some(Background::Color(c::BG_HL()))
                    } else if hovered {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        None
                    },
                    text_color: name_color,
                    border: Border {
                        color: if active {
                            c::CYAN()
                        } else {
                            Color::TRANSPARENT
                        },
                        width: if active { 1.0 } else { 0.0 },
                        radius: Radius::from(4.0),
                    },
                    shadow: Shadow::default(),
                    snap: false,
                }
            })
            .into()
    }

    /// Workspace for the persistent home-terminal tab: a status bar with a
    /// restart control above the home shell's PTY. Shows a spawn-failure hint
    /// if the shell could never be started.
    fn terminal_workspace(&self) -> Element<'_, Msg> {
        let inner: Element<'_, Msg> = match self.app.active_home_terminal() {
            Some(s) => column![self.home_terminal_bar(s), self.pty(PtyPane::Agent, s)]
                .height(Length::Fill)
                .into(),
            None => empty_terminals_workspace(),
        };

        container(inner)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG())),
                ..Default::default()
            })
            .into()
    }

    /// Status bar for the home terminal. Unlike `sess_bar` there is no kill
    /// action — the terminal is permanent — only a restart that relaunches the
    /// shell at `~`. When the shell has exited the restart button is the
    /// affordance the user reaches for.
    fn home_terminal_bar(&self, s: &Session) -> Element<'_, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let (dot_color, label) = if running {
            (c::GREEN(), "running")
        } else {
            (c::FG_MUTE(), "exited")
        };
        let bar_text = |content: String, color: Color| {
            text(content)
                .font(UI_FONT)
                .size(12)
                .line_height(1.0)
                .height(18)
                .align_y(iced::alignment::Vertical::Center)
                .color(color)
        };

        let status: Element<'_, Msg> = row![dot(dot_color), bar_text(label.to_string(), dot_color)]
            .spacing(6)
            .align_y(iced::Alignment::Center)
            .into();

        let ctx = crate::gui::rows::terminal_context(s).unwrap_or_else(|| "~".to_string());
        let ctx = crate::gui::widgets::truncate_middle(&ctx, 80);
        let identity = row![bar_text(ctx, c::FG())]
            .spacing(6)
            .align_y(iced::Alignment::Center);

        let bar = row![
            status,
            vline(),
            container(identity).width(Length::Fill).clip(true),
            bar_text("~".to_string(), c::FG_MUTE()),
            vline(),
            tool_btn("restart", "restart", false, Msg::RestartHomeTerminal),
            tool_btn(
                "zen",
                if self.app.chrome_visible {
                    "zen"
                } else {
                    "exit zen"
                },
                false,
                Msg::ToggleZen,
            ),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill)
        .padding(Padding::from([0, 16]));

        let bar_container = container(bar)
            .height(SESSBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![bar_container, divider_h(c::BORDER_SOFT())].into()
    }

    fn sess_bar(&self, si: usize, s: &Session) -> Element<'_, Msg> {
        let running = matches!(
            *s.status.lock().unwrap_or_else(|e| e.into_inner()),
            SessionStatus::Running
        );
        let context = session_context_title(s);
        let show_progress = running
            && context
                .as_deref()
                .map(is_in_progress_title)
                .unwrap_or(false);
        // Visual hierarchy: session/project label is the strongest (13px,
        // weight-600, FG); the branch and context title are secondary
        // (12px, FG_DIM).
        let sess_text_sized = |content: String, size: f32, color: Color, bold: bool| {
            let t = text(content)
                .font(UI_FONT)
                .size(size)
                .line_height(1.0)
                .align_y(iced::alignment::Vertical::Center)
                .color(color);
            if bold {
                t.font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..UI_FONT
                })
            } else {
                t
            }
        };
        // Force single-line rendering (see rows::single_line docs): iced 0.13's
        // text widget ignores wrapping::None, so long labels word-wrap to a
        // second line inside the outer clip(true) container unless each text
        // is itself clipped to exactly one line height.
        let single = |content: String, size: f32, color: Color, bold: bool| -> Element<'_, Msg> {
            single_line(sess_text_sized(content, size, color, bold), size)
        };

        let mut identity = row![
            icon(s.agent.icon_name(), 13.0, c::FG()),
            single(s.label.clone(), 13.0, c::FG(), true),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        // Branchless sessions (e.g. host terminals) skip the branch segment
        // entirely — otherwise the header shows two dots with nothing between.
        if !s.branch.trim().is_empty() {
            identity = identity
                .push(single("·".to_string(), 13.0, c::FG_MUTE(), false))
                .push(single(s.branch.clone(), 12.0, c::FG_DIM(), false));
        }

        if let Some(title) = context {
            let title = crate::gui::widgets::truncate_middle(&title, 80);
            let session_context: Element<'_, Msg> = if show_progress {
                let phase = ((self.blink_tick / 5) % 3) as usize;
                let step_dot = |i| dot(if i == phase { c::GREEN() } else { c::FG_MUTE() });
                row![
                    step_dot(0),
                    step_dot(1),
                    step_dot(2),
                    single("in progress".to_string(), 12.0, c::GREEN(), false),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                single(title, 12.0, c::FG_DIM(), false)
            };
            identity = identity
                .push(single("·".to_string(), 12.0, c::FG_MUTE(), false))
                .push(session_context);
        }

        // Resolve the session's (project, worktree) indices so the run button
        // can target the right worktree, and only show it when the project has
        // a run script configured.
        let coords = self
            .app
            .store
            .projects
            .iter()
            .position(|p| p.name == s.project)
            .and_then(|pi| {
                let wts: &[Worktree] = if pi == self.app.proj_idx {
                    &self.app.worktrees
                } else {
                    self.wt_cache.get(&pi).map(|v| v.as_slice()).unwrap_or(&[])
                };
                wts.iter()
                    .position(|w| w.path == s.wt_path)
                    .map(|wi| (pi, wi))
            });
        let run_btn: Element<'_, Msg> = match coords {
            Some((proj, wt))
                if self.app.store.projects[proj]
                    .scripts
                    .run
                    .as_deref()
                    .is_some_and(|s| !s.trim().is_empty()) =>
            {
                tool_btn("play", "run script", false, Msg::RunScript { proj, wt })
            }
            _ => Space::new().width(0).into(),
        };

        let bar = row![
            container(identity).width(Length::Fill).clip(true),
            vline(),
            run_btn,
            tool_btn_toggle(
                "term",
                "terminal",
                false,
                self.term_panel_open,
                Msg::ToggleTermPanel
            ),
            tool_btn(
                "zen",
                if self.app.chrome_visible {
                    "zen"
                } else {
                    "exit zen"
                },
                false,
                Msg::ToggleZen,
            ),
            tool_btn(
                "trash",
                if self.pending_kill == Some(si) {
                    "confirm kill"
                } else {
                    "kill"
                },
                true,
                // Two-step confirm, targeting the session this bar renders —
                // never a fallback index.
                if self.pending_kill == Some(si) {
                    Msg::KillSession(si)
                } else {
                    Msg::RequestKillSession(si)
                },
            ),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center)
        .height(Length::Fill)
        .padding(Padding::from([0, 16]));

        let bar_container = container(bar)
            .height(SESSBAR_H)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![bar_container, divider_h(c::BORDER_SOFT())].into()
    }

    fn pty(&self, pane: PtyPane, s: &Session) -> Element<'_, Msg> {
        // Per-session row snapshot + canvas cache. Switching to a quiet
        // session returns the cached geometry with zero draw work; switching
        // to a session that produced output re-snaps the rows and clears the
        // canvas cache, then draws once.
        // The `dirty` Arc's address is the cache key. A dropped session can
        // free that address and a newly spawned one reuse it — safe only
        // because every session add/remove (incl. home-terminal new/close/
        // restart) fully clears this cache, so no stale entry can alias.
        // Resolve once per tile per frame: a pinned "Project theme" makes
        // this PTY's *content* (fill, default fg, cursor, ANSI 0-15) render
        // in that theme instead of the global one. App chrome (header,
        // borders, rail, appbar) is untouched — it always uses `c::*`
        // against the global active theme.
        let pty_theme = self
            .app
            .project_theme_override(&s.project)
            .unwrap_or_else(crate::theme::current);

        let key = Arc::as_ptr(&s.dirty) as usize;
        let (rows, cache, cursor_pos) = {
            let mut map = self.pty_cache.borrow_mut();
            let entry = map.entry(key);
            let needs_rebuild = match &entry {
                std::collections::hash_map::Entry::Occupied(_) => {
                    s.dirty.swap(false, Ordering::Relaxed)
                }
                std::collections::hash_map::Entry::Vacant(_) => {
                    s.dirty.store(false, Ordering::Relaxed);
                    true
                }
            };
            let entry = entry.or_insert_with(|| PtyCacheEntry {
                rows: Arc::new(Vec::new()),
                cache: Arc::new(iced::widget::canvas::Cache::default()),
                cursor_pos: None,
            });
            if needs_rebuild {
                let parser = s.parser.lock().unwrap_or_else(|e| e.into_inner());
                let screen = parser.screen();
                let (h, w) = screen.size();
                let mut new_rows = Vec::with_capacity(h as usize);
                for r in 0..h {
                    new_rows.push(rebuild_row_runs(screen, r, w, &pty_theme));
                }
                entry.rows = Arc::new(new_rows);
                entry.cache.clear();
                entry.cursor_pos = if screen.hide_cursor() {
                    None
                } else {
                    Some(screen.cursor_position())
                };
            }
            (
                Arc::clone(&entry.rows),
                Arc::clone(&entry.cache),
                entry.cursor_pos,
            )
        };

        let rows_len = rows.len() as f32;
        let cols = rows
            .first()
            .map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>())
            .unwrap_or(0) as f32;
        // Cursor blinks at ~500 ms on / 500 ms off (tick interval = 60 ms,
        // so 8–9 ticks per half-period; use mod 16 with threshold 8).
        let cursor_visible = self.blink_tick % 16 < 8;
        // Translate the scrollback-stable selection into the current viewport.
        // Each endpoint clamps to the visible window; a selection entirely off
        // one edge isn't painted. The selection lives in the pane that owns
        // it (Agent/Panel, or the focused tile in grid view), so only paint it
        // there — otherwise a selection in one pane would mis-render against
        // another's grid.
        let selection = if pane == self.selection_pane() {
            self.pty_selection
        } else {
            None
        }
        .and_then(|(a, b)| {
            let (h, sb) = {
                let p = s.parser.lock().ok()?;
                (
                    p.screen().size().0 as isize,
                    p.screen().scrollback() as isize,
                )
            };
            if h == 0 {
                return None;
            }
            let to_vr = |c: &super::state::AbsCell| (h - 1) - (c.a_row as isize - sb);
            let (ra, rb) = (to_vr(&a), to_vr(&b));
            if (ra < 0 && rb < 0) || (ra > h - 1 && rb > h - 1) {
                return None;
            }
            let cell = |c: &super::state::AbsCell, r: isize| PtyCell {
                row: r.clamp(0, h - 1) as usize,
                col: c.col,
            };
            Some((cell(&a, ra), cell(&b, rb)))
        });
        let program = PtyProgram {
            pane,
            rows,
            cache,
            selection,
            cursor: cursor_pos,
            cursor_visible,
            default_fg: c::fg_of(&pty_theme),
            cursor_color: c::fg_of(&pty_theme),
        };
        let body: Element<'_, Msg> = canvas_widget(program)
            .width(Length::Fixed((cols * CELL_W).max(CELL_W)))
            .height(Length::Fixed((rows_len * CELL_H).max(CELL_H)))
            .into();

        // While the split is live, tint the focused PTY's top edge so it's clear
        // which terminal will receive keystrokes. Suppressed when the panel is
        // closed (only one PTY is interactive then).
        let focused = self.term_panel_open && pane == self.focused_input_pane();
        container(
            scrollable(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .direction(scrollable::Direction::Vertical(
                    scrollable::Scrollbar::new().width(0).scroller_width(0),
                )),
        )
        .padding(Padding::from([12, 16]))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(c::bg_of(&pty_theme))),
            border: if focused {
                Border {
                    color: c::CYAN(),
                    width: 1.0,
                    radius: Radius::from(0.0),
                }
            } else {
                Border::default()
            },
            ..Default::default()
        })
        .into()
    }

    fn grid_tile(&self, tile_order_idx: usize, si: usize, s: &Session) -> Element<'_, Msg> {
        use super::metrics::TILE_HEAD_H;

        let focused = self.grid_focused == Some(si);
        let is_drag_src = self
            .grid_drag
            .as_ref()
            .map_or(false, |d| d.source_idx == tile_order_idx);
        let is_drop_zone = self.grid_drag.as_ref().map_or(false, |d| {
            d.hover_idx == tile_order_idx && d.source_idx != tile_order_idx
        });

        // ── tile header ────────────────────────────────────────────────
        let tile_btn = |icon_name, msg| {
            button(
                container(icon(icon_name, 10.0, c::FG_MUTE()))
                    .center_x(18)
                    .center_y(18),
            )
            .on_press(msg)
            .padding(0)
            .style(|_, _| button::Style {
                background: None,
                text_color: c::FG_MUTE(),
                border: Border::default(),
                shadow: Shadow::default(),
                snap: false,
            })
        };
        let confirming_kill = self.pending_kill == Some(si);
        let kill_btn = button(
            container(icon(
                "trash",
                10.0,
                if confirming_kill {
                    c::RED()
                } else {
                    c::FG_MUTE()
                },
            ))
            .center_x(18)
            .center_y(18),
        )
        .on_press(if confirming_kill {
            Msg::KillSession(si)
        } else {
            Msg::RequestKillSession(si)
        })
        .padding(0)
        .style(move |_, _| button::Style {
            background: None,
            text_color: if confirming_kill {
                c::RED()
            } else {
                c::FG_MUTE()
            },
            border: Border::default(),
            shadow: Shadow::default(),
            snap: false,
        });
        // Waiting-for-input: drives both the header's "respond" chip below and
        // (later) the tile border. Attention wins over the focused-cyan border.
        use super::activity::ActivityState;
        let waiting = matches!(self.activity_state(s), ActivityState::WaitingForInput);

        // "respond" chip: only shown while this tile is waiting for input.
        // Pulses via `attention_pulse` so it stays visible without demanding
        // constant attention. Placed left of `num_hint` in the header.
        let respond_chip: Element<'_, Msg> = if waiting {
            let a = 1.0 - 0.35 * self.attention_pulse();
            let amber = Color { a, ..c::AMBER() };
            let amber_bg = Color {
                a: a * 0.08,
                ..c::AMBER()
            };
            let inner: Element<'_, Msg> = if tile_order_idx >= 9 {
                text("respond").font(MONO_FONT).size(9).color(amber).into()
            } else {
                let n = tile_order_idx + 1;
                let chord: Element<'_, Msg> = if cfg!(target_os = "macos") {
                    row![
                        icon("command", 9.0, amber),
                        text(n.to_string()).font(MONO_FONT).size(9).color(amber),
                    ]
                    .spacing(1)
                    .align_y(iced::Alignment::Center)
                    .into()
                } else {
                    text(format!("{}+{}", platform_mod_label(), n))
                        .font(MONO_FONT)
                        .size(9)
                        .color(amber)
                        .into()
                };
                row![
                    text("respond · ").font(MONO_FONT).size(9).color(amber),
                    chord,
                ]
                .spacing(1)
                .align_y(iced::Alignment::Center)
                .into()
            };
            container(inner)
                .padding(Padding::from([1, 4]))
                .style(move |_| container::Style {
                    background: Some(Background::Color(amber_bg)),
                    border: Border {
                        color: amber,
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            Space::new().width(0).into()
        };

        // Shortcut-number hint: the first 9 tiles (tile_order positions 0..9)
        // are reachable via the platform modifier + 1..9 (see
        // select_visible_session). Show the full chord so the key is
        // unambiguous. On macOS we render the ⌘ glyph as an SVG icon (the
        // bundled font has no U+2318); elsewhere the modifier is spelled out as
        // text via platform_mod_label(), matching the overlay.
        let num_hint: Element<'_, Msg> = if tile_order_idx < 9 {
            let n = tile_order_idx + 1;
            let hint_color = if focused { c::FG_DIM() } else { c::FG_MUTE() };
            let inner: Element<'_, Msg> = if cfg!(target_os = "macos") {
                row![
                    icon("command", 9.0, hint_color),
                    text(n.to_string())
                        .font(MONO_FONT)
                        .size(9)
                        .color(hint_color),
                ]
                .spacing(1)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                text(format!("{}+{}", platform_mod_label(), n))
                    .font(MONO_FONT)
                    .size(9)
                    .color(hint_color)
                    .into()
            };
            container(inner)
                .padding(Padding::from([1, 4]))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: 3.0.into(),
                    },
                    ..Default::default()
                })
                .into()
        } else {
            Space::new().width(0).into()
        };
        // Branchless sessions (e.g. host terminals) skip the branch segment
        // entirely — otherwise the header shows a trailing dot with nothing after.
        let branch_seg: Element<'_, Msg> = if s.branch.trim().is_empty() {
            Space::new().width(0).into()
        } else {
            row![
                text("·").size(10).color(c::FG_MUTE()),
                text(s.branch.clone()).size(10).color(c::FG_MUTE()),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into()
        };
        let header_row = row![
            icon(s.agent.icon_name(), 11.0, c::FG_DIM()),
            text(s.agent.label())
                .font(UI_BOLD)
                .size(10)
                .color(c::FG_DIM()),
            text("·").size(10).color(c::FG_MUTE()),
            text(s.project.clone()).size(10).color(c::FG_MUTE()),
            branch_seg,
            Space::new().width(Length::Fill),
            respond_chip,
            num_hint,
            tile_btn("zen", Msg::GridTileZen(si)),
            kill_btn,
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
        .padding(Padding::from([0, 6]));

        let header_bg = if focused { c::BG_HL() } else { c::BG_STRIP() };
        let header = iced::widget::mouse_area(
            container(header_row)
                .height(TILE_HEAD_H)
                .align_y(iced::Alignment::Center)
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(header_bg)),
                    ..Default::default()
                }),
        )
        .on_press(Msg::GridDragStart(tile_order_idx));

        // ── tile body (header + PTY) ───────────────────────────────────
        let tile_body: Element<'_, Msg> = column![
            header,
            divider_h(c::BORDER_SOFT()),
            // Reuse the existing pty() renderer with PtyPane::Tile(si).
            // Selection paints here when this tile is `grid_focused` — see
            // `selection_pane`.
            self.pty(PtyPane::Tile(si), s),
        ]
        .height(Length::Fill)
        .into();

        // Drop-zone overlay: cyan inset when this tile is the drag target.
        let with_drop: Element<'_, Msg> = if is_drop_zone {
            stack![
                tile_body,
                container(Space::new().width(Length::Fill).height(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| container::Style {
                        border: Border {
                            color: c::CYAN(),
                            width: 1.5,
                            radius: Radius::from(0.0),
                        },
                        background: Some(Background::Color(Color {
                            a: 0.06,
                            ..c::CYAN()
                        })),
                        ..Default::default()
                    }),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            tile_body
        };

        // Drag-source dim: semi-transparent BG overlay to show "lifted" state.
        let with_dim: Element<'_, Msg> = if is_drag_src {
            stack![
                with_drop,
                container(Space::new().width(Length::Fill).height(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(|_| {
                        let mut bg = c::BG();
                        bg.a = 0.72;
                        container::Style {
                            background: Some(Background::Color(bg)),
                            ..Default::default()
                        }
                    }),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        } else {
            with_drop
        };

        // Waiting-for-input: solid amber 1.5px border (no blink).
        // Overrides the focused-cyan border — attention wins.
        let (border_color, border_width) = if waiting {
            (c::AMBER(), 1.5f32)
        } else if focused {
            (c::CYAN(), 1.5f32)
        } else {
            (Color::TRANSPARENT, 0.0)
        };

        // Full-tile scrim overlay when waiting for input. Layered on top of
        // the tile-header "respond" chip above and the appbar "needs you"
        // pill elsewhere — this doesn't replace either, it's the third and
        // most attention-grabbing signal for a tile that needs a response.
        let with_scrim: Element<'_, Msg> = if waiting {
            // Opacity pulse (~2.4s): 40-tick triangle wave, alpha 0.7..1.0.
            let phase = (self.blink_tick % 40) as f32;
            let t = (phase - 20.0).abs() / 20.0;
            let text_alpha = 0.7 + 0.3 * t;
            let amber_pulsed = Color {
                a: text_alpha,
                ..c::AMBER()
            };

            let sub_line: String = if tile_order_idx < 9 {
                format!(
                    "click to respond · {}+{}",
                    platform_mod_label(),
                    tile_order_idx + 1
                )
            } else {
                "click to respond".to_string()
            };

            let scrim_content: Element<'_, Msg> = container(
                column![
                    text("N E E D S   A T T E N T I O N")
                        .font(UI_BOLD)
                        .size(20)
                        .color(amber_pulsed),
                    text(sub_line).font(MONO_FONT).size(10).color(c::FG_MUTE()),
                ]
                .spacing(8)
                .align_x(iced::Alignment::Center),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center)
            .style(|_| container::Style {
                // Darker theme-derived scrim: BG_STRIP is the theme's deepest
                // surface, so the wash tracks the active theme (iced has no
                // backdrop blur, so opacity does the softening).
                background: Some(Background::Color(Color {
                    a: 0.92,
                    ..c::BG_STRIP()
                })),
                ..Default::default()
            })
            .into();

            // Wrap in mouse_area so clicking the scrim focuses/acknowledges
            // the tile, same as clicking the header elsewhere on the tile.
            let clickable_scrim: Element<'_, Msg> = iced::widget::mouse_area(scrim_content)
                .on_press(Msg::GridDragStart(tile_order_idx))
                .into();

            stack![with_dim, clickable_scrim]
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            with_dim
        };

        // on_enter fires even while a button is held — the GridDragHover handler
        // ignores it when no drag is active.
        iced::widget::mouse_area(
            container(with_scrim)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    border: Border {
                        color: border_color,
                        width: border_width,
                        radius: Radius::from(0.0),
                    },
                    ..Default::default()
                }),
        )
        .on_enter(Msg::GridDragHover(tile_order_idx))
        .into()
    }

    /// The pane that currently owns keyboard/scroll/selection input. Mirrors the
    /// routing logic in `focused_session*`: the panel only wins while it is open
    /// *and* `focused_pane` selects it; otherwise input belongs to the agent.
    pub(super) fn focused_input_pane(&self) -> PtyPane {
        if self.term_panel_open && matches!(self.focused_pane, FocusedPane::Panel) {
            PtyPane::Panel
        } else {
            PtyPane::Agent
        }
    }

    /// The pane that currently owns `pty_selection` — like
    /// `focused_input_pane`, but grid-view-aware: while grid view is showing,
    /// the focused tile owns any selection instead of the (unrendered) Agent
    /// pane.
    pub(super) fn selection_pane(&self) -> PtyPane {
        if self.grid_view {
            self.grid_focused
                .map(PtyPane::Tile)
                .unwrap_or(PtyPane::Agent)
        } else {
            self.focused_input_pane()
        }
    }

    // ── status bar ────────────────────────────────────────────────────────
    fn statusbar(&self) -> Element<'_, Msg> {
        let running = self
            .app
            .sessions
            .iter()
            .filter(|s| {
                matches!(
                    *s.status.lock().unwrap_or_else(|e| e.into_inner()),
                    SessionStatus::Running
                )
            })
            .count();
        let backend = if self.app.use_tmux() {
            "tmux"
        } else {
            "native"
        };
        let theme_name = self
            .app
            .store
            .theme
            .clone()
            .unwrap_or_else(|| "tokyonight".into());

        let mut left = row![
            row![
                dot(if running > 0 {
                    c::GREEN()
                } else {
                    c::FG_MUTE()
                }),
                text(format!("{running}"))
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::FG_DIM()),
                text("RUNNING").font(MONO_FONT).size(10).color(c::FG_MUTE()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            row![
                text("BACKEND").font(MONO_FONT).size(10).color(c::FG_MUTE()),
                text(backend).font(MONO_FONT).size(10).color(c::FG_DIM()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            row![
                text("THEME").font(MONO_FONT).size(10).color(c::FG_MUTE()),
                text(theme_name).font(MONO_FONT).size(10).color(c::FG_DIM()),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Center);

        if self.app.skip_permissions_enabled() {
            left = left.push(keycap(
                text("bypass")
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::YELLOW())
                    .into(),
            ));
        }

        let toast: Element<'_, Msg> = match &self.app.toast {
            Some(t) => {
                let color = match t.kind {
                    crate::app::ToastKind::Error => c::RED(),
                    crate::app::ToastKind::Info => c::GREEN(),
                };
                text(t.message.clone())
                    .font(MONO_FONT)
                    .size(10)
                    .color(color)
                    .into()
            }
            None => Space::new().width(0).into(),
        };

        let modifier = platform_mod_label();
        // Build a footer-hint style button: a keycap chip (mod icon + key on
        // macOS, "{mod}+{key}" text elsewhere) followed by a muted mono
        // label, matching the palette footer's `footer_hint` chrome — but
        // wrapped in a button since these still need `on_press`.
        let hint_button = |key: &str, label: &'static str, msg: Msg| -> Element<'_, Msg> {
            let keycap_content: Element<'_, Msg> = if cfg!(target_os = "macos") {
                row![
                    icon("command", 9.0, c::FG_DIM()),
                    text(key.to_string())
                        .font(MONO_FONT)
                        .size(10)
                        .color(c::FG_DIM()),
                ]
                .spacing(1)
                .align_y(iced::Alignment::Center)
                .into()
            } else {
                text(format!("{modifier}+{key}"))
                    .font(MONO_FONT)
                    .size(10)
                    .color(c::FG_DIM())
                    .into()
            };
            let content = row![keycap(keycap_content), text(label).font(MONO_FONT).size(10),]
                .spacing(6)
                .align_y(iced::Alignment::Center);
            button(content)
                .padding(0)
                .on_press(msg)
                .style(|_, status| button::Style {
                    background: None,
                    text_color: if matches!(status, button::Status::Hovered) {
                        c::FG()
                    } else {
                        c::FG_MUTE()
                    },
                    ..Default::default()
                })
                .into()
        };

        let overlay_key = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::ShortcutOverlay))
            .map(|d| d.display_keys)
            .unwrap_or("/");
        let shortcuts_chip = hint_button(overlay_key, "shortcuts", Msg::OpenShortcutOverlay);

        let palette_key = SHORTCUTS
            .iter()
            .find(|d| d.action == Some(GlobalShortcut::NewSession))
            .map(|d| d.display_keys)
            .unwrap_or("p");
        let palette_chip = hint_button(palette_key, "palette", Msg::OpenSessionLauncher);

        let right = row![
            palette_chip,
            Space::new().width(14),
            shortcuts_chip,
            Space::new().width(14),
            text(format!("v{}", env!("CARGO_PKG_VERSION")))
                .font(MONO_FONT)
                .size(10)
                .color(c::FG_MUTE()),
        ]
        .align_y(iced::Alignment::Center);

        let bar = row![
            left,
            Space::new().width(24),
            toast,
            Space::new().width(Length::Fill),
            right,
        ]
        .padding(Padding::from([0, 16]))
        .align_y(iced::Alignment::Center)
        .height(Length::Fill);

        let bar_container = container(bar)
            .height(STATUS_H - 1.0)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                ..Default::default()
            });

        column![divider_h(c::BORDER_SOFT()), bar_container]
            .width(Length::Fill)
            .height(STATUS_H)
            .into()
    }

    // ── modal layer ───────────────────────────────────────────────────────
    fn modal_layer(&self) -> Element<'_, Msg> {
        let panel: Element<'_, Msg> = match &self.app.modal {
            Modal::Input {
                title,
                buffer,
                note,
            } => self.input_modal(title, buffer, note.as_deref()),
            Modal::Confirm {
                title,
                prompt,
                destructive,
                kind,
            } => self.confirm_modal(title, prompt, *destructive, kind),
            Modal::AddProject {
                step,
                path,
                dir_sel,
                name,
                git,
                init_git,
                note,
            } => {
                self.add_project_modal(*step, path, *dir_sel, name, git, *init_git, note.as_deref())
            }
            Modal::RemoveProject {
                name,
                worktrees,
                also_remove_worktrees,
                in_progress,
                done,
                current,
                errors,
                ..
            } => self.remove_project_modal(
                name,
                worktrees,
                *also_remove_worktrees,
                *in_progress,
                *done,
                current,
                errors,
            ),
            Modal::Message(message) => self.message_modal(message),
            Modal::TmuxChoice => self.tmux_choice_modal(),
            Modal::AgentPicker {
                project,
                wt_path,
                sel,
            } => self.agent_picker_modal(project, wt_path, *sel),
            Modal::ThemePicker {
                sel_dark,
                sel_light,
                tab,
                follow_system,
                scope,
                project_use_default,
                ..
            } => self.theme_picker_modal(
                *sel_dark,
                *sel_light,
                *tab,
                *follow_system,
                scope.clone(),
                *project_use_default,
            ),
            Modal::Settings => self.settings_modal(),
            Modal::ShortcutOverlay => self.shortcut_overlay_modal(),
            Modal::Updating => self.updating_modal(),
            Modal::Teardown => self.teardown_modal(),
            Modal::ScriptsEditor => self.project_settings_modal(),
            // Onboarding never reaches the modal layer: `view()` returns
            // `onboarding_view(...)` directly while it's active (see above),
            // full-viewport with no sidebar/statusbar/scrim behind it.
            Modal::Onboarding { .. } => unreachable!("onboarding short-circuits in view()"),
            // The palette already returns a `Length::Fill` x `Length::Fill`
            // element that top-aligns itself internally (see
            // `session_launcher_modal`), so wrapping it in the shared
            // center_x/center_y container below is a no-op — it stays
            // top-dropped instead of vertically centered like every other
            // modal.
            Modal::SessionLauncher {
                input,
                selected,
                browse_all,
                options,
                switch,
                row_actions,
                settings,
            } => self.session_launcher_modal(
                input,
                *selected,
                *browse_all,
                options.as_ref(),
                *switch,
                row_actions.as_ref(),
                *settings,
            ),
            _ => Space::new().width(0).into(),
        };

        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::SCRIM())),
                ..Default::default()
            })
            .into()
    }

    fn input_modal<'a>(
        &'a self,
        title: &'a str,
        buffer: &'a str,
        note: Option<&'a str>,
    ) -> Element<'a, Msg> {
        let field = text_input("", buffer)
            .id(modal_input_id())
            .font(UI_FONT)
            .size(14)
            .padding(0)
            .on_input(Msg::InputPathChanged)
            .on_submit(Msg::ModalSubmit)
            .style(palette_input_style);

        let input_zone = container(
            row![icon("git-branch", 16.0, c::FG_MUTE()), field]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([14, 16]));

        let mut buttons_zone = column![].spacing(8);
        if let Some(note) = note {
            buttons_zone = buttons_zone.push(text(note.to_string()).size(12).color(c::RED()));
        }
        buttons_zone = buttons_zone.push(
            row![
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("Submit", ModalBtn::Primary, Msg::ModalSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        );

        let body = column![
            modal_header(title, c::MAGENTA()),
            divider_h(c::BORDER_SOFT()),
            input_zone,
            divider_h(c::BORDER_SOFT()),
            container(buttons_zone).padding(Padding::from([12, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("⏎", "confirm"), ("esc", "cancel")]),
        ];

        modal_panel(body.into(), 480.0)
    }

    /// The windowed directory-match list shared by the add-project pick step
    /// and the onboarding project step: up to `window` rows that scroll to
    /// keep the selection visible, with muted "↑N/↓N more" hints when entries
    /// sit above or below the window. Results are memoized in `dir_cache`
    /// because `view()` runs every tick.
    fn dir_matches(
        &self,
        buffer: &str,
        dir_sel: usize,
        window: usize,
        on_pick: fn(String) -> Msg,
    ) -> Element<'_, Msg> {
        let entries = {
            let mut cache = self.dir_cache.borrow_mut();
            match cache.as_ref() {
                Some((k, v)) if k == buffer => v.clone(),
                _ => {
                    let v = crate::app::list_dirs(buffer);
                    *cache = Some((buffer.to_string(), v.clone()));
                    v
                }
            }
        };
        let total = entries.len();
        let shown = total.min(window);
        // Scroll the window so dir_sel stays visible.
        let start = dir_sel
            .saturating_sub(window - 1)
            .min(total.saturating_sub(window));
        let above = start;
        let below = total.saturating_sub(start + shown);
        let rows =
            shown + usize::from(above > 0) + usize::from(below > 0) + usize::from(total == 0);
        let mut matches_col = Column::new()
            .spacing(0)
            .height(Length::Fixed(rows.max(1) as f32 * ROW_H));
        if entries.is_empty() {
            matches_col = matches_col.push(
                container(text("No matches").size(12).color(c::FG_MUTE()))
                    .height(ROW_H)
                    .padding(Padding::from([0, 10]))
                    .align_y(iced::Alignment::Center),
            );
        } else {
            let more = |n: usize, arrow: char| {
                container(
                    text(format!("{arrow}{n} more"))
                        .size(11)
                        .color(c::FG_MUTE()),
                )
                .height(ROW_H)
                .padding(Padding::from([0, 10]))
                .align_y(iced::Alignment::Center)
            };
            if above > 0 {
                matches_col = matches_col.push(more(above, '↑'));
            }
            for (i, path) in entries.into_iter().skip(start).take(shown).enumerate() {
                let active = start + i == dir_sel;
                // Rows show just the directory name — the buffer above already
                // carries the parent path, and full paths wrap illegibly.
                let label = format!("{}/", crate::app::path_basename(&path));
                matches_col = matches_col.push(launcher_row(
                    text(label)
                        .font(UI_FONT)
                        .size(12)
                        .color(if active { c::FG() } else { c::FG_DIM() })
                        .wrapping(iced::widget::text::Wrapping::None),
                    active,
                    true,
                    on_pick(path),
                    ROW_H,
                ));
            }
            if below > 0 {
                matches_col = matches_col.push(more(below, '↓'));
            }
        }
        container(matches_col)
            .width(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                border: Border {
                    color: c::BORDER_SOFT(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            })
            .into()
    }

    /// The two-step add-project modal: pick a folder (native picker, drop, or
    /// typed path), then confirm the details with the git probe inline.
    #[allow(clippy::too_many_arguments)]
    fn add_project_modal<'a>(
        &'a self,
        step: AddProjectStep,
        path: &'a str,
        dir_sel: usize,
        name: &'a str,
        git: &'a GitProbe,
        init_git: bool,
        note: Option<&'a str>,
    ) -> Element<'a, Msg> {
        let accent = c::MAGENTA();
        let step_no = match step {
            AddProjectStep::PickSource => 1,
            AddProjectStep::Details => 2,
        };
        let header = modal_header_row(
            row![
                text("Add project").size(13).color(accent),
                Space::new().width(Length::Fill),
                text(format!("Step {step_no} of 2"))
                    .size(11)
                    .color(c::FG_MUTE()),
            ]
            .align_y(iced::Alignment::Center)
            .into(),
        );

        let mut body = column![].spacing(12);
        #[allow(unused_assignments)]
        let mut footer: Option<Element<'a, Msg>> = None;

        match step {
            AddProjectStep::PickSource => {
                // Hero action: a full-width primary Browse button with the
                // drop affordance as its caption.
                let accent_soft = Color { a: 0.45, ..accent };
                let browse = button(
                    container(
                        text(if self.picker_open {
                            "Waiting for the folder picker…"
                        } else {
                            "Browse for folder…"
                        })
                        .size(13),
                    )
                    .width(Length::Fill)
                    .align_x(iced::Alignment::Center),
                )
                .on_press(Msg::AddProjectBrowse)
                .width(Length::Fill)
                .padding(Padding::from([10, 12]))
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(Background::Color(if hovered {
                            c::BG_HOVER()
                        } else {
                            c::BG_HL()
                        })),
                        text_color: c::FG(),
                        border: Border {
                            color: if hovered { accent } else { accent_soft },
                            width: 1.0,
                            radius: Radius::from(5.0),
                        },
                        shadow: Shadow::default(),
                        snap: false,
                    }
                });
                let drop_hint = container(
                    text("Or drop a folder anywhere in this window")
                        .size(11)
                        .color(c::FG_MUTE()),
                )
                .width(Length::Fill)
                .align_x(iced::Alignment::Center);

                let or_divider = row![
                    container(divider_h(c::BORDER_SOFT())).width(Length::Fill),
                    text("Or type a path").size(11).color(c::FG_MUTE()),
                    container(divider_h(c::BORDER_SOFT())).width(Length::Fill),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center);

                let path_input = text_input("~/code/my-repo", path)
                    .id(modal_input_id())
                    .font(UI_FONT)
                    .size(13)
                    .padding(Padding::from([8, 12]))
                    .on_input(Msg::AddProjectPathChanged)
                    .on_submit(Msg::AddProjectChooseTyped)
                    .style(input_field_style);

                body = body
                    .push(Space::new().height(2))
                    .push(browse)
                    .push(drop_hint)
                    .push(Space::new().height(2))
                    .push(or_divider)
                    .push(path_input)
                    .push(self.dir_matches(path, dir_sel, 6, Msg::ModalPickDir));

                if let Some(note) = note {
                    body = body.push(text(note.to_string()).size(12).color(c::RED()));
                }
                body = body.push(
                    row![
                        Space::new().width(Length::Fill),
                        modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );
                footer = Some(modal_footer_hints(&[
                    ("tab", "complete"),
                    ("↑↓", "select"),
                    ("⏎", "continue"),
                    ("esc", "cancel"),
                ]));
            }
            AddProjectStep::Details => {
                let chip = container(
                    row![
                        icon("folder", 14.0, c::FG_DIM()),
                        text(path.to_string())
                            .size(12)
                            .color(c::FG())
                            .wrapping(iced::widget::text::Wrapping::None),
                        Space::new().width(Length::Fill),
                        modal_action("Change", ModalBtn::Plain, Msg::AddProjectChangeSource),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .padding(Padding::from([6, 10]))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG_STRIP())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                });

                let badge: Element<'a, Msg> = match git {
                    GitProbe::Repo { branch } => row![
                        icon("git", 14.0, c::GREEN()),
                        text(format!("Git repository · branch {branch}"))
                            .size(12)
                            .color(c::GREEN()),
                    ]
                    .spacing(7)
                    .align_y(iced::Alignment::Center)
                    .into(),
                    GitProbe::NotRepo => row![
                        icon("no-git", 14.0, c::AMBER()),
                        text("Not a git repository").size(12).color(c::AMBER()),
                    ]
                    .spacing(7)
                    .align_y(iced::Alignment::Center)
                    .into(),
                };

                // The placeholder is the default (folder basename); typing
                // overrides it without having to clear pre-filled text.
                let default_name = crate::app::path_basename(path);
                let name_input = text_input(&default_name, name)
                    .id(modal_name_id())
                    .font(UI_FONT)
                    .size(13)
                    .padding(Padding::from([8, 12]))
                    .on_input(Msg::AddProjectNameChanged)
                    .on_submit(Msg::AddProjectSubmit)
                    .style(input_field_style);

                body = body
                    .push(text("Folder").size(11).color(c::FG_MUTE()))
                    .push(chip)
                    .push(badge)
                    .push(
                        row![
                            text("Name").size(11).color(c::FG_MUTE()),
                            Space::new().width(Length::Fill),
                            text(format!("Empty uses '{default_name}'"))
                                .size(11)
                                .color(c::FG_MUTE()),
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .push(name_input);

                if matches!(git, GitProbe::NotRepo) {
                    body = body.push(modal_checkbox(
                        "Initialize Git repository".into(),
                        init_git,
                        accent,
                        Some(Msg::AddProjectToggleInitGit),
                    ));
                    if !init_git {
                        body = body.push(
                            text("Sessions will run directly in the project folder, no worktrees")
                                .size(11)
                                .color(c::FG_MUTE()),
                        );
                    }
                }
                if let Some(note) = note {
                    body = body.push(text(note.to_string()).size(12).color(c::RED()));
                }
                body = body.push(
                    row![
                        Space::new().width(Length::Fill),
                        modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                        modal_action("Add project", ModalBtn::Primary, Msg::AddProjectSubmit),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                );
                footer = Some(modal_footer_hints(&[("⏎", "add"), ("esc", "back")]));
            }
        }

        let mut panel_body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if let Some(footer) = footer {
            panel_body = panel_body.push(divider_h(c::BORDER_SOFT())).push(footer);
        }

        modal_panel(panel_body.into(), 640.0)
    }

    fn confirm_modal<'a>(
        &'a self,
        title: &'a str,
        prompt: &'a str,
        destructive: bool,
        kind: &'a ConfirmKind,
    ) -> Element<'a, Msg> {
        let accent = if destructive { c::RED() } else { c::MAGENTA() };
        let confirm_label = match kind {
            ConfirmKind::Quit => "Quit",
            _ if destructive => "Remove",
            _ => "Confirm",
        };
        let confirm_label_lower = match kind {
            ConfirmKind::Quit => "quit",
            _ if destructive => "remove",
            _ => "confirm",
        };
        let body_zone = column![
            text(prompt.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ModalConfirm(false)),
                modal_action(
                    confirm_label,
                    if destructive {
                        ModalBtn::Danger
                    } else {
                        ModalBtn::Primary
                    },
                    Msg::ModalConfirm(true)
                ),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let footer = if destructive {
            modal_footer_hints(&[("y", confirm_label_lower), ("esc", "cancel")])
        } else {
            modal_footer_hints(&[("⏎", "confirm"), ("esc", "cancel")])
        };

        let body = column![
            modal_header(title, accent),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            footer,
        ];

        modal_panel(body.into(), 480.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn remove_project_modal<'a>(
        &'a self,
        name: &'a str,
        worktrees: &'a [String],
        also_remove: bool,
        in_progress: bool,
        done: usize,
        current: &'a str,
        errors: &'a [String],
    ) -> Element<'a, Msg> {
        use iced::widget::progress_bar;
        use iced::widget::progress_bar::Style as ProgressStyle;

        let accent = c::RED();
        let total = worktrees.len();
        let prompt = if total == 0 {
            format!("'{name}' will be unregistered from Grove. Files on disk stay put.")
        } else {
            format!(
                "'{name}' will be unregistered from Grove. Non-main worktrees stay on disk unless you opt in below."
            )
        };
        let session_note = "Running sessions for this project will be stopped.";

        let mut body = column![
            text(prompt)
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            text(session_note)
                .size(12)
                .color(c::FG_MUTE())
                .wrapping(iced::widget::text::Wrapping::Word),
        ]
        .spacing(12);

        if total > 0 {
            let label = if total == 1 {
                "Delete 1 non-main worktree from disk".to_string()
            } else {
                format!("Delete {total} non-main worktrees from disk")
            };
            let cb = modal_checkbox(
                label,
                also_remove,
                c::RED(),
                if in_progress {
                    None
                } else {
                    Some(Msg::ToggleRemoveWorktrees)
                },
            );
            body = body
                .push(divider_h(c::BORDER_SOFT()))
                .push(Space::new().height(2))
                .push(cb);
        }

        if in_progress {
            let frac = if total == 0 {
                1.0
            } else {
                (done as f32 / total as f32).clamp(0.0, 1.0)
            };
            let status = if done >= total {
                "Finishing…".to_string()
            } else {
                format!("Removing {} of {}: {}", done + 1, total, current)
            };
            body = body
                .push(divider_h(c::BORDER_SOFT()))
                .push(Space::new().height(4))
                .push(
                    text(status)
                        .size(11)
                        .color(c::FG_MUTE())
                        .wrapping(iced::widget::text::Wrapping::None),
                )
                .push(
                    progress_bar(0.0..=1.0, frac)
                        .girth(6.0)
                        .style(|_| ProgressStyle {
                            background: Background::Color(c::BG_STRIP()),
                            bar: Background::Color(c::RED()),
                            border: Border {
                                color: c::BORDER(),
                                width: 1.0,
                                radius: Radius::from(4.0),
                            },
                        }),
                );
        } else {
            body = body.push(divider_h(c::BORDER_SOFT())).push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                    modal_action("Remove", ModalBtn::Danger, Msg::ConfirmRemoveProject),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
        }

        if !errors.is_empty() {
            let summary = format!("{} worktree(s) failed to remove", errors.len());
            body = body.push(
                text(summary)
                    .size(11)
                    .color(c::RED())
                    .wrapping(iced::widget::text::Wrapping::Word),
            );
        }

        let mut panel_body = column![
            modal_header("Remove project", accent),
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if !in_progress {
            panel_body = panel_body
                .push(divider_h(c::BORDER_SOFT()))
                .push(modal_footer_hints(&[
                    ("y", "remove"),
                    ("space", "toggle delete"),
                    ("esc", "cancel"),
                ]));
        }

        modal_panel(panel_body.into(), 520.0)
    }

    fn message_modal<'a>(&'a self, message: &'a str) -> Element<'a, Msg> {
        let body_zone = column![
            text(message.to_string())
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("Close", ModalBtn::Primary, Msg::ModalCancel),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            modal_header("Notice", c::CYAN()),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "close")]),
        ];

        modal_panel(body.into(), 480.0)
    }

    fn teardown_modal(&self) -> Element<'_, Msg> {
        use crate::app::TeardownStage;
        let td = match &self.app.teardown {
            Some(td) => td,
            None => return Space::new().width(0).into(),
        };
        let wt_name = crate::app::path_basename(&td.wt_path);
        let done = matches!(td.stage, TeardownStage::Done { .. });
        let running = matches!(td.stage, TeardownStage::RunningScript);

        let header = modal_header(&format!("Delete worktree / {wt_name}"), c::RED());

        let mut body = column![].spacing(12);

        // Embedded teardown-script PTY (read-only) while it runs / after it
        // exits, until removal completes and the session is dropped.
        if let Some(s) = &td.session {
            let pty = container(self.pty(PtyPane::Agent, s))
                .width(Length::Fill)
                .height(Length::Fixed(220.0))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::BG())),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                });
            body = body.push(pty);
        }

        body = body.push(
            text(td.message.clone())
                .size(13)
                .color(if done { c::FG_DIM() } else { c::FG_MUTE() })
                .wrapping(iced::widget::text::Wrapping::Word),
        );

        let buttons = if done {
            row![
                Space::new().width(Length::Fill),
                modal_action("Close", ModalBtn::Primary, Msg::ModalCancel),
            ]
        } else if running {
            // Let the user proceed without waiting for a hung teardown script.
            row![
                Space::new().width(Length::Fill),
                modal_action("Skip & remove", ModalBtn::Plain, Msg::ModalCancel),
            ]
        } else {
            row![Space::new().width(Length::Fill)]
        }
        .spacing(8)
        .align_y(iced::Alignment::Center);

        body = body.push(Space::new().height(4)).push(buttons);

        // Esc always dismisses here (`cancel_modal` gates by stage): skip &
        // remove while the teardown script runs, close once removal is done.
        // Mid-removal there's no dismissal (an in-flight `git worktree
        // remove` can't be safely interrupted), so the hint is omitted then.
        let footer = if done {
            Some(modal_footer_hints(&[("esc", "close")]))
        } else if running {
            Some(modal_footer_hints(&[("esc", "skip & remove")]))
        } else {
            None
        };

        let mut panel_body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if let Some(footer) = footer {
            panel_body = panel_body.push(divider_h(c::BORDER_SOFT())).push(footer);
        }

        modal_panel(panel_body.into(), 560.0)
    }

    /// Per-project modal: lifecycle scripts editor plus (new) the "Project
    /// theme" row. Still backed by `Modal::ScriptsEditor` / `self.scripts_editor`
    /// — only the presentation grew a second section.
    fn project_settings_modal(&self) -> Element<'_, Msg> {
        use super::state::ScriptField;
        let ed = match &self.scripts_editor {
            Some(ed) => ed,
            None => return Space::new().width(0).into(),
        };

        // ── PROJECT THEME ────────────────────────────────────────────────
        let themes_enabled = self.app.project_themes_enabled();
        let project = self.app.store.projects.get(ed.proj);
        // The pin itself always stays persisted regardless of the toggle —
        // but while Project themes is off nothing actually applies it, so
        // the displayed value must show "Default" rather than the stale
        // pinned name (which would otherwise look active when it isn't).
        let pinned_name = if themes_enabled {
            project.and_then(|p| p.theme.as_deref())
        } else {
            None
        };
        let value_text = pinned_name.unwrap_or("Default (follow app)");
        let value_color = if pinned_name.is_some() {
            c::CYAN()
        } else {
            c::FG_DIM()
        };

        let theme_row: Element<'_, Msg> = if themes_enabled {
            modal_list_row(
                row![
                    text("Project theme").size(12).color(c::FG()),
                    Space::new().width(Length::Fill),
                    text(value_text.to_string()).size(12).color(value_color),
                    Space::new().width(8),
                    icon("chev-right", 12.0, c::FG_MUTE()),
                ]
                .align_y(iced::Alignment::Center),
                false,
                Msg::OpenProjectThemePicker { proj: ed.proj },
            )
        } else {
            container(
                row![
                    text("Project theme").size(12).color(c::FG_MUTE()),
                    Space::new().width(Length::Fill),
                    text(value_text.to_string()).size(12).color(c::FG_MUTE()),
                ]
                .align_y(iced::Alignment::Center),
            )
            .height(ROW_H)
            .padding(Padding::from([0, 10]))
            .into()
        };
        let theme_caption = if themes_enabled {
            "Pin every PTY in this project to a specific theme"
        } else {
            "Enable Project themes in Settings to use this"
        };
        let project_theme_section = column![
            section_header("PROJECT THEME", 0.0, 0.0),
            Space::new().height(2),
            theme_row,
            container(text(theme_caption).size(11).color(c::FG_MUTE()))
                .padding(Padding::from([0, 10])),
        ]
        .spacing(4);

        let field = |label: &str, desc: &str, placeholder: &str, content, which: ScriptField| {
            // Shrink height grows the editor with its content (Iced sizes a
            // Shrink text_editor to its measured line count), so it never
            // scrolls internally — the outer scroll area absorbs any overflow.
            let editor = iced::widget::text_editor(content)
                .height(Length::Shrink)
                .font(iced::Font::MONOSPACE)
                .size(12)
                .padding(8)
                .placeholder(placeholder.to_string())
                .style(|_, status| {
                    use iced::widget::text_editor::Status;
                    // Cyan border on focus mirrors the modal accent and tells the
                    // user which field has keyboard focus without relying on color
                    // alone (the caret and selection move with it too).
                    let border_color = match status {
                        Status::Focused { .. } => c::CYAN(),
                        Status::Hovered => c::BORDER(),
                        _ => c::BORDER_SOFT(),
                    };
                    iced::widget::text_editor::Style {
                        background: Background::Color(c::BG_STRIP()),
                        border: Border {
                            color: border_color,
                            width: 1.0,
                            radius: Radius::from(4.0),
                        },
                        placeholder: c::FG_MUTE(),
                        value: c::FG(),
                        selection: c::BG_HL(),
                    }
                })
                .on_action(move |a| Msg::ScriptsEditorAction(which, a));
            column![
                text(label.to_string()).size(12).color(c::FG()),
                text(desc.to_string())
                    .size(11)
                    .color(c::FG_MUTE())
                    .wrapping(iced::widget::text::Wrapping::Word),
                editor,
            ]
            .spacing(5)
        };

        let fields = column![
            field(
                "Setup",
                "Runs once when a new worktree is created, inside the new worktree's directory. \
                 Use it to install dependencies, copy ignored env files, or start the services \
                 an agent needs before you begin working.",
                "npm install",
                &ed.setup,
                ScriptField::Setup,
            ),
            field(
                "Run",
                "Runs on demand when you press the play button (worktree row or session header). \
                 It opens an interactive terminal tab, so it suits dev servers, test watchers, \
                 or any command you want to watch and interact with.",
                "npm run dev",
                &ed.run,
                ScriptField::Run,
            ),
            field(
                "Teardown",
                "Runs when you delete the worktree, before it is removed from disk. Use it to \
                 stop services, tear down databases, or clean up anything setup created. \
                 Deletion proceeds once it exits.",
                "docker compose down",
                &ed.teardown,
                ScriptField::Teardown,
            ),
        ]
        .spacing(16);

        // The fields size to their content (min-height) and only scroll once
        // they exceed `max_height` — so on a tall enough window no scrollbar
        // appears at all.
        let scroll_area = container(
            ghost_scrollable(container(fields).padding(Padding::from([0, 10])))
                .height(Length::Shrink),
        )
        .max_height(480.0);

        let header = modal_header(
            &format!("Project Settings — {}", ed.project_name),
            c::CYAN(),
        );

        let body_zone = column![
            project_theme_section,
            column![
                section_header("LIFECYCLE SCRIPTS", 0.0, 0.0),
                Space::new().height(2),
                container(
                    text("Shell snippets shared by every worktree of this project, run via $SHELL -lc. Leave a field blank to disable that step.")
                        .size(11)
                        .color(c::FG_MUTE())
                        .wrapping(iced::widget::text::Wrapping::Word),
                )
                .padding(Padding::from([0, 10])),
                Space::new().height(4),
                scroll_area,
            ]
            .spacing(4),
            row![
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ScriptsEditorCancel),
                modal_action("Save", ModalBtn::Primary, Msg::ScriptsEditorSave),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "cancel")]),
        ];

        modal_panel(body.into(), 560.0)
    }

    fn tmux_choice_modal(&self) -> Element<'_, Msg> {
        let body_zone = column![
            text("Use tmux for new sessions? Existing sessions keep their current backend.")
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
            Space::new().height(8),
            row![
                Space::new().width(Length::Fill),
                modal_action("Native", ModalBtn::Plain, Msg::ChooseTmux(false)),
                modal_action("Tmux", ModalBtn::Primary, Msg::ChooseTmux(true)),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            modal_header("Session backend", c::CYAN()),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("⏎", "tmux"), ("n", "native"), ("esc", "close")]),
        ];

        modal_panel(body.into(), 480.0)
    }

    fn agent_picker_modal<'a>(
        &'a self,
        project: &'a str,
        wt_path: &'a str,
        sel: usize,
    ) -> Element<'a, Msg> {
        let wt_name = crate::app::path_basename(wt_path);
        let title = if project.is_empty() {
            format!("Start session / {wt_name}")
        } else {
            format!("Start session / {project} / {wt_name}")
        };

        const AGENT_ROW_H: f32 = 32.0;
        let mut list = Column::new().spacing(2);
        for (i, agent) in self.app.available_agents.iter().enumerate() {
            let active = i == sel;
            let is_default = self.app.store.default_agent == Some(*agent);
            let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
            let icon_slot = container(icon(agent.icon_name(), 16.0, icon_color))
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center);
            let label = row![
                icon_slot,
                text(cap(agent.label()))
                    .size(12)
                    .color(if active { c::FG() } else { c::FG_DIM() }),
                Space::new().width(Length::Fill),
                text(if is_default { "Default" } else { "" })
                    .size(11)
                    .color(c::FG_MUTE()),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);

            list = list.push(launcher_row(
                label,
                active,
                true,
                Msg::AgentPickerSelect(i),
                AGENT_ROW_H,
            ));
        }

        let list_zone = container(list).padding(8).width(Length::Fill);

        let body_zone = column![
            list_zone,
            Space::new().height(8),
            row![
                modal_action("Default", ModalBtn::Plain, Msg::AgentPickerToggleDefault),
                Space::new().width(Length::Fill),
                modal_action("Cancel", ModalBtn::Plain, Msg::ModalCancel),
                modal_action("Launch", ModalBtn::Primary, Msg::AgentPickerSubmit),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(12);

        let body = column![
            modal_header(&title, c::MAGENTA()),
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("↑↓", "choose"), ("⏎", "launch"), ("esc", "cancel")]),
        ];

        modal_panel(body.into(), 500.0)
    }

    /// Recents-first command palette (Agent View "+ New session", mod+n, grid
    /// pill). Three states driven by `Modal::SessionLauncher`: root (empty
    /// input, no options) shows recents + actions; typing/browse-all shows
    /// every project×worktree combo fuzzy-filtered by `input`; options shows
    /// the resolved row plus a plain list of agents to launch it with. Esc is
    /// the only way to close — no header, no close button.
    ///
    /// Zoned layout: input zone / 1px divider / list zone (fits content, up
    /// to a 380px cap, then scrolls) / 1px divider / footer hint strip — the
    /// footer's own bottom corners are rounded to stay flush with the panel.
    fn session_launcher_modal<'a>(
        &'a self,
        input: &'a str,
        selected: usize,
        browse_all: bool,
        options: Option<&'a LauncherOptions>,
        switch: Option<usize>,
        row_actions: Option<&'a crate::app::RowActionsState>,
        settings: Option<LauncherSettings>,
    ) -> Element<'a, Msg> {
        // A cue chip shell shared by the "options" and "switch to session"
        // states' leading slot: mono, cyan text over a soft cyan tint.
        let cue_chip = |label: &'static str| -> Element<'a, Msg> {
            container(text(label).font(MONO_FONT).size(10).color(c::CYAN()))
                .padding(Padding::from([2, 6]))
                .style(|_| container::Style {
                    background: Some(Background::Color(c::SEL_TINT_SOFT())),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                })
                .into()
        };
        // In options/switch/settings state, the leading glyph slot becomes a
        // static cue chip instead of the search icon; the typed text
        // underneath is unchanged.
        let leading: Element<'a, Msg> = if options.is_some() {
            cue_chip("options")
        } else if switch.is_some() {
            cue_chip("SWITCH TO SESSION")
        } else if let Some(ls) = settings {
            cue_chip(settings_pane_cue(&ls.pane))
        } else {
            icon("search", 16.0, c::FG_MUTE())
        };
        let placeholder = if switch.is_some() {
            "Filter sessions…"
        } else if let Some(ls) = settings {
            settings_pane_placeholder(&ls.pane)
        } else {
            "Search projects, worktrees, agents…"
        };
        let field = text_input(placeholder, input)
            .id(modal_input_id())
            .font(UI_FONT)
            .size(14)
            .padding(0)
            .on_input(Msg::LauncherInputChanged)
            .style(palette_input_style);
        let input_zone = container(
            row![leading, field]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([14, 16]));

        let mut body = column![input_zone, divider_h(c::BORDER_SOFT())];

        // The inline warning under a *selected* Permissions row (B3) — the
        // same string E1's Permissions pane promotes, one shade up from a
        // throwaway caption (11 · FG_DIM), left-padded past the 24px icon
        // slot so it aligns with the row's label column. Shared by the
        // drill-in Root list and the root/typing direct-match list.
        let danger_caption = || -> Element<'a, Msg> {
            container(
                text("Skip lets agents run any command without asking.")
                    .size(11)
                    .color(c::FG_DIM()),
            )
            .padding(Padding {
                top: 4.0,
                bottom: 2.0,
                left: 44.0,
                right: 12.0,
            })
            .into()
        };

        if let Some(ls) = settings {
            match ls.pane {
                SettingsPane::Root => {
                    // Settings drill-in root: every `SettingRow`, grouped
                    // under its 4 section headers (C1 in the palette
                    // redesign mock), fuzzy-filtered by `input` — headers
                    // for a section with zero remaining rows are dropped
                    // (C2). While `resizing` (D4), the App-size row's value
                    // slot swaps for the live zoom stepper.
                    let rows = self.settings_rows_filtered(input);
                    let list_zone: Element<'a, Msg> = if rows.is_empty() {
                        container(text("No matching settings").size(12).color(c::FG_MUTE()))
                            .padding(Padding::from([30, 16]))
                            .width(Length::Fill)
                            .align_x(iced::alignment::Horizontal::Center)
                            .into()
                    } else {
                        let mut list = Column::new().spacing(2);
                        let mut printed_section: Option<&'static str> = None;
                        for (i, s) in rows.iter().enumerate() {
                            let section = s.section();
                            if printed_section != Some(section) {
                                let top = if printed_section.is_none() { 0.0 } else { 12.0 };
                                list = list.push(section_header(section, top, 6.0));
                                printed_section = Some(section);
                            }
                            let active = i == ls.selected;
                            let content: Element<'a, Msg> =
                                if ls.resizing && *s == SettingRow::AppSize {
                                    self.appsize_stepper_row_content()
                                } else {
                                    self.setting_row_content(*s, input)
                                };
                            list = list.push(launcher_row(
                                content,
                                active,
                                true,
                                Msg::LauncherSettingActivate(i),
                                PALETTE_ROW_H,
                            ));
                            // Danger settings warn inline before you ever
                            // change them (B3, same string E1's pane
                            // promotes) — only under the selected row, so
                            // the list doesn't permanently grow a caption.
                            if active && *s == SettingRow::Permissions {
                                list = list.push(danger_caption());
                            }
                            // Update-available actions expand in place under
                            // the CheckUpdates row (E3). Guarded on the live
                            // upgrade state, not just the flag: SkipVersion
                            // (or a background re-check) can invalidate the
                            // strip while it's open.
                            if *s == SettingRow::CheckUpdates {
                                if let (Some(strip_sel), UpgradeState::Available(_)) =
                                    (ls.update_actions, &self.upgrade)
                                {
                                    list = list.push(self.update_actions_strip(strip_sel));
                                }
                            }
                        }
                        container(
                            ghost_scrollable(list)
                                .id(launcher_settings_scrollable_id())
                                .height(Length::Shrink),
                        )
                        .padding(8)
                        .max_height(380.0)
                        .width(Length::Fill)
                        .into()
                    };
                    body = body.push(list_zone);
                    body = body.push(divider_h(c::BORDER_SOFT()));
                    let footer_row: Element<'a, Msg> = if rows.is_empty() {
                        // Nothing to choose or change (E4) — only the way
                        // back is worth hinting.
                        row![footer_hint("esc", "back")]
                            .spacing(14)
                            .align_y(iced::Alignment::Center)
                            .into()
                    } else if ls.resizing {
                        row![
                            footer_hint("←/→", "adjust"),
                            footer_hint("0", "reset"),
                            footer_hint("⏎", "done"),
                            footer_hint("esc", "done"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into()
                    } else if ls.update_actions.is_some() {
                        row![
                            footer_hint("←→", "choose"),
                            footer_hint("⏎", "run"),
                            footer_hint("esc", "back"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into()
                    } else {
                        row![
                            footer_hint("↑↓", "choose"),
                            footer_hint("⏎", "change"),
                            footer_hint("esc", "back"),
                            Space::new().width(Length::Fill),
                            text("Changes save automatically.")
                                .size(11)
                                .color(c::FG_MUTE()),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into()
                    };
                    body = body.push(footer_container(footer_row));
                }
                SettingsPane::Theme {
                    kind,
                    follow_system,
                    ..
                } => {
                    // Theme sub-pane (D1): pinned context row + Dark/Light/
                    // System mode row above a fuzzy-filtered, live-previewing
                    // theme list — see `Grove::theme_pane_select`/
                    // `theme_pane_set_kind`/`theme_pane_set_system`.
                    let context_row = container(
                        row![
                            text("App theme").size(13).color(c::FG()),
                            Space::new().width(Length::Fill),
                            text(crate::theme::current().name.to_string())
                                .size(12)
                                .color(c::FG_DIM()),
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .height(PALETTE_ROW_H)
                    .padding(Padding::from([0.0, 12.0]))
                    .align_y(iced::Alignment::Center)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::BG_HL())),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(6.0),
                        },
                        ..Default::default()
                    });

                    let mode_seg = container(
                        row![
                            seg_button(
                                "Dark",
                                !follow_system && kind == crate::theme::ThemeKind::Dark,
                                SegSide::Left,
                                Msg::LauncherThemePaneDark,
                            ),
                            seg_button(
                                "Light",
                                !follow_system && kind == crate::theme::ThemeKind::Light,
                                SegSide::Mid,
                                Msg::LauncherThemePaneLight,
                            ),
                            seg_button(
                                "System",
                                follow_system,
                                SegSide::Right,
                                Msg::LauncherThemePaneSystem,
                            ),
                        ]
                        .spacing(0),
                    )
                    .style(|_| container::Style {
                        border: Border {
                            color: c::BORDER(),
                            width: 1.0,
                            radius: Radius::from(6.0),
                        },
                        ..Default::default()
                    });
                    let mode_row = container(
                        row![
                            text("Mode").size(11).color(c::FG_MUTE()),
                            Space::new().width(Length::Fill),
                            mode_seg,
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .padding(Padding::from([8, 12]));

                    let theme_rows = theme_pane_rows(kind, input);
                    let current_name = crate::theme::current().name;
                    let theme_list: Element<'a, Msg> = if theme_rows.is_empty() {
                        container(text("No matching themes").size(12).color(c::FG_MUTE()))
                            .padding(Padding::from([30, 16]))
                            .width(Length::Fill)
                            .align_x(iced::alignment::Horizontal::Center)
                            .into()
                    } else {
                        let mut list = Column::new().spacing(2);
                        for (i, t) in theme_rows.iter().enumerate() {
                            let active = i == ls.selected;
                            let m = (!input.is_empty()).then(|| {
                                crate::gui::launcher::fuzzy_match_indices(input, t.name, "", "")
                            });
                            let ranges: &[(usize, usize)] =
                                m.as_ref().map(|m| m.project.as_slice()).unwrap_or(&[]);
                            let label_el = highlighted_line(t.name, ranges, c::FG(), UI_FONT, 13.0);
                            let mut content = row![label_el]
                                .spacing(8)
                                .align_y(iced::Alignment::Center)
                                .push(Space::new().width(Length::Fill));
                            if t.name == current_name {
                                content = content.push(icon("check", 12.0, c::CYAN()));
                            }
                            list = list.push(launcher_row(
                                content,
                                active,
                                true,
                                Msg::LauncherThemePaneSelect(i),
                                36.0,
                            ));
                        }
                        container(
                            ghost_scrollable(list)
                                .id(launcher_theme_scrollable_id())
                                .height(Length::Shrink),
                        )
                        .max_height(280.0)
                        .width(Length::Fill)
                        .into()
                    };

                    body = body.push(
                        container(column![context_row, mode_row, theme_list].spacing(0))
                            .padding(8)
                            .width(Length::Fill),
                    );
                    body = body.push(divider_h(c::BORDER_SOFT()));
                    body = body.push(footer_container(
                        row![
                            footer_hint("↑↓", "preview"),
                            footer_hint("tab", "mode"),
                            footer_hint("⏎", "apply"),
                            footer_hint("esc", "back"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into(),
                    ));
                }
                SettingsPane::ProjectTheme {
                    proj,
                    kind,
                    preview,
                } => {
                    // Project theme sub-pane: same shape as the app Theme
                    // pane above, minus the System segment (a project
                    // override is always a concrete pick or "Use app
                    // theme") — see `Grove::project_theme_pane_select`/
                    // `_set_kind`.
                    let proj_name = self
                        .app
                        .store
                        .projects
                        .get(proj)
                        .map(|p| p.name.as_str())
                        .unwrap_or("(project removed)");
                    let context_row = container(
                        row![
                            text("Project theme").size(13).color(c::FG()),
                            Space::new().width(Length::Fill),
                            text(proj_name).size(12).color(c::FG_DIM()),
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .width(Length::Fill)
                    .height(PALETTE_ROW_H)
                    .padding(Padding::from([0.0, 12.0]))
                    .align_y(iced::Alignment::Center)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::BG_HL())),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(6.0),
                        },
                        ..Default::default()
                    });

                    let mode_seg = container(
                        row![
                            seg_button(
                                "Dark",
                                kind == crate::theme::ThemeKind::Dark,
                                SegSide::Left,
                                Msg::LauncherThemePaneDark,
                            ),
                            seg_button(
                                "Light",
                                kind == crate::theme::ThemeKind::Light,
                                SegSide::Right,
                                Msg::LauncherThemePaneLight,
                            ),
                        ]
                        .spacing(0),
                    )
                    .style(|_| container::Style {
                        border: Border {
                            color: c::BORDER(),
                            width: 1.0,
                            radius: Radius::from(6.0),
                        },
                        ..Default::default()
                    });
                    let mode_row = container(
                        row![
                            text("Mode").size(11).color(c::FG_MUTE()),
                            Space::new().width(Length::Fill),
                            mode_seg,
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .padding(Padding::from([8, 12]));

                    let rows = project_theme_pane_rows(kind, input);
                    let theme_list: Element<'a, Msg> = if rows.is_empty() {
                        container(text("No matching themes").size(12).color(c::FG_MUTE()))
                            .padding(Padding::from([30, 16]))
                            .width(Length::Fill)
                            .align_x(iced::alignment::Horizontal::Center)
                            .into()
                    } else {
                        let mut list = Column::new().spacing(2);
                        for (i, row_theme) in rows.iter().enumerate() {
                            let active = i == ls.selected;
                            let is_current = row_theme.map(|t| t.name) == preview.map(|t| t.name);
                            let content: Element<'a, Msg> = match row_theme {
                                Some(t) => {
                                    let m = (!input.is_empty()).then(|| {
                                        crate::gui::launcher::fuzzy_match_indices(
                                            input, t.name, "", "",
                                        )
                                    });
                                    let ranges: &[(usize, usize)] =
                                        m.as_ref().map(|m| m.project.as_slice()).unwrap_or(&[]);
                                    let label_el =
                                        highlighted_line(t.name, ranges, c::FG(), UI_FONT, 13.0);
                                    let mut c = row![label_el]
                                        .spacing(8)
                                        .align_y(iced::Alignment::Center)
                                        .push(Space::new().width(Length::Fill));
                                    if is_current {
                                        c = c.push(icon("check", 12.0, c::CYAN()));
                                    }
                                    c.into()
                                }
                                None => {
                                    let mut c =
                                        row![text("Use app theme").size(13).color(c::FG_MUTE())]
                                            .spacing(8)
                                            .align_y(iced::Alignment::Center)
                                            .push(Space::new().width(Length::Fill));
                                    if is_current {
                                        c = c.push(icon("check", 12.0, c::CYAN()));
                                    }
                                    c.into()
                                }
                            };
                            list = list.push(launcher_row(
                                content,
                                active,
                                true,
                                Msg::LauncherThemePaneSelect(i),
                                36.0,
                            ));
                        }
                        container(
                            ghost_scrollable(list)
                                .id(launcher_theme_scrollable_id())
                                .height(Length::Shrink),
                        )
                        .max_height(280.0)
                        .width(Length::Fill)
                        .into()
                    };

                    body = body.push(
                        container(column![context_row, mode_row, theme_list].spacing(0))
                            .padding(8)
                            .width(Length::Fill),
                    );
                    body = body.push(divider_h(c::BORDER_SOFT()));
                    let footer_row: Element<'a, Msg> = if rows.is_empty() {
                        row![footer_hint("esc", "back")]
                            .spacing(14)
                            .align_y(iced::Alignment::Center)
                            .into()
                    } else {
                        row![
                            footer_hint("↑↓", "preview"),
                            footer_hint("tab", "dark/light"),
                            footer_hint("⏎", "apply"),
                            footer_hint("esc", "back"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into()
                    };
                    body = body.push(footer_container(footer_row));
                }
                SettingsPane::Backend => {
                    // Binary enum picker (D2): no filtering, 2 fixed rows.
                    let tmux_on = self.app.use_tmux();
                    let current = if tmux_on { 1 } else { 0 };
                    let rows: [(&str, &str); 2] = [
                        ("Native", "spawn PTYs directly"),
                        ("Tmux", "sessions survive restarts"),
                    ];
                    let mut list = Column::new().spacing(2);
                    for (i, (label, desc)) in rows.iter().enumerate() {
                        let active = i == ls.selected;
                        let mut content = row![
                            text(*label).size(13).color(c::FG()),
                            text(format!("— {desc}")).size(11).color(c::FG_MUTE()),
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center)
                        .push(Space::new().width(Length::Fill));
                        if i == current {
                            content = content.push(icon("check", 12.0, c::CYAN()));
                        }
                        list = list.push(launcher_row(
                            content,
                            active,
                            true,
                            Msg::LauncherSettingsPaneActivate(i),
                            PALETTE_ROW_H,
                        ));
                    }
                    let note = container(
                        text("Applies to new sessions; running sessions keep their backend.")
                            .size(11)
                            .color(c::FG_MUTE()),
                    )
                    .padding(Padding::from([6, 12]));
                    body = body.push(
                        container(column![list, note].spacing(0))
                            .padding(8)
                            .width(Length::Fill),
                    );
                    body = body.push(divider_h(c::BORDER_SOFT()));
                    body = body.push(footer_container(
                        row![
                            footer_hint("↑↓", "choose"),
                            footer_hint("⏎", "apply"),
                            footer_hint("esc", "back"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into(),
                    ));
                }
                SettingsPane::Permissions => {
                    // Skip permissions confirms first (E1): no filtering, 2
                    // fixed rows; the highlighted Skip row promotes to a red
                    // wash + a warning caption instead of the usual cyan
                    // selection tint.
                    let skip_on = self.app.skip_permissions_enabled();
                    let current = if skip_on { 1 } else { 0 };
                    let rows: [(&str, &str); 2] = [
                        ("Ask", "agents ask before running commands"),
                        ("Skip", "run any command without asking"),
                    ];
                    let mut list = Column::new().spacing(2);
                    for (i, (label, desc)) in rows.iter().enumerate() {
                        let active = i == ls.selected;
                        let is_skip = i == 1;
                        let danger = is_skip && active;
                        let label_color = if danger { c::RED() } else { c::FG() };
                        let mut content = row![
                            text(*label).size(13).color(label_color),
                            text(format!("— {desc}")).size(11).color(c::FG_MUTE()),
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center)
                        .push(Space::new().width(Length::Fill));
                        if i == current {
                            content = content.push(icon("check", 12.0, c::CYAN()));
                        }
                        let msg = Msg::LauncherSettingsPaneActivate(i);
                        let row_el: Element<'a, Msg> = if danger {
                            button(
                                container(content)
                                    .width(Length::Fill)
                                    .height(PALETTE_ROW_H)
                                    .align_y(iced::Alignment::Center)
                                    .padding(Padding::from([0.0, 12.0])),
                            )
                            .on_press(msg)
                            .width(Length::Fill)
                            .padding(0)
                            .style(|_, _| button::Style {
                                background: Some(Background::Color(c::RED_WASH())),
                                text_color: c::FG(),
                                border: Border {
                                    color: Color::TRANSPARENT,
                                    width: 0.0,
                                    radius: Radius::from(6.0),
                                },
                                shadow: Shadow::default(),
                                snap: false,
                            })
                            .into()
                        } else {
                            launcher_row(content, active, true, msg, PALETTE_ROW_H)
                        };
                        list = list.push(row_el);
                    }
                    let mut pane_col = column![list].spacing(0);
                    if ls.selected == 1 {
                        pane_col = pane_col.push(
                            container(
                                text("Skip lets agents run any command without asking.")
                                    .size(11)
                                    .color(c::FG_DIM()),
                            )
                            .padding(Padding::from([6, 12])),
                        );
                    }
                    body = body.push(container(pane_col).padding(8).width(Length::Fill));
                    body = body.push(divider_h(c::BORDER_SOFT()));
                    body = body.push(footer_container(
                        row![
                            footer_hint("↑↓", "choose"),
                            footer_hint("⏎", "confirm"),
                            footer_hint("esc", "back"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into(),
                    ));
                }
                SettingsPane::DefaultAgent => {
                    // Default agent picker (D3): mirrors OPEN WITH's list —
                    // uninstalled tools are visible but inert (see
                    // `Grove::default_agent_pane_row_installed`).
                    let mut list = Column::new().spacing(2);
                    for (i, &agent) in crate::agent::Agent::ALL.iter().enumerate() {
                        let active = i == ls.selected;
                        let installed = self.default_agent_pane_row_installed(agent);
                        let is_default = self.app.store.default_agent == Some(agent);
                        let label_color = if installed { c::FG() } else { c::FG_MUTE() };
                        let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
                        let icon_slot = container(icon(agent.icon_name(), 16.0, icon_color))
                            .width(24.0)
                            .align_x(iced::alignment::Horizontal::Center);
                        let status_text = if agent == crate::agent::Agent::Terminal
                            || self.settings_tools.is_empty()
                        {
                            None
                        } else {
                            self.settings_tools
                                .iter()
                                .find(|t| t.agent == agent)
                                .map(|st| {
                                    if st.detecting {
                                        ("Detecting…".to_string(), c::FG_MUTE())
                                    } else if !st.installed {
                                        ("Not installed".to_string(), c::FG_MUTE())
                                    } else {
                                        (
                                            st.version
                                                .clone()
                                                .unwrap_or_else(|| "installed".to_string()),
                                            c::FG_DIM(),
                                        )
                                    }
                                })
                        };
                        let mut content = row![
                            icon_slot,
                            text(cap(agent.label())).size(13).color(label_color),
                        ]
                        .spacing(8)
                        .align_y(iced::Alignment::Center)
                        .push(Space::new().width(Length::Fill));
                        if let Some((text_s, color)) = status_text {
                            content = content.push(text(text_s).size(12).color(color));
                            content = content.push(Space::new().width(10));
                        }
                        if is_default {
                            content = content.push(slot_badge("Default"));
                            content = content.push(Space::new().width(6));
                            content = content.push(icon("check", 12.0, c::CYAN()));
                        }
                        list = list.push(launcher_row(
                            content,
                            active,
                            true,
                            Msg::LauncherSettingsPaneActivate(i),
                            36.0,
                        ));
                    }
                    body = body.push(container(list).padding(8).width(Length::Fill));
                    body = body.push(divider_h(c::BORDER_SOFT()));
                    body = body.push(footer_container(
                        row![
                            footer_hint("↑↓", "choose"),
                            footer_hint("⏎", "set default"),
                            footer_hint("esc", "back"),
                        ]
                        .spacing(14)
                        .align_y(iced::Alignment::Center)
                        .into(),
                    ));
                }
            }
        } else if let Some(sel) = switch {
            // "Switch to session" drill-in: every active session across
            // every project/worktree. Waiting sessions keep the sidebar's
            // amber tint/left bar; the currently-focused session's icon
            // renders yellow (same idiom as the recents' active-row accent).
            let session_ids = self.switch_session_rows(input);
            let list_zone: Element<'a, Msg> = if session_ids.is_empty() {
                container(text("No matching sessions").size(12).color(c::FG_MUTE()))
                    .padding(Padding::from([30, 16]))
                    .width(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .into()
            } else {
                let mut list = Column::new().spacing(2);
                for (i, &si) in session_ids.iter().enumerate() {
                    let Some(s) = self.app.sessions.get(si) else {
                        continue;
                    };
                    let highlighted = i == sel;
                    let waiting = matches!(
                        self.activity_state(s),
                        crate::gui::activity::ActivityState::WaitingForInput
                    );
                    let is_active = self.app.active_session == Some(si);
                    let icon_color = if is_active { c::YELLOW() } else { c::FG_MUTE() };
                    let label = if waiting {
                        format!("{} (waiting)", cap(s.agent.label()))
                    } else {
                        cap(s.agent.label())
                    };
                    let subtitle =
                        format!("{} / {}", s.project, crate::app::path_basename(&s.wt_path));
                    let icon_slot = container(icon(s.agent.icon_name(), 16.0, icon_color))
                        .width(24.0)
                        .align_x(iced::alignment::Horizontal::Center);
                    let title_color = if highlighted { c::FG() } else { c::FG_DIM() };
                    let mut content = row![
                        icon_slot,
                        column![
                            text(label).font(UI_FONT).size(13).color(title_color),
                            text(subtitle)
                                .font(MONO_FONT)
                                .size(10.5)
                                .color(c::FG_MUTE()),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center);
                    if highlighted {
                        content = content
                            .push(Space::new().width(Length::Fill))
                            .push(keycap_text("⏎", c::FG_DIM()));
                    }
                    let row_el = launcher_row(
                        content,
                        highlighted,
                        true,
                        Msg::LauncherSwitchSessionPick(si),
                        PALETTE_ROW_H,
                    );
                    // Waiting sessions keep the sidebar's amber tint + 3px
                    // left accent bar, same idiom as `rows.rs`'s waiting row.
                    let row_el = if waiting {
                        let tint = Color {
                            a: 0.12,
                            ..c::AMBER()
                        };
                        let bar: Element<'a, Msg> = container(
                            container(Space::new().width(3.0))
                                .width(3.0)
                                .height(Length::Fill)
                                .style(|_| container::Style {
                                    background: Some(Background::Color(c::AMBER())),
                                    ..Default::default()
                                }),
                        )
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(iced::Alignment::Start)
                        .into();
                        container(stack![row_el, bar])
                            .height(PALETTE_ROW_H)
                            .width(Length::Fill)
                            .style(move |_| container::Style {
                                background: Some(Background::Color(tint)),
                                border: Border {
                                    color: Color::TRANSPARENT,
                                    width: 0.0,
                                    radius: Radius::from(6.0),
                                },
                                ..Default::default()
                            })
                            .into()
                    } else {
                        row_el
                    };
                    list = list.push(row_el);
                }
                container(ghost_scrollable(list).height(Length::Shrink))
                    .padding(8)
                    .max_height(380.0)
                    .width(Length::Fill)
                    .into()
            };
            body = body.push(list_zone);
            body = body.push(divider_h(c::BORDER_SOFT()));
            body = body.push(footer_container(
                row![
                    footer_hint("↑↓", "choose"),
                    footer_hint("⏎", "switch"),
                    footer_hint("esc", "back"),
                ]
                .spacing(14)
                .into(),
            ));
        } else if let Some(r) = options {
            let worktrees = self.launcher_worktrees(r.proj);
            let agent = self
                .app
                .available_agents
                .get(r.agent)
                .copied()
                .unwrap_or(crate::agent::Agent::Terminal);
            let pname = self
                .app
                .store
                .projects
                .get(r.proj)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            let wt_name = worktrees
                .get(r.wt)
                .map(|w| {
                    if w.branch.is_empty() {
                        crate::app::path_basename(&w.path)
                    } else {
                        w.branch.clone()
                    }
                })
                .unwrap_or_default();
            let subtitle = format!("{pname} / {wt_name}");

            // Pinned context row: quiet, non-interactive — just the currently
            // selected agent's icon/label live-updating as ↑↓ moves.
            let context_row =
                container(self.palette_agent_content(agent, subtitle, &[], &[], None, true))
                    .width(Length::Fill)
                    .height(PALETTE_ROW_H)
                    .padding(Padding::from([0.0, 12.0]))
                    .align_y(iced::Alignment::Center)
                    .style(|_| container::Style {
                        background: Some(Background::Color(c::BG_HL())),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(6.0),
                        },
                        ..Default::default()
                    });

            // Plain agent list: one 36px row per available agent.
            let mut agent_list = Column::new().spacing(2);
            for (i, ag) in self.app.available_agents.iter().enumerate() {
                let active = i == r.agent;
                let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
                let icon_slot = container(icon(ag.icon_name(), 16.0, icon_color))
                    .width(24.0)
                    .align_x(iced::alignment::Horizontal::Center);
                let label_color = if active { c::FG() } else { c::FG_DIM() };
                let mut content = row![
                    icon_slot,
                    text(cap(ag.label()))
                        .font(UI_FONT)
                        .size(13)
                        .color(label_color),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(keycap_text("⏎", c::FG_DIM()));
                }
                agent_list = agent_list.push(launcher_row(
                    content,
                    active,
                    true,
                    Msg::LauncherOptionsPick(i),
                    36.0,
                ));
            }

            let list_zone = container(
                column![
                    context_row,
                    section_header("OPEN WITH", 12.0, 6.0),
                    agent_list,
                ]
                .spacing(0),
            )
            .padding(8)
            .width(Length::Fill);
            body = body.push(list_zone);
            body = body.push(divider_h(c::BORDER_SOFT()));
            body = body.push(footer_container(
                row![
                    footer_hint("↑↓", "choose"),
                    footer_hint("⏎", "launch"),
                    footer_hint("esc", "back"),
                ]
                .spacing(14)
                .into(),
            ));
        } else {
            let rows = self.palette_rows(input, browse_all);
            let zero_projects = self.app.store.projects.is_empty();
            let root_mode = input.is_empty() && !browse_all && !zero_projects;

            let list_zone: Element<'a, Msg> = if rows.is_empty() {
                container(text("No matches").size(12).color(c::FG_MUTE()))
                    .padding(Padding::from([30, 16]))
                    .width(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .into()
            } else {
                let mut list = Column::new().spacing(2);
                let mut printed_recent = false;
                let mut printed_actions = false;
                // Typed list only: settings matches sort first (see
                // `palette_rows`), and their presence labels the two groups —
                // SETTINGS above, SESSIONS below (B2 in the palette redesign
                // mock). A pure session list stays headerless as before.
                let has_settings = rows.iter().any(|r| matches!(r, PaletteRow::Setting(_)));
                let mut printed_settings = false;
                let mut printed_sessions = false;
                for (i, row) in rows.iter().enumerate() {
                    if root_mode {
                        let is_recent = matches!(row, PaletteRow::Recent { .. });
                        if is_recent && !printed_recent {
                            list = list.push(section_header("RECENT", 0.0, 6.0));
                            printed_recent = true;
                        } else if !is_recent && !printed_actions {
                            let top = if printed_recent { 12.0 } else { 0.0 };
                            list = list.push(section_header("ACTIONS", top, 6.0));
                            printed_actions = true;
                        }
                    } else if has_settings {
                        let is_setting = matches!(row, PaletteRow::Setting(_));
                        if is_setting && !printed_settings {
                            list = list.push(section_header("SETTINGS", 0.0, 6.0));
                            printed_settings = true;
                        } else if !is_setting && !printed_sessions {
                            let top = if printed_settings { 12.0 } else { 0.0 };
                            list = list.push(section_header("SESSIONS", top, 6.0));
                            printed_sessions = true;
                        }
                    }
                    list =
                        list.push(self.palette_row_view(i, row, i == selected, input, root_mode));
                    // Danger settings warn inline in the direct-match list
                    // too (B3), before the user ever drills in.
                    if i == selected && matches!(row, PaletteRow::Setting(SettingRow::Permissions))
                    {
                        list = list.push(danger_caption());
                    }
                    let row_identity = match row {
                        PaletteRow::Recent {
                            proj, wt_path, agent, ..
                        }
                        | PaletteRow::Combo {
                            proj, wt_path, agent, ..
                        } => Some((*proj, wt_path.as_str(), *agent)),
                        _ => None,
                    };
                    if let (Some((rp, rw, rag)), Some(ra)) = (row_identity, row_actions) {
                        if rp == ra.proj && rw == ra.wt_path && rag == ra.agent {
                            let is_main = self
                                .launcher_worktrees(rp)
                                .iter()
                                .find(|w| w.path == rw)
                                .map(|w| w.is_main)
                                .unwrap_or(false);
                            list = list
                                .push(self.palette_row_actions_strip(ra.proj, ra.action, is_main));
                        }
                    }
                }
                container(ghost_scrollable(list).height(Length::Shrink))
                    .padding(8)
                    .max_height(380.0)
                    .width(Length::Fill)
                    .into()
            };
            body = body.push(list_zone);
            body = body.push(divider_h(c::BORDER_SOFT()));
            body = body.push(if row_actions.is_some() {
                // Row-actions strip open: the footer reflects that sub-state
                // directly rather than the underlying highlighted row — ⏎
                // runs the selected strip action (e.g. "Delete worktree"),
                // not "open/launch".
                footer_container(
                    row![
                        footer_hint("↑↓", "choose"),
                        footer_hint("⏎", "run"),
                        footer_hint("esc", "back"),
                    ]
                    .spacing(14)
                    .into(),
                )
            } else {
                // Recent/Combo (project/worktree) rows expose the
                // tab->actions strip; settings rows get their own ⏎ verb
                // (toggle / open) with no tab hint at all, since tab just
                // mirrors enter there; every other row keeps the plain
                // tab->options hint.
                let highlighted = rows.get(selected);
                let highlighted_is_row = matches!(
                    highlighted,
                    Some(PaletteRow::Recent { .. }) | Some(PaletteRow::Combo { .. })
                );
                let setting_enter_label: Option<&'static str> = match highlighted {
                    Some(PaletteRow::Setting(
                        SettingRow::ProjectThemes | SettingRow::Telemetry,
                    )) => Some("toggle"),
                    Some(PaletteRow::Setting(_)) | Some(PaletteRow::Settings) => Some("open"),
                    _ => None,
                };
                // Row-highlighted footer orders tab before ⏎ ("navigate · tab
                // actions · ⏎ open/launch · close"); every other state keeps
                // ⏎ before tab ("navigate · ⏎ launch · tab options · close")
                // — matches the palette redesign mock's D1 vs. D2/D3 ordering.
                let mid: Element<'a, Msg> = if let Some(enter_label) = setting_enter_label {
                    row![footer_hint("⏎", enter_label)].spacing(14).into()
                } else if highlighted_is_row {
                    row![
                        footer_hint("tab", "actions"),
                        footer_hint("⏎", "open/launch"),
                    ]
                    .spacing(14)
                    .into()
                } else {
                    row![footer_hint("⏎", "launch"), footer_hint("tab", "options"),]
                        .spacing(14)
                        .into()
                };
                footer_container(
                    row![
                        footer_hint("↑↓", "navigate"),
                        mid,
                        footer_hint("esc", "close"),
                    ]
                    .spacing(14)
                    .into(),
                )
            });
        }

        let panel = container(body)
            .width(Length::Fixed(640.0))
            // Same 1px inset as `modal_panel`: keeps the footer strip from
            // painting over the panel's border.
            .padding(1.0)
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_RAIL())),
                text_color: Some(c::FG()),
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(12.0),
                },
                shadow: Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                    offset: Vector::new(0.0, 12.0),
                    blur_radius: 40.0,
                },
                ..Default::default()
            });

        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Top)
            .padding(Padding {
                top: 96.0,
                ..Default::default()
            })
            .into()
    }

    /// Icon (in a fixed 24px slot, so titles align across rows regardless of
    /// icon glyph width) + agent label + mono-muted "project / worktree"
    /// subtitle — the visual idiom shared by `Recent`/`Combo` rows and the
    /// options-state pinned context row (same idiom as `attention_dropdown`).
    /// `agent_ranges`/`subtitle_ranges` are typing-state fuzzy-match char
    /// ranges to render cyan (pass `&[]` where nothing should highlight, e.g.
    /// root-state `Recent` rows and the options-state context row). `trailing`,
    /// if given, right-aligns after a filling gap — the row's ⌘-digit or ⏎
    /// keycap.
    fn palette_agent_content<'a>(
        &'a self,
        agent: crate::agent::Agent,
        subtitle: String,
        agent_ranges: &[(usize, usize)],
        subtitle_ranges: &[(usize, usize)],
        trailing: Option<Element<'a, Msg>>,
        active: bool,
    ) -> Element<'a, Msg> {
        let title = cap(agent.label());
        let title_el = highlighted_line(&title, agent_ranges, c::FG(), UI_FONT, 13.0);
        let subtitle_el =
            highlighted_line(&subtitle, subtitle_ranges, c::FG_MUTE(), MONO_FONT, 10.5);
        // The agent glyph lights up yellow on the selected row (and the
        // options-state context row); resting rows keep it muted.
        let icon_color = if active { c::YELLOW() } else { c::FG_MUTE() };
        let icon_slot = container(icon(agent.icon_name(), 16.0, icon_color))
            .width(24.0)
            .align_x(iced::alignment::Horizontal::Center);

        let mut content = row![icon_slot, column![title_el, subtitle_el].spacing(2)]
            .spacing(8)
            .align_y(iced::Alignment::Center);
        if let Some(t) = trailing {
            content = content.push(Space::new().width(Length::Fill)).push(t);
        }
        content.into()
    }

    /// Row content shared by a root-mode `PaletteRow::Setting` match and a
    /// Settings-drill-in row: icon slot + label (cyan fuzzy-highlighted
    /// against `input`) + right-aligned live value, plus a trailing chevron
    /// on the rows that drill into a deeper level (the two toggles flip in
    /// place, so they go without).
    fn setting_row_content<'a>(&'a self, s: SettingRow, input: &str) -> Element<'a, Msg> {
        let label = s.label();
        let value = self.setting_value(s);
        let m = (!input.is_empty())
            .then(|| crate::gui::launcher::fuzzy_match_indices(input, label, &value, s.section()));
        let label_ranges: &[(usize, usize)] =
            m.as_ref().map(|m| m.project.as_slice()).unwrap_or(&[]);
        let label_el = highlighted_line(label, label_ranges, c::FG(), UI_FONT, 13.0);

        let icon_slot: Element<'a, Msg> =
            if matches!(s, SettingRow::ProjectThemes | SettingRow::Telemetry) {
                // A non-interactive checkbox glyph modeled on
                // `modal_checkbox`'s checked-state border/background colors
                // (`settings_modal`'s own toggle rows) — the whole row is
                // already the click target here, so this isn't the real
                // `checkbox` widget, just its visual idiom.
                let checked = value == "On";
                let box_el = container(if checked {
                    icon("check", 10.0, c::MAGENTA())
                } else {
                    Space::new().width(10.0).height(10.0).into()
                })
                .width(14.0)
                .height(14.0)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::Alignment::Center)
                .style(move |_| container::Style {
                    background: Some(Background::Color(if checked {
                        c::BG_HL()
                    } else {
                        c::BG()
                    })),
                    border: Border {
                        color: if checked { c::MAGENTA() } else { c::BORDER() },
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    ..Default::default()
                });
                container(box_el)
                    .width(24.0)
                    .align_x(iced::alignment::Horizontal::Center)
                    .into()
            } else {
                container(icon(s.icon_name(), 16.0, c::FG_MUTE()))
                    .width(24.0)
                    .align_x(iced::alignment::Horizontal::Center)
                    .into()
            };

        // Async status renders inline in the value slot (E2): CheckUpdates
        // mirrors `settings_modal`'s status line — a spinner while checking,
        // green once a release is known to be available. Every other state
        // (and every other setting) keeps the plain FG_DIM value.
        let value_el: Element<'a, Msg> = if s == SettingRow::CheckUpdates {
            match &self.upgrade {
                UpgradeState::Checking => row![
                    super::icons::spinner(11.0, c::FG_MUTE(), self.blink_tick),
                    Space::new().width(6),
                    text(value).size(12).color(c::FG_MUTE()),
                ]
                .align_y(iced::Alignment::Center)
                .into(),
                UpgradeState::Available(_) => text(value).size(12).color(c::GREEN()).into(),
                _ => text(value).size(12).color(c::FG_DIM()).into(),
            }
        } else {
            text(value).size(12).color(c::FG_DIM()).into()
        };

        let mut content = row![
            icon_slot,
            label_el,
            Space::new().width(Length::Fill),
            value_el,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        // The chevron promises a deeper level — toggles flip in place and
        // never open one, so they don't get it.
        if !matches!(s, SettingRow::ProjectThemes | SettingRow::Telemetry) {
            content =
                content
                    .push(Space::new().width(8))
                    .push(icon("chev-right", 12.0, c::FG_MUTE()));
        }
        content.into()
    }

    /// The App-size row's value slot while `LauncherSettings::resizing` is
    /// set (D4): the same live zoom stepper trio `settings_modal` uses
    /// (view.rs's own `app_size_row`), instead of `setting_row_content`'s
    /// usual right-aligned value + chevron.
    fn appsize_stepper_row_content<'a>(&'a self) -> Element<'a, Msg> {
        let icon_slot = container(icon(SettingRow::AppSize.icon_name(), 16.0, c::FG_MUTE()))
            .width(24.0)
            .align_x(iced::alignment::Horizontal::Center);
        let stepper = container(
            row![
                control_icon_btn("minus", Msg::ZoomOut, 20.0, 13.0),
                control_btn_sized(
                    format!("{:.0}%", self.ui_zoom * 100.0),
                    Msg::ZoomReset,
                    12,
                    2
                ),
                control_icon_btn("plus", Msg::ZoomIn, 20.0, 13.0),
            ]
            .spacing(0)
            .align_y(iced::Alignment::Center),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        row![
            icon_slot,
            text(SettingRow::AppSize.label()).size(13).color(c::FG()),
            Space::new().width(Length::Fill),
            stepper,
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    }

    /// The update-available actions strip expanded under the drill-in's
    /// Check-for-updates row (E3): the same pill-button treatment
    /// `settings_modal`'s update-actions row uses (`modal_action_sized`'s
    /// visual language), laid out horizontally, plus a cyan selection ring
    /// on the keyboard-selected action (←→/Tab move it — the palette rows'
    /// ↑↓ stay reserved for the list cursor). The action list comes from
    /// `update_available_actions` so the render and the keyboard nav can
    /// never disagree about what index N runs.
    fn update_actions_strip<'a>(&'a self, sel: usize) -> Element<'a, Msg> {
        let method_unknown = matches!(self.upgrade_method, crate::upgrade::InstallMethod::Unknown);
        let mut strip = row![].spacing(8).align_y(iced::Alignment::Center);
        for (i, action) in update_available_actions(method_unknown)
            .into_iter()
            .enumerate()
        {
            let active = i == sel;
            let primary = matches!(action, UpdateAction::UpdateNow);
            strip = strip.push(
                button(text(action.label()).size(11))
                    .on_press(Msg::LauncherUpdateActionPick(i))
                    .padding(Padding::from([5, 12]))
                    .style(move |_, status| {
                        let hovered = matches!(status, button::Status::Hovered);
                        let bg = if active {
                            c::SEL_TINT_SOFT()
                        } else if hovered {
                            c::BG_HOVER()
                        } else if primary {
                            c::BG_HL()
                        } else {
                            c::BG()
                        };
                        button::Style {
                            background: Some(Background::Color(bg)),
                            text_color: if active || primary {
                                c::FG()
                            } else {
                                c::FG_DIM()
                            },
                            border: Border {
                                color: if active { c::SEL_RING() } else { c::BORDER() },
                                width: 1.0,
                                radius: Radius::from(4.0),
                            },
                            shadow: Shadow::default(),
                            snap: false,
                        }
                    }),
            );
        }
        container(strip)
            .padding(Padding {
                top: 4.0,
                bottom: 4.0,
                left: 12.0,
                right: 12.0,
            })
            .width(Length::Fill)
            .into()
    }

    /// Render one row of the root/typing/browse-all list. `input` recomputes
    /// the typing-state fuzzy-match highlight ranges for `Combo` rows
    /// (root-state `Recent` rows never highlight, since the query is empty
    /// there); `root_mode` gates the ⌘-digit chip on `Recent` rows — hidden
    /// while typing/browsing, per the redesign. Every row, active or not,
    /// swaps its natural trailing chip (digit / ⌘T) for a ⏎ keycap when it's
    /// the current selection.
    fn palette_row_view<'a>(
        &'a self,
        i: usize,
        row: &super::update::PaletteRow,
        active: bool,
        input: &str,
        root_mode: bool,
    ) -> Element<'a, Msg> {
        let enter_chip = || keycap_text("⏎", c::FG_DIM());
        // Action rows share the session rows' 24px icon rail so titles align.
        let icon_slot = |name: &'static str, color: Color| {
            container(icon(name, 16.0, color))
                .width(24.0)
                .align_x(iced::alignment::Horizontal::Center)
        };
        match row {
            PaletteRow::Recent {
                proj,
                wt_path,
                agent,
            }
            | PaletteRow::Combo {
                proj,
                wt_path,
                agent,
            } => {
                let pname = self
                    .app
                    .store
                    .projects
                    .get(*proj)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                let wt_name = self
                    .launcher_worktrees(*proj)
                    .iter()
                    .find(|w| &w.path == wt_path)
                    .map(|w| {
                        if w.branch.is_empty() {
                            crate::app::path_basename(&w.path)
                        } else {
                            w.branch.clone()
                        }
                    })
                    .unwrap_or_else(|| crate::app::path_basename(wt_path));
                let subtitle = format!("{pname} / {wt_name}");
                let is_recent = matches!(row, PaletteRow::Recent { .. });

                let m = (!input.is_empty()).then(|| {
                    crate::gui::launcher::fuzzy_match_indices(
                        input,
                        &pname,
                        &wt_name,
                        agent.label(),
                    )
                });
                let agent_ranges: &[(usize, usize)] =
                    m.as_ref().map(|m| m.agent.as_slice()).unwrap_or(&[]);
                // The subtitle is "{pname} / {wt_name}"; the worktree match's
                // ranges (computed against `wt_name` alone) need shifting by
                // that prefix's char length to land in the right place.
                let prefix_len = pname.chars().count() + 3;
                let subtitle_ranges: Vec<(usize, usize)> = m
                    .as_ref()
                    .map(|m| {
                        m.project
                            .iter()
                            .copied()
                            .chain(
                                m.worktree
                                    .iter()
                                    .map(|(s, e)| (s + prefix_len, e + prefix_len)),
                            )
                            .collect()
                    })
                    .unwrap_or_default();

                let trailing = if active {
                    Some(enter_chip())
                } else if is_recent && root_mode {
                    digit_label(i).map(|d| mod_key_chip(d, c::FG_MUTE()))
                } else {
                    None
                };

                launcher_row(
                    self.palette_agent_content(
                        *agent,
                        subtitle,
                        agent_ranges,
                        &subtitle_ranges,
                        trailing,
                        active,
                    ),
                    active,
                    true,
                    Msg::LauncherActivate(i),
                    PALETTE_ROW_H,
                )
            }
            PaletteRow::NewSession => {
                let mut content = row![
                    icon_slot("plus", c::MAGENTA()),
                    text("New session…").size(13).color(c::MAGENTA()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(content, active, Msg::LauncherActivate(i), 36.0, 6.0, 12.0)
            }
            PaletteRow::TerminalHome => {
                let content = row![
                    icon_slot("term", c::FG_MUTE()),
                    text("Terminal at ~").size(13).color(if active {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    }),
                    Space::new().width(Length::Fill),
                    if active {
                        enter_chip()
                    } else {
                        mod_key_chip("t", c::FG_DIM())
                    },
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                modal_list_row_sized(content, active, Msg::LauncherActivate(i), 36.0, 6.0, 12.0)
            }
            PaletteRow::TerminalWt => {
                let label = self
                    .app
                    .active_session
                    .and_then(|si| self.app.sessions.get(si))
                    .map(|s| {
                        format!(
                            "Terminal in {}/{}",
                            s.project,
                            crate::app::path_basename(&s.wt_path)
                        )
                    })
                    .unwrap_or_else(|| "Terminal in worktree".to_string());
                let mut content = row![
                    icon_slot("term", c::FG_MUTE()),
                    text(label)
                        .size(13)
                        .color(if active { c::FG() } else { c::FG_DIM() }),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(content, active, Msg::LauncherActivate(i), 36.0, 6.0, 12.0)
            }
            PaletteRow::AddProject => {
                let mut content = row![
                    icon_slot("plus", c::MAGENTA()),
                    text("Add project…").size(13).color(c::MAGENTA()),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content
                        .push(Space::new().width(Length::Fill))
                        .push(enter_chip());
                }
                modal_list_row_sized(content, active, Msg::LauncherActivate(i), 36.0, 6.0, 12.0)
            }
            PaletteRow::SwitchToSession => {
                // Neutral FG (never magenta — that's reserved for create
                // actions): a "swap sessions" idiom via the restart glyph,
                // plus a tab-hint chip and chevron drill-in affordance.
                // Outside zen the row is visible but inert — forced muted
                // regardless of keyboard highlight, with a "zen only" hint
                // in place of the tab/chevron affordance; Enter/Tab on it
                // are swallowed (see `launcher_activate`/
                // `launcher_enter_row_actions`).
                let switchable = self.switch_to_session_active();
                let label_color = if !switchable {
                    c::FG_MUTE()
                } else if active {
                    c::FG()
                } else {
                    c::FG_DIM()
                };
                let icon_color = if switchable {
                    c::FG_DIM()
                } else {
                    c::FG_MUTE()
                };
                let mut content = row![
                    icon_slot("restart", icon_color),
                    text("Switch to session…").size(13).color(label_color),
                    Space::new().width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if !switchable {
                    content = content.push(
                        text("zen only")
                            .font(MONO_FONT)
                            .size(10.5)
                            .color(c::FG_MUTE()),
                    );
                } else if active {
                    content = content.push(enter_chip());
                } else {
                    content = content.push(keycap_text("tab", c::FG_MUTE())).push(icon(
                        "chev-right",
                        12.0,
                        c::FG_MUTE(),
                    ));
                }
                modal_list_row_sized(content, active, Msg::LauncherActivate(i), 36.0, 6.0, 12.0)
            }
            PaletteRow::Settings => {
                // Unlike the other ACTIONS rows, this one shows no ⏎ chip
                // when selected — Tab (not Enter, though Enter also works;
                // see `launcher_activate`) is the primary gesture into the
                // drill-in, so the selected row surfaces a "tab" keycap
                // instead (B1 in the palette redesign mock).
                let mut content = row![
                    icon_slot("cog", c::FG_MUTE()),
                    text("Settings…")
                        .size(13)
                        .color(if active { c::FG() } else { c::FG_DIM() }),
                    Space::new().width(Length::Fill),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);
                if active {
                    content = content.push(keycap_text("tab", c::FG_DIM()));
                }
                modal_list_row_sized(content, active, Msg::LauncherActivate(i), 36.0, 6.0, 12.0)
            }
            PaletteRow::Setting(s) => {
                let content = self.setting_row_content(*s, input);
                launcher_row(
                    content,
                    active,
                    true,
                    Msg::LauncherActivate(i),
                    PALETTE_ROW_H,
                )
            }
        }
    }

    /// The inline contextual-action strip revealed by Tab under a
    /// highlighted `Recent`/`Combo` row: "Launch session…" (magenta, plus
    /// OPEN WITH agent picker) and "Delete worktree" (red, trash). `action`
    /// (`0`/`1`) is the currently-selected action within the strip.
    /// The inline row-actions strip. `is_main` selects the second action:
    /// the project's default/base checkout can't be deleted (`start_delete`
    /// bounces it to a "can't remove the project's main checkout" message),
    /// so its strip offers "Create worktree…" there instead of "Delete
    /// worktree". `0.0` left/right padding here is deliberate — the strip
    /// must render exactly as wide as the highlighted row card above it, and
    /// `modal_list_row_sized`'s own row buttons are already `Length::Fill`
    /// with their own internal `pad_x`, so any outer horizontal padding here
    /// would inset the strip relative to that row. Any configured lifecycle
    /// scripts (setup/run/teardown) are appended after the theme row, via
    /// `row_action_scripts`.
    fn palette_row_actions_strip<'a>(
        &'a self,
        proj: usize,
        action: usize,
        is_main: bool,
    ) -> Element<'a, Msg> {
        let icon_slot = |name: &'static str, color: Color| {
            container(icon(name, 13.0, color))
                .width(20.0)
                .align_x(iced::alignment::Horizontal::Center)
        };
        let action_row = |idx: usize, name: &'static str, label: &'static str, color: Color| {
            let active = idx == action;
            let content = row![icon_slot(name, color), text(label).size(12).color(color),]
                .spacing(8)
                .align_y(iced::Alignment::Center);
            modal_list_row_sized(
                content,
                active,
                Msg::LauncherRowActionPick(idx),
                30.0,
                4.0,
                12.0,
            )
        };
        let second = if is_main {
            action_row(1, "plus", "Create worktree…", c::MAGENTA())
        } else {
            action_row(1, "trash", "Delete worktree", c::RED())
        };
        let mut rows = column![
            action_row(0, "play", "Launch session…", c::MAGENTA()),
            second
        ]
        .spacing(1);
        if self.app.project_themes_enabled() {
            // "contrast" mirrors `SettingRow::Theme::icon_name()` — the app
            // theme row's own icon, reused here since this is the same idea
            // scoped to one project.
            rows = rows.push(action_row(2, "contrast", "Project theme…", c::CYAN()));
        }
        let base = if self.app.project_themes_enabled() {
            3
        } else {
            2
        };
        for (i, (kind, _)) in self.row_action_scripts(proj).into_iter().enumerate() {
            let (label, color) = match kind {
                "setup" => ("Setup script", c::GREEN()),
                "run" => ("Run script", c::CYAN()),
                "teardown" => ("Teardown script", c::AMBER()),
                _ => continue,
            };
            rows = rows.push(action_row(base + i, "play", label, color));
        }
        container(rows)
            .padding(Padding {
                top: 0.0,
                bottom: 4.0,
                left: 0.0,
                right: 0.0,
            })
            .width(Length::Fill)
            .into()
    }

    fn settings_modal(&self) -> Element<'_, Msg> {
        use iced::Alignment::Center;

        // A muted, indented one-liner used under section headers and rows to
        // explain what a control does (throwaway caption: 11 · regular ·
        // fg-mute).
        let caption = |s: &'static str| -> Element<'_, Msg> {
            container(text(s).size(11).color(c::FG_MUTE()))
                .padding(Padding::from([0, 10]))
                .into()
        };
        // One shade up from a throwaway caption — reserved for the single
        // safety-relevant caption (skip-permissions).
        let caption_promoted = |s: &'static str| -> Element<'_, Msg> {
            container(text(s).size(11).color(c::FG_DIM()))
                .padding(Padding::from([0, 10]))
                .into()
        };
        // The "Default" badge and "Set default" button share an identical
        // footprint (fixed width, same padding/radius) so the action-cell
        // column stays aligned regardless of which state a row is in.
        const SLOT_W: f32 = 84.0;
        let slot_badge = |label: &'static str| -> Element<'_, Msg> {
            container(
                text(label)
                    .size(11)
                    .color(c::FG())
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fill),
            )
            .width(Length::Fixed(SLOT_W))
            .padding(Padding::from([4, 12]))
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_HL())),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            })
            .into()
        };
        let slot_action = |label: &'static str, msg: Msg| -> Element<'_, Msg> {
            button(
                text(label)
                    .size(11)
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fill),
            )
            .on_press(msg)
            .width(Length::Fixed(SLOT_W))
            .padding(Padding::from([4, 12]))
            .style(|_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: if hovered {
                        Some(Background::Color(c::BG_HOVER()))
                    } else {
                        Some(Background::Color(c::BG()))
                    },
                    text_color: c::FG_DIM(),
                    border: Border {
                        color: c::BORDER(),
                        width: 1.0,
                        radius: Radius::from(4.0),
                    },
                    shadow: Shadow::default(),
                    snap: false,
                }
            })
            .into()
        };
        // Missing tools reserve the same fixed-width, same-padding footprint
        // as a real slot but render nothing, so the column of badges/buttons
        // above and below it doesn't shift.
        let slot_none = || -> Element<'_, Msg> {
            container(Space::new().width(Length::Fill))
                .width(Length::Fixed(SLOT_W))
                .padding(Padding::from([4, 12]))
                .into()
        };

        // ── header ─────────────────────────────────────────────────────────
        let header = modal_header_row(
            row![
                text("Settings").size(13).color(c::MAGENTA()),
                Space::new().width(Length::Fill),
                text("Changes save automatically.")
                    .size(11)
                    .color(c::FG_MUTE()),
                Space::new().width(10),
                icon_btn("close", Msg::ModalCancel),
            ]
            .align_y(Center)
            .into(),
        );

        // ── appearance ───────────────────────────────────────────────────────
        let theme_row = modal_list_row(
            row![
                text("App theme").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                text(crate::theme::current().name.to_string())
                    .size(12)
                    .color(c::FG_DIM()),
                Space::new().width(8),
                icon("chev-right", 12.0, c::FG_MUTE()),
            ]
            .align_y(Center),
            false,
            Msg::OpenThemePicker,
        );

        let zoom = container(
            row![
                control_icon_btn("minus", Msg::ZoomOut, 20.0, 13.0),
                control_btn_sized(
                    format!("{:.0}%", self.ui_zoom * 100.0),
                    Msg::ZoomReset,
                    12,
                    2
                ),
                control_icon_btn("plus", Msg::ZoomIn, 20.0, 13.0),
            ]
            .spacing(0)
            .align_y(Center),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        let app_size_row = container(
            row![
                text("App size").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                zoom,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let project_themes_row = container(modal_checkbox(
            "Project themes".into(),
            self.app.project_themes_enabled(),
            c::MAGENTA(),
            Some(Msg::ProjectThemesToggle),
        ))
        .height(ROW_H)
        .align_y(Center)
        .padding(Padding::from([0, 10]));

        let appearance = column![
            section_header("APPEARANCE", 0.0, 0.0),
            Space::new().height(2),
            theme_row,
            app_size_row,
            project_themes_row,
            caption("Let each project pin its PTYs to a specific theme"),
        ]
        .spacing(4);

        // ── agents / terminal ────────────────────────────────────────────
        let tmux_on = self.app.use_tmux();
        let backend_seg = container(
            row![
                seg_button("Native", !tmux_on, SegSide::Left, Msg::BackendNative),
                seg_button("Tmux", tmux_on, SegSide::Right, Msg::BackendTmux),
            ]
            .spacing(0),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });
        let backend_row = container(
            row![
                text("Backend").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                backend_seg,
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let skip_perms_on = self.app.skip_permissions_enabled();
        let skip_perms_row = container(
            row![
                text("Permissions").size(12).color(c::FG()),
                Space::new().width(Length::Fill),
                skip_perms_seg(
                    skip_perms_on,
                    Msg::SkipPermissionsEnable,
                    Msg::SkipPermissionsDisable
                ),
            ]
            .align_y(Center),
        )
        .height(ROW_H)
        .padding(Padding::from([0, 10]));

        let telemetry_row = container(modal_checkbox(
            "Share anonymous usage data".into(),
            self.app.telemetry_enabled(),
            c::MAGENTA(),
            Some(Msg::TelemetryToggle),
        ))
        .height(ROW_H)
        .align_y(Center)
        .padding(Padding::from([0, 10]));

        let agents_terminal = column![
            section_header("AGENTS / TERMINAL", 0.0, 0.0),
            Space::new().height(2),
            backend_row,
            skip_perms_row,
            caption_promoted("Skip lets agents run any command without asking."),
            telemetry_row,
        ]
        .spacing(4);

        // ── tools ─────────────────────────────────────────────────────────
        let tools_header = container(
            row![
                section_header("TOOLS", 0.0, 0.0),
                Space::new().width(Length::Fill),
                icon_btn("restart", Msg::RefreshTools),
            ]
            .align_y(Center),
        )
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 0.0,
            right: 10.0,
        });

        let mut tools = Column::new().spacing(0);
        for st in &self.settings_tools {
            // Install state is carried by shape as well as color so it survives
            // grayscale: a filled ● (green) for installed, a hollow ○ (muted)
            // for missing — both at the app's 7px status-dot diameter.
            let status_dot: Element<'_, Msg> = if st.installed {
                dot(c::GREEN())
            } else {
                container(Space::new().width(7))
                    .width(7)
                    .height(7)
                    .style(|_| container::Style {
                        border: Border {
                            color: c::FG_MUTE(),
                            width: 1.0,
                            radius: Radius::from(3.5),
                        },
                        ..Default::default()
                    })
                    .into()
            };
            // Missing tools recede: dim the label and mute the status. Present
            // tools keep full-strength labels; version numbers read as data —
            // status text stays FG_MUTE (not FG_DIM, which is reserved for
            // live values like version strings).
            let (status, status_color) = if st.detecting {
                ("Detecting…".to_string(), c::FG_MUTE())
            } else if !st.installed {
                ("Not installed".to_string(), c::FG_MUTE())
            } else {
                (
                    st.version
                        .clone()
                        .unwrap_or_else(|| "installed".to_string()),
                    c::FG_DIM(),
                )
            };
            let label_color = if st.installed { c::FG() } else { c::FG_DIM() };
            let agent_label = cap(st.agent.label());
            let is_default = self.app.store.default_agent == Some(st.agent);
            let selector: Element<'_, Msg> = if is_default {
                // The chosen default reads as a selected control (filled
                // highlight), not a category tag — magenta stays reserved for
                // the modal's identity accent.
                slot_badge("Default")
            } else if st.installed {
                slot_action("Set default", Msg::SetDefaultAgent(st.agent))
            } else {
                slot_none()
            };
            let row = container(
                row![
                    status_dot,
                    Space::new().width(8),
                    icon(st.agent.icon_name(), 14.0, label_color),
                    Space::new().width(8),
                    text(agent_label).size(12).color(label_color),
                    Space::new().width(Length::Fill),
                    text(status).size(12).color(status_color),
                    Space::new().width(16),
                    selector,
                ]
                .align_y(Center),
            )
            .height(ROW_H)
            .padding(Padding::from([0, 10]));
            tools = tools.push(row);
        }

        let tools_section = column![tools_header, Space::new().height(2), tools].spacing(4);

        // ── body (scrolls once content exceeds the cap) ─────────────────────
        let sections = column![
            appearance,
            divider_h(c::BORDER_SOFT()),
            agents_terminal,
            divider_h(c::BORDER_SOFT()),
            tools_section,
        ]
        .spacing(8);

        let scroll_cap = (self.window_size.height - 220.0).max(160.0);
        let scroll_body = container(ghost_scrollable(sections)).max_height(scroll_cap);

        // ── updates — the version/status strip merges into the shared
        // footer chrome below; update-available actions and the release
        // notes preview stay in the body, right under the scroll area. ────
        let current_ver = env!("CARGO_PKG_VERSION");
        let status_line: Element<'_, Msg> = match &self.upgrade {
            UpgradeState::Idle => text("Not checked yet").size(11).color(c::FG_MUTE()).into(),
            UpgradeState::Checking => row![
                super::icons::spinner(11.0, c::FG_MUTE(), self.blink_tick),
                Space::new().width(6),
                text("Checking…").size(11).color(c::FG_MUTE()),
            ]
            .align_y(Center)
            .into(),
            UpgradeState::UpToDate => text("Up to date").size(11).color(c::FG_DIM()).into(),
            UpgradeState::Error(e) => text(format!("Check failed: {e}"))
                .size(11)
                .color(c::FG_MUTE())
                .into(),
            UpgradeState::Available(r) => text(format!("Update available: {}", r.tag))
                .size(11)
                .color(c::GREEN())
                .into(),
            // Updating/Updated/UpdateFailed are shown in the progress modal.
            _ => text("Updating…").size(11).color(c::FG_DIM()).into(),
        };
        let refresh: Element<'_, Msg> = if matches!(self.upgrade, UpgradeState::Checking) {
            container(super::icons::spinner(12.0, c::FG_MUTE(), self.blink_tick)).into()
        } else {
            icon_btn("restart", Msg::CheckForUpdates { manual: true })
        };

        let mut extra = column![].spacing(4);
        if let UpgradeState::Available(r) = &self.upgrade {
            let mut actions = row![].spacing(8).align_y(Center);
            // Hide "update now" for Unknown installs (notify-only).
            if !matches!(self.upgrade_method, crate::upgrade::InstallMethod::Unknown) {
                actions = actions.push(modal_action_sized(
                    "Update now",
                    ModalBtn::Primary,
                    11,
                    Msg::StartUpdate,
                ));
            }
            actions = actions.push(modal_action_sized(
                "Skip version",
                ModalBtn::Plain,
                11,
                Msg::SkipVersion,
            ));
            // No opener crate exists in this codebase; offer the URL as a
            // clipboard action instead of dead text.
            actions = actions.push(modal_action_sized(
                "Copy URL",
                ModalBtn::Plain,
                11,
                Msg::CopyReleaseUrl,
            ));
            extra = extra.push(Space::new().height(2)).push(actions);

            if !r.body.is_empty() {
                let truncated: String = r
                    .body
                    .lines()
                    .take(6)
                    .collect::<Vec<_>>()
                    .join("\n")
                    .chars()
                    .take(300)
                    .collect();
                extra = extra
                    .push(Space::new().height(4))
                    .push(text(truncated).size(11).color(c::FG_MUTE()));
            }
        }

        let body_zone = column![scroll_body, extra].spacing(10);

        // The version/status strip merges into the shared footer chrome,
        // with an [esc] close hint trailing on the right.
        let footer = modal_footer_row(
            row![
                text(format!("v{current_ver}")).size(11).color(c::FG_DIM()),
                status_line,
                refresh,
                Space::new().width(Length::Fill),
                modal_action_sized("View changelog", ModalBtn::Plain, 11, Msg::OpenChangelog),
                Space::new().width(10),
                footer_hint("esc", "close"),
            ]
            .spacing(10)
            .align_y(Center)
            .into(),
        );

        let body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body_zone).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            footer,
        ];

        modal_panel(body.into(), 580.0)
    }

    /// Two-column keyboard-shortcut reference (mod+/). On macOS the ⌘ is
    /// rendered as the SVG `command` icon (the bundled fonts have no
    /// U+2318 glyph); elsewhere key labels stay plain text via
    /// `platform_mod_label()`.
    fn shortcut_overlay_modal(&self) -> Element<'_, Msg> {
        let m = platform_mod_label();
        // Alt-chord rows layer Alt on top of the platform modifier instead of
        // using it plain, e.g. "cmd+alt+n" / "ctrl+alt+n" (never
        // "ctrl+shift+alt+n" — see `requires_alt` on `ShortcutDef`).
        let alt_m = if cfg!(target_os = "macos") {
            "cmd+alt"
        } else {
            "ctrl+alt"
        };
        let key_label = |d: &ShortcutDef| {
            if d.literal {
                // Already the complete chord text (e.g. the terminal-panel
                // resize, which is Ctrl+Shift on every platform, not `mod`).
                d.display_keys.to_string()
            } else if d.requires_alt {
                format!("{alt_m}+{}", d.display_keys)
            } else {
                format!("{m}+{}", d.display_keys)
            }
        };
        let screen = self.current_screen();

        // Registry entries visible on this screen: Global or matching current screen.
        let visible: Vec<&ShortcutDef> = SHORTCUTS
            .iter()
            .filter(|d| super::update::scope_allows(d.scopes, screen))
            .collect();

        // Does the visible set span more than one scope? (Global vs current-screen)
        let has_global = visible.iter().any(|d| d.scopes.contains(&Scope::Global));
        let has_screen = visible
            .iter()
            .any(|d| d.scopes.contains(&Scope::Screen(screen)));
        let grouped = has_global && has_screen;

        // Static display-only rows the behavioral registry deliberately omits.
        let static_rows: [(String, &'static str); 2] = [
            (format!("{m}+c / {m}+v"), "Copy / paste in session"),
            ("esc".into(), "Close modals"),
        ];

        // Render a key-chord string as an Element, swapping any "cmd"
        // occurrence for the SVG ⌘ icon on macOS (with the "+" right after
        // it dropped, e.g. "cmd+alt+n" -> ⌘ "alt+n", "cmd+c / cmd+v" ->
        // ⌘ "c / " ⌘ "v"). Non-mac and literal chords (no "cmd" substring)
        // render unchanged as plain text.
        let chord_keys = |keys: &str| -> Element<'_, Msg> {
            if !cfg!(target_os = "macos") || !keys.contains("cmd") {
                return keycap_text(keys.to_string(), c::FG_DIM());
            }
            let mut parts = keys.split("cmd");
            let mut els: Vec<Element<'_, Msg>> = Vec::new();
            if let Some(first) = parts.next() {
                if !first.is_empty() {
                    els.push(
                        text(first.to_string())
                            .font(MONO_FONT)
                            .size(11)
                            .color(c::FG_DIM())
                            .into(),
                    );
                }
            }
            for part in parts {
                els.push(icon("command", 10.0, c::FG_DIM()));
                let rest = part.strip_prefix('+').unwrap_or(part);
                if !rest.is_empty() {
                    els.push(
                        text(rest.to_string())
                            .font(MONO_FONT)
                            .size(11)
                            .color(c::FG_DIM())
                            .into(),
                    );
                }
            }
            keycap(
                Row::with_children(els)
                    .spacing(1)
                    .align_y(iced::Alignment::Center)
                    .into(),
            )
        };

        let make_row = |keys: String, desc: &'static str| {
            row![
                container(chord_keys(&keys)).width(Length::Fixed(170.0)),
                text(desc).size(11).color(c::FG_DIM()),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
        };

        // Split a flat list of (keys, desc) rows into the two-column layout.
        let two_columns = |rows: Vec<(String, &'static str)>| {
            let mut cols = row![].spacing(24);
            if rows.is_empty() {
                return cols; // chunks(0) would panic on an empty list
            }
            let half = rows.len().div_ceil(2);
            for chunk in rows.chunks(half) {
                let mut col = Column::new().spacing(6);
                for (keys, desc) in chunk {
                    col = col.push(make_row(keys.clone(), desc));
                }
                cols = cols.push(col.width(Length::FillPortion(1)));
            }
            cols
        };

        let mut body = column![].spacing(12);

        if grouped {
            // Global section: registry Global rows + the static copy/paste/esc rows.
            let mut global_rows: Vec<(String, &'static str)> = visible
                .iter()
                .filter(|d| d.scopes.contains(&Scope::Global))
                .map(|d| (key_label(*d), d.description))
                .collect();
            for (keys, desc) in static_rows.iter() {
                global_rows.push((keys.clone(), desc));
            }
            // Screen section: registry rows scoped to the current screen.
            let screen_rows: Vec<(String, &'static str)> = visible
                .iter()
                .filter(|d| d.scopes.contains(&Scope::Screen(screen)))
                .map(|d| (key_label(*d), d.description))
                .collect();

            if !global_rows.is_empty() {
                body = body.push(section_header("GLOBAL", 0.0, 0.0));
                body = body.push(two_columns(global_rows));
            }
            if !screen_rows.is_empty() {
                body = body.push(section_header(&screen.label().to_uppercase(), 0.0, 0.0));
                body = body.push(two_columns(screen_rows));
            }
        } else {
            // Single scope (all-Global today): render a flat, headerless list, one
            // shortcut per row, derived straight from the registry. (The old
            // hand-authored overlay combined a couple of related pairs onto single
            // lines; we keep the registry as the sole source of order and text
            // rather than re-introducing a parallel display layout.)
            let mut rows: Vec<(String, &'static str)> = visible
                .iter()
                .map(|d| (key_label(*d), d.description))
                .collect();
            for (keys, desc) in static_rows.iter() {
                rows.push((keys.clone(), desc));
            }
            body = body.push(two_columns(rows));
        }

        let panel_body = column![
            modal_header("Keyboard shortcuts", c::MAGENTA()),
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "close")]),
        ];

        modal_panel(panel_body.into(), 640.0)
    }

    fn updating_modal(&self) -> Element<'_, Msg> {
        use iced::Alignment::Center;

        let header = modal_header("Updating Grove", c::MAGENTA());

        // Keys are blocked while the update is in flight (see
        // `Modal::Updating` in update.rs), so no footer hint appears then;
        // once it lands on Updated/Failed, Esc is wired to dismiss.
        let footer = match &self.upgrade {
            UpgradeState::Updating(_) => None,
            UpgradeState::Updated => Some(modal_footer_hints(&[("esc", "later")])),
            UpgradeState::UpdateFailed(_) => Some(modal_footer_hints(&[("esc", "close")])),
            _ => None,
        };

        let body: Element<'_, Msg> = match &self.upgrade {
            UpgradeState::Updating(stage) => {
                let label = match stage {
                    crate::upgrade::Stage::Downloading => "Downloading…",
                    crate::upgrade::Stage::Building => "Building…",
                    crate::upgrade::Stage::Installing => "Installing…",
                    crate::upgrade::Stage::Done => "Finishing…",
                };
                row![
                    super::icons::spinner(16.0, c::FG_DIM(), self.blink_tick),
                    Space::new().width(10),
                    text(label).size(12).color(c::FG()),
                ]
                .align_y(Center)
                .into()
            }
            UpgradeState::Updated => column![
                text("Update installed. Restart Grove to apply")
                    .size(12)
                    .color(c::FG()),
                Space::new().height(10),
                row![
                    modal_action("Restart", ModalBtn::Primary, Msg::RestartApp),
                    Space::new().width(8),
                    modal_action("Later", ModalBtn::Plain, Msg::ModalCancel),
                ]
                .align_y(Center),
            ]
            .into(),
            UpgradeState::UpdateFailed(e) => column![
                text("Update failed").size(12).color(c::FG()),
                Space::new().height(6),
                text(e.clone()).size(11).color(c::FG_MUTE()),
                Space::new().height(10),
                modal_action("Close", ModalBtn::Plain, Msg::ModalCancel),
            ]
            .into(),
            _ => text("Updating…").size(12).color(c::FG_DIM()).into(),
        };

        let mut content = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(body).padding(Padding::from([14, 16])),
        ];
        if let Some(footer) = footer {
            content = content.push(divider_h(c::BORDER_SOFT())).push(footer);
        }
        modal_panel(content.into(), 420.0)
    }

    fn theme_picker_modal(
        &self,
        sel_dark: usize,
        sel_light: usize,
        tab: crate::theme::ThemeKind,
        follow_system: bool,
        scope: crate::app::ThemePickerScope,
        project_use_default: bool,
    ) -> Element<'_, Msg> {
        use crate::app::ThemePickerScope;
        let is_project = matches!(scope, ThemePickerScope::Project(_));
        let themes = crate::theme::themes_of(tab);
        let sel = match tab {
            crate::theme::ThemeKind::Dark => sel_dark,
            crate::theme::ThemeKind::Light => sel_light,
        };

        // Same segmented control as the appbar backend switch and the sidebar
        // view switch — one vocabulary for "choose one of N".
        let tabs = container(
            row![
                seg_button(
                    "Dark",
                    matches!(tab, crate::theme::ThemeKind::Dark),
                    SegSide::Left,
                    Msg::ThemePickerSwitchTab,
                ),
                seg_button(
                    "Light",
                    matches!(tab, crate::theme::ThemeKind::Light),
                    SegSide::Right,
                    Msg::ThemePickerSwitchTab,
                ),
            ]
            .spacing(0),
        )
        .style(|_| container::Style {
            border: Border {
                color: c::BORDER(),
                width: 1.0,
                radius: Radius::from(6.0),
            },
            ..Default::default()
        });

        let mut list = Column::new().spacing(0);
        if is_project {
            list = list.push(modal_list_row(
                text("Default (follow app)")
                    .size(12)
                    .color(if project_use_default {
                        c::FG()
                    } else {
                        c::FG_DIM()
                    }),
                project_use_default,
                Msg::ThemePickerSelectDefault,
            ));
        }
        for (i, th) in themes.iter().enumerate() {
            let active = i == sel && !(is_project && project_use_default);
            let name = th.name.to_string();
            list = list.push(modal_list_row(
                text(name)
                    .size(12)
                    .color(if active { c::FG() } else { c::FG_DIM() }),
                active,
                Msg::ThemePickerSelect(i),
            ));
        }

        let list_h = ((themes.len() + if is_project { 1 } else { 0 }).min(12) as f32) * ROW_H;
        let scroller = container(ghost_scrollable(list).id(theme_picker_scrollable_id()))
            .width(Length::Fill)
            .height(Length::Fixed(list_h))
            .style(|_| container::Style {
                background: Some(Background::Color(c::BG_STRIP())),
                border: Border {
                    color: c::BORDER(),
                    width: 1.0,
                    radius: Radius::from(4.0),
                },
                ..Default::default()
            });

        let title = match &scope {
            ThemePickerScope::App => "Theme".to_string(),
            ThemePickerScope::Project(name) => {
                // Resolve by name (projects are keyed by unique name, not a
                // stable index) — fall back gracefully if it was removed
                // while the picker was open.
                let still_exists = self.app.store.projects.iter().any(|p| &p.name == name);
                if still_exists {
                    format!("Project theme — {name}")
                } else {
                    "Project theme".to_string()
                }
            }
        };

        let mut body = column![].spacing(12);
        if !is_project {
            body = body.push(modal_checkbox(
                "Follow system appearance".into(),
                follow_system,
                c::MAGENTA(),
                Some(Msg::ThemePickerToggleSystem),
            ));
        }
        body = body
            .push(tabs)
            .push(scroller)
            .push(Space::new().height(8))
            .push(
                row![
                    Space::new().width(Length::Fill),
                    modal_action("Cancel", ModalBtn::Plain, Msg::ThemePickerCancel),
                    modal_action("Apply", ModalBtn::Primary, Msg::ThemePickerSubmit),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );

        let panel_body = column![
            modal_header(&title, c::MAGENTA()),
            container(body).padding(Padding::from([16, 20])),
        ];

        modal_panel(panel_body.into(), 460.0)
    }

    /// The first-run onboarding wizard: a full-viewport page (no modal
    /// chrome, no sidebar/statusbar/scrim behind it) that walks the user
    /// through four steps in grove's own quiet visual language. `view()`
    /// returns this directly while `Modal::Onboarding` is active, bypassing
    /// the modal layer entirely (see the top of `view()`).
    ///
    /// Layout: one hard left axis. Rail, wordmark/tagline, bullets,
    /// headings, descriptions, labels and inputs all sit flush to the left
    /// edge of a fixed 560px column, which is itself horizontally centered
    /// in the viewport. The column sits slightly above true center via
    /// proportional spacers (44/56 split) rather than `center_y`.
    fn onboarding_view<'a>(
        &'a self,
        step: OnboardStep,
        path: &'a str,
        dir_sel: usize,
        name: Option<&'a str>,
        note: Option<&'a str>,
        agent_sel: usize,
        perms_skip: bool,
    ) -> Element<'a, Msg> {
        use iced::Alignment::Center;

        // Entrance animation: eases 0 → 1 over `.quick()` (200ms, `EaseOut`)
        // whenever the step changes (and on first show). Drives a fade
        // (text/dot alpha) and an 8px settle (top padding on the centered
        // column) — see `Grove::onb_step_anim`.
        let t = self
            .onb_step_anim
            .interpolate(0.0_f32, 1.0_f32, std::time::Instant::now());
        let slide_pad = 8.0 * (1.0 - t);
        let fg = Color { a: t, ..c::FG() };
        let fg_dim = Color {
            a: t,
            ..c::FG_DIM()
        };

        // ── progress rail ───────────────────────────────────────────────────
        let mut rail = Row::new().spacing(10).align_y(Center);
        for &s in OnboardStep::flow() {
            let (dotc, txtc) = if s == step {
                (
                    Color {
                        a: t,
                        ..c::MAGENTA()
                    },
                    c::FG(),
                )
            } else if s.index_in() < step.index_in() {
                (c::MAGENTA(), c::FG_DIM())
            } else {
                (c::BORDER(), c::FG_MUTE())
            };
            rail = rail.push(
                row![dot(dotc), text(s.label()).size(10).color(txtc)]
                    .spacing(5)
                    .align_y(Center),
            );
        }

        // ── step body ────────────────────────────────────────────────────────
        let body: Element<'_, Msg> = match step {
            OnboardStep::Welcome => column![
                row![
                    icon("grid", 32.0, Color { a: t, ..c::CYAN() }),
                    text("grove").size(32).font(UI_BOLD).color(fg),
                ]
                .spacing(10)
                .align_y(Center),
                text("a worktree launchpad for AI coding agents")
                    .size(15)
                    .color(fg_dim),
                Space::new().height(20),
                onboard_point(
                    "Sessions are the unit of work",
                    "Every agent you spawn lives in a managed session that survives navigation; switch between them in two keystrokes.",
                ),
                onboard_point(
                    "Worktrees, not branches",
                    "Grove treats Git worktrees as a first-class primitive: create, list, and run agents inside them.",
                ),
                onboard_point(
                    "Quiet and keyboard-first",
                    "The app stays out of the way so terminal output stays primary. This takes about a minute.",
                ),
            ]
            .spacing(10)
            .into(),

            OnboardStep::Environment => {
                let mut list = Column::new().spacing(6);
                let rows = [
                    (on_path("git"), false, "Git", "Version control"),
                    (
                        crate::agent::Agent::Claude.available(),
                        false,
                        "Claude",
                        "Claude Code",
                    ),
                    (
                        crate::agent::Agent::Codex.available(),
                        false,
                        "Codex",
                        "Codex CLI",
                    ),
                    (
                        crate::agent::Agent::OpenCode.available(),
                        false,
                        "OpenCode",
                        "OpenCode CLI",
                    ),
                    (
                        self.app.tmux_available,
                        true,
                        "tmux",
                        "Persists sessions across restarts",
                    ),
                ];
                for (found, optional, n, meta) in rows {
                    list = list.push(onboard_env_row(found, optional, n, meta));
                }
                column![
                    text("Environment").size(20).color(fg),
                    text("Grove spawns agents from your PATH; it doesn't install or authenticate them. Only Git is required to get going.")
                        .size(13)
                        .color(fg_dim)
                        .wrapping(iced::widget::text::Wrapping::Word),
                    Space::new().height(4),
                    list,
                ]
                .spacing(10)
                .into()
            }

            OnboardStep::Project => {
                let path_input = text_input("~/code/my-repo", path)
                    .id(modal_input_id())
                    .font(UI_FONT)
                    .size(14)
                    .padding(Padding::from([8, 12]))
                    .on_input(Msg::OnbPathChanged)
                    .on_submit(Msg::OnbNext)
                    .style(input_field_style);

                let browse = modal_action(
                    if self.picker_open {
                        "Waiting…"
                    } else {
                        "Browse…"
                    },
                    ModalBtn::Plain,
                    Msg::AddProjectBrowse,
                );

                let mut col = column![
                    text("Add your first project").size(20).color(fg),
                    text("Point Grove at a Git repository, or any plain folder for ad-hoc sessions.")
                        .size(13)
                        .color(fg_dim)
                        .wrapping(iced::widget::text::Wrapping::Word),
                    // iced has no letter-spacing; the gaps are literal
                    // characters (single space between letters, three
                    // between words) — copied verbatim from the mock.
                    text("R E P O S I T O R Y   O R   F O L D E R")
                        .size(11)
                        .color(c::FG_MUTE()),
                    row![path_input, browse]
                        .spacing(8)
                        .align_y(Center),
                ]
                .spacing(8);

                // Matches appear only once the user starts typing; an empty
                // field would list the cwd's directories as noise.
                if !path.trim().is_empty() {
                    col = col
                        .push(text("M A T C H E S").size(11).color(c::FG_MUTE()))
                        .push(self.dir_matches(path, dir_sel, 5, Msg::OnbPickDir));
                }

                if let Some(name) = name {
                    let name_input = text_input("project name", name)
                        .id(modal_name_id())
                        .font(UI_FONT)
                        .size(14)
                        .padding(Padding::from([8, 12]))
                        .on_input(Msg::OnbNameChanged)
                        .on_submit(Msg::OnbNext)
                        .style(input_field_style);
                    col = col
                        .push(text("N A M E").size(11).color(c::FG_MUTE()))
                        .push(name_input);
                }

                if let Some(note) = note {
                    col = col.push(text(note.to_string()).size(12).color(c::RED()));
                }
                col = col.push(
                    text("Tab to complete · ↑↓ to select · Enter to continue · Or skip setup")
                        .size(11)
                        .color(c::FG_MUTE()),
                );
                col.into()
            }

            OnboardStep::Session => {
                let mut col = column![text("Start your first session").size(20).color(fg),]
                    .spacing(8);

                match self.app.store.projects.last() {
                    Some(p) => {
                        col = col.push(
                            text(format!("Launch an agent inside {}.", p.name))
                                .size(13)
                                .color(fg_dim)
                                .wrapping(iced::widget::text::Wrapping::Word),
                        );
                        let mut list = Column::new().spacing(0);
                        for (i, agent) in self.app.available_agents.iter().enumerate() {
                            let active = i == agent_sel;
                            list = list.push(modal_list_row(
                                text(cap(agent.label()))
                                    .size(13)
                                    .color(if active { c::FG() } else { c::FG_DIM() }),
                                active,
                                Msg::OnbAgentSelect(i),
                            ));
                        }
                        let list_h = (self.app.available_agents.len().max(1) as f32) * ROW_H;
                        col = col.push(
                            container(list)
                                .width(Length::Fill)
                                .height(Length::Fixed(list_h))
                                .style(|_| container::Style {
                                    background: Some(Background::Color(c::BG_STRIP())),
                                    border: Border {
                                        color: c::BORDER(),
                                        width: 1.0,
                                        radius: Radius::from(4.0),
                                    },
                                    ..Default::default()
                                }),
                        );
                    }
                    None => {
                        col = col.push(
                            text("No project added. You can add one any time from the sidebar. Finish to start using Grove.")
                                .size(13)
                                .color(fg_dim)
                                .wrapping(iced::widget::text::Wrapping::Word),
                        );
                    }
                }
                col = col
                    .push(Space::new().height(4))
                    .push(
                        row![
                            text("P E R M I S S I O N S").size(11).color(c::FG_MUTE()),
                            Space::new().width(20),
                            skip_perms_seg(
                                perms_skip,
                                Msg::OnbPermsSelect(true),
                                Msg::OnbPermsSelect(false)
                            ),
                        ]
                        .align_y(Center),
                    )
                    .push(
                        text(if perms_skip {
                            "Skip: agents run any command without asking"
                        } else {
                            "Safe: agents ask before running commands"
                        })
                        .size(11)
                        .color(if perms_skip { c::YELLOW() } else { c::FG_MUTE() }),
                    );
                col.into()
            }
        };

        // ── footer ────────────────────────────────────────────────────────────
        let next_label = match step {
            OnboardStep::Welcome => "Get started",
            OnboardStep::Session => "Launch session",
            _ => "Continue",
        };
        let count = format!("{} / {}", step.index_in() + 1, OnboardStep::flow().len());
        let mut footer = row![
            text(count).size(12).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            modal_action("Skip setup", ModalBtn::Plain, Msg::OnbSkip),
        ]
        .spacing(8)
        .align_y(Center);
        if step.prev().is_some() {
            footer = footer.push(modal_action("Back", ModalBtn::Plain, Msg::OnbBack));
        }
        footer = footer.push(modal_action(next_label, ModalBtn::Primary, Msg::OnbNext));

        // Small top-left wordmark — the wizard's only persistent chrome.
        // Distinct from the (larger, centered) wordmark the Welcome step's
        // `body` renders as part of its own content.
        let brand = row![
            icon("grid", 15.0, c::CYAN()),
            text("grove").font(UI_BOLD).size(14).color(c::MAGENTA()),
        ]
        .spacing(8)
        .align_y(Center);

        // One hard left axis: rail and body both sit flush to the left edge
        // of a fixed 560px column. The column itself is horizontally
        // centered in the viewport, but nothing inside it is — no centered
        // text anywhere in the content.
        let content = column![rail, container(body).width(Length::Fixed(560.0))]
            .width(Length::Fixed(560.0))
            .align_x(iced::Alignment::Start)
            .spacing(22)
            .padding(Padding {
                top: slide_pad,
                ..Padding::ZERO
            });

        // Vertical bias: the column sits slightly above true center (~44%
        // from the top) via a proportional 44/56 spacer split, rather than
        // `center_y`.
        let centered = container(
            column![
                Space::new().height(Length::FillPortion(44)),
                content,
                Space::new().height(Length::FillPortion(56)),
            ]
            .width(Length::Fill)
            .align_x(Center),
        )
        .width(Length::Fill)
        .height(Length::Fill);

        column![
            container(brand).padding(Padding::from([16, 20])),
            centered,
            container(footer)
                .width(Length::Fill)
                .padding(Padding::from([16, 20])),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    // ── changelog modal ───────────────────────────────────────────────────

    fn changelog_modal(&self) -> Element<'_, Msg> {
        use super::state::ChangelogState;
        use iced::Alignment::Center;

        let header = modal_header_row(
            row![
                text("Changelog").size(13).color(c::MAGENTA()),
                Space::new().width(Length::Fill),
                icon_btn("close", Msg::CloseChangelog),
            ]
            .align_y(Center)
            .into(),
        );

        let inner: Element<'_, Msg> = match &self.changelog {
            ChangelogState::Idle | ChangelogState::Loading => row![
                super::icons::spinner(16.0, c::FG_DIM(), self.blink_tick),
                Space::new().width(10),
                text("Loading\u{2026}").size(12).color(c::FG_MUTE()),
            ]
            .align_y(Center)
            .into(),
            ChangelogState::Error(e) => text(format!("Couldn't load changelog: {e}"))
                .size(12)
                .color(c::FG_MUTE())
                .into(),
            ChangelogState::Loaded(notes) if notes.is_empty() => {
                text("No releases yet.").size(12).color(c::FG_MUTE()).into()
            }
            ChangelogState::Loaded(notes) => {
                let mut list = Column::new().spacing(18);
                for n in notes {
                    let mut head = row![text(n.tag.clone()).size(13).font(UI_BOLD).color(c::FG()),]
                        .spacing(8)
                        .align_y(Center);
                    if !n.name.is_empty() && n.name != n.tag {
                        head = head.push(text(n.name.clone()).size(13).color(c::FG_DIM()));
                    }
                    if !n.date.is_empty() {
                        head = head.push(Space::new().width(Length::Fill));
                        head = head.push(text(n.date.clone()).size(11).color(c::FG_MUTE()));
                    }
                    let body_text = crate::upgrade::clean_markdown(&n.body);
                    let entry = column![
                        head,
                        Space::new().height(4),
                        text(body_text).size(12).color(c::FG_MUTE()),
                    ]
                    .spacing(0);
                    list = list.push(entry);
                }
                ghost_scrollable(container(list))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        };

        let body = column![
            header,
            divider_h(c::BORDER_SOFT()),
            container(inner)
                .width(Length::Fill)
                .height(Length::Fixed(420.0))
                .padding(Padding::from([14, 16])),
            divider_h(c::BORDER_SOFT()),
            modal_footer_hints(&[("esc", "close")]),
        ]
        .spacing(0);

        let panel = modal_panel(body.into(), 600.0);

        // Centered overlay on a dim backdrop, matching the settings modal.
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(c::SCRIM())),
                ..Default::default()
            })
            .into()
    }
}

/// The Settings drill-in's leading cue-chip label for the current pane —
/// shared by the input zone's chip and (indirectly, via the same match
/// shape) nothing else, kept as a free function since it needs no `self`.
fn settings_pane_cue(pane: &SettingsPane) -> &'static str {
    match pane {
        SettingsPane::Root => "SETTINGS",
        SettingsPane::Theme { .. } => "THEME",
        SettingsPane::Backend => "BACKEND",
        SettingsPane::Permissions => "PERMISSIONS",
        SettingsPane::DefaultAgent => "DEFAULT AGENT",
        SettingsPane::ProjectTheme { .. } => "PROJECT THEME",
    }
}

/// The Settings drill-in's search-field placeholder for the current pane.
/// Root and Theme actually filter on it; Backend/Permissions/DefaultAgent
/// show it but ignore what's typed (see `handle_modal_key`'s settings
/// branch) — their lists are short and fixed, nothing to filter.
fn settings_pane_placeholder(pane: &SettingsPane) -> &'static str {
    match pane {
        SettingsPane::Root => "Search settings…",
        SettingsPane::Theme { .. } => "Search themes…",
        SettingsPane::Backend | SettingsPane::Permissions | SettingsPane::DefaultAgent => "Search…",
        SettingsPane::ProjectTheme { .. } => "Search themes…",
    }
}

/// Sentence-cases a lowercase identifier (e.g. `Agent::label()`, which stays
/// lowercase because it's shared with non-UI call sites) for display only.
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// One bulleted value-prop line on the welcome step: a magenta mark, a bold
/// lead, and a muted explanation that wraps.
fn onboard_point<'a>(lead: &'a str, body: &'a str) -> Element<'a, Msg> {
    row![
        // A drawn marker, not a glyph: the bundled fonts have no U+25xx box
        // characters, so a text bullet renders as tofu. Nudged down to sit on
        // the lead line's baseline.
        container(dot(c::MAGENTA())).padding(Padding {
            top: 6.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }),
        column![
            text(lead).size(14).color(c::FG()),
            text(body)
                .size(13)
                .color(c::FG_DIM())
                .wrapping(iced::widget::text::Wrapping::Word),
        ]
        .spacing(2),
    ]
    .spacing(10)
    .into()
}

/// One detected-tool row on the environment step: a status dot, the tool name,
/// a muted description, and a right-aligned found/missing/optional tag.
fn onboard_env_row<'a>(
    found: bool,
    optional: bool,
    name: &'a str,
    meta: &'a str,
) -> Element<'a, Msg> {
    let (dotc, tag, tagc) = if found {
        (c::GREEN(), "Found", c::GREEN())
    } else if optional {
        (c::AMBER(), "Optional", c::AMBER())
    } else {
        (c::FG_MUTE(), "Missing", c::FG_MUTE())
    };
    container(
        row![
            dot(dotc),
            text(name.to_string()).size(13).color(c::FG()),
            text(meta.to_string()).size(12).color(c::FG_MUTE()),
            Space::new().width(Length::Fill),
            text(tag).size(11).color(tagc),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8, 12]))
    .style(|_| container::Style {
        background: Some(Background::Color(c::BG_STRIP())),
        border: Border {
            color: c::BORDER(),
            width: 1.0,
            radius: Radius::from(4.0),
        },
        ..Default::default()
    })
    .into()
}

/// Cheap PATH scan for a bare binary name — used to report `git`/`tmux`
/// presence on the onboarding environment step without shelling out.
fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let p = dir.join(bin);
            std::fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false)
        })
    })
}

//! Everything that renders the palette to an `Element`: the modal shell
//! (`session_launcher_modal`) and every pane/row view it composes.

mod panes;
mod rows;
mod settings_panes;
mod settings_rows;

use super::state::{LauncherSettings, Msg, RowActionsState};
use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, UI_FONT};
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg as GMsg};
use crate::gui::view::modal_input_id;
use crate::gui::widgets::{divider_h, palette_input_style};
use iced::border::Radius;
use iced::widget::{column, container, row, text, text_input, Column};
use iced::{Background, Border, Color, Element, Length, Padding, Shadow, Vector};
use settings_rows::{settings_pane_cue, settings_pane_placeholder};

impl Grove {
    /// Recents-first command palette (Agent View "+ New session", mod+n, grid
    /// pill). Two list states driven by `Modal::SessionLauncher`: root (empty
    /// input) shows recents + actions; typing/browse-all shows every
    /// project×worktree combo fuzzy-filtered by `input`. Esc is the only way
    /// to close — no header, no close button.
    ///
    /// Zoned layout: input zone / 1px divider / list zone (fits content, up
    /// to a 380px cap, then scrolls) / 1px divider / footer hint strip — the
    /// footer's own bottom corners are rounded to stay flush with the panel.
    /// The list-zone-through-footer stretch belongs to whichever state is
    /// live — `settings_body` (`settings_panes.rs`), `switch_pane` or
    /// `root_pane` (`panes.rs`); this function owns only the shared input
    /// zone, the dispatch, and the panel shell.
    pub(in crate::gui) fn session_launcher_modal<'a>(
        &'a self,
        input: &'a str,
        selected: usize,
        browse_all: bool,
        switch: Option<usize>,
        row_actions: Option<&'a RowActionsState>,
        settings: Option<&'a LauncherSettings>,
    ) -> Element<'a, GMsg> {
        // A cue chip shell shared by the "switch to session" and settings
        // states' leading slot: mono, cyan text over a soft cyan tint.
        let cue_chip = |label: &'static str| -> Element<'a, GMsg> {
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
        // In switch/settings state, the leading glyph slot becomes a
        // static cue chip instead of the search icon; the typed text
        // underneath is unchanged.
        let leading: Element<'a, GMsg> = if switch.is_some() {
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
            .on_input(|s| GMsg::SessionLauncher(Msg::InputChanged(s)))
            .on_paste(|s| GMsg::SessionLauncher(Msg::InputPasted(s)))
            .style(palette_input_style);
        let input_zone = container(
            row![leading, field]
                .spacing(8)
                .align_y(iced::Alignment::Center),
        )
        .padding(Padding::from([14, 16]));

        let body: Column<'a, GMsg> = column![input_zone, divider_h(c::BORDER_SOFT())];

        // Exactly one state is live at a time; each appends its own list
        // zone, divider and footer to `body`.
        let body = if let Some(ls) = settings {
            self.settings_body(body, input, ls)
        } else if let Some(sel) = switch {
            self.switch_pane(body, input, sel)
        } else {
            self.root_pane(body, input, selected, browse_all, row_actions)
        };

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
}

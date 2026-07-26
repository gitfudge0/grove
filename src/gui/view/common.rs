//! Small render helpers and stable-id functions shared across the view
//! submodules: modal focus ids, scrollable ids, chip/hint renderers, and
//! misc string/session helpers.

use crate::gui::icons::icon;
use crate::gui::metrics::{MONO_FONT, ROW_H, UI_FONT};
use crate::gui::palette as c;
use crate::gui::state::{Grove, Msg};
use crate::gui::update::platform_mod_label;
use crate::gui::widgets::{keycap, launcher_row};
use fs_err as fs;
use grove_core::session::Session;
use iced::border::Radius;
use iced::widget::{container, rich_text, row, span, text, text_input, Column, Id};
use iced::{Background, Border, Color, Element, Length, Padding};

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
pub(in crate::gui) fn input_field_style(
    _t: &iced::Theme,
    status: text_input::Status,
) -> text_input::Style {
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

/// Stable id for `Modal::ThemeManager`'s list scrollable — the same list
/// (rename/duplicate/delete/edit) and the editor's own 11-row list share
/// this id, both scrolled programmatically by `Grove::scroll_theme_manager_
/// editor_to_selection` (row moves in the editor) and the list-view's own
/// selection scroll.
pub fn theme_manager_scrollable_id() -> Id {
    Id::new("theme-manager-list")
}

/// Stable id for the palette Settings drill-in's Root list scrollable —
/// same idiom again: 8 rows plus section headers overflow the 380px cap, so
/// cursor moves (and sub-pane exits landing near the bottom) must scroll
/// the selection into view from `update`.
pub fn launcher_settings_scrollable_id() -> Id {
    Id::new("launcher-settings-list")
}

/// Stable id for the palette's root/typed list scrollable — same idiom
/// again: ↑↓ moves the selection without moving the viewport on its own, so
/// the selected row must be scrolled into view from `update`, same as the
/// Settings drill-in's Root list.
pub fn launcher_palette_scrollable_id() -> Id {
    Id::new("launcher-palette-list")
}

/// A mod+key hint chip: on macOS the modifier renders as the ⌘ glyph icon,
/// elsewhere as `platform_mod_label()`. Used for the palette's ⌘T action-row
/// chip (`color` = `FG_DIM`) and its ⌘1…⌘N recent-row digit chips (`color` =
/// `FG_MUTE`, a quieter shade so they recede behind the row text).
pub(in crate::gui) fn mod_key_chip<'a>(key: &'static str, color: Color) -> Element<'a, Msg> {
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

/// A mod-chorded footer hint (e.g. "⌘D duplicate" / "ctrl+shift+D duplicate"
/// off mac) — `footer_hint`'s idiom, but with `mod_key_chip`'s platform-aware
/// modifier rendering instead of a bare keycap. Used by the Theme sub-pane's
/// duplicate/rename/edit/delete actions, which are ⌘-chorded so they never
/// collide with typing a theme name into the search filter.
pub(in crate::gui) fn footer_mod_hint<'a>(
    key: &'static str,
    label: &'static str,
) -> Element<'a, Msg> {
    row![
        mod_key_chip(key, c::FG_DIM()),
        text(label).font(MONO_FONT).size(10).color(c::FG_MUTE()),
    ]
    .spacing(6)
    .align_y(iced::Alignment::Center)
    .into()
}

/// Render `s` as rich text, coloring the character ranges in `ranges` cyan
/// (the typing-state fuzzy-match highlight) and everything else
/// `base_color`. `ranges` are **char** indices from
/// `launcher::fuzzy_match_indices`, not byte offsets. Falls back to a plain
/// `text` widget when there's nothing to highlight.
pub(in crate::gui) fn highlighted_line<'a>(
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
pub(in crate::gui) fn digit_label(i: usize) -> Option<&'static str> {
    ["1", "2", "3", "4", "5", "6"].get(i).copied()
}

pub(super) fn session_context_title(s: &Session) -> Option<String> {
    let salt: [&str; 2] = [&s.label, s.agent.label()];
    crate::gui::rows::cached_context(s, 2, &salt, |raw| {
        if raw.eq_ignore_ascii_case(&s.label) || raw.eq_ignore_ascii_case(s.agent.label()) {
            return None;
        }
        // OSC titles often start with emoji or box-drawing characters that the
        // UI font (IBM Plex Sans) can't render — strip them so the sess_bar
        // never shows a tofu box. The sidebar applies the same filter.
        crate::gui::rows::sanitize_ui_text(raw)
    })
}

pub(super) fn is_in_progress_title(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    lower.contains("in progress") || lower.contains("in-progress") || lower.contains("in_progress")
}

impl Grove {
    /// Wrap `content` with a small hint label shown on hover. Styled to match
    /// the app's other floating surfaces (BG_STRIP background, BORDER border).
    pub(super) fn hint<'a>(
        content: impl Into<Element<'a, Msg>>,
        label: &'a str,
    ) -> Element<'a, Msg> {
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

    /// The windowed directory-match list shared by the add-project pick step
    /// and the onboarding project step: up to `window` rows that scroll to
    /// keep the selection visible, with muted "↑N/↓N more" hints when entries
    /// sit above or below the window. Results are memoized in `dir_cache`
    /// because `view()` runs every tick.
    pub(in crate::gui) fn dir_matches(
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
}

/// Sentence-cases a lowercase identifier (e.g. `Agent::label()`, which stays
/// lowercase because it's shared with non-UI call sites) for display only.
pub(in crate::gui) fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Cheap PATH scan for a bare binary name — used to report `git`'s presence
/// on the onboarding environment step without shelling out.
///
/// The result is cached for the lifetime of the process: `$PATH` and what's
/// installed on it don't change while Grove is running, and this is called
/// from `view()`'s render path (via the onboarding screen), so re-scanning
/// `$PATH` every frame is a syscall storm.
///
// ponytail: this means if the user installs `git` while Grove is open, the
// onboarding screen won't notice until restart. If that ever matters, add a
// manual re-detect action that clears the cache instead of re-scanning per
// frame. Cached as a single `OnceLock<bool>` rather than a name->bool map
// because the only call site always checks "git" — a map would be
// speculative generality for a scan that has exactly one caller.
fn on_path_uncached(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let p = dir.join(bin);
            fs::metadata(&p).map(|m| m.is_file()).unwrap_or(false)
        })
    })
}

pub(in crate::gui) fn git_on_path() -> bool {
    static CACHE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| on_path_uncached("git"))
}

use std::{ops::Deref, sync::Arc};

use crate::{ThemeMode, theme::DEFAULT_THEME_COLORS};

use gpui::{Background, Fill, Hsla};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ThemeToken {
    pub color: Hsla,
    pub background: Background,
}

impl ThemeToken {
    pub fn new(color: Hsla, background: Background) -> Self {
        Self { color, background }
    }
}

impl Deref for ThemeToken {
    type Target = Hsla;

    fn deref(&self) -> &Self::Target {
        &self.color
    }
}

impl From<Hsla> for ThemeToken {
    fn from(color: Hsla) -> Self {
        Self {
            color,
            background: color.into(),
        }
    }
}

impl From<ThemeToken> for Hsla {
    fn from(token: ThemeToken) -> Self {
        token.color
    }
}

impl From<ThemeToken> for Background {
    fn from(token: ThemeToken) -> Self {
        token.background
    }
}

impl From<ThemeToken> for Fill {
    fn from(token: ThemeToken) -> Self {
        Fill::Color(token.background)
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, JsonSchema)]
pub struct ThemeColor {
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub accordion: Hsla,
    pub accordion_hover: Hsla,
    pub background: Hsla,
    pub border: Hsla,
    pub button: Hsla,
    pub button_active: Hsla,
    pub button_foreground: Hsla,
    pub button_hover: Hsla,
    /// Fallback to `danger`.
    pub button_danger: Hsla,
    /// Fallback to `danger_active`.
    pub button_danger_active: Hsla,
    /// Fallback to `danger_foreground`.
    pub button_danger_foreground: Hsla,
    /// Fallback to `danger_hover`.
    pub button_danger_hover: Hsla,
    /// Fallback to `info`.
    pub button_info: Hsla,
    /// Fallback to `info_active`.
    pub button_info_active: Hsla,
    /// Fallback to `info_foreground`.
    pub button_info_foreground: Hsla,
    /// Fallback to `info_hover`.
    pub button_info_hover: Hsla,
    /// Fallback to `primary`.
    pub button_primary: Hsla,
    /// Fallback to `primary_active`.
    pub button_primary_active: Hsla,
    /// Fallback to `primary_foreground`.
    pub button_primary_foreground: Hsla,
    /// Fallback to `primary_hover`.
    pub button_primary_hover: Hsla,
    /// Fallback to `secondary`.
    pub button_secondary: Hsla,
    /// Fallback to `secondary_active`.
    pub button_secondary_active: Hsla,
    /// Fallback to `secondary_foreground`.
    pub button_secondary_foreground: Hsla,
    /// Fallback to `secondary_hover`.
    pub button_secondary_hover: Hsla,
    /// Fallback to `success`.
    pub button_success: Hsla,
    /// Fallback to `success_active`.
    pub button_success_active: Hsla,
    /// Fallback to `success_foreground`.
    pub button_success_foreground: Hsla,
    /// Fallback to `success_hover`.
    pub button_success_hover: Hsla,
    /// Fallback to `warning`.
    pub button_warning: Hsla,
    /// Fallback to `warning_active`.
    pub button_warning_active: Hsla,
    /// Fallback to `warning_foreground`.
    pub button_warning_foreground: Hsla,
    /// Fallback to `warning_hover`.
    pub button_warning_hover: Hsla,
    pub group_box: Hsla,
    pub group_box_foreground: Hsla,
    pub caret: Hsla,
    pub chart_1: Hsla,
    pub chart_2: Hsla,
    pub chart_3: Hsla,
    pub chart_4: Hsla,
    pub chart_5: Hsla,
    pub chart_bullish: Hsla,
    pub chart_bearish: Hsla,
    pub danger: Hsla,
    pub danger_active: Hsla,
    pub danger_foreground: Hsla,
    pub danger_hover: Hsla,
    pub description_list_label: Hsla,
    pub description_list_label_foreground: Hsla,
    pub drag_border: Hsla,
    pub drop_target: Hsla,
    pub foreground: Hsla,
    pub info: Hsla,
    pub info_active: Hsla,
    pub info_foreground: Hsla,
    pub info_hover: Hsla,
    pub input: Hsla,
    pub link: Hsla,
    pub link_active: Hsla,
    pub link_hover: Hsla,
    pub list: Hsla,
    pub list_active: Hsla,
    pub list_active_border: Hsla,
    pub list_even: Hsla,
    pub list_head: Hsla,
    pub list_hover: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    pub primary: Hsla,
    pub primary_active: Hsla,
    pub primary_foreground: Hsla,
    pub primary_hover: Hsla,
    pub progress_bar: Hsla,
    pub ring: Hsla,
    pub scrollbar: Hsla,
    pub scrollbar_thumb: Hsla,
    pub scrollbar_thumb_hover: Hsla,
    pub secondary: Hsla,
    pub secondary_active: Hsla,
    pub secondary_foreground: Hsla,
    pub secondary_hover: Hsla,
    pub selection: Hsla,
    pub sidebar: Hsla,
    pub sidebar_accent: Hsla,
    pub sidebar_accent_foreground: Hsla,
    pub sidebar_border: Hsla,
    pub sidebar_foreground: Hsla,
    pub sidebar_primary: Hsla,
    pub sidebar_primary_foreground: Hsla,
    pub skeleton: Hsla,
    pub slider_bar: Hsla,
    pub slider_thumb: Hsla,
    pub success: Hsla,
    pub success_foreground: Hsla,
    pub success_hover: Hsla,
    pub success_active: Hsla,
    pub switch: Hsla,
    pub switch_thumb: Hsla,
    pub tab: Hsla,
    pub tab_active: Hsla,
    pub tab_active_foreground: Hsla,
    pub tab_bar: Hsla,
    pub tab_bar_segmented: Hsla,
    pub tab_foreground: Hsla,
    pub table: Hsla,
    pub table_active: Hsla,
    pub table_active_border: Hsla,
    pub table_even: Hsla,
    pub table_head: Hsla,
    pub table_head_foreground: Hsla,
    pub table_foot: Hsla,
    pub table_foot_foreground: Hsla,
    pub table_hover: Hsla,
    pub table_row_border: Hsla,
    pub title_bar: Hsla,
    pub title_bar_border: Hsla,
    pub status_bar: Hsla,
    pub status_bar_border: Hsla,
    pub tiles: Hsla,
    pub warning: Hsla,
    pub warning_active: Hsla,
    pub warning_hover: Hsla,
    pub warning_foreground: Hsla,
    pub overlay: Hsla,
    /// Only works on Linux; other platforms can't change the window border color.
    pub window_border: Hsla,

    pub red: Hsla,
    pub red_light: Hsla,
    pub green: Hsla,
    pub green_light: Hsla,
    pub blue: Hsla,
    pub blue_light: Hsla,
    pub yellow: Hsla,
    pub yellow_light: Hsla,
    pub magenta: Hsla,
    pub magenta_light: Hsla,
    pub cyan: Hsla,
    pub cyan_light: Hsla,
}

macro_rules! define_theme_tokens {
    ($($field:ident),+ $(,)?) => {
        #[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, JsonSchema)]
        pub struct ThemeTokens {
            $(pub $field: ThemeToken,)+
        }

        impl From<ThemeColor> for ThemeTokens {
            fn from(colors: ThemeColor) -> Self {
                Self {
                    $($field: colors.$field.into(),)+
                }
            }
        }

        impl From<&ThemeColor> for ThemeTokens {
            fn from(colors: &ThemeColor) -> Self {
                Self::from(*colors)
            }
        }
    };
}

define_theme_tokens! {
    accent,
    accent_foreground,
    accordion,
    accordion_hover,
    background,
    border,
    button,
    button_active,
    button_foreground,
    button_hover,
    button_danger,
    button_danger_active,
    button_danger_foreground,
    button_danger_hover,
    button_info,
    button_info_active,
    button_info_foreground,
    button_info_hover,
    button_primary,
    button_primary_active,
    button_primary_foreground,
    button_primary_hover,
    button_secondary,
    button_secondary_active,
    button_secondary_foreground,
    button_secondary_hover,
    button_success,
    button_success_active,
    button_success_foreground,
    button_success_hover,
    button_warning,
    button_warning_active,
    button_warning_foreground,
    button_warning_hover,
    group_box,
    group_box_foreground,
    caret,
    chart_1,
    chart_2,
    chart_3,
    chart_4,
    chart_5,
    chart_bullish,
    chart_bearish,
    danger,
    danger_active,
    danger_foreground,
    danger_hover,
    description_list_label,
    description_list_label_foreground,
    drag_border,
    drop_target,
    foreground,
    info,
    info_active,
    info_foreground,
    info_hover,
    input,
    link,
    link_active,
    link_hover,
    list,
    list_active,
    list_active_border,
    list_even,
    list_head,
    list_hover,
    muted,
    muted_foreground,
    popover,
    popover_foreground,
    primary,
    primary_active,
    primary_foreground,
    primary_hover,
    progress_bar,
    ring,
    scrollbar,
    scrollbar_thumb,
    scrollbar_thumb_hover,
    secondary,
    secondary_active,
    secondary_foreground,
    secondary_hover,
    selection,
    sidebar,
    sidebar_accent,
    sidebar_accent_foreground,
    sidebar_border,
    sidebar_foreground,
    sidebar_primary,
    sidebar_primary_foreground,
    skeleton,
    slider_bar,
    slider_thumb,
    success,
    success_foreground,
    success_hover,
    success_active,
    switch,
    switch_thumb,
    tab,
    tab_active,
    tab_active_foreground,
    tab_bar,
    tab_bar_segmented,
    tab_foreground,
    table,
    table_active,
    table_active_border,
    table_even,
    table_head,
    table_head_foreground,
    table_foot,
    table_foot_foreground,
    table_hover,
    table_row_border,
    title_bar,
    title_bar_border,
    status_bar,
    status_bar_border,
    tiles,
    warning,
    warning_active,
    warning_hover,
    warning_foreground,
    overlay,
    window_border,
    red,
    red_light,
    green,
    green_light,
    blue,
    blue_light,
    yellow,
    yellow_light,
    magenta,
    magenta_light,
    cyan,
    cyan_light,
}

impl ThemeColor {
    pub fn light() -> Arc<Self> {
        DEFAULT_THEME_COLORS[&ThemeMode::Light].0.clone()
    }

    pub fn dark() -> Arc<Self> {
        DEFAULT_THEME_COLORS[&ThemeMode::Dark].0.clone()
    }
}

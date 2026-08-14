use std::{rc::Rc, sync::Arc};

use gpui::{Background, Hsla, SharedString, px};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::highlighter::{HighlightTheme, HighlightThemeStyle};

use super::color::{
    try_parse_background, try_parse_background_clamped, try_parse_color, try_parse_theme_color,
};
use super::{Colorize, Theme, ThemeColor, ThemeMode, ThemeToken, ThemeTokens};

fn try_parse_theme_token(value: &str) -> anyhow::Result<ThemeToken> {
    Ok(ThemeToken::new(
        try_parse_theme_color(value)?,
        try_parse_background(value)?,
    ))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThemeSet {
    pub name: SharedString,
    pub author: Option<SharedString>,
    pub url: Option<SharedString>,
    #[serde(rename = "themes")]
    pub themes: Vec<ThemeConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ThemeConfig {
    pub is_default: bool,
    pub name: SharedString,
    /// Default is light.
    pub mode: ThemeMode,

    /// Default 16.
    #[serde(rename = "font.size")]
    pub font_size: Option<f32>,
    /// Default is system font: `.SystemUIFont`.
    #[serde(rename = "font.family")]
    pub font_family: Option<SharedString>,
    /// Default is platform-specific: macOS `Menlo`, Windows `Consolas`, Linux `DejaVu Sans Mono`.
    #[serde(rename = "mono_font.family")]
    pub mono_font_family: Option<SharedString>,
    /// Default 13.
    #[serde(rename = "mono_font.size")]
    pub mono_font_size: Option<f32>,

    /// Default 6.
    #[serde(rename = "radius")]
    pub radius: Option<usize>,
    /// Large elements like Dialogs/Notifications; default 8.
    #[serde(rename = "radius.lg")]
    pub radius_lg: Option<usize>,
    /// Default true.
    #[serde(rename = "shadow")]
    pub shadow: Option<bool>,

    pub colors: ThemeConfigColors,
    /// Compatible with the `style` section in a Zed theme (zed-industries/zed assets/themes/ayu/ayu.json).
    pub highlight: Option<HighlightThemeStyle>,
}

#[derive(Debug, Default, Clone, JsonSchema, Serialize, Deserialize)]
pub struct ThemeConfigColors {
    #[serde(rename = "accent.background")]
    pub accent: Option<SharedString>,
    #[serde(rename = "accent.foreground")]
    pub accent_foreground: Option<SharedString>,
    #[serde(rename = "accordion.background")]
    pub accordion: Option<SharedString>,
    #[serde(rename = "accordion.hover.background")]
    pub accordion_hover: Option<SharedString>,
    #[serde(rename = "background")]
    pub background: Option<SharedString>,
    #[serde(rename = "border")]
    pub border: Option<SharedString>,
    #[serde(rename = "button.background")]
    pub button: Option<SharedString>,
    #[serde(rename = "button.active.background")]
    pub button_active: Option<SharedString>,
    #[serde(rename = "button.foreground")]
    pub button_foreground: Option<SharedString>,
    #[serde(rename = "button.hover.background")]
    pub button_hover: Option<SharedString>,
    /// Fallback to `danger`.
    #[serde(rename = "button.danger.background")]
    pub button_danger: Option<SharedString>,
    /// Fallback to `danger_active`.
    #[serde(rename = "button.danger.active.background")]
    pub button_danger_active: Option<SharedString>,
    /// Fallback to `danger_foreground`.
    #[serde(rename = "button.danger.foreground")]
    pub button_danger_foreground: Option<SharedString>,
    /// Fallback to `danger_hover`.
    #[serde(rename = "button.danger.hover.background")]
    pub button_danger_hover: Option<SharedString>,
    /// Fallback to `info`.
    #[serde(rename = "button.info.background")]
    pub button_info: Option<SharedString>,
    /// Fallback to `info_active`.
    #[serde(rename = "button.info.active.background")]
    pub button_info_active: Option<SharedString>,
    /// Fallback to `info_foreground`.
    #[serde(rename = "button.info.foreground")]
    pub button_info_foreground: Option<SharedString>,
    /// Fallback to `info_hover`.
    #[serde(rename = "button.info.hover.background")]
    pub button_info_hover: Option<SharedString>,
    /// Fallback to `primary`.
    #[serde(rename = "button.primary.background")]
    pub button_primary: Option<SharedString>,
    /// Fallback to `primary_active`.
    #[serde(rename = "button.primary.active.background")]
    pub button_primary_active: Option<SharedString>,
    /// Fallback to `primary_foreground`.
    #[serde(rename = "button.primary.foreground")]
    pub button_primary_foreground: Option<SharedString>,
    /// Fallback to `primary_hover`.
    #[serde(rename = "button.primary.hover.background")]
    pub button_primary_hover: Option<SharedString>,
    /// Fallback to `secondary`.
    #[serde(rename = "button.secondary.background")]
    pub button_secondary: Option<SharedString>,
    /// Fallback to `secondary_active`.
    #[serde(rename = "button.secondary.active.background")]
    pub button_secondary_active: Option<SharedString>,
    /// Fallback to `secondary_foreground`.
    #[serde(rename = "button.secondary.foreground")]
    pub button_secondary_foreground: Option<SharedString>,
    /// Fallback to `secondary_hover`.
    #[serde(rename = "button.secondary.hover.background")]
    pub button_secondary_hover: Option<SharedString>,
    /// Fallback to `success`.
    #[serde(rename = "button.success.background")]
    pub button_success: Option<SharedString>,
    /// Fallback to `success_active`.
    #[serde(rename = "button.success.active.background")]
    pub button_success_active: Option<SharedString>,
    /// Fallback to `success_foreground`.
    #[serde(rename = "button.success.foreground")]
    pub button_success_foreground: Option<SharedString>,
    /// Fallback to `success_hover`.
    #[serde(rename = "button.success.hover.background")]
    pub button_success_hover: Option<SharedString>,
    /// Fallback to `warning`.
    #[serde(rename = "button.warning.background")]
    pub button_warning: Option<SharedString>,
    /// Fallback to `warning_active`.
    #[serde(rename = "button.warning.active.background")]
    pub button_warning_active: Option<SharedString>,
    /// Fallback to `warning_foreground`.
    #[serde(rename = "button.warning.foreground")]
    pub button_warning_foreground: Option<SharedString>,
    /// Fallback to `warning_hover`.
    #[serde(rename = "button.warning.hover.background")]
    pub button_warning_hover: Option<SharedString>,
    #[serde(rename = "group_box.background")]
    pub group_box: Option<SharedString>,
    #[serde(rename = "group_box.foreground")]
    pub group_box_foreground: Option<SharedString>,
    #[serde(rename = "group_box.title.foreground")]
    pub group_box_title_foreground: Option<SharedString>,
    /// Blinking cursor color.
    #[serde(rename = "caret")]
    pub caret: Option<SharedString>,
    #[serde(rename = "chart.1")]
    pub chart_1: Option<SharedString>,
    #[serde(rename = "chart.2")]
    pub chart_2: Option<SharedString>,
    #[serde(rename = "chart.3")]
    pub chart_3: Option<SharedString>,
    #[serde(rename = "chart.4")]
    pub chart_4: Option<SharedString>,
    #[serde(rename = "chart.5")]
    pub chart_5: Option<SharedString>,
    #[serde(rename = "chart_bullish")]
    pub chart_bullish: Option<SharedString>,
    #[serde(rename = "chart_bearish")]
    pub chart_bearish: Option<SharedString>,
    #[serde(rename = "danger.background")]
    pub danger: Option<SharedString>,
    #[serde(rename = "danger.active.background")]
    pub danger_active: Option<SharedString>,
    #[serde(rename = "danger.foreground")]
    pub danger_foreground: Option<SharedString>,
    #[serde(rename = "danger.hover.background")]
    pub danger_hover: Option<SharedString>,
    #[serde(rename = "description_list.label.background")]
    pub description_list_label: Option<SharedString>,
    #[serde(rename = "description_list.label.foreground")]
    pub description_list_label_foreground: Option<SharedString>,
    #[serde(rename = "drag.border")]
    pub drag_border: Option<SharedString>,
    #[serde(rename = "drop_target.background")]
    pub drop_target: Option<SharedString>,
    #[serde(rename = "foreground")]
    pub foreground: Option<SharedString>,
    #[serde(rename = "info.background")]
    pub info: Option<SharedString>,
    #[serde(rename = "info.active.background")]
    pub info_active: Option<SharedString>,
    #[serde(rename = "info.foreground")]
    pub info_foreground: Option<SharedString>,
    #[serde(rename = "info.hover.background")]
    pub info_hover: Option<SharedString>,
    #[serde(rename = "input.border")]
    pub input: Option<SharedString>,
    #[serde(rename = "link")]
    pub link: Option<SharedString>,
    #[serde(rename = "link.active")]
    pub link_active: Option<SharedString>,
    #[serde(rename = "link.hover")]
    pub link_hover: Option<SharedString>,
    #[serde(rename = "list.background")]
    pub list: Option<SharedString>,
    #[serde(rename = "list.active.background")]
    pub list_active: Option<SharedString>,
    #[serde(rename = "list.active.border")]
    pub list_active_border: Option<SharedString>,
    #[serde(rename = "list.even.background")]
    pub list_even: Option<SharedString>,
    #[serde(rename = "list.head.background")]
    pub list_head: Option<SharedString>,
    #[serde(rename = "list.hover.background")]
    pub list_hover: Option<SharedString>,
    #[serde(rename = "muted.background")]
    pub muted: Option<SharedString>,
    #[serde(rename = "muted.foreground")]
    pub muted_foreground: Option<SharedString>,
    #[serde(rename = "popover.background")]
    pub popover: Option<SharedString>,
    #[serde(rename = "popover.foreground")]
    pub popover_foreground: Option<SharedString>,
    #[serde(rename = "primary.background")]
    pub primary: Option<SharedString>,
    #[serde(rename = "primary.active.background")]
    pub primary_active: Option<SharedString>,
    #[serde(rename = "primary.foreground")]
    pub primary_foreground: Option<SharedString>,
    #[serde(rename = "primary.hover.background")]
    pub primary_hover: Option<SharedString>,
    #[serde(rename = "progress.bar.background")]
    pub progress_bar: Option<SharedString>,
    /// Focus ring.
    #[serde(rename = "ring")]
    pub ring: Option<SharedString>,
    #[serde(rename = "scrollbar.background")]
    pub scrollbar: Option<SharedString>,
    #[serde(rename = "scrollbar.thumb.background")]
    pub scrollbar_thumb: Option<SharedString>,
    #[serde(rename = "scrollbar.thumb.hover.background")]
    pub scrollbar_thumb_hover: Option<SharedString>,
    #[serde(rename = "secondary.background")]
    pub secondary: Option<SharedString>,
    #[serde(rename = "secondary.active.background")]
    pub secondary_active: Option<SharedString>,
    #[serde(rename = "secondary.foreground")]
    pub secondary_foreground: Option<SharedString>,
    #[serde(rename = "secondary.hover.background")]
    pub secondary_hover: Option<SharedString>,
    #[serde(rename = "selection.background")]
    pub selection: Option<SharedString>,
    #[serde(rename = "sidebar.background")]
    pub sidebar: Option<SharedString>,
    #[serde(rename = "sidebar.accent.background")]
    pub sidebar_accent: Option<SharedString>,
    #[serde(rename = "sidebar.accent.foreground")]
    pub sidebar_accent_foreground: Option<SharedString>,
    #[serde(rename = "sidebar.border")]
    pub sidebar_border: Option<SharedString>,
    #[serde(rename = "sidebar.foreground")]
    pub sidebar_foreground: Option<SharedString>,
    #[serde(rename = "sidebar.primary.background")]
    pub sidebar_primary: Option<SharedString>,
    #[serde(rename = "sidebar.primary.foreground")]
    pub sidebar_primary_foreground: Option<SharedString>,
    #[serde(rename = "skeleton.background")]
    pub skeleton: Option<SharedString>,
    #[serde(rename = "slider.background")]
    pub slider_bar: Option<SharedString>,
    #[serde(rename = "slider.thumb.background")]
    pub slider_thumb: Option<SharedString>,
    #[serde(rename = "success.background")]
    pub success: Option<SharedString>,
    #[serde(rename = "success.foreground")]
    pub success_foreground: Option<SharedString>,
    #[serde(rename = "success.hover.background")]
    pub success_hover: Option<SharedString>,
    #[serde(rename = "success.active.background")]
    pub success_active: Option<SharedString>,
    #[serde(rename = "switch.background")]
    pub switch: Option<SharedString>,
    #[serde(rename = "switch.thumb.background")]
    pub switch_thumb: Option<SharedString>,
    #[serde(rename = "tab.background")]
    pub tab: Option<SharedString>,
    #[serde(rename = "tab.active.background")]
    pub tab_active: Option<SharedString>,
    #[serde(rename = "tab.active.foreground")]
    pub tab_active_foreground: Option<SharedString>,
    #[serde(rename = "tab_bar.background")]
    pub tab_bar: Option<SharedString>,
    #[serde(rename = "tab_bar.segmented.background")]
    pub tab_bar_segmented: Option<SharedString>,
    #[serde(rename = "tab.foreground")]
    pub tab_foreground: Option<SharedString>,
    #[serde(rename = "table.background")]
    pub table: Option<SharedString>,
    #[serde(rename = "table.active.background")]
    pub table_active: Option<SharedString>,
    #[serde(rename = "table.active.border")]
    pub table_active_border: Option<SharedString>,
    #[serde(rename = "table.even.background")]
    pub table_even: Option<SharedString>,
    #[serde(rename = "table.head.background")]
    pub table_head: Option<SharedString>,
    #[serde(rename = "table.head.foreground")]
    pub table_head_foreground: Option<SharedString>,
    #[serde(rename = "table.foot.background")]
    pub table_foot: Option<SharedString>,
    #[serde(rename = "table.foot.foreground")]
    pub table_foot_foreground: Option<SharedString>,
    #[serde(rename = "table.hover.background")]
    pub table_hover: Option<SharedString>,
    #[serde(rename = "table.row.border")]
    pub table_row_border: Option<SharedString>,
    #[serde(rename = "title_bar.background")]
    pub title_bar: Option<SharedString>,
    #[serde(rename = "title_bar.border")]
    pub title_bar_border: Option<SharedString>,
    #[serde(rename = "status_bar.background")]
    pub status_bar: Option<SharedString>,
    #[serde(rename = "status_bar.border")]
    pub status_bar_border: Option<SharedString>,
    #[serde(rename = "tiles.background")]
    pub tiles: Option<SharedString>,
    #[serde(rename = "warning.background")]
    pub warning: Option<SharedString>,
    #[serde(rename = "warning.active.background")]
    pub warning_active: Option<SharedString>,
    #[serde(rename = "warning.hover.background")]
    pub warning_hover: Option<SharedString>,
    #[serde(rename = "warning.foreground")]
    pub warning_foreground: Option<SharedString>,
    #[serde(rename = "overlay")]
    pub overlay: Option<SharedString>,
    /// Linux only — other platforms can't change the window border color.
    #[serde(rename = "window.border")]
    pub window_border: Option<SharedString>,

    #[serde(rename = "base.blue")]
    blue: Option<String>,
    #[serde(rename = "base.blue.light")]
    blue_light: Option<String>,
    #[serde(rename = "base.cyan")]
    cyan: Option<String>,
    #[serde(rename = "base.cyan.light")]
    cyan_light: Option<String>,
    #[serde(rename = "base.green")]
    green: Option<String>,
    #[serde(rename = "base.green.light")]
    green_light: Option<String>,
    #[serde(rename = "base.magenta")]
    magenta: Option<String>,
    #[serde(rename = "base.magenta.light")]
    magenta_light: Option<String>,
    #[serde(rename = "base.red")]
    red: Option<String>,
    #[serde(rename = "base.red.light")]
    red_light: Option<String>,
    #[serde(rename = "base.yellow")]
    yellow: Option<String>,
    #[serde(rename = "base.yellow.light")]
    yellow_light: Option<String>,
}

impl ThemeColor {
    pub(crate) fn apply_config(
        &mut self,
        config: &ThemeConfig,
        default_theme: &ThemeColor,
    ) -> ThemeTokens {
        let colors = config.colors.clone();
        let default_tokens = ThemeTokens::from(default_theme);
        let mut tokens = default_tokens;

        macro_rules! apply_color {
            ($config_field:ident) => {
                if let Some(value) = &colors.$config_field {
                    self.$config_field =
                        try_parse_color(value).unwrap_or(default_theme.$config_field);
                } else {
                    self.$config_field = default_theme.$config_field;
                }
                tokens.$config_field = self.$config_field.into();
            };
            ($config_field:ident, fallback = $fallback:expr) => {
                let fallback: gpui::Hsla = ($fallback).into();
                if let Some(value) = &colors.$config_field {
                    self.$config_field = try_parse_color(value).unwrap_or(fallback);
                } else {
                    self.$config_field = fallback;
                }
                tokens.$config_field = self.$config_field.into();
            };
        }

        macro_rules! apply_background_color {
            ($config_field:ident) => {
                let token = if let Some(value) = &colors.$config_field {
                    if let Ok(token) = try_parse_theme_token(&value) {
                        token
                    } else {
                        default_tokens.$config_field
                    }
                } else {
                    default_tokens.$config_field
                };
                self.$config_field = token.color;
                tokens.$config_field = token;
            };
            ($config_field:ident, fallback = $fallback:expr) => {
                let fallback: ThemeToken = ($fallback).into();
                let token = if let Some(value) = &colors.$config_field {
                    if let Ok(token) = try_parse_theme_token(&value) {
                        token
                    } else {
                        fallback
                    }
                } else {
                    fallback
                };
                self.$config_field = token.color;
                tokens.$config_field = token;
            };
        }

        apply_background_color!(background);

        apply_color!(red);
        apply_color!(
            red_light,
            fallback = self.background.blend(self.red.opacity(0.8))
        );
        apply_color!(green);
        apply_color!(
            green_light,
            fallback = self.background.blend(self.green.opacity(0.8))
        );
        apply_color!(blue);
        apply_color!(
            blue_light,
            fallback = self.background.blend(self.blue.opacity(0.8))
        );
        apply_color!(magenta);
        apply_color!(
            magenta_light,
            fallback = self.background.blend(self.magenta.opacity(0.8))
        );
        apply_color!(yellow);
        apply_color!(
            yellow_light,
            fallback = self.background.blend(self.yellow.opacity(0.8))
        );
        apply_color!(cyan);
        apply_color!(
            cyan_light,
            fallback = self.background.blend(self.cyan.opacity(0.8))
        );

        apply_color!(border);
        apply_color!(foreground);
        apply_color!(input, fallback = self.border);
        apply_background_color!(muted);
        apply_color!(
            muted_foreground,
            fallback = self.muted.blend(self.foreground.opacity(0.7))
        );

        let active_darken = if config.mode.is_dark() { 0.2 } else { 0.1 };
        let hover_opacity = 0.9;
        let transparent = gpui::transparent_black();
        let button_background = if config.mode.is_dark() {
            self.input.mix_oklab(transparent, 0.3)
        } else {
            self.background
        };
        apply_background_color!(button, fallback = button_background);
        apply_color!(button_foreground, fallback = self.foreground);
        apply_background_color!(
            button_hover,
            fallback = self.input.mix_oklab(transparent, 0.5)
        );
        apply_background_color!(
            button_active,
            fallback = self.input.mix_oklab(transparent, 0.7)
        );
        apply_background_color!(primary);
        apply_color!(primary_foreground, fallback = self.foreground);
        apply_background_color!(
            primary_hover,
            fallback = self.background.blend(self.primary.opacity(hover_opacity))
        );
        apply_background_color!(
            primary_active,
            fallback = self.primary.darken(active_darken)
        );
        apply_background_color!(button_primary, fallback = tokens.primary);
        apply_color!(
            button_primary_foreground,
            fallback = self.primary_foreground
        );
        apply_background_color!(button_primary_hover, fallback = tokens.primary_hover);
        apply_background_color!(button_primary_active, fallback = tokens.primary_active);
        apply_background_color!(secondary);
        apply_color!(secondary_foreground, fallback = self.foreground);
        apply_background_color!(
            secondary_hover,
            fallback = self.background.blend(self.secondary.opacity(hover_opacity))
        );
        apply_background_color!(
            secondary_active,
            fallback = self.secondary.darken(active_darken)
        );
        apply_background_color!(button_secondary, fallback = tokens.secondary);
        apply_color!(
            button_secondary_foreground,
            fallback = self.secondary_foreground
        );
        apply_background_color!(button_secondary_hover, fallback = tokens.secondary_hover);
        apply_background_color!(button_secondary_active, fallback = tokens.secondary_active);
        apply_background_color!(success, fallback = self.green);
        apply_color!(success_foreground, fallback = self.primary_foreground);
        apply_background_color!(
            success_hover,
            fallback = self.background.blend(self.success.opacity(hover_opacity))
        );
        apply_background_color!(
            success_active,
            fallback = self.success.darken(active_darken)
        );
        apply_background_color!(
            button_success,
            fallback = self.success.mix_oklab(transparent, 0.2)
        );
        apply_color!(button_success_foreground, fallback = self.success);
        apply_background_color!(
            button_success_hover,
            fallback = self.success.mix_oklab(transparent, 0.3)
        );
        apply_background_color!(
            button_success_active,
            fallback = self.success.mix_oklab(transparent, 0.4)
        );
        apply_background_color!(info, fallback = self.cyan);
        apply_color!(info_foreground, fallback = self.primary_foreground);
        apply_background_color!(
            info_hover,
            fallback = self.background.blend(self.info.opacity(hover_opacity))
        );
        apply_background_color!(info_active, fallback = self.info.darken(active_darken));
        apply_background_color!(
            button_info,
            fallback = self.info.mix_oklab(transparent, 0.2)
        );
        apply_color!(button_info_foreground, fallback = self.info);
        apply_background_color!(
            button_info_hover,
            fallback = self.info.mix_oklab(transparent, 0.3)
        );
        apply_background_color!(
            button_info_active,
            fallback = self.info.mix_oklab(transparent, 0.4)
        );
        apply_background_color!(warning, fallback = self.yellow);
        apply_color!(warning_foreground, fallback = self.primary_foreground);
        apply_background_color!(
            warning_hover,
            fallback = self.background.blend(self.warning.opacity(0.9))
        );
        apply_background_color!(
            warning_active,
            fallback = self.background.blend(self.warning.darken(active_darken))
        );
        apply_background_color!(
            button_warning,
            fallback = self.warning.mix_oklab(transparent, 0.2)
        );
        apply_color!(button_warning_foreground, fallback = self.warning);
        apply_background_color!(
            button_warning_hover,
            fallback = self.warning.mix_oklab(transparent, 0.3)
        );
        apply_background_color!(
            button_warning_active,
            fallback = self.warning.mix_oklab(transparent, 0.4)
        );

        apply_background_color!(accent, fallback = tokens.secondary);
        apply_color!(accent_foreground, fallback = self.foreground);
        apply_background_color!(accordion, fallback = tokens.background);
        apply_background_color!(accordion_hover, fallback = self.accent.opacity(0.8));
        apply_background_color!(
            group_box,
            fallback = self
                .background
                .blend(
                    self.secondary
                        .opacity(if config.mode.is_dark() { 0.3 } else { 0.4 })
                )
        );
        apply_color!(group_box_foreground, fallback = self.foreground);
        apply_color!(caret, fallback = self.primary);
        apply_color!(chart_1, fallback = self.blue.lighten(0.4));
        apply_color!(chart_2, fallback = self.blue.lighten(0.2));
        apply_color!(chart_3, fallback = self.blue);
        apply_color!(chart_4, fallback = self.blue.darken(0.2));
        apply_color!(chart_5, fallback = self.blue.darken(0.4));
        apply_color!(chart_bullish, fallback = self.green);
        apply_color!(chart_bearish, fallback = self.red);
        apply_background_color!(danger, fallback = self.red);
        apply_background_color!(danger_active, fallback = self.danger.darken(active_darken));
        apply_color!(danger_foreground, fallback = self.primary_foreground);
        apply_background_color!(
            danger_hover,
            fallback = self.background.blend(self.danger.opacity(0.9))
        );
        apply_background_color!(
            button_danger,
            fallback = self.danger.mix_oklab(transparent, 0.2)
        );
        apply_color!(button_danger_foreground, fallback = self.danger);
        apply_background_color!(
            button_danger_hover,
            fallback = self.danger.mix_oklab(transparent, 0.3)
        );
        apply_background_color!(
            button_danger_active,
            fallback = self.danger.mix_oklab(transparent, 0.4)
        );
        apply_background_color!(
            description_list_label,
            fallback = self.background.blend(self.border.opacity(0.2))
        );
        apply_color!(
            description_list_label_foreground,
            fallback = self.muted_foreground
        );
        apply_color!(drag_border, fallback = self.primary.opacity(0.65));
        apply_background_color!(drop_target, fallback = self.primary.opacity(0.2));
        apply_color!(link, fallback = self.primary);
        apply_color!(link_active, fallback = self.link);
        apply_color!(link_hover, fallback = self.link);
        apply_background_color!(list, fallback = tokens.background);
        apply_background_color!(
            list_active,
            fallback = self.background.blend(self.primary.opacity(0.1))
        );
        apply_color!(
            list_active_border,
            fallback = self.background.blend(self.primary.opacity(0.6))
        );
        apply_background_color!(list_even, fallback = tokens.list);
        apply_background_color!(list_head, fallback = tokens.list);
        apply_background_color!(list_hover, fallback = self.accent.opacity(0.6));
        apply_background_color!(popover, fallback = tokens.background);
        apply_color!(popover_foreground, fallback = self.foreground);
        apply_background_color!(progress_bar, fallback = tokens.primary);
        apply_color!(ring, fallback = self.blue);
        apply_background_color!(scrollbar, fallback = tokens.background);
        apply_background_color!(scrollbar_thumb, fallback = tokens.accent);
        apply_background_color!(scrollbar_thumb_hover, fallback = tokens.scrollbar_thumb);
        apply_background_color!(selection, fallback = tokens.primary);
        apply_background_color!(
            sidebar,
            fallback = self.background.blend(self.border.opacity(0.15))
        );
        apply_background_color!(sidebar_accent, fallback = tokens.accent);
        apply_color!(sidebar_accent_foreground, fallback = self.accent_foreground);
        apply_color!(sidebar_border, fallback = self.border);
        apply_color!(sidebar_foreground, fallback = self.foreground);
        apply_background_color!(sidebar_primary, fallback = tokens.primary);
        apply_color!(
            sidebar_primary_foreground,
            fallback = self.primary_foreground
        );
        apply_background_color!(skeleton, fallback = tokens.secondary);
        apply_background_color!(slider_bar, fallback = tokens.primary);
        apply_background_color!(slider_thumb, fallback = self.primary_foreground);
        apply_background_color!(switch, fallback = tokens.secondary_active);
        apply_background_color!(switch_thumb, fallback = tokens.background);
        apply_background_color!(tab, fallback = tokens.background);
        apply_background_color!(tab_active, fallback = tokens.background);
        apply_color!(tab_active_foreground, fallback = self.foreground);
        apply_background_color!(tab_bar, fallback = tokens.background);
        apply_background_color!(tab_bar_segmented, fallback = tokens.secondary);
        apply_color!(tab_foreground, fallback = self.foreground);
        apply_background_color!(table, fallback = tokens.list);
        apply_background_color!(table_active, fallback = tokens.list_active);
        apply_color!(table_active_border, fallback = self.list_active_border);
        apply_background_color!(table_even, fallback = tokens.list_even);
        apply_background_color!(table_head, fallback = tokens.list_head);
        apply_color!(table_head_foreground, fallback = self.muted_foreground);
        apply_background_color!(table_foot, fallback = tokens.list_head);
        apply_color!(table_foot_foreground, fallback = self.muted_foreground);
        apply_background_color!(table_hover, fallback = tokens.list_hover);
        apply_color!(table_row_border, fallback = self.border);
        apply_background_color!(title_bar, fallback = tokens.background);
        apply_color!(title_bar_border, fallback = self.border);
        apply_background_color!(status_bar, fallback = tokens.title_bar);
        apply_color!(status_bar_border, fallback = self.title_bar_border);
        apply_background_color!(tiles, fallback = tokens.background);
        apply_background_color!(overlay);
        apply_color!(window_border, fallback = self.border);

        // TODO: apply default fallback colors to highlight.
        let clamp_alpha = |raw: Option<&str>, color: Hsla, background: Background, max: f32| {
            let base = color.a;
            let target = base.min(max);
            let color = color.alpha(target);
            let background = raw
                .and_then(|value| try_parse_background_clamped(value, max).ok())
                .unwrap_or_else(|| {
                    let factor = if base > 0. { target / base } else { 1. };
                    background.opacity(factor)
                });
            (color, ThemeToken::new(color, background))
        };

        (self.list_active, tokens.list_active) = clamp_alpha(
            colors.list_active.as_deref(),
            self.list_active,
            tokens.list_active.background,
            0.2,
        );
        (self.table_active, tokens.table_active) = clamp_alpha(
            colors.table_active.as_deref(),
            self.table_active,
            tokens.table_active.background,
            0.2,
        );
        (self.selection, tokens.selection) = clamp_alpha(
            colors.selection.as_deref(),
            self.selection,
            tokens.selection.background,
            0.3,
        );

        tokens
    }
}

impl Theme {
    pub fn apply_config(&mut self, config: &Rc<ThemeConfig>) {
        if config.mode.is_dark() {
            self.dark_theme = config.clone();
        } else {
            self.light_theme = config.clone();
        }
        if let Some(style) = &config.highlight {
            let highlight_theme = Arc::new(HighlightTheme {
                name: config.name.to_string(),
                appearance: config.mode,
                style: style.clone(),
            });
            self.highlight_theme = highlight_theme.clone();
        }

        let default_colors = if config.mode.is_dark() {
            ThemeColor::dark()
        } else {
            ThemeColor::light()
        };

        if let Some(font_size) = config.font_size {
            self.font_size = px(font_size);
        }
        if let Some(font_family) = &config.font_family {
            self.font_family = font_family.clone();
        }
        if let Some(mono_font_family) = &config.mono_font_family {
            self.mono_font_family = mono_font_family.clone();
        }
        if let Some(mono_font_size) = config.mono_font_size {
            self.mono_font_size = px(mono_font_size);
        }
        if let Some(radius) = config.radius {
            self.radius = px(radius as f32);
        }
        if let Some(radius_lg) = config.radius_lg {
            self.radius_lg = px(radius_lg as f32);
        }
        if let Some(shadow) = config.shadow {
            self.shadow = shadow;
        }

        self.tokens = self.colors.apply_config(&config, &default_colors);
        self.mode = config.mode;
    }
}

#[cfg(test)]
mod tests {
    use gpui::{linear_color_stop, linear_gradient};

    use crate::{Theme, ThemeConfig, ThemeMode, ThemeSet, try_parse_color};

    #[test]
    fn test_apply_config_preserves_gradient_background_and_solid_color_fallback() {
        let config = serde_json::from_value::<ThemeConfig>(serde_json::json!({
            "name": "Gradient",
            "mode": "light",
            "colors": {
                "primary.background": "linear-gradient(135deg, #4F46E5, #06B6D4)",
                "button.primary.hover.background": "linear-gradient(to right, red-500 25%, blue-600 75%)"
            }
        }))
        .unwrap();

        let mut theme = Theme::default();
        theme.apply_config(&std::rc::Rc::new(config));

        let primary_from = try_parse_color("#4F46E5").unwrap();
        let primary_to = try_parse_color("#06B6D4").unwrap();
        assert_eq!(theme.primary, primary_from);
        assert_eq!(theme.tokens.primary.color, primary_from);
        assert_eq!(
            theme.tokens.primary.background,
            linear_gradient(
                135.,
                linear_color_stop(primary_from, 0.),
                linear_color_stop(primary_to, 1.)
            )
        );
        assert_eq!(
            theme.tokens.button_primary.background,
            theme.tokens.primary.background
        );
        assert_eq!(
            theme.tokens.button_primary_hover.background,
            linear_gradient(
                90.,
                linear_color_stop(crate::red_500(), 0.25),
                linear_color_stop(crate::blue_600(), 0.75)
            )
        );
        assert_eq!(theme.mode, ThemeMode::Light);
    }

    #[test]
    fn test_aurora_theme_parses_gradient_backgrounds() {
        let theme_set =
            serde_json::from_str::<ThemeSet>(include_str!("../../../../themes/aurora.json"))
                .unwrap();
        assert_eq!(theme_set.themes.len(), 1);
        assert!(theme_set.themes.iter().all(|theme| !theme.mode.is_dark()));

        let light = theme_set
            .themes
            .iter()
            .find(|theme| theme.name.as_ref() == "Aurora Light")
            .unwrap();
        let mut theme = Theme::default();
        theme.apply_config(&std::rc::Rc::new(light.clone()));

        assert_ne!(
            theme.tokens.button_primary.background,
            theme.button_primary.into()
        );
        assert_eq!(theme.tokens.background.background, theme.background.into());
        assert_eq!(theme.button_primary, try_parse_color("#1E293B").unwrap());
        assert_eq!(theme.background, try_parse_color("#FFFFFF").unwrap());
        assert_ne!(
            theme.tokens.progress_bar.background,
            theme.progress_bar.into()
        );
        assert_ne!(
            theme.tokens.scrollbar_thumb.background,
            theme.scrollbar_thumb.into()
        );
        assert_ne!(theme.tokens.switch.background, theme.switch.into());
        assert_ne!(
            theme.tokens.switch_thumb.background,
            theme.switch_thumb.into()
        );
        assert_ne!(theme.tokens.title_bar.background, theme.title_bar.into());
        assert_ne!(theme.tokens.status_bar.background, theme.status_bar.into());
    }

    #[test]
    fn test_apply_config_clamps_highlight_alpha_per_gradient_stop() {
        let config = serde_json::from_value::<ThemeConfig>(serde_json::json!({
            "name": "Highlight",
            "mode": "light",
            "colors": {
                // Must be capped to 0.2, not attenuated twice.
                "list.active.background": "#3b82f6",
                // The opaque `to` stop must be clamped independently.
                "table.active.background": "linear-gradient(#bfdbfe33, #3b82f6)",
                // A transparent `from` stop used to leave the opaque `to` stop unclamped (base==0 factor fallback bug).
                "selection.background": "linear-gradient(#3b82f600, #3b82f6)",
            }
        }))
        .unwrap();

        let mut theme = Theme::default();
        theme.apply_config(&std::rc::Rc::new(config));

        let blue = try_parse_color("#3b82f6").unwrap();
        assert_eq!(theme.list_active, blue.alpha(0.2));
        assert_eq!(theme.tokens.list_active.background, blue.alpha(0.2).into());

        let faint = try_parse_color("#bfdbfe33").unwrap();
        assert_eq!(
            theme.tokens.table_active.background,
            linear_gradient(
                180.,
                linear_color_stop(faint.alpha(faint.a.min(0.2)), 0.),
                linear_color_stop(blue.alpha(0.2), 1.),
            )
        );

        let clear = try_parse_color("#3b82f600").unwrap();
        assert_eq!(
            theme.tokens.selection.background,
            linear_gradient(
                180.,
                linear_color_stop(clear.alpha(clear.a.min(0.3)), 0.),
                linear_color_stop(blue.alpha(0.3), 1.),
            )
        );
    }
}

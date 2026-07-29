/// One settings entry surfaced by the palette, either as a root-mode direct
/// match (`PaletteRow::Setting`) or as a row in the Settings drill-in.
/// Ordering of the variants is the drill-in's display order within its
/// section (see `SettingRow::ALL`, `section`) — enum panes for the "enum"
/// rows (`Theme`/`Backend`/`Permissions`/`DefaultAgent`/`AppSize`) are a
/// later phase; here they're inert stubs (see `Grove::activate_setting`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum SettingRow {
    Theme,
    AppSize,
    ProjectThemes,
    Backend,
    Permissions,
    Telemetry,
    Chrome,
    DefaultAgent,
    CheckUpdates,
}

impl SettingRow {
    /// Every setting, in section/definition (= drill-in display) order.
    pub(in crate::gui) const ALL: [SettingRow; 9] = [
        SettingRow::Theme,
        SettingRow::AppSize,
        SettingRow::ProjectThemes,
        SettingRow::Backend,
        SettingRow::Permissions,
        SettingRow::Telemetry,
        SettingRow::Chrome,
        SettingRow::DefaultAgent,
        SettingRow::CheckUpdates,
    ];

    pub(in crate::gui) fn label(self) -> &'static str {
        match self {
            SettingRow::Theme => "App theme",
            SettingRow::AppSize => "App size",
            SettingRow::ProjectThemes => "Project themes",
            SettingRow::Backend => "Backend",
            SettingRow::Permissions => "Permissions",
            SettingRow::Telemetry => "Telemetry",
            SettingRow::Chrome => "Claude in Chrome",
            SettingRow::DefaultAgent => "Default agent",
            SettingRow::CheckUpdates => "Check for updates",
        }
    }

    /// Name of the inline SVG sprite (see `gui::icons`) shown in this row's
    /// leading 24px icon slot. `ProjectThemes`/`Telemetry` render a checkbox
    /// glyph instead (see `palette_row_view`'s `Setting` arm) and never
    /// consult this. Several picks are the nearest existing sprite standing
    /// in for one the redesign mock uses that isn't in `icons.rs` — see the
    /// per-arm comments below.
    pub(in crate::gui) fn icon_name(self) -> &'static str {
        match self {
            // Mock uses a dedicated palette glyph; `contrast` (the existing
            // light/dark toggle icon) is the closest stand-in for "theme".
            SettingRow::Theme => "contrast",
            // Mock's own choice — already in `icons.rs`.
            SettingRow::AppSize => "grid",
            SettingRow::ProjectThemes => "check",
            // Mock uses a monitor glyph; `term` (terminal) is the closest
            // existing sprite for "backend".
            SettingRow::Backend => "term",
            // Mock uses a shield glyph; `ring` (a plain protective circle)
            // is the closest existing stand-in.
            SettingRow::Permissions => "ring",
            SettingRow::Telemetry => "check",
            SettingRow::Chrome => "check",
            // Mock uses a bot glyph; `sparkle` is the closest existing
            // "agent/AI" stand-in.
            SettingRow::DefaultAgent => "sparkle",
            // Matches the existing refresh icon used for the same action in
            // `settings_modal` (view.rs).
            SettingRow::CheckUpdates => "restart",
        }
    }

    pub(in crate::gui) fn section(self) -> &'static str {
        match self {
            SettingRow::Theme | SettingRow::AppSize | SettingRow::ProjectThemes => "APPEARANCE",
            SettingRow::Backend
            | SettingRow::Permissions
            | SettingRow::Telemetry
            | SettingRow::Chrome => "AGENTS / TERMINAL",
            SettingRow::DefaultAgent => "TOOLS",
            SettingRow::CheckUpdates => "UPDATES",
        }
    }

    /// Rows that flip in place instead of opening a pane. Toggles render a
    /// checkbox glyph rather than their icon, get no chevron, and re-anchor
    /// the palette cursor after flipping.
    pub(in crate::gui) fn is_toggle(self) -> bool {
        matches!(
            self,
            SettingRow::ProjectThemes | SettingRow::Telemetry | SettingRow::Chrome
        )
    }
}

#[cfg(test)]
mod tests {
    use super::SettingRow;

    #[test]
    fn setting_row_label_section_and_icon_are_total_and_nonempty() {
        for s in SettingRow::ALL {
            assert!(!s.label().is_empty());
            assert!(!s.section().is_empty());
            assert!(!s.icon_name().is_empty());
        }
        // Spot-check a few, so a typo'd match arm can't silently return the
        // wrong (but still non-empty) string for the wrong variant.
        assert_eq!(SettingRow::Telemetry.label(), "Telemetry");
        assert_eq!(SettingRow::Telemetry.section(), "AGENTS / TERMINAL");
        assert_eq!(SettingRow::CheckUpdates.label(), "Check for updates");
        assert_eq!(SettingRow::CheckUpdates.section(), "UPDATES");
    }

    #[test]
    fn settings_row_keyword_matches_root_query() {
        // `palette_rows` needs a full `Grove` to construct (sessions, PTYs,
        // store, …) — impractical in a unit test — so this exercises the
        // exact keyword condition it uses to surface `PaletteRow::Settings`
        // while typing at root (not `browse_all`): test (b), "typed input
        // 'settings' yields a Settings row".
        assert!(crate::gui::launcher::fuzzy_match(
            "settings", "settings", "", ""
        ));
        assert!(crate::gui::launcher::fuzzy_match("set", "settings", "", ""));
        assert!(!crate::gui::launcher::fuzzy_match(
            "zzz", "settings", "", ""
        ));
    }
}

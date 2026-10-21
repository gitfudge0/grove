//! Native macOS menus. Existing actions remain the owners of application behavior.
use crate::{
    keymap as k,
    views::shell::{CloseWindow, Quit},
};
use gpui::{actions, App, Global, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType};
use gpui_component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};

actions!(
    app_menus,
    [
        About,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        ZoomWindow,
        BringAllToFront
    ]
);

/// Values are captured from the same state used by the shell's action guards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MenuState {
    pub window: bool,
    pub navigation: bool,
    pub switcher: bool,
    pub worktree: bool,
    pub close_session: bool,
    pub sessions: bool,
    pub waiting: bool,
    pub terminal_edit: Option<(bool, bool)>,
    pub grid: bool,
    pub zen: bool,
    pub collapsed: bool,
    pub list: bool,
}
impl Global for MenuState {}

pub(crate) fn install(cx: &mut App) {
    cx.set_global(MenuState::default());
    cx.on_action(|_: &Quit, cx| {
        if cx.windows().is_empty() {
            cx.quit();
        }
    });
    cx.on_action(|_: &Hide, cx| cx.hide())
        .on_action(|_: &HideOthers, cx| cx.hide_other_apps())
        .on_action(|_: &ShowAll, cx| cx.unhide_other_apps())
        .on_action(|_: &BringAllToFront, cx| {
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
        });
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    cx.set_menus(menus(MenuState::default()));
}

pub(crate) fn refresh(state: MenuState, cx: &mut App) {
    if cx.try_global::<MenuState>() == Some(&state) {
        return;
    }
    cx.set_global(state);
    cx.set_menus(menus(state));
}

fn menus(s: MenuState) -> Vec<Menu> {
    let nav = |item: MenuItem| item.disabled(!s.navigation);
    let mut menus = vec![
        Menu::new("Grove").items([
            MenuItem::action("About Grove", About).disabled(!s.window),
            MenuItem::separator(),
            nav(MenuItem::action("Settings…", k::Settings)),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Grove", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Grove", Quit),
        ]),
        Menu::new("File").items([
            nav(MenuItem::action("New Session…", k::NewSession)),
            MenuItem::action("New Session in Worktree…", k::NewSessionInWorktree)
                .disabled(!s.navigation || !s.worktree),
            nav(MenuItem::action("New Terminal", k::NewHomeTerminal)),
            MenuItem::separator(),
            MenuItem::action("Close Session / Terminal", k::CloseFocusedSession)
                .disabled(!s.navigation || !s.close_session),
            MenuItem::action("Close Window", CloseWindow).disabled(!s.window),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy)
                .disabled(s.terminal_edit.is_some_and(|(copy, _)| !copy)),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll)
                .disabled(s.terminal_edit.is_some_and(|(_, all)| !all)),
        ]),
        Menu::new("View").items([
            nav(MenuItem::action("Grid", k::ToggleGrid).checked(s.grid)),
            nav(MenuItem::action("Zen", k::ToggleZen).checked(s.zen)),
            nav(MenuItem::action(
                if s.collapsed {
                    "Show Sidebar"
                } else {
                    "Hide Sidebar"
                },
                k::ToggleSidebar,
            )),
            nav(MenuItem::action("Session List", k::ToggleRailMode).checked(s.list)),
            MenuItem::separator(),
            MenuItem::action("Zoom In", k::ZoomIn).disabled(!s.window),
            MenuItem::action("Zoom Out", k::ZoomOut).disabled(!s.window),
            MenuItem::action("Actual Size", k::ZoomReset).disabled(!s.window),
        ]),
        Menu::new("Session").items([
            MenuItem::action("Switch Workspace or Session…", k::SwitchSession)
                .disabled(!s.switcher),
            MenuItem::separator(),
            MenuItem::action("Next Session", k::NextSession).disabled(!s.navigation || !s.sessions),
            MenuItem::action("Previous Session", k::PrevSession)
                .disabled(!s.navigation || !s.sessions),
            MenuItem::action("Jump to Session Needing You", k::JumpToWaitingSession)
                .disabled(!s.navigation || !s.waiting),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize).disabled(!s.window),
            MenuItem::action("Zoom", ZoomWindow).disabled(!s.window),
            MenuItem::separator(),
            MenuItem::action("Bring All to Front", BringAllToFront).disabled(!s.window),
        ]),
        Menu::new("Help").items([nav(MenuItem::action(
            "Keyboard Shortcuts",
            k::ShortcutOverlay,
        ))]),
    ];
    // The pinned macOS backend validates listener presence, ignoring `disabled`.
    // An unavailable action must therefore also be absent from the dispatch tree.
    for item in menus.iter_mut().flat_map(|menu| &mut menu.items) {
        if let MenuItem::Action {
            action,
            os_action,
            disabled: true,
            ..
        } = item
        {
            *action = Box::new(gpui::NoAction);
            *os_action = None;
        }
    }
    menus
}

#[cfg(test)]
mod tests {
    use super::*;
    fn find<'a>(menus: &'a [Menu], name: &str) -> &'a MenuItem {
        menus
            .iter()
            .flat_map(|menu| &menu.items)
            .find(|item| matches!(item, MenuItem::Action {name: n, ..} if n == name))
            .expect("menu command")
    }
    fn menus_for_close() -> Vec<Menu> {
        menus(MenuState {
            window: true,
            navigation: true,
            close_session: true,
            ..MenuState::default()
        })
    }
    #[test]
    fn unavailable_contexts_disable_commands_without_changing_close_actions() {
        let menus = menus(MenuState::default());
        assert_eq!(
            menus.iter().map(|m| m.name.as_ref()).collect::<Vec<_>>(),
            ["Grove", "File", "Edit", "View", "Session", "Window", "Help"]
        );
        assert!(find(&menus, "New Session…").is_disabled());
        assert!(find(&menus, "Close Window").is_disabled());
        assert!(!find(&menus, "Quit Grove").is_disabled());
        assert!(
            matches!(find(&menus,"Close Session / Terminal"), MenuItem::Action {action,..} if action.as_any().is::<gpui::NoAction>())
        );
        let enabled = menus_for_close();
        assert!(
            matches!(find(&enabled,"Close Session / Terminal"), MenuItem::Action {action,..} if action.as_any().is::<k::CloseFocusedSession>())
        );
        assert!(
            matches!(find(&enabled,"Close Window"), MenuItem::Action {action,..} if action.as_any().is::<CloseWindow>())
        );
    }
    #[test]
    fn selection_and_view_state_control_availability_and_checks() {
        let menus = menus(MenuState {
            window: true,
            navigation: true,
            grid: true,
            collapsed: true,
            ..MenuState::default()
        });
        assert!(!find(&menus, "New Session…").is_disabled());
        assert!(find(&menus, "New Session in Worktree…").is_disabled());
        assert!(find(&menus, "Close Session / Terminal").is_disabled());
        assert!(find(&menus, "Grid").is_checked());
        assert!(!find(&menus, "Show Sidebar").is_checked());
        assert!(!find(&menus, "Zen").is_checked());
        assert!(find(&menus, "Jump to Session Needing You").is_disabled());
        let terminal = super::menus(MenuState {
            terminal_edit: Some((false, false)),
            ..MenuState::default()
        });
        assert!(find(&terminal, "Copy").is_disabled());
        assert!(find(&terminal, "Select All").is_disabled());
        assert!(
            matches!(find(&menus,"Copy"), MenuItem::Action {os_action:Some(OsAction::Copy),action,..} if action.as_any().is::<Copy>())
        );
    }
}

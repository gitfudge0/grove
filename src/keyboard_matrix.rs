//! One table-driven test per contract, covering every `SHORTCUTS` row ×
//! {Workspace, Grid, Zen, each modal} × armed/disarmed, asserting the dispatch
//! target: PTY bytes, a named action, or swallowed. Expressed against the two
//! pure deciders — [`crate::modal`]'s verdict table and [`crate::keymap`]'s
//! registry — since that's exactly where the runtime decision is made.

#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::keymap::{
    self, contexts_for, keystrokes_for, GlobalShortcut, Scope, Screen, ShortcutDef, SHORTCUTS,
};
use crate::modal::{
    bound_chords, escape_should_dismiss, key_verdict, KeyCtx, Modal, ModalKey, ModalKeyVerdict,
    ModalKind, ModalMods,
};

/// Where a keystroke ends up.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Pty,
    Action(&'static str),
    Swallowed,
}

fn sample_modal(kind: ModalKind) -> Modal {
    use crate::modal::*;
    match kind {
        ModalKind::Input => Modal::Input {
            title: "t".into(),
            buffer: String::new(),
            note: None,
        },
        ModalKind::Confirm => Modal::Confirm {
            title: "t".into(),
            prompt: "p".into(),
            destructive: true,
            kind: ConfirmKind::Quit,
        },
        ModalKind::AddProject => Modal::AddProject(Box::default()),
        ModalKind::RemoveProject => Modal::RemoveProject {
            idx: 0,
            name: "p".into(),
            project_path: "/p".into(),
            worktrees: vec![],
            also_remove_worktrees: false,
            in_progress: false,
            done: 0,
            current: String::new(),
            errors: vec![],
        },
        ModalKind::ArchiveProject => Modal::ArchiveProject {
            idx: 0,
            name: "p".into(),
            sessions: vec![],
        },
        ModalKind::ArchivedProjects => Modal::ArchivedProjects,
        ModalKind::Message => Modal::Message("m".into()),
        ModalKind::TmuxChoice => Modal::TmuxChoice,
        ModalKind::AgentPicker => Modal::AgentPicker {
            project: "p".into(),
            wt_path: "/w".into(),
            sel: 0,
        },
        ModalKind::SessionLauncher => Modal::SessionLauncher(Box::default()),
        ModalKind::ThemePicker => Modal::ThemePicker {
            sel_dark: 0,
            sel_light: 0,
            dark_tab: true,
            original: "tokyonight-storm".into(),
            follow_system: false,
            scope: ThemePickerScope::App,
            project_use_default: false,
            return_to: ThemePickerReturn::Close,
        },
        ModalKind::ThemeManager => Modal::ThemeManager {
            selected: 0,
            rename: None,
            rename_error: None,
            pending_delete: None,
            editor: None,
        },
        ModalKind::Settings => Modal::Settings,
        ModalKind::ShortcutOverlay => Modal::ShortcutOverlay,
        ModalKind::Teardown => Modal::Teardown {
            wt_path: "/w".into(),
            project_path: "/p".into(),
            stage: TeardownStage::RunningScript,
            message: String::new(),
            removal_started: false,
        },
        ModalKind::ScriptsEditor => Modal::ScriptsEditor(Box::default()),
        ModalKind::Updating => Modal::Updating,
        ModalKind::Changelog => Modal::Changelog {
            return_to_settings: true,
        },
        ModalKind::DiffViewer => Modal::DiffViewer {
            wt_path: "/w".into(),
        },
        ModalKind::Onboarding => Modal::Onboarding {
            step: OnboardStep::Project,
            path: String::new(),
            dir_sel: 0,
            name: None,
            note: None,
            added_proj: None,
            agent_sel: 0,
            perms_skip: false,
            name_focused: false,
        },
    }
}

/// While a modal is open the workspace drops its screen key-context, so no screen-scoped chord fires from behind the scrim.
fn dispatch_with_modal(kind: ModalKind, key: ModalKey, mods: ModalMods, ctx: KeyCtx) -> Target {
    let modal = sample_modal(kind);
    match key_verdict(&modal, key, mods, ctx) {
        ModalKeyVerdict::FallThrough => Target::Action("modal-delegate"),
        ModalKeyVerdict::Ignore => Target::Swallowed,
        ModalKeyVerdict::Close => Target::Action("cancel"),
        ModalKeyVerdict::Submit => Target::Action("submit"),
        ModalKeyVerdict::Move(_) => Target::Action("move"),
        ModalKeyVerdict::Custom(_) => Target::Action("custom"),
    }
}

fn dispatch_on_screen(def: &ShortcutDef, screen: Screen) -> Target {
    let Some(action) = def.action else {
        if is_non_mac_platform_mod_letter_row(def) {
            return Target::Swallowed;
        }
        return Target::Pty;
    };
    let allowed = contexts_for(def).into_iter().any(|ctx| match ctx {
        None => true,
        Some(name) => name == screen.key_context(),
    });
    if allowed {
        Target::Action(match action {
            GlobalShortcut::NewSession => "NewSession",
            GlobalShortcut::NewSessionInWorktree => "NewSessionInWorktree",
            GlobalShortcut::Settings => "Settings",
            GlobalShortcut::ToggleZen => "ToggleZen",
            GlobalShortcut::ToggleGrid => "ToggleGrid",
            GlobalShortcut::ZoomIn => "ZoomIn",
            GlobalShortcut::ZoomOut => "ZoomOut",
            GlobalShortcut::ZoomReset => "ZoomReset",
            GlobalShortcut::NextSession => "NextSession",
            GlobalShortcut::PrevSession => "PrevSession",
            GlobalShortcut::SelectSession(_) => "SelectSession",
            GlobalShortcut::ShortcutOverlay => "ShortcutOverlay",
            GlobalShortcut::CloseFocusedSession => "CloseFocusedSession",
            GlobalShortcut::NewHomeTerminal => "NewHomeTerminal",
            GlobalShortcut::ToggleTerminal => "ToggleTerminal",
            GlobalShortcut::ToggleTermPanel => "ToggleTermPanel",
            GlobalShortcut::ToggleRailMode => "ToggleRailMode",
            GlobalShortcut::JumpToWaitingSession => "JumpToWaitingSession",
            GlobalShortcut::GridMove(..) => "GridMove",
            GlobalShortcut::GridSwap(..) => "GridSwap",
            GlobalShortcut::ScrollHalfPage(_) => "ScrollHalfPage",
            GlobalShortcut::SwitchSession => "SwitchSession",
        })
    } else {
        Target::Pty
    }
}

/// On non-mac, grid-nav rows resolve to ctrl-shift-letter, which `key_to_bytes` swallows
/// outright as the app's global-shortcut modifier, before it ever reaches the terminal.
fn is_non_mac_platform_mod_letter_row(def: &ShortcutDef) -> bool {
    cfg!(not(target_os = "macos"))
        && matches!(def.description, "Move focus in grid" | "Move tile in grid")
}

const SCREENS: [Screen; 3] = [Screen::Workspace, Screen::Grid, Screen::Zen];

const PLATFORM: ModalMods = ModalMods {
    ctrl: false,
    alt: false,
    shift: false,
    platform: true,
};

#[test]
fn escape_despite_capture() {
    for kind in [
        ModalKind::Input,
        ModalKind::AddProject,
        ModalKind::Onboarding,
        ModalKind::ScriptsEditor,
    ] {
        let t = dispatch_with_modal(kind, ModalKey::Escape, ModalMods::NONE, KeyCtx::default());
        assert_ne!(t, Target::Pty, "{kind:?} must claim Escape");
        assert_ne!(t, Target::Swallowed, "{kind:?} must act on Escape");
    }
    assert!(!escape_should_dismiss(false, false, false, false));
    assert!(escape_should_dismiss(true, false, false, false));
}

#[test]
fn palette_arrow_carve_outs() {
    for kind in ModalKind::ALL {
        let claims = kind.wants_arrows();
        assert_eq!(
            claims,
            matches!(kind, ModalKind::SessionLauncher | ModalKind::AddProject),
            "{kind:?}"
        );
    }
    for key in [ModalKey::Left, ModalKey::Right] {
        assert_eq!(
            dispatch_with_modal(
                ModalKind::SessionLauncher,
                key,
                ModalMods::NONE,
                KeyCtx::default()
            ),
            Target::Action("modal-delegate")
        );
    }
    for def in SHORTCUTS {
        for ks in keystrokes_for(def) {
            assert!(
                !ks.ends_with("left") && !ks.ends_with("right") || ks.contains('-'),
                "a bare arrow must never be a global chord: {ks}"
            );
        }
    }
}

#[test]
fn cmd_chord_suppression_in_the_palette() {
    assert_eq!(
        dispatch_with_modal(
            ModalKind::SessionLauncher,
            ModalKey::Char('d'),
            PLATFORM,
            KeyCtx::default()
        ),
        Target::Action("modal-delegate")
    );
    // Drift guard restated here so the matrix owns it too.
    for kind in ModalKind::ALL {
        for chord in bound_chords(kind) {
            let ctx = KeyCtx {
                update_in_flight: false,
                is_shortcut_overlay_chord: kind == ModalKind::ShortcutOverlay,
            };
            let verdict = key_verdict(&sample_modal(kind), chord.key, chord.mods, ctx);
            assert!(
                !matches!(
                    verdict,
                    ModalKeyVerdict::Ignore | ModalKeyVerdict::FallThrough
                ),
                "{kind:?} binds {chord:?} but the table returns {verdict:?}"
            );
        }
    }
}

#[test]
fn alt_escape_reaches_the_pty() {
    for def in SHORTCUTS {
        for ks in keystrokes_for(def) {
            assert!(
                !ks.contains("escape"),
                "no registry row may bind Escape: {ks}"
            );
        }
    }
    let bytes = crate::terminal::keys::key_to_bytes(
        &gpui::Keystroke {
            modifiers: gpui::Modifiers {
                alt: true,
                ..Default::default()
            },
            key: "escape".into(),
            key_char: None,
        },
        false,
    );
    assert_eq!(
        bytes.as_deref(),
        Some(b"\x1b\x1b".as_slice()),
        "alt+escape must reach the PTY as ESC ESC"
    );
}

#[test]
fn two_step_confirm_kill_arming_and_disarming() {
    assert!(escape_should_dismiss(true, false, false, false));
    assert!(escape_should_dismiss(false, true, false, false));
    assert!(escape_should_dismiss(false, false, true, false));
    assert!(escape_should_dismiss(false, false, false, true));
    assert!(!escape_should_dismiss(false, false, false, false));
    // A modal outranks all of it — the modal machine is asked first.
    assert_eq!(
        dispatch_with_modal(
            ModalKind::Settings,
            ModalKey::Escape,
            ModalMods::NONE,
            KeyCtx::default()
        ),
        Target::Action("cancel")
    );
}

/// Two halves: a bound screen-scoped row falls through off its own screen; a display-only
/// row (`action: None`) falls through on every screen including its own.
#[test]
fn screen_scoped_rows_fall_through_off_their_screen() {
    let mut display_only = 0;
    let mut bound = 0;
    for def in SHORTCUTS {
        let screens: Vec<Screen> = def
            .scopes
            .iter()
            .filter_map(|s| match s {
                Scope::Screen(sc) => Some(*sc),
                Scope::Global => None,
            })
            .collect();
        if screens.is_empty() || def.scopes.contains(&Scope::Global) {
            continue;
        }
        if def.action.is_none() {
            display_only += 1;
            for screen in SCREENS {
                let expected = if is_non_mac_platform_mod_letter_row(def) {
                    Target::Swallowed
                } else {
                    Target::Pty
                };
                assert_eq!(
                    dispatch_on_screen(def, screen),
                    expected,
                    "display-only {:?} must resolve to {expected:?} on {screen:?}",
                    def.description
                );
            }
            continue;
        }
        bound += 1;
        for screen in SCREENS {
            let target = dispatch_on_screen(def, screen);
            if screens.contains(&screen) {
                assert_ne!(target, Target::Pty, "{:?} on {screen:?}", def.description);
            } else {
                assert_eq!(
                    target,
                    Target::Pty,
                    "{:?} must fall through on {screen:?}",
                    def.description
                );
            }
        }
    }
    assert!(
        display_only > 0,
        "the registry must still carry its display-only screen rows"
    );
    let probe = ShortcutDef {
        action: Some(GlobalShortcut::ToggleGrid),
        triggers: &["g"],
        display_keys: "g",
        description: "probe",
        scopes: &[Scope::Screen(Screen::Grid)],
        requires_alt: false,
        literal: false,
    };
    assert_eq!(
        dispatch_on_screen(&probe, Screen::Grid),
        Target::Action("ToggleGrid")
    );
    assert_eq!(dispatch_on_screen(&probe, Screen::Zen), Target::Pty);
    assert_eq!(dispatch_on_screen(&probe, Screen::Workspace), Target::Pty);
    let _ = bound;
}

/// Nothing is ever silently swallowed with no modal open, except the one non-mac ctrl-shift-letter exception.
#[test]
fn every_registry_row_on_every_screen_resolves_to_an_action_or_the_pty() {
    for def in SHORTCUTS {
        for screen in SCREENS {
            let t = dispatch_on_screen(def, screen);
            if is_non_mac_platform_mod_letter_row(def) {
                assert_eq!(
                    t,
                    Target::Swallowed,
                    "{:?} on {screen:?} resolved to {t:?}",
                    def.description
                );
                continue;
            }
            assert!(
                matches!(t, Target::Action(_) | Target::Pty),
                "{:?} on {screen:?} resolved to {t:?}",
                def.description
            );
        }
    }
}

#[test]
fn no_registry_chord_fires_from_behind_a_modal() {
    for kind in ModalKind::ALL {
        for def in SHORTCUTS {
            let Some(first) = def.display_keys.chars().next() else {
                continue;
            };
            let mods = if def.requires_alt {
                ModalMods {
                    alt: true,
                    ..PLATFORM
                }
            } else {
                PLATFORM
            };
            let t = dispatch_with_modal(kind, ModalKey::Char(first), mods, KeyCtx::default());
            assert_ne!(
                t,
                Target::Pty,
                "{kind:?} let {:?} reach the PTY from behind the scrim",
                def.description
            );
        }
    }
}

#[test]
fn arming_never_changes_a_modal_verdict() {
    for kind in ModalKind::ALL {
        for key in [
            ModalKey::Escape,
            ModalKey::Enter,
            ModalKey::Tab,
            ModalKey::Space,
            ModalKey::Up,
            ModalKey::Down,
            ModalKey::Left,
            ModalKey::Right,
            ModalKey::Char('y'),
            ModalKey::Char('n'),
            ModalKey::Char('q'),
        ] {
            let modal = sample_modal(kind);
            let a = key_verdict(&modal, key, ModalMods::NONE, KeyCtx::default());
            let b = key_verdict(
                &modal,
                key,
                ModalMods::NONE,
                KeyCtx {
                    update_in_flight: false,
                    is_shortcut_overlay_chord: false,
                },
            );
            assert_eq!(a, b, "{kind:?} {key:?}");
        }
    }
}

/// The one KeyCtx input that changes a verdict: an in-flight update refuses Updating's Escape.
#[test]
fn only_updating_reads_the_in_flight_flag() {
    for kind in ModalKind::ALL {
        let modal = sample_modal(kind);
        let idle = key_verdict(&modal, ModalKey::Escape, ModalMods::NONE, KeyCtx::default());
        let busy = key_verdict(
            &modal,
            ModalKey::Escape,
            ModalMods::NONE,
            KeyCtx {
                update_in_flight: true,
                is_shortcut_overlay_chord: false,
            },
        );
        if kind == ModalKind::Updating {
            assert_ne!(idle, busy, "Updating must refuse Escape mid-update");
        } else {
            assert_eq!(idle, busy, "{kind:?} must ignore the in-flight flag");
        }
    }
}

#[test]
fn every_modal_kind_has_a_unique_key_context() {
    let mut seen = std::collections::HashSet::new();
    for kind in ModalKind::ALL {
        assert!(seen.insert(kind.key_context()), "{kind:?}");
    }
}

#[test]
fn every_registry_row_with_a_label_reaches_the_overlay() {
    for def in SHORTCUTS {
        if def.display_keys.is_empty() {
            continue;
        }
        let shown = SCREENS
            .into_iter()
            .any(|s| crate::views::modals::settings::scope_allows(def, s));
        assert!(shown, "{:?} is invisible on every screen", def.description);
    }
}

/// Pins that all three iced carve-outs (carried decision 3) survive without either static.
#[test]
fn the_three_carve_outs_survive_without_either_static() {
    assert_eq!(
        dispatch_with_modal(
            ModalKind::Input,
            ModalKey::Escape,
            ModalMods::NONE,
            KeyCtx::default()
        ),
        Target::Action("cancel")
    );
    assert_ne!(
        dispatch_with_modal(
            ModalKind::SessionLauncher,
            ModalKey::Char('k'),
            PLATFORM,
            KeyCtx::default()
        ),
        Target::Pty
    );
    assert!(ModalKind::SessionLauncher.wants_arrows());
    assert!(!ModalKind::Settings.wants_arrows());
    assert_eq!(
        keymap::platform_mod_prefix()
            .trim_end_matches('-')
            .replace('-', "+"),
        keymap::platform_mod_label()
    );
}

// Grove registers "<ModalKind> > Input" bindings that win over gpui-component's plain "Input"
// ones by registration order. These two tests pin that the bindings exist and out-rank them.

/// gpui-component's "Input" bindings first, Grove's second — matches real registration order.
fn keymap_as_registered() -> gpui::Keymap {
    use gpui_component::input::{
        Enter, Escape, IndentInline, MoveDown, MoveLeft, MoveRight, MoveUp,
    };

    let mut bindings = vec![
        gpui::KeyBinding::new("up", MoveUp, Some("Input")),
        gpui::KeyBinding::new("down", MoveDown, Some("Input")),
        gpui::KeyBinding::new("left", MoveLeft, Some("Input")),
        gpui::KeyBinding::new("right", MoveRight, Some("Input")),
        gpui::KeyBinding::new("tab", IndentInline, Some("Input")),
        gpui::KeyBinding::new(
            "enter",
            Enter {
                secondary: false,
                shift: false,
            },
            Some("Input"),
        ),
        gpui::KeyBinding::new("escape", Escape, Some("Input")),
    ];
    bindings.extend(keymap::bindings());
    gpui::Keymap::new(bindings)
}

fn modal_field_stack(kind: ModalKind) -> Vec<gpui::KeyContext> {
    vec![
        gpui::KeyContext::try_from(kind.key_context()).unwrap(),
        gpui::KeyContext::try_from("Input").unwrap(),
    ]
}

fn winning_action(kind: ModalKind, key: &str) -> String {
    let keymap = keymap_as_registered();
    let keystroke = gpui::Keystroke::parse(key).unwrap();
    let (matched, _) = keymap.bindings_for_input(&[keystroke], &modal_field_stack(kind));
    matched
        .first()
        .map_or_else(|| "<none>".to_string(), |b| b.action().name().to_string())
}

#[test]
fn every_single_line_modal_binds_the_keys_its_field_would_swallow() {
    use crate::views::modals::input::InputPolicy;

    for kind in ModalKind::ALL {
        let policy = InputPolicy::for_modal(kind);
        if policy.multi_line {
            continue;
        }
        let ctx = format!("{} > Input", kind.key_context());
        let mut keys: Vec<&str> = vec!["up", "down", "enter"];
        if policy.wants_tab {
            keys.push("tab");
            keys.push("shift-tab");
        }
        if policy.wants_arrows {
            keys.push("left");
            keys.push("right");
        }
        for key in keys {
            let found = keymap::modal_input_bindings().into_iter().any(|b| {
                b.predicate().is_some_and(|p| p.to_string() == ctx)
                    && b.keystrokes().len() == 1
                    && b.keystrokes()[0].inner().unparse() == key
            });
            assert!(found, "{kind:?} has no `{ctx}` binding for {key}");
        }
    }
}

/// Fails if `bindings()` is ever registered before `gpui_component::init`.
#[test]
fn the_modal_binding_out_ranks_the_input_binding() {
    for key in ["up", "down", "enter", "left", "right"] {
        let winner = winning_action(ModalKind::SessionLauncher, key);
        assert!(
            winner.starts_with("grove_modal::"),
            "{key} in the palette went to {winner}"
        );
    }
    assert!(winning_action(ModalKind::AddProject, "tab").starts_with("grove_modal::"));
    assert!(!winning_action(ModalKind::SessionLauncher, "tab").starts_with("grove_modal::"));
    for key in ["up", "down", "enter"] {
        let winner = winning_action(ModalKind::ScriptsEditor, key);
        assert!(
            winner.starts_with("grove_modal::"),
            "the scripts editor lost {key} to {winner}"
        );
    }
    assert!(!winning_action(ModalKind::ScriptsEditor, "tab").starts_with("grove_modal::"));
    assert_eq!(
        winning_action(ModalKind::SessionLauncher, "escape"),
        "input::Escape"
    );
}

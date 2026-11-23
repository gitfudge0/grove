use super::*;

fn sid(n: u64) -> SessionId {
    SessionId::from_raw(n)
}

fn fixture() -> TreeSnapshot {
    TreeSnapshot {
        total_projects: 3,
        projects: vec![
            SnapshotProject {
                idx: 0,
                name: "alpha".into(),
                is_git: true,
                has_run: true,
                worktrees: vec![
                    SnapshotWorktree {
                        path: "/a".into(),
                        name: "alpha".into(),
                        branch: "main".into(),
                        is_main: true,
                        sessions: vec![sid(1), sid(2)],
                    },
                    SnapshotWorktree {
                        path: "/a-x".into(),
                        name: "a-x".into(),
                        branch: "feature".into(),
                        is_main: false,
                        sessions: vec![],
                    },
                ],
                sessions: vec![sid(1), sid(2)],
            },
            SnapshotProject {
                idx: 2,
                name: "gamma".into(),
                is_git: true,
                has_run: false,
                worktrees: vec![SnapshotWorktree {
                    path: "/g".into(),
                    name: "gamma".into(),
                    branch: "main".into(),
                    is_main: true,
                    sessions: vec![sid(3)],
                }],
                sessions: vec![sid(3)],
            },
        ],
    }
}

/// `sessions.rs:225-246` + `mod.rs:1143-1156`.
#[test]
fn select_session_moves_the_highlight_and_clears_the_kill_arms() {
    let snap = fixture();
    let mut w = WorkspaceState {
        terminal_focused: true,
        pending_kill: Some(sid(9)),
        pending_kill_terminal: Some(4),
        open_agent_menu: Some((0, 0)),
        ..WorkspaceState::default()
    };

    w.select_session(sid(3), &snap);

    assert_eq!(w.active_session(), Some(sid(3)));
    assert!(!w.terminal_focused());
    assert_eq!(w.pending_kill(), None);
    assert_eq!(w.pending_kill_terminal(), None);
    assert_eq!(w.open_agent_menu(), None);
    assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));
}

/// The reported bug: launching from the palette while the grid is up left `grid_focused` on the previous tile, so the keyboard stayed there too.
#[test]
fn select_session_carries_the_grid_focus_with_it() {
    let snap = fixture();
    let mut w = WorkspaceState {
        grid_view: true,
        grid_focused: Some(sid(1)),
        ..WorkspaceState::default()
    };

    w.select_session(sid(3), &snap);

    assert_eq!(w.grid_focused(), Some(sid(3)));

    let mut w = WorkspaceState {
        grid_focused: Some(sid(1)),
        ..WorkspaceState::default()
    };
    w.select_session(sid(3), &snap);
    assert_eq!(w.grid_focused(), Some(sid(1)));
}

/// `sessions.rs:35-50` + `mod.rs:1164-1183`.
#[test]
fn select_worktree_repoints_the_active_session_at_the_first_one() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.select_worktree(0, 0, &snap);
    assert_eq!(w.active_session(), Some(sid(1)));
    assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
    assert!(w.worktree_collapsed(0, 0));
    w.select_worktree(0, 0, &snap);
    assert!(!w.worktree_collapsed(0, 0));
}

#[test]
fn select_worktree_leaves_a_session_already_in_that_worktree_alone() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.select_session(sid(2), &snap);
    w.select_worktree(0, 0, &snap);
    assert_eq!(w.active_session(), Some(sid(2)));
}

#[test]
fn select_worktree_clears_the_active_session_when_the_worktree_has_none() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.select_session(sid(1), &snap);
    w.select_worktree(0, 1, &snap);
    assert_eq!(w.active_session(), None);
}

/// `sessions.rs:22-33` + `mod.rs:1121-1130`.
#[test]
fn select_project_toggles_collapse_and_switches_the_active_project() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.select_project(2);
    assert_eq!(w.proj_idx(), 2);
    assert!(w.project_collapsed(2));
    w.select_project(2);
    assert!(!w.project_collapsed(2));
    let _ = snap;
}

#[test]
fn begin_add_worktree_preserves_all_expansion_state() {
    let mut w = WorkspaceState {
        collapsed: [0, 2].into_iter().collect(),
        collapsed_wt: [(0, 1), (2, 0)].into_iter().collect(),
        open_agent_menu: Some((0, 1)),
        pending_kill: Some(sid(1)),
        pending_kill_terminal: Some(2),
        ..WorkspaceState::default()
    };

    w.begin_add_worktree(2);

    assert_eq!(w.proj_idx(), 2);
    assert!(w.tree_touched);
    assert!(w.project_collapsed(0));
    assert!(w.project_collapsed(2));
    assert!(w.worktree_collapsed(0, 1));
    assert!(w.worktree_collapsed(2, 0));
    assert_eq!(w.open_agent_menu(), None);
    assert_eq!(w.pending_kill(), None);
    assert_eq!(w.pending_kill_terminal(), None);
}

#[test]
fn focus_added_worktree_opens_only_the_new_target_and_selects_its_session() {
    let mut snap = fixture();
    snap.projects[0].worktrees.push(SnapshotWorktree {
        path: "/a-new".into(),
        name: "a-new".into(),
        branch: "new".into(),
        is_main: false,
        sessions: vec![sid(4)],
    });
    let mut w = WorkspaceState {
        collapsed: [0, 2].into_iter().collect(),
        collapsed_wt: [(0, 0), (0, 1), (0, 2), (2, 0)].into_iter().collect(),
        active_session: Some(sid(1)),
        ..WorkspaceState::default()
    };

    w.focus_added_worktree(0, 2, &snap);

    assert_eq!((w.proj_idx(), w.wt_idx()), (0, 2));
    assert_eq!(w.active_session(), Some(sid(4)));
    assert!(!w.project_collapsed(0));
    assert!(!w.worktree_collapsed(0, 2));
    assert!(w.project_collapsed(2));
    assert!(w.worktree_collapsed(0, 0));
    assert!(w.worktree_collapsed(0, 1));
    assert!(w.worktree_collapsed(2, 0));

    snap.projects[0].worktrees[2].sessions.clear();
    w.focus_added_worktree(0, 2, &snap);
    assert_eq!(w.active_session(), None);
}

/// `sessions.rs:90-103`.
#[test]
fn select_home_terminal_is_bounds_checked_and_clears_both_kill_arms() {
    let mut w = WorkspaceState {
        pending_kill: Some(sid(1)),
        pending_kill_terminal: Some(0),
        ..WorkspaceState::default()
    };

    w.select_home_terminal(5, 2);
    assert_eq!(w.active_terminal(), None);
    assert!(!w.terminal_focused());

    w.select_home_terminal(1, 2);
    assert_eq!(w.active_terminal(), Some(1));
    assert!(w.terminal_focused());
    assert_eq!(w.pending_kill(), None);
    assert_eq!(w.pending_kill_terminal(), None);
}

/// Second mod+w on the same session confirms the kill; a different session re-arms instead (`shortcuts.rs:501-527`).
#[test]
fn close_focused_session_confirms_only_on_a_second_press_of_the_same_target() {
    let mut w = WorkspaceState::default();
    assert!(!w.close_focused_session(sid(1)));
    assert_eq!(w.pending_kill(), Some(sid(1)));

    assert!(!w.close_focused_session(sid(2)));
    assert_eq!(w.pending_kill(), Some(sid(2)));

    assert!(w.close_focused_session(sid(2)));
}

#[test]
fn close_focused_terminal_confirms_only_on_a_second_press_of_the_same_target() {
    let mut w = WorkspaceState::default();
    assert!(!w.close_focused_terminal(0));
    assert_eq!(w.pending_kill_terminal(), Some(0));

    assert!(!w.close_focused_terminal(1));
    assert_eq!(w.pending_kill_terminal(), Some(1));

    assert!(w.close_focused_terminal(1));
}

/// `sessions.rs:109-113`.
#[test]
fn close_home_terminal_shifts_the_pending_confirmation_across_the_removal() {
    let mut w = WorkspaceState {
        pending_kill_terminal: Some(1),
        ..WorkspaceState::default()
    };
    w.close_home_terminal(1, 2);
    assert_eq!(w.pending_kill_terminal(), None);

    w.pending_kill_terminal = Some(2);
    w.close_home_terminal(1, 2);
    assert_eq!(w.pending_kill_terminal(), Some(1));

    w.pending_kill_terminal = Some(0);
    w.close_home_terminal(1, 2);
    assert_eq!(w.pending_kill_terminal(), Some(0));
}

#[test]
fn closing_the_last_home_terminal_leaves_the_terminal_tab() {
    let mut w = WorkspaceState::default();
    w.select_home_terminal(0, 1);
    w.close_home_terminal(0, 0);
    assert_eq!(w.active_terminal(), None);
    assert!(!w.terminal_focused());
}

/// `sessions.rs:365-405`.
#[test]
fn cycle_session_wraps_in_visible_order() {
    let snap = fixture();
    let order = [sid(1), sid(2), sid(3)];
    let mut w = WorkspaceState::default();

    w.cycle_session(true, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(1)));
    w.cycle_session(true, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(2)));
    w.cycle_session(true, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(3)));
    w.cycle_session(true, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(1)));
    w.cycle_session(false, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(3)));
}

#[test]
fn cycle_session_from_the_terminal_tab_returns_to_the_last_agent_session() {
    let snap = fixture();
    let order = [sid(1), sid(2), sid(3)];
    let mut w = WorkspaceState::default();
    w.select_session(sid(2), &snap);
    w.select_home_terminal(0, 1);
    assert!(w.terminal_focused());

    w.cycle_session(true, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(2)));
    assert!(!w.terminal_focused());
    w.cycle_session(true, &order, &snap);
    assert_eq!(w.active_session(), Some(sid(3)));
}

#[test]
fn cycle_session_on_an_empty_order_is_a_no_op() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.cycle_session(true, &[], &snap);
    assert_eq!(w.active_session(), None);
}

/// `mod.rs:1213-1242`.
#[test]
fn apply_tree_expand_all_clears_both_sets() {
    let snap = fixture();
    let mut w = WorkspaceState {
        tree_expand: TreeExpand::All,
        ..WorkspaceState::default()
    };
    w.collapsed.insert(0);
    w.collapsed_wt.insert((0, 1));
    w.apply_tree_expand(&snap);
    assert!(!w.project_collapsed(0));
    assert!(!w.worktree_collapsed(0, 1));
}

#[test]
fn apply_tree_expand_collapsed_collapses_every_active_project_only() {
    let snap = fixture();
    let mut w = WorkspaceState {
        tree_expand: TreeExpand::Collapsed,
        ..WorkspaceState::default()
    };
    w.apply_tree_expand(&snap);
    assert!(w.project_collapsed(0));
    assert!(w.project_collapsed(2));
    assert!(!w.project_collapsed(1));
}

#[test]
fn apply_tree_expand_sessions_only_collapses_the_sessionless() {
    let snap = fixture();
    let mut w = WorkspaceState {
        tree_expand: TreeExpand::SessionsOnly,
        ..WorkspaceState::default()
    };
    w.apply_tree_expand(&snap);
    assert!(!w.project_collapsed(0));
    assert!(!w.project_collapsed(2));
    assert!(!w.worktree_collapsed(0, 0));
    assert!(w.worktree_collapsed(0, 1));
    assert!(!w.worktree_collapsed(2, 0));
}

#[test]
fn sessions_only_collapses_a_project_with_no_sessionful_worktree() {
    let mut snap = fixture();
    snap.projects[1].worktrees[0].sessions.clear();
    let mut w = WorkspaceState {
        tree_expand: TreeExpand::SessionsOnly,
        ..WorkspaceState::default()
    };
    w.apply_tree_expand(&snap);
    assert!(w.project_collapsed(2));
    assert!(w.worktree_collapsed(2, 0));
}

/// `sessions.rs:14-20`.
#[test]
fn toggle_collapse_all_advances_the_ring_and_clears_transients() {
    let snap = fixture();
    let mut w = WorkspaceState {
        open_agent_menu: Some((0, 0)),
        pending_kill: Some(sid(1)),
        pending_kill_terminal: Some(0),
        ..WorkspaceState::default()
    };
    assert_eq!(w.tree_expand(), TreeExpand::SessionsOnly);

    w.toggle_collapse_all(&snap);
    assert_eq!(w.tree_expand(), TreeExpand::All);
    assert!(!w.project_collapsed(0));
    assert_eq!(w.open_agent_menu(), None);
    assert_eq!(w.pending_kill(), None);
    assert_eq!(w.pending_kill_terminal(), None);

    w.toggle_collapse_all(&snap);
    assert_eq!(w.tree_expand(), TreeExpand::Collapsed);
    assert!(w.project_collapsed(0));
    w.toggle_collapse_all(&snap);
    assert_eq!(w.tree_expand(), TreeExpand::All);
}

#[test]
fn rail_mode_toggles_and_clears_transients() {
    let mut w = WorkspaceState {
        open_agent_menu: Some((0, 0)),
        pending_kill: Some(sid(1)),
        hovered_wt: Some((0, 0)),
        ..WorkspaceState::default()
    };
    assert_eq!(w.rail_mode(), RailMode::Tree);
    assert_eq!(w.toggle_rail_mode(), RailMode::Sessions);
    assert_eq!(w.rail_mode(), RailMode::Sessions);
    assert_eq!(w.open_agent_menu(), None);
    assert_eq!(w.pending_kill(), None);
    assert_eq!(w.hovered_wt(), None);
    assert_eq!(w.toggle_rail_mode(), RailMode::Tree);
}

#[test]
fn new_skips_an_archived_first_project_when_seeding_proj_idx() {
    use grove_core::storage::Project;
    let project = |name: &str, path: &str, archived: bool| Project {
        name: name.to_string(),
        path: path.to_string(),
        scripts: grove_core::storage::ProjectScripts::default(),
        archived,
        worktree_dir: None,
    };
    let store = Store {
        projects: vec![project("alpha", "/a", true), project("beta", "/b", false)],
        ..Store::default()
    };
    assert_eq!(WorkspaceState::new(&store, 1280.0).proj_idx(), 1);
}

#[test]
fn rail_mode_round_trips_from_the_store() {
    let store = Store {
        rail_sessions: true,
        ..Store::default()
    };
    assert_eq!(
        WorkspaceState::new(&store, 1600.0).rail_mode(),
        RailMode::Sessions
    );
    assert_eq!(
        WorkspaceState::new(&Store::default(), 1600.0).rail_mode(),
        RailMode::Tree
    );
}

#[test]
fn sync_default_tree_expands_only_sessionful_projects_and_picks_the_first() {
    let mut snap = fixture();
    snap.projects[0].worktrees[0].sessions.clear();
    snap.projects[0].sessions.clear();
    let mut w = WorkspaceState::default();

    w.sync_default_tree(&snap);
    assert!(w.project_collapsed(0));
    assert!(!w.project_collapsed(2));
    assert_eq!((w.proj_idx(), w.wt_idx()), (2, 0));

    w.tree_touched = true;
    w.proj_idx = 0;
    w.wt_idx = 0;
    w.sync_default_tree(&snap);
    assert!(w.project_collapsed(0));
    assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
}

/// `sessions.rs:270-280` — the index dance this design removes.
#[test]
fn removing_a_session_only_clears_the_active_one() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.select_session(sid(2), &snap);
    w.on_session_removed(sid(1));
    assert_eq!(w.active_session(), Some(sid(2)));
    w.on_session_removed(sid(2));
    assert_eq!(w.active_session(), None);
}

#[test]
fn on_project_removed_shifts_every_true_index() {
    let mut w = WorkspaceState {
        proj_idx: 2,
        collapsed: [0, 2, 3].into_iter().collect(),
        collapsed_wt: [(0, 0), (2, 1), (3, 0)].into_iter().collect(),
        hovered_wt: Some((3, 0)),
        ..WorkspaceState::default()
    };
    w.on_project_removed(0);
    assert_eq!(w.proj_idx(), 1);
    assert!(!w.project_collapsed(0));
    assert!(w.project_collapsed(1)); // was 2
    assert!(w.project_collapsed(2)); // was 3
    assert!(w.worktree_collapsed(1, 1)); // was (2, 1)
    assert!(w.worktree_collapsed(2, 0)); // was (3, 0)
    assert_eq!(w.hovered_wt(), Some((2, 0))); // was (3, 0)

    let mut w = WorkspaceState {
        proj_idx: 1,
        collapsed: [1].into_iter().collect(),
        collapsed_wt: [(1, 0)].into_iter().collect(),
        hovered_wt: Some((1, 0)),
        ..WorkspaceState::default()
    };
    w.on_project_removed(1);
    assert_eq!(w.proj_idx(), 0);
    assert!(!w.project_collapsed(1));
    assert!(!w.worktree_collapsed(1, 0));
    assert_eq!(w.hovered_wt(), None);

    let mut w = WorkspaceState {
        proj_idx: 0,
        collapsed: [0].into_iter().collect(),
        ..WorkspaceState::default()
    };
    w.on_project_removed(2);
    assert_eq!(w.proj_idx(), 0);
    assert!(w.project_collapsed(0));
}

#[test]
fn selection_is_one_directional() {
    let snap = fixture();
    let mut w = WorkspaceState::default();
    w.select_worktree(2, 0, &snap);
    w.select_session(sid(1), &snap);
    assert_eq!((w.proj_idx(), w.wt_idx()), (0, 0));
    w.select_worktree(0, 1, &snap);
    assert_eq!((w.proj_idx(), w.wt_idx()), (0, 1));
    assert_eq!(w.active_session(), None);
}

/// `src/gui/metrics.rs:244-251`.
#[test]
fn clamp_sidebar_width_bounds() {
    assert!((clamp_sidebar_width(900.0, 1280.0) - 640.0).abs() < f32::EPSILON);
    assert!((clamp_sidebar_width(900.0, 800.0) - 400.0).abs() < f32::EPSILON);
    assert!((clamp_sidebar_width(900.0, 500.0) - 220.0).abs() < f32::EPSILON);
    assert!((clamp_sidebar_width(10.0, 500.0) - 220.0).abs() < f32::EPSILON);
    assert!((clamp_sidebar_width(300.0, 1280.0) - 300.0).abs() < f32::EPSILON);
}

#[test]
fn new_seeds_the_sidebar_width_through_the_clamp() {
    let store = Store {
        sidebar_width: Some(9000.0),
        ..Store::default()
    };
    let w = WorkspaceState::new(&store, 1280.0);
    assert!((w.sidebar_width() - 640.0).abs() < f32::EPSILON);

    let w = WorkspaceState::new(&Store::default(), 1280.0);
    assert!((w.sidebar_width() - RAIL_W).abs() < f32::EPSILON);
}

/// Bare Escape's carve-out (`update/mod.rs:789-804`): each of the four armed states alone makes Escape a dismissal that clears **all** of them.
#[test]
fn escape_dismiss_clears_every_armed_state() {
    let mut w = WorkspaceState::default();
    assert!(!w.escape_dismiss(), "nothing armed: Escape reaches the PTY");

    let arm: [fn(&mut WorkspaceState); 4] = [
        |w| w.arm_kill(sid(1)),
        |w| w.arm_kill_terminal(0),
        |w| w.set_open_agent_menu(Some((0, 0))),
        WorkspaceState::toggle_attention_queue,
    ];
    for (i, f) in arm.iter().enumerate() {
        let mut w = WorkspaceState::default();
        f(&mut w);
        assert!(w.escape_dismiss(), "armed state {i} must be dismissed");
        assert!(w.pending_kill().is_none());
        assert!(w.pending_kill_terminal().is_none());
        assert!(w.open_agent_menu().is_none());
        assert!(!w.attention_queue_open());
        assert!(!w.escape_dismiss(), "state {i}: only one Escape is eaten");
    }
}

use crate::keymap::Screen;

fn live(ids: &[u64]) -> Vec<LiveTile> {
    ids.iter()
        .map(|&n| LiveTile {
            id: sid(n),
            key: format!("p::/w{n}"),
        })
        .collect()
}

fn keys(ids: &[u64]) -> Vec<String> {
    ids.iter().map(|n| format!("p::/w{n}")).collect()
}

/// The `chromeless_grid_is_not_a_nameable_screen` guard: every grid-entry path sets `chrome_visible` first (`layout.rs:222-227`).
#[test]
fn entering_the_grid_from_zen_is_never_a_chromeless_grid() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_zen(&l, &[]);
    assert_eq!(w.screen(), Screen::Zen);

    w.toggle_grid(&l, &[]);
    assert!(w.chrome_visible());
    assert!(!w.zen());
    assert_eq!(w.screen(), Screen::Grid);
}

/// `layout.rs:63-79` — zen entered from the grid restores it, with the tile order it had.
#[test]
fn zen_entered_from_the_grid_returns_to_the_same_grid() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let before = w.tile_order().to_vec();
    assert_eq!(before, vec![sid(1), sid(2), sid(3)]);

    w.toggle_zen(&l, &[]);
    assert_eq!(w.screen(), Screen::Zen);
    assert!(w.grid_view_before_zen());
    assert_eq!(w.active_session(), Some(sid(1)));

    w.toggle_zen(&l, &[]);
    assert_eq!(w.screen(), Screen::Grid);
    assert!(!w.grid_view_before_zen());
    assert_eq!(w.tile_order(), before.as_slice());
}

/// `layout.rs:98-102` — the other entry point round-trips to the single-session workspace, not to a grid.
#[test]
fn zen_entered_from_the_workspace_returns_to_the_workspace() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_zen(&l, &[]);
    assert_eq!(w.screen(), Screen::Zen);
    assert!(!w.grid_view_before_zen());
    w.toggle_zen(&l, &[]);
    assert_eq!(w.screen(), Screen::Workspace);
    assert!(w.tile_order().is_empty());
}

/// An empty grid still drops out of grid view rather than stacking zen on a chrome-less grid (`layout.rs:88-95`).
#[test]
fn zen_from_an_empty_grid_still_leaves_grid_view() {
    let mut w = WorkspaceState::default();
    w.toggle_grid(&[], &[]);
    assert_eq!(w.screen(), Screen::Grid);
    w.toggle_zen(&[], &[]);
    assert_eq!(w.screen(), Screen::Zen);
    assert!(!w.grid_view());
    assert!(w.grid_view_before_zen());
}

/// `layout.rs:204-206` — a manual `mod+g` cancels the restore intent, so a later zen-exit does not resurrect a grid.
#[test]
fn a_manual_grid_toggle_cancels_the_zen_restore_intent() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    w.toggle_zen(&l, &[]);
    assert!(w.grid_view_before_zen());

    w.toggle_grid(&l, &[]);
    assert!(!w.grid_view_before_zen());
    assert_eq!(w.screen(), Screen::Grid);

    w.toggle_grid(&l, &[]);
    assert_eq!(w.screen(), Screen::Workspace);
    w.toggle_zen(&l, &[]);
    w.toggle_zen(&l, &[]);
    assert_eq!(w.screen(), Screen::Workspace);
}

/// `layout.rs:258-262`.
#[test]
fn exit_grid_carries_the_focused_tile_into_the_active_session() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    w.set_grid_focus(Some(sid(3)));
    w.toggle_grid(&l, &[]);
    assert_eq!(w.active_session(), Some(sid(3)));
    assert!(w.tile_order().is_empty());
    assert_eq!(w.grid_focused(), None);
    assert_eq!(
        w.take_grid_order_to_persist(),
        Some(vec![sid(1), sid(2), sid(3)])
    );
}

/// `layout.rs:222-252` — the saved order wins, new sessions append, and the active session keeps its tile focused.
#[test]
fn enter_grid_rebuilds_the_order_from_the_saved_keys() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState {
        active_session: Some(sid(2)),
        ..WorkspaceState::default()
    };
    w.enter_grid(&l, &keys(&[3, 1]));
    assert_eq!(w.tile_order(), [sid(3), sid(1), sid(2)]);
    assert_eq!(w.grid_focused(), Some(sid(2)));
    assert_eq!(w.take_pending_acks(), vec![sid(2)]);
}

/// `layout.rs:276-306`.
#[test]
fn teardown_drops_dead_tiles_and_refocuses_the_first() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    w.set_grid_focus(Some(sid(2)));

    w.reconcile_after_teardown(&live(&[1, 3]), &keys(&[1, 2, 3]));
    assert_eq!(w.tile_order(), [sid(1), sid(3)]);
    assert_eq!(w.grid_focused(), Some(sid(1)));
    assert!(w.grid_view());
}

#[test]
fn teardown_keeps_a_still_live_focus() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    w.set_grid_focus(Some(sid(3)));
    w.reconcile_after_teardown(&live(&[1, 3]), &keys(&[1, 2, 3]));
    assert_eq!(w.grid_focused(), Some(sid(3)));
}

#[test]
fn teardown_with_nothing_left_falls_out_of_grid_view() {
    let l = live(&[1]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    w.reconcile_after_teardown(&[], &keys(&[1]));
    assert!(w.tile_order().is_empty());
    assert_eq!(w.grid_focused(), None);
    assert!(!w.grid_view());
    assert_eq!(w.screen(), Screen::Workspace);
}

#[test]
fn teardown_outside_the_grid_just_clears_the_bookkeeping() {
    let mut w = WorkspaceState {
        tile_order: vec![sid(1)],
        grid_focused: Some(sid(1)),
        ..WorkspaceState::default()
    };
    w.reconcile_after_teardown(&live(&[1]), &[]);
    assert!(w.tile_order().is_empty());
    assert_eq!(w.grid_focused(), None);
}

#[test]
fn reconcile_after_teardown_adds_a_session_spawned_while_the_grid_is_up() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);

    w.reconcile_after_teardown(&live(&[1, 2, 3]), &[]);
    assert_eq!(w.tile_order(), [sid(1), sid(2), sid(3)]);
}

/// `update/mod.rs:1071-1094` over `grid::grid_neighbor`. 3 tiles → cols=2: left column 0/2, right column 1.
#[test]
fn grid_move_walks_the_neighbor_and_takes_the_session_with_it() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let _ = w.take_pending_acks();
    assert_eq!(w.grid_focused(), Some(sid(1))); // tile 0

    w.grid_move(1, 0); // → tile 1
    assert_eq!(w.grid_focused(), Some(sid(2)));
    assert_eq!(w.active_session(), Some(sid(2)));
    assert_eq!(w.take_pending_acks(), vec![sid(2)]);

    w.grid_move(-1, 0); // back to tile 0
    assert_eq!(w.grid_focused(), Some(sid(1)));
    w.grid_move(0, 1); // down to tile 2
    assert_eq!(w.grid_focused(), Some(sid(3)));
}

#[test]
fn grid_move_off_the_edge_or_on_an_empty_grid_is_a_no_op() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    w.grid_move(-1, 0);
    assert_eq!(w.grid_focused(), Some(sid(1)));
    w.grid_move(1, 0);
    w.grid_move(0, 1);
    assert_eq!(w.grid_focused(), Some(sid(2)));

    let mut empty = WorkspaceState::default();
    empty.grid_move(1, 0);
    assert_eq!(empty.grid_focused(), None);
}

/// `update/mod.rs:1102-1116` — the tile moves, the focused **session** does not change.
#[test]
fn grid_swap_moves_the_tile_and_keeps_focus_on_its_session() {
    let l = live(&[1, 2, 3, 4]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    assert_eq!(w.grid_focused(), Some(sid(1)));

    w.grid_swap(1, 0);
    assert_eq!(w.tile_order(), [sid(2), sid(1), sid(3), sid(4)]);
    assert_eq!(w.grid_focused(), Some(sid(1)));
    assert_eq!(w.active_session(), Some(sid(1)));
    let Some(slide) = w.grid_slide() else {
        unreachable!("a swap records a slide");
    };
    assert_eq!(slide.tiles, [(1, -1, 0), (0, 1, 0)]);
    assert_eq!(
        w.take_grid_order_to_persist(),
        Some(vec![sid(2), sid(1), sid(3), sid(4)])
    );

    w.grid_swap(1, 0);
    assert_eq!(w.tile_order(), [sid(2), sid(1), sid(3), sid(4)]);
}

/// `sessions.rs:396-405`.
#[test]
fn mod_n_in_the_grid_indexes_the_tile_order() {
    let l = live(&[1, 2, 3]);
    let mut w = WorkspaceState::default();
    w.enter_grid(&l, &keys(&[3, 2, 1]));
    w.grid_view = true;
    w.select_tile_by_index(1);
    assert_eq!(w.active_session(), Some(sid(2)));
    assert_eq!(w.grid_focused(), Some(sid(2)));
    w.select_tile_by_index(9);
    assert_eq!(w.active_session(), Some(sid(2)));
}

/// `shortcuts.rs:528-557` + `update/mod.rs:472-500`.
#[test]
fn the_terminal_tab_restores_the_grid_only_when_entered_from_it() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);

    let out = w.toggle_terminal_tab(true, &l, &[]);
    assert!(!out.spawn_home_terminal);
    assert!(w.terminal_focused());
    assert!(!w.grid_view());
    assert!(w.grid_view_before_terminal());

    w.toggle_terminal_tab(true, &l, &[]);
    assert!(!w.terminal_focused());
    assert!(w.grid_view());
    assert!(!w.grid_view_before_terminal());

    w.toggle_grid(&l, &[]);
    assert_eq!(w.screen(), Screen::Workspace);
    w.toggle_terminal_tab(true, &l, &[]);
    w.toggle_terminal_tab(true, &l, &[]);
    assert!(!w.grid_view());
}

/// Recorded ambiguity 3 (`update/mod.rs:472-475`): in zen `mod+t` is a pure content swap.
#[test]
fn the_terminal_tab_never_touches_the_chrome() {
    let l = live(&[1]);
    let mut w = WorkspaceState::default();
    w.toggle_zen(&l, &[]);
    assert!(!w.chrome_visible());
    w.toggle_terminal_tab(true, &l, &[]);
    assert!(!w.chrome_visible());
    assert!(w.terminal_focused());
    w.toggle_terminal_tab(true, &l, &[]);
    assert!(!w.chrome_visible());
    assert_eq!(w.screen(), Screen::Zen);
}

/// `update/mod.rs:493-497` — the transition only reports the spawn.
#[test]
fn the_first_terminal_tab_entry_asks_for_a_spawn() {
    let mut w = WorkspaceState {
        pending_kill: Some(sid(1)),
        pending_kill_terminal: Some(0),
        ..WorkspaceState::default()
    };
    let out = w.toggle_terminal_tab(false, &[], &[]);
    assert!(out.spawn_home_terminal);
    assert_eq!(w.pending_kill(), None);
    assert_eq!(w.pending_kill_terminal(), None);
}

/// `sessions.rs:62-88`.
#[test]
fn the_panel_refuses_to_open_without_a_worktree_to_anchor_to() {
    let mut w = WorkspaceState {
        active_session: Some(sid(1)),
        ..WorkspaceState::default()
    };
    assert!(!w.toggle_term_panel(false));
    assert!(!w.term_panel_open());
    assert_eq!(w.focused_pane(), FocusedPane::Agent);

    assert!(w.toggle_term_panel(true));
    assert!(w.term_panel_open());
    assert_eq!(w.focused_pane(), FocusedPane::Panel);
    assert!(w.panel_focused());

    assert!(!w.toggle_term_panel(false));
    assert!(!w.term_panel_open());
    assert_eq!(w.focused_pane(), FocusedPane::Agent);
    assert!(!w.panel_focused());
}

/// The reported bug: the panel used to be one global bool, so leaving it open in session A leaked into session B on switch.
#[test]
fn term_panel_open_is_tracked_per_session() {
    let mut w = WorkspaceState {
        active_session: Some(sid(1)),
        ..WorkspaceState::default()
    };
    assert!(w.toggle_term_panel(true));
    assert!(w.term_panel_open());

    w.active_session = Some(sid(2));
    assert!(!w.term_panel_open());

    w.active_session = Some(sid(1));
    assert!(w.term_panel_open());

    assert!(!w.toggle_term_panel(true));
    assert!(!w.term_panel_open());
    w.active_session = Some(sid(2));
    assert!(!w.term_panel_open());
}

/// `layout.rs:533-542`.
#[test]
fn the_panel_portion_steps_are_clamped_and_no_op_at_the_bounds() {
    let mut w = WorkspaceState::default();
    assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION);
    #[allow(clippy::cast_possible_wrap)]
    let step = TERM_PANEL_PORTION_STEP as i16;
    w.adjust_term_panel_portion(step);
    assert_eq!(w.term_panel_portion(), 45);
    for _ in 0..20 {
        w.adjust_term_panel_portion(step);
    }
    assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MAX);
    for _ in 0..40 {
        w.adjust_term_panel_portion(-step);
    }
    assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MIN);

    w.set_term_panel_portion(999);
    assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MAX);
    w.set_term_panel_portion(0);
    assert_eq!(w.term_panel_portion(), TERM_PANEL_PORTION_MIN);
}

/// `src/gui/metrics.rs:482-501`, ported verbatim.
#[test]
fn term_portion_for_cursor_maps_and_clamps() {
    let win = 1280.0;
    let sidebar = RAIL_W;
    let work_left = sidebar + crate::views::tokens::DIVIDER_DRAG_HIT_W;
    let work_w = win - work_left;
    let mid = work_left + work_w * 0.5;
    assert_eq!(term_portion_for_cursor(mid, win, sidebar), 50);
    assert_eq!(
        term_portion_for_cursor(win - 1.0, win, sidebar),
        TERM_PANEL_PORTION_MIN
    );
    assert_eq!(
        term_portion_for_cursor(work_left + 1.0, win, sidebar),
        TERM_PANEL_PORTION_MAX
    );
}

/// `pty_input.rs:128-158`.
#[test]
fn focus_pane_only_counts_while_the_panel_is_open_and_ignores_tiles() {
    let mut w = WorkspaceState {
        active_session: Some(sid(1)),
        ..WorkspaceState::default()
    };
    w.focus_pane(PtyPane::Panel);
    assert_eq!(w.focused_pane(), FocusedPane::Agent);

    w.toggle_term_panel(true);
    w.focus_pane(PtyPane::Agent);
    assert_eq!(w.focused_pane(), FocusedPane::Agent);
    w.focus_pane(PtyPane::Panel);
    assert_eq!(w.focused_pane(), FocusedPane::Panel);
    w.focus_pane(PtyPane::Tile(sid(1)));
    assert_eq!(w.focused_pane(), FocusedPane::Panel);

    w.focus_pane(PtyPane::Agent);
    w.reset_focused_pane();
    assert_eq!(w.focused_pane(), FocusedPane::Panel);
}

/// `pty_input.rs:170-178` — the fallback that stops a shell-less panel from eating every keystroke.
#[test]
fn a_worktree_with_no_panel_shell_routes_input_to_the_agent() {
    let mut w = WorkspaceState {
        active_session: Some(sid(1)),
        ..WorkspaceState::default()
    };
    assert_eq!(w.input_target(false), PtyPane::Agent);

    w.toggle_term_panel(true);
    assert!(w.panel_focused());
    assert_eq!(w.input_target(false), PtyPane::Agent);
    assert_eq!(w.input_target(true), PtyPane::Panel);

    w.focus_pane(PtyPane::Agent);
    assert_eq!(w.input_target(true), PtyPane::Agent);

    w.toggle_term_panel(false);
    assert_eq!(w.input_target(true), PtyPane::Agent);
}

/// `layout.rs:308-342` — press focuses and arms, enter tracks, release commits.
#[test]
fn a_tile_drag_focuses_on_press_and_swaps_on_release() {
    let l = live(&[1, 2, 3, 4]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let _ = w.take_pending_acks();
    let _ = w.take_grid_order_to_persist();

    w.grid_drag_start(2);
    assert_eq!(w.grid_focused(), Some(sid(3)));
    assert_eq!(w.active_session(), Some(sid(3)));
    assert_eq!(w.take_pending_acks(), vec![sid(3)]);
    assert_eq!(
        w.grid_reorder_drag(),
        Some(GridDrag {
            source_idx: 2,
            hover_idx: 2
        })
    );

    w.grid_drag_hover(0);
    assert_eq!(w.grid_reorder_drag().map(|d| d.hover_idx), Some(0));

    assert!(w.grid_pointer_end());
    assert_eq!(w.tile_order(), [sid(3), sid(2), sid(1), sid(4)]);
    assert_eq!(w.grid_focused(), Some(sid(3)));
    assert!(w.grid_pointer_drag().is_none());
    assert!(w.grid_slide().is_some());
    assert!(w.take_grid_order_to_persist().is_some());
}

#[test]
fn a_hover_with_no_armed_drag_is_a_no_op_and_a_null_drag_does_not_swap() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let _ = w.take_grid_order_to_persist();

    w.grid_drag_hover(1);
    assert!(w.grid_reorder_drag().is_none());
    assert!(!w.grid_pointer_end());
    assert_eq!(w.tile_order(), [sid(1), sid(2)]);

    w.grid_drag_start(1);
    assert!(!w.grid_pointer_end());
    assert_eq!(w.tile_order(), [sid(1), sid(2)]);
    assert!(w.grid_slide().is_none());
    assert!(w.take_grid_order_to_persist().is_none());
}

#[test]
fn resize_drag_owns_pointer_without_reordering_or_moving_focus() {
    let l = live(&[1, 2, 3, 4]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let focused = w.grid_focused();
    let order = w.tile_order().to_vec();
    let boundary = GridBoundary {
        axis: GridAxis::Columns,
        boundary: 0,
        column: None,
    };

    assert!(w.grid_resize_drag_start(boundary, 200.0, 800.0, 0.1));
    assert!(matches!(
        w.grid_pointer_drag(),
        Some(GridPointerDrag::Resize(_))
    ));
    assert!(w.grid_reorder_drag().is_none());
    assert!(w.grid_resize_drag_update(280.0, 0.0));
    assert!((w.grid_sizing().column_weights()[0] - 0.6).abs() < 1e-6);
    assert!(!w.grid_pointer_end());
    assert_eq!(w.tile_order(), order);
    assert_eq!(w.grid_focused(), focused);
    assert!(w.take_grid_order_to_persist().is_none());
}

#[test]
fn resize_drag_uses_its_original_baseline_after_a_clamp() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let boundary = GridBoundary {
        axis: GridAxis::Columns,
        boundary: 0,
        column: None,
    };
    assert!(w.grid_resize_drag_start(boundary, 100.0, 100.0, 0.2));
    assert!(w.grid_resize_drag_update(1000.0, 0.0));
    assert!((w.grid_sizing().column_weights()[0] - 0.8).abs() < 1e-6);
    assert!(w.grid_resize_drag_update(110.0, 0.0));
    assert!((w.grid_sizing().column_weights()[0] - 0.6).abs() < 1e-6);
}

#[test]
fn a_missing_left_button_cancels_pointer_ownership_without_committing_reorder() {
    let mut w = WorkspaceState::default();
    w.toggle_grid(&live(&[1, 2]), &[]);
    let _ = w.take_grid_order_to_persist();
    let original = w.tile_order().to_vec();
    w.grid_drag_start(0);
    w.grid_drag_hover(1);

    assert!(w.grid_pointer_cancel());
    assert_eq!(w.tile_order(), original);
    assert!(w.grid_pointer_drag().is_none());
    assert!(!w.grid_pointer_cancel());
    assert!(w.take_grid_order_to_persist().is_none());
}

#[test]
fn keyboard_resize_selects_the_focused_adjacent_boundary_and_never_reorders() {
    let l = live(&[1, 2, 3, 4]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&l, &[]);
    let order = w.tile_order().to_vec();
    assert!(w.enter_grid_resize_mode());
    assert_eq!(w.grid_resize_label().as_deref(), Some("columns 1 / 2"));
    assert!(w.grid_resize_step(1, 0, 5.0, 0.1));
    assert!((w.grid_sizing().column_weights()[0] - 0.55).abs() < 1e-6);
    assert_eq!(w.tile_order(), order);
    assert_eq!(w.grid_focused(), Some(sid(1)));

    assert!(w.grid_resize_step(0, 1, 1.0, 0.1));
    assert_eq!(
        w.grid_resize_label().as_deref(),
        Some("column 1 rows 1 / 2")
    );
    assert!((w.grid_sizing().row_weights(0)[0] - 0.51).abs() < 1e-6);
    w.exit_grid_resize_mode();
    assert!(w.grid_resize_mode().is_none());
}

#[test]
fn keyboard_resize_reports_a_new_selection_even_when_transfer_is_clamped() {
    let mut w = WorkspaceState::default();
    w.toggle_grid(&live(&[1, 2, 3, 4]), &[]);
    assert!(w.enter_grid_resize_mode());

    // An equal-share floor clamps both pairs, but changing axis still changes the status hint.
    assert!(!w.grid_resize_step(1, 0, 5.0, 0.5));
    assert_eq!(w.grid_resize_label().as_deref(), Some("columns 1 / 2"));
    assert!(w.grid_resize_step(0, 1, 5.0, 0.5));
    assert_eq!(
        w.grid_resize_label().as_deref(),
        Some("column 1 rows 1 / 2")
    );
    // Repeating the same clamped request changes neither selection nor weights.
    assert!(!w.grid_resize_step(0, 1, 5.0, 0.5));
}

#[test]
fn changing_grid_focus_exits_keyboard_resize_mode() {
    let mut w = WorkspaceState::default();
    w.toggle_grid(&live(&[1, 2]), &[]);
    assert!(w.enter_grid_resize_mode());

    w.set_grid_focus(Some(sid(2)));
    assert_eq!(w.grid_focused(), Some(sid(2)));
    assert!(w.grid_resize_mode().is_none());
}

#[test]
fn topology_change_resets_all_session_only_weights_and_exits_resize() {
    let mut w = WorkspaceState::default();
    w.toggle_grid(&live(&[1, 2, 3, 4]), &[]);
    assert!(w.enter_grid_resize_mode());
    assert!(w.grid_resize_step(1, 0, 5.0, 0.1));
    assert_ne!(w.grid_sizing().column_weights(), &[0.5, 0.5]);

    w.reconcile_after_teardown(&live(&[1, 2, 3]), &[]);
    assert_eq!(w.grid_sizing().column_weights(), &[0.5, 0.5]);
    assert_eq!(w.grid_sizing().row_weights(0), &[0.5, 0.5]);
    assert_eq!(w.grid_sizing().row_weights(1), &[1.0]);
    assert!(w.grid_resize_mode().is_none());
    assert!(w.grid_pointer_drag().is_none());
}

#[test]
fn resetting_one_split_leaves_every_other_split_unchanged() {
    let mut w = WorkspaceState::default();
    w.toggle_grid(&live(&[1, 2, 3, 4]), &[]);
    assert!(w.enter_grid_resize_mode());
    assert!(w.grid_resize_step(1, 0, 5.0, 0.1));
    assert!(w.grid_resize_step(0, 1, 5.0, 0.1));
    let rows_before = w.grid_sizing().row_weights(0).to_vec();
    assert!(w.reset_grid_boundary(GridBoundary {
        axis: GridAxis::Columns,
        boundary: 0,
        column: None,
    }));
    assert_eq!(w.grid_sizing().column_weights(), &[0.5, 0.5]);
    assert_eq!(w.grid_sizing().row_weights(0), rows_before);
}

#[test]
fn leaving_the_grid_for_zen_clears_the_transient_resize_context() {
    let tiles = live(&[1, 2]);
    let mut w = WorkspaceState::default();
    w.toggle_grid(&tiles, &[]);
    assert!(w.enter_grid_resize_mode());
    w.toggle_zen(&tiles, &[]);
    assert!(!w.grid_view());
    assert!(w.grid_resize_mode().is_none());
    assert!(w.grid_pointer_drag().is_none());
}

/// `layout.rs:257-269` — leaving the grid re-anchors the panel, so a stale `Panel` intent cannot type into another worktree's shell.
#[test]
fn exit_grid_re_anchors_the_focused_pane() {
    let l = live(&[1, 2]);
    let mut w = WorkspaceState {
        active_session: Some(sid(1)),
        ..WorkspaceState::default()
    };
    w.toggle_term_panel(true);
    assert_eq!(w.focused_pane(), FocusedPane::Panel);
    w.toggle_grid(&l, &[]);
    w.toggle_grid(&l, &[]);
    assert_eq!(w.focused_pane(), FocusedPane::Panel);

    w.toggle_term_panel(false);
    w.toggle_grid(&l, &[]);
    w.toggle_grid(&l, &[]);
    assert_eq!(w.focused_pane(), FocusedPane::Agent);
}

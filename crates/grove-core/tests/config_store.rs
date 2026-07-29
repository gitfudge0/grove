//! Exercises `grove_core::storage`'s real on-disk write path against genuine
//! temp files. Unit tests inside `src/storage.rs` already cover
//! `write_atomic` and `resolve_config_dir` against tempdir-scoped paths (see
//! commit 3d5d086) using `std::env::temp_dir()` directly, but they run
//! in-process against paths the test itself controls end to end; this file
//! instead treats `grove_core::storage` purely as an external library
//! consumed through `tempfile::TempDir` fixtures, matching how the rest of
//! the crate (and the GUI) actually calls it.
//!
//! `save()`/`load()` route through `config_path()` -> `config_dir()` -> the
//! private `resolve_config_dir(dirs::config_dir())`, which used to be
//! hardwired to the real user config directory with no caller-injectable
//! base path — calling them from a test would have read or clobbered the
//! developer's actual `~/.config/grove/projects.json`. `config_dir()` now
//! honours the `GROVE_CONFIG_DIR` env var as a highest-precedence override
//! (see `storage::CONFIG_DIR_ENV`), which makes `save`/`load` safely testable
//! against a `tempfile::TempDir` — see `save_then_load_round_trips_through_real_paths`
//! below. `GROVE_CONFIG_DIR` is process-global and `cargo test` runs the
//! tests in this binary concurrently, so that test is serialized behind
//! `CONFIG_DIR_ENV_TEST_LOCK`.
//!
//! The rest of this file still limits itself to what the public API exposes
//! with a caller-supplied path directly: `write_atomic`, plus `Store`'s own
//! `serde_json` contract (which is what `save`/`load` are thin wrappers
//! around).

use fs_err as fs;
use grove_core::agent::Agent;
use grove_core::storage::{self, write_atomic, Project, RecentLaunch, Store};

/// Serializes every test in this binary that sets `GROVE_CONFIG_DIR`, since
/// `std::env::set_var` mutates process-global state and `cargo test` runs
/// tests in a binary concurrently by default. Mirrors `theme.rs`'s
/// `CUSTOM_TEST_LOCK` pattern.
static CONFIG_DIR_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `write_atomic` must genuinely replace pre-existing content at the
/// destination — not append to it or leave the old bytes visible in any way
/// — and must leave no `.json.tmp` sibling behind once the rename completes.
#[test]
fn write_atomic_replaces_existing_content_and_leaves_no_tmp_residue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("projects.json");

    write_atomic(
        &dest,
        b"{\"projects\":[\"old-and-much-longer-placeholder\"]}",
    )
    .expect("first write_atomic");
    write_atomic(&dest, b"{\"projects\":[]}").expect("second write_atomic");

    let written = fs::read(&dest).expect("read back");
    assert_eq!(
        written, b"{\"projects\":[]}",
        "second write must fully replace the first, not append or merge"
    );

    let tmp = dest.with_extension("json.tmp");
    assert!(
        !tmp.exists(),
        ".json.tmp sibling must not survive a completed write_atomic"
    );

    // No stray files of any name left behind in the directory either.
    let entries: Vec<_> = fs::read_dir(dir.path())
        .expect("read_dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert_eq!(
        entries,
        vec![std::ffi::OsString::from("projects.json")],
        "directory must contain exactly the destination file, nothing else"
    );
}

/// Writing into a path whose parent directory does not exist on disk must
/// surface as an `Err` from `write_atomic`, not a panic — `write_atomic`
/// does not create parent directories on the caller's behalf.
#[test]
fn write_atomic_with_missing_parent_directory_returns_err() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("does-not-exist").join("projects.json");

    let result = write_atomic(&dest, b"{}");

    assert!(
        result.is_err(),
        "write_atomic into a nonexistent parent directory must return Err, not panic"
    );
}

/// A `Store` populated with every field, including nested `Project` and
/// `RecentLaunch` entries, must round-trip exactly through the same
/// serialize -> write_atomic -> read -> deserialize pipeline `save()`/`load()`
/// use internally, when driven against a caller-supplied tempdir path.
#[test]
fn store_round_trips_through_write_atomic_and_manual_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("projects.json");

    let original = Store {
        projects: vec![
            Project {
                name: "myapp".into(),
                path: "/home/user/myapp".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                theme: Some("dracula".into()),
            },
            Project {
                name: "other".into(),
                path: "/tmp/other".into(),
                scripts: grove_core::storage::ProjectScripts::default(),
                theme: None,
            },
        ],
        default_agent: Some(Agent::Claude),
        theme: Some("tokyonight".into()),
        tmux_enabled: Some(false),
        ui_zoom: Some(1.1),
        sidebar_width: Some(280.0),
        onboarded: true,
        last_update_check: Some(1_700_000_000),
        skipped_version: Some("v0.9.0".into()),
        dangerously_skip_permissions_enabled: Some(true),
        chrome_enabled: Some(true),
        telemetry_enabled: Some(false),
        grid_order: vec!["myapp::/home/user/myapp".into()],
        theme_follow_system: true,
        theme_dark: Some("tokyonight".into()),
        theme_light: Some("tokyonight-day".into()),
        project_themes_enabled: true,
        recent_launches: vec![RecentLaunch {
            project: "myapp".into(),
            wt_path: "/home/user/myapp".into(),
            agent: Agent::Claude,
        }],
    };

    let serialized = serde_json::to_string_pretty(&original).expect("serialize Store");
    write_atomic(&dest, serialized.as_bytes()).expect("write_atomic Store");

    // Mirrors `load()`'s own read + parse steps exactly, just against a
    // caller-supplied path instead of the hardwired real config path.
    let read_back = fs::read_to_string(&dest).expect("read back Store json");
    let recovered: Store = serde_json::from_str(&read_back).expect("deserialize Store");

    assert_eq!(recovered.projects.len(), 2);
    assert_eq!(recovered.projects[0].name, "myapp");
    assert_eq!(recovered.projects[0].theme.as_deref(), Some("dracula"));
    assert_eq!(recovered.projects[1].path, "/tmp/other");
    assert_eq!(recovered.default_agent, Some(Agent::Claude));
    assert_eq!(recovered.theme.as_deref(), Some("tokyonight"));
    assert_eq!(recovered.tmux_enabled, Some(false));
    assert!(recovered.onboarded);
    assert_eq!(recovered.last_update_check, Some(1_700_000_000));
    assert_eq!(recovered.skipped_version.as_deref(), Some("v0.9.0"));
    assert_eq!(recovered.dangerously_skip_permissions_enabled, Some(true));
    assert_eq!(recovered.telemetry_enabled, Some(false));
    assert_eq!(recovered.grid_order, original.grid_order);
    assert!(recovered.theme_follow_system);
    assert!(recovered.project_themes_enabled);
    assert_eq!(recovered.recent_launches, original.recent_launches);
}

/// A file containing malformed JSON at the location `load()` would read from
/// must fail to parse as a `Store` — the same `serde_json::from_str` call
/// `load()` makes internally before it backs up the corrupt file and returns
/// `Err`. This exercises the parse failure against bytes that actually went
/// through a real `write_atomic` + `fs_err::read_to_string` round trip
/// (rather than a string literal handed straight to `serde_json`, as the
/// existing unit test does), so it also proves `write_atomic` faithfully
/// preserves arbitrary — including invalid — byte content.
#[test]
fn malformed_json_on_disk_fails_to_parse_as_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let dest = dir.path().join("projects.json");

    write_atomic(&dest, b"{ this is not valid json at all !!!").expect("write_atomic");

    let read_back = fs::read_to_string(&dest).expect("read back");
    let result: Result<Store, _> = serde_json::from_str(&read_back);

    assert!(
        result.is_err(),
        "malformed JSON on disk must fail to parse into a Store, not silently default"
    );
}

/// End-to-end `storage::save` -> `storage::load` round trip against a
/// `tempfile::TempDir`, made possible by the `GROVE_CONFIG_DIR` override.
/// Previously impossible from an integration test: without the override,
/// both functions resolve through the real `~/.config/grove`, so exercising
/// them here would have read or clobbered the developer's actual config.
#[test]
fn save_then_load_round_trips_through_real_paths() {
    let _lock = CONFIG_DIR_ENV_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let dir = tempfile::tempdir().expect("tempdir");
    std::env::set_var(storage::CONFIG_DIR_ENV, dir.path());

    let original = Store {
        projects: vec![Project {
            name: "myapp".into(),
            path: "/home/user/myapp".into(),
            scripts: grove_core::storage::ProjectScripts::default(),
            theme: Some("dracula".into()),
        }],
        default_agent: Some(Agent::Codex),
        theme: Some("tokyonight".into()),
        onboarded: true,
        recent_launches: vec![RecentLaunch {
            project: "myapp".into(),
            wt_path: "/home/user/myapp".into(),
            agent: Agent::Codex,
        }],
        ..Default::default()
    };

    storage::save(&original).expect("save");

    // The config file must land inside the override directory, not the real
    // config dir.
    let config_path = storage::config_path().expect("config_path");
    assert!(config_path.starts_with(dir.path()));
    assert!(config_path.exists());

    let loaded = storage::load().expect("load");

    assert_eq!(loaded.projects.len(), 1);
    assert_eq!(loaded.projects[0].name, "myapp");
    assert_eq!(loaded.projects[0].theme.as_deref(), Some("dracula"));
    assert_eq!(loaded.default_agent, Some(Agent::Codex));
    assert_eq!(loaded.theme.as_deref(), Some("tokyonight"));
    assert!(loaded.onboarded);
    assert_eq!(loaded.recent_launches, original.recent_launches);

    std::env::remove_var(storage::CONFIG_DIR_ENV);
}

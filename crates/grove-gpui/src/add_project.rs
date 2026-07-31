//! The add-project wizard's PURE half, ported from `src/gui/add_project.rs`
//! plus the three helpers it leans on in `src/app/util.rs` (`cycle` :5-10,
//! `path_basename` :23-29, `list_dirs` :31-71, `shellexpand_tilde` :73-85).
//!
//! No gpui types. The directory match list is a pure function of the typed
//! path, so it is tested against a temp tree, never against `$HOME`.

// The wizard's pure surface is ported whole from the iced oracle; the live-edit
// setters are driven by gpui-component's own `InputState` rather than by hand,
// so they have no caller here.
#![allow(dead_code)]

use fs_err as fs;

use crate::modal::{AddProjectState, AddProjectStep};

/// Result of probing the chosen folder for a git repository
/// (`add_project.rs:47-51`). Transient wizard state, never persisted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum GitProbe {
    Repo {
        branch: String,
    },
    #[default]
    NotRepo,
}

/// `src/app/util.rs:5-10`.
pub fn cycle(cur: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as i32 + delta).rem_euclid(len as i32) as usize
}

/// `src/app/util.rs:23-29`.
pub fn path_basename(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(p)
        .to_string()
}

/// `src/app/util.rs:73-85`.
pub fn shellexpand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    s.to_string()
}

/// Directories matching the typed buffer, sorted case-insensitively
/// (`src/app/util.rs:31-71`). A trailing `/` lists the directory itself;
/// otherwise the last segment is a prefix filter. Dotfiles are hidden unless
/// the prefix itself starts with a dot.
pub fn list_dirs(buffer: &str) -> Vec<String> {
    let expanded = shellexpand_tilde(buffer);
    let (dir, prefix) = if expanded.is_empty() {
        (std::path::PathBuf::from("."), String::new())
    } else if expanded.ends_with('/') {
        (
            std::path::PathBuf::from(expanded.trim_end_matches('/')),
            String::new(),
        )
    } else {
        let pb = std::path::PathBuf::from(&expanded);
        let parent = pb.parent().map_or_else(
            || std::path::PathBuf::from("."),
            std::path::Path::to_path_buf,
        );
        let name = pb
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let parent = if parent.as_os_str().is_empty() {
            std::path::PathBuf::from(".")
        } else {
            parent
        };
        (parent, name)
    };
    let Ok(rd) = fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out: Vec<String> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') || prefix.starts_with('.'))
        .filter(|n| n.starts_with(&prefix))
        .map(|n| format!("{}/{}", dir.display(), n))
        .collect();
    out.sort_by_key(|a| a.to_lowercase());
    out
}

/// The wizard's opening state (`add_project.rs:126-137`).
pub fn opened() -> AddProjectState {
    AddProjectState {
        step: AddProjectStep::PickSource,
        path: "~/".into(),
        dir_sel: 0,
        name: String::new(),
        note: None,
        init_git: true,
        git_branch: None,
    }
}

/// Live edit of the step-1 path buffer (`add_project.rs:146-155`). Guarded to
/// the pick-source step, and it resets the match cursor and clears the note.
pub fn set_path(st: &mut AddProjectState, s: String) {
    if st.step == AddProjectStep::PickSource {
        st.path = s;
        st.dir_sel = 0;
        st.note = None;
    }
}

/// Live edit of the step-2 name field (`add_project.rs:157-162`).
pub fn set_name(st: &mut AddProjectState, s: String) {
    st.name = s;
    st.note = None;
}

/// `add_project.rs:164-178`.
pub fn dir_move(st: &mut AddProjectState, delta: i32) {
    if st.step != AddProjectStep::PickSource {
        return;
    }
    let entries = list_dirs(&st.path);
    if entries.is_empty() {
        st.dir_sel = 0;
        return;
    }
    st.dir_sel = cycle(st.dir_sel, delta, entries.len());
}

/// `add_project.rs:179-195`. Picking a row rewrites the buffer to that
/// directory **with a trailing slash**, so the next list is its contents.
pub fn dir_pick(st: &mut AddProjectState) {
    if st.step != AddProjectStep::PickSource {
        return;
    }
    let entries = list_dirs(&st.path);
    if let Some(pick) = entries.get(st.dir_sel) {
        st.path = format!("{pick}/");
        st.dir_sel = 0;
    }
}

/// What [`choose`] decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChooseOutcome {
    /// Advanced to the details step; the probe is the caller's to keep.
    Advanced(GitProbe),
    /// Rejected; `st.note` carries why and the step stays put.
    Rejected,
}

/// Step-1 Enter: feed the typed buffer into the choose funnel
/// (`add_project.rs:196-209`). Guarded to the pick-source step so a doubled
/// Enter cannot fall through and submit the details step.
pub fn choose_typed(st: &mut AddProjectState) -> ChooseOutcome {
    if st.step != AddProjectStep::PickSource {
        return ChooseOutcome::Rejected;
    }
    let pb = std::path::PathBuf::from(shellexpand_tilde(st.path.trim()));
    choose(st, &pb)
}

/// The single funnel for all three folder sources (native picker, drop, typed
/// path): validate, canonicalize, probe git upfront, advance
/// (`add_project.rs:210-242`).
pub fn choose(st: &mut AddProjectState, pb: &std::path::Path) -> ChooseOutcome {
    if !pb.is_dir() {
        st.note = Some("not a folder; choose a directory".into());
        return ChooseOutcome::Rejected;
    }
    let abs = match fs::canonicalize(pb) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(e) => {
            st.note = Some(format!("cannot resolve path: {e}"));
            return ChooseOutcome::Rejected;
        }
    };
    let probe = if grove_core::git::is_repo(&abs) {
        GitProbe::Repo {
            branch: grove_core::git::current_branch(&abs),
        }
    } else {
        GitProbe::NotRepo
    };
    st.step = AddProjectStep::Details;
    st.path = abs;
    st.note = None;
    ChooseOutcome::Advanced(probe)
}

/// "change" from the details step: back to pick-source. The (possibly edited)
/// name is kept so a round trip does not lose it (`add_project.rs:243-255`).
pub fn change_source(st: &mut AddProjectState) {
    st.step = AddProjectStep::PickSource;
    st.note = None;
}

/// What [`validate_submit`] decided. The caller performs the effects; nothing
/// here touches the store or the filesystem beyond the git probe it is given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Register `(name, path)`; run `git init` first if `init_git`.
    Register {
        name: String,
        path: String,
        init_git: bool,
    },
    /// `st.note` carries the reason.
    Rejected,
}

/// Final submit from the details step (`add_project.rs:256-304`). The name
/// field is a pure override: left empty, the folder's basename is used.
/// Nothing is persisted until every check has passed.
pub fn validate_submit(
    st: &mut AddProjectState,
    probe: &GitProbe,
    existing: &[(String, String)],
) -> SubmitOutcome {
    if st.step != AddProjectStep::Details {
        return SubmitOutcome::Rejected;
    }
    let path = st.path.clone();
    let typed = st.name.trim().to_string();
    let name = if typed.is_empty() {
        path_basename(&path)
    } else {
        typed
    };
    if name.is_empty() {
        st.note = Some("name required".into());
        return SubmitOutcome::Rejected;
    }
    if existing.iter().any(|(n, _)| *n == name) {
        st.note = Some(format!("project '{name}' already exists"));
        return SubmitOutcome::Rejected;
    }
    if let Some((n, _)) = existing.iter().find(|(_, p)| *p == path) {
        st.note = Some(format!("folder already added as '{n}'"));
        return SubmitOutcome::Rejected;
    }
    let init_git = *probe == GitProbe::NotRepo && st.init_git;
    SubmitOutcome::Register {
        name,
        path,
        init_git,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp tree with a known shape; `list_dirs` is never pointed at `$HOME`.
    struct Tree(std::path::PathBuf);

    impl Tree {
        fn new(name: &str, dirs: &[&str], files: &[&str]) -> Self {
            let root = std::env::temp_dir().join(format!(
                "grove_add_project_{name}_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            let _ = fs::create_dir_all(&root);
            for d in dirs {
                let _ = fs::create_dir_all(root.join(d));
            }
            for f in files {
                let _ = fs::write(root.join(f), b"x");
            }
            Self(root)
        }
        fn path(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cycle_wraps_both_ways_and_is_zero_on_empty() {
        assert_eq!(cycle(0, -1, 3), 2);
        assert_eq!(cycle(2, 1, 3), 0);
        assert_eq!(cycle(5, 1, 0), 0);
    }

    #[test]
    fn path_basename_falls_back_to_the_whole_string() {
        assert_eq!(path_basename("/a/b/grove"), "grove");
        assert_eq!(path_basename(""), "");
    }

    #[test]
    fn list_dirs_lists_a_trailing_slash_directory_sorted_and_dirs_only() {
        let t = Tree::new("slash", &["beta", "Alpha", "gamma"], &["notadir"]);
        let got = list_dirs(&format!("{}/", t.path()));
        let names: Vec<String> = got.iter().map(|p| path_basename(p)).collect();
        assert_eq!(names, vec!["Alpha", "beta", "gamma"], "{got:?}");
    }

    #[test]
    fn list_dirs_filters_by_the_last_segment_as_a_prefix() {
        let t = Tree::new("prefix", &["grove", "grove-core", "other"], &[]);
        let got = list_dirs(&format!("{}/grove", t.path()));
        let names: Vec<String> = got.iter().map(|p| path_basename(p)).collect();
        assert_eq!(names, vec!["grove", "grove-core"], "{got:?}");
    }

    #[test]
    fn list_dirs_hides_dotfiles_unless_the_prefix_starts_with_a_dot() {
        let t = Tree::new("dots", &[".hidden", "shown"], &[]);
        let plain: Vec<String> = list_dirs(&format!("{}/", t.path()))
            .iter()
            .map(|p| path_basename(p))
            .collect();
        assert_eq!(plain, vec!["shown"]);
        // A bare trailing "." is a path component Rust drops, so the prefix
        // has to carry a real character after the dot.
        let dotted: Vec<String> = list_dirs(&format!("{}/.h", t.path()))
            .iter()
            .map(|p| path_basename(p))
            .collect();
        assert_eq!(dotted, vec![".hidden"]);
    }

    #[test]
    fn list_dirs_on_an_unreadable_path_is_empty_not_a_panic() {
        assert!(list_dirs("/definitely/not/here/at/all/").is_empty());
    }

    #[test]
    fn set_path_is_ignored_on_the_details_step() {
        let mut st = opened();
        st.step = AddProjectStep::Details;
        st.path = "/kept".into();
        set_path(&mut st, "/typed".into());
        assert_eq!(st.path, "/kept");
    }

    #[test]
    fn set_path_resets_the_cursor_and_clears_the_note() {
        let mut st = opened();
        st.dir_sel = 4;
        st.note = Some("stale".into());
        set_path(&mut st, "/x".into());
        assert_eq!((st.dir_sel, st.note.clone()), (0, None));
    }

    #[test]
    fn dir_move_cycles_over_the_live_match_list() {
        let t = Tree::new("move", &["a", "b", "c"], &[]);
        let mut st = opened();
        st.path = format!("{}/", t.path());
        dir_move(&mut st, 1);
        assert_eq!(st.dir_sel, 1);
        dir_move(&mut st, -2);
        assert_eq!(st.dir_sel, 2, "wraps backwards over three entries");
    }

    #[test]
    fn dir_move_on_an_empty_list_parks_at_zero() {
        let mut st = opened();
        st.path = "/definitely/not/here/".into();
        st.dir_sel = 3;
        dir_move(&mut st, 1);
        assert_eq!(st.dir_sel, 0);
    }

    #[test]
    fn dir_pick_rewrites_the_buffer_with_a_trailing_slash() {
        let t = Tree::new("pick", &["only"], &[]);
        let mut st = opened();
        st.path = format!("{}/", t.path());
        dir_pick(&mut st);
        assert!(st.path.ends_with("/only/"), "{}", st.path);
        assert_eq!(st.dir_sel, 0);
    }

    #[test]
    fn choose_rejects_a_file_and_a_missing_path_with_a_note() {
        let t = Tree::new("reject", &[], &["afile"]);
        let mut st = opened();
        assert_eq!(
            choose(
                &mut st,
                &std::path::PathBuf::from(format!("{}/afile", t.path()))
            ),
            ChooseOutcome::Rejected
        );
        assert_eq!(st.note.as_deref(), Some("not a folder; choose a directory"));
        assert_eq!(st.step, AddProjectStep::PickSource, "the step stays put");
    }

    #[test]
    fn choose_advances_canonicalizes_and_clears_the_note() {
        let t = Tree::new("advance", &["proj"], &[]);
        let mut st = opened();
        st.note = Some("stale".into());
        let out = choose(
            &mut st,
            &std::path::PathBuf::from(format!("{}/proj", t.path())),
        );
        assert_eq!(out, ChooseOutcome::Advanced(GitProbe::NotRepo));
        assert_eq!(st.step, AddProjectStep::Details);
        assert!(st.path.ends_with("/proj"), "{}", st.path);
        assert_eq!(st.note, None);
    }

    #[test]
    fn choose_typed_is_a_no_op_off_the_pick_source_step() {
        let mut st = opened();
        st.step = AddProjectStep::Details;
        assert_eq!(choose_typed(&mut st), ChooseOutcome::Rejected);
        assert_eq!(st.step, AddProjectStep::Details);
    }

    #[test]
    fn change_source_goes_back_but_keeps_the_edited_name() {
        let mut st = opened();
        st.step = AddProjectStep::Details;
        st.name = "renamed".into();
        st.note = Some("stale".into());
        change_source(&mut st);
        assert_eq!(st.step, AddProjectStep::PickSource);
        assert_eq!(st.name, "renamed");
        assert_eq!(st.note, None);
    }

    fn details(path: &str, name: &str) -> AddProjectState {
        let mut st = opened();
        st.step = AddProjectStep::Details;
        st.path = path.into();
        st.name = name.into();
        st
    }

    #[test]
    fn submit_falls_back_to_the_folder_basename_when_the_name_is_blank() {
        let mut st = details("/tmp/grove-demo", "   ");
        assert_eq!(
            validate_submit(
                &mut st,
                &GitProbe::Repo {
                    branch: "main".into()
                },
                &[]
            ),
            SubmitOutcome::Register {
                name: "grove-demo".into(),
                path: "/tmp/grove-demo".into(),
                init_git: false,
            }
        );
    }

    #[test]
    fn submit_rejects_a_duplicate_name_and_a_duplicate_folder() {
        let existing = vec![("taken".to_string(), "/other".to_string())];
        let mut st = details("/fresh", "taken");
        assert_eq!(
            validate_submit(&mut st, &GitProbe::NotRepo, &existing),
            SubmitOutcome::Rejected
        );
        assert_eq!(st.note.as_deref(), Some("project 'taken' already exists"));

        let mut st = details("/other", "fresh");
        assert_eq!(
            validate_submit(&mut st, &GitProbe::NotRepo, &existing),
            SubmitOutcome::Rejected
        );
        assert_eq!(st.note.as_deref(), Some("folder already added as 'taken'"));
    }

    #[test]
    fn init_git_is_only_requested_for_a_non_repo_with_the_box_ticked() {
        let mut st = details("/p", "p");
        st.init_git = true;
        let SubmitOutcome::Register { init_git, .. } =
            validate_submit(&mut st, &GitProbe::NotRepo, &[])
        else {
            unreachable!()
        };
        assert!(init_git);

        let mut st = details("/p", "p");
        st.init_git = true;
        let SubmitOutcome::Register { init_git, .. } = validate_submit(
            &mut st,
            &GitProbe::Repo {
                branch: "main".into(),
            },
            &[],
        ) else {
            unreachable!()
        };
        assert!(!init_git, "an existing repo is never re-initialized");

        let mut st = details("/p", "p");
        st.init_git = false;
        let SubmitOutcome::Register { init_git, .. } =
            validate_submit(&mut st, &GitProbe::NotRepo, &[])
        else {
            unreachable!()
        };
        assert!(!init_git);
    }

    #[test]
    fn submit_is_a_no_op_from_the_pick_source_step() {
        let mut st = opened();
        assert_eq!(
            validate_submit(&mut st, &GitProbe::NotRepo, &[]),
            SubmitOutcome::Rejected
        );
    }
}

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::collections::HashSet;

#[test]
fn conservative_subcategories_preserve_context_and_parent_fallback() {
    let classifier = ChangeClassifier::default();
    for (path, parent, child) in [
        (
            "tests/fixtures/users.json",
            ChangeGroup::Tests,
            Some("Fixtures & snapshots"),
        ),
        ("tests/unit/parser.rs", ChangeGroup::Tests, Some("Unit")),
        (
            "tests/integration/client.rs",
            ChangeGroup::Tests,
            Some("Integration"),
        ),
        ("e2e/login.spec.ts", ChangeGroup::Tests, Some("End-to-end")),
        ("src/parser_test.go", ChangeGroup::Tests, None),
        ("src/unit/price.rs", ChangeGroup::Code, None),
        (
            ".github/workflows/test.yml",
            ChangeGroup::Config,
            Some("CI/CD"),
        ),
        (
            "Cargo.lock",
            ChangeGroup::Config,
            Some("Dependencies & lockfiles"),
        ),
        (
            "Dockerfile",
            ChangeGroup::Config,
            Some("Deployment & infrastructure"),
        ),
        (
            "vite.config.ts",
            ChangeGroup::Config,
            Some("Build & tooling"),
        ),
        (
            ".env.example",
            ChangeGroup::Config,
            Some("Environment & app settings"),
        ),
        (
            "install.sh",
            ChangeGroup::Code,
            Some("Scripts & automation"),
        ),
        (
            "scripts/clean.py",
            ChangeGroup::Code,
            Some("Scripts & automation"),
        ),
        ("assets/logo.png", ChangeGroup::Code, Some("Assets")),
        ("data/users.json", ChangeGroup::Uncategorized, None),
        ("data/users.yaml", ChangeGroup::Uncategorized, None),
        ("data/unknown.xyz", ChangeGroup::Uncategorized, None),
        (".hidden/data.json", ChangeGroup::Uncategorized, None),
    ] {
        assert_eq!(
            classifier.classify(path),
            (parent, child.map(str::to_owned)),
            "{path}"
        );
    }
}

#[test]
fn project_mappings_are_ordered_case_sensitive_and_segment_safe() {
    let classifier = ChangeClassifier::from_json(
        br#"{"rules":[
            {"path":"src/ui/special.rs","category":"code","subcategory":"Special"},
            {"path":"src/ui/","category":"code","subcategory":"UI"},
            {"path":"data/custom.json","category":"config"}
        ]}"#,
    )
    .unwrap();
    assert_eq!(
        classifier.classify("src/ui/special.rs").1.as_deref(),
        Some("Special")
    );
    assert_eq!(
        classifier.classify("src/ui/button.rs").1.as_deref(),
        Some("UI")
    );
    assert_eq!(
        classifier.classify("src/uikit/button.rs"),
        (ChangeGroup::Code, None)
    );
    assert_eq!(
        classifier.classify("src/UI/button.rs"),
        (ChangeGroup::Code, None)
    );
    assert_eq!(
        classifier.classify("data/custom.json"),
        (ChangeGroup::Config, None)
    );
    assert!(!classifier.matches("src/ui/special.rs", ChangeGroup::Code, Some("UI")));
    assert!(classifier.matches("src/ui/special.rs", ChangeGroup::Code, None));
}

#[test]
fn invalid_or_missing_mappings_fall_back_without_losing_files() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path().to_str().unwrap();
    assert_eq!(ChangeClassifier::load(root), ChangeClassifier::default());
    fs::create_dir(repo.path().join(".grove")).unwrap();
    let path = repo.path().join(".grove/change-categories.json");
    for contents in [
        r#"{"rules":[{"path":"../src/","category":"code"}]}"#.to_owned(),
        r#"{"rules":[{"path":"src/","category":"fiction"}]}"#.to_owned(),
        r#"{"rules":[{"path":"src/","category":"code","subcategory":""}]}"#.to_owned(),
        "invalid JSON".to_owned(),
        " ".repeat(64 * 1024 + 1),
    ] {
        fs::write(&path, contents).unwrap();
        let classifier = ChangeClassifier::load(root);
        assert_eq!(
            classifier.classify("src/main.rs"),
            (ChangeGroup::Code, None)
        );
    }
}

#[test]
fn hierarchical_totals_and_filters_cover_every_file_once() {
    let files = [
        ("tests/unit/a.rs", 3, 1),
        ("tests/unit/b.rs", 2, 0),
        ("tests/fixtures/a.json", 4, 2),
        ("tests/generic.rs", 1, 1),
        ("data/unknown.json", 5, 0),
    ]
    .map(|(path, added, removed)| FileChange {
        path: path.into(),
        status: Status::Modified,
        added,
        removed,
    });
    let summary = summarize(&files);
    assert_eq!(summary.files, 5);
    assert_eq!(
        summary.groups,
        vec![
            (ChangeGroup::Tests, 10, 4),
            (ChangeGroup::Uncategorized, 5, 0)
        ]
    );
    assert_eq!(
        summary.subgroups,
        vec![
            (ChangeGroup::Tests, "Fixtures & snapshots".into(), 4, 2),
            (ChangeGroup::Tests, "Unit".into(), 5, 1),
        ]
    );
    for (group, added, removed) in &summary.groups {
        let filtered: Vec<_> = files
            .iter()
            .filter(|f| summary.classifier.matches(&f.path, *group, None))
            .collect();
        assert_eq!(filtered.iter().map(|f| f.added).sum::<u32>(), *added);
        assert_eq!(filtered.iter().map(|f| f.removed).sum::<u32>(), *removed);
    }
    for (group, child, added, removed) in &summary.subgroups {
        let filtered: Vec<_> = files
            .iter()
            .filter(|f| summary.classifier.matches(&f.path, *group, Some(child)))
            .collect();
        assert_eq!(filtered.iter().map(|f| f.added).sum::<u32>(), *added);
        assert_eq!(filtered.iter().map(|f| f.removed).sum::<u32>(), *removed);
    }
}

#[test]
fn change_groups_classify_by_path() {
    for (path, group) in [
        ("src/views/sidebar.rs", ChangeGroup::Code),
        ("tests/checkout.rs", ChangeGroup::Tests),
        ("web/app/__tests__/menu.tsx", ChangeGroup::Tests),
        ("pkg/parse_test.go", ChangeGroup::Tests),
        ("ui/button.spec.ts", ChangeGroup::Tests),
        ("test_utils.py", ChangeGroup::Tests),
        ("README.md", ChangeGroup::Docs),
        ("docs/guide.html", ChangeGroup::Docs),
        ("LICENSE-MIT", ChangeGroup::Docs),
        ("Cargo.lock", ChangeGroup::Config),
        (".github/workflows/ci.yml", ChangeGroup::Config),
        (".gitignore", ChangeGroup::Config),
        ("src/latest.rs", ChangeGroup::Code),
        ("src/contest.rs", ChangeGroup::Code),
        ("src/views/tests.rs", ChangeGroup::Tests),
    ] {
        assert_eq!(ChangeGroup::of(path), group, "{path}");
    }
}

#[test]
fn summarize_totals_nonempty_groups_in_order() {
    let change = |path: &str, added, removed| FileChange {
        path: path.into(),
        status: Status::Modified,
        added,
        removed,
    };
    let summary = summarize(&[
        change("tests/a.rs", 10, 1),
        change("src/a.rs", 5, 2),
        change("src/b.rs", 3, 4),
        change("Cargo.toml", 1, 1),
    ]);
    assert_eq!(summary.files, 4);
    assert_eq!(
        summary.groups,
        vec![
            (ChangeGroup::Code, 8, 6),
            (ChangeGroup::Tests, 10, 1),
            (ChangeGroup::Config, 1, 1),
        ]
    );
}

#[test]
fn numstat_common_case() {
    let out = "12\t3\tsrc/main.rs\n";
    let entries = parse_numstat(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].added, Some(12));
    assert_eq!(entries[0].removed, Some(3));
    assert_eq!(entries[0].path, "src/main.rs");
    assert_eq!(entries[0].rename_from, None);
}

#[test]
fn numstat_binary_dash_dash() {
    let out = "-\t-\tassets/logo.png\n";
    let entries = parse_numstat(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].added, None);
    assert_eq!(entries[0].removed, None);
    assert_eq!(entries[0].path, "assets/logo.png");
}

#[test]
fn numstat_rename_full_path() {
    let out = "5\t2\told/name.rs => new/name.rs\n";
    let entries = parse_numstat(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "new/name.rs");
    assert_eq!(entries[0].rename_from.as_deref(), Some("old/name.rs"));
}

#[test]
fn numstat_rename_common_affix() {
    let out = "1\t1\tsrc/{old.rs => new.rs}\n";
    let entries = parse_numstat(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "src/new.rs");
    assert_eq!(entries[0].rename_from.as_deref(), Some("src/old.rs"));
}

#[test]
fn numstat_malformed_line_skipped() {
    let out = "not a valid line\n5\t1\tok.rs\n";
    let entries = parse_numstat(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "ok.rs");
}

#[test]
fn numstat_overflow_counts_skipped() {
    let out = "99999999999999\t1\tbig.rs\n";
    assert_eq!(parse_numstat(out).len(), 0);
}

#[test]
fn numstat_empty_input() {
    assert_eq!(parse_numstat("").len(), 0);
    assert_eq!(parse_numstat("\n").len(), 0);
}

#[test]
fn numstat_lopsided_dash_is_malformed() {
    let out = "-\t3\tweird.bin\n";
    assert_eq!(parse_numstat(out).len(), 0);
}

#[test]
fn status_porcelain_untracked() {
    let out = "?? new_file.txt\n";
    let entries = parse_status_porcelain(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].x, '?');
    assert_eq!(entries[0].y, '?');
    assert_eq!(entries[0].path, "new_file.txt");
}

#[test]
fn status_porcelain_rename_takes_destination() {
    let out = "R  old.rs -> new.rs\n";
    let entries = parse_status_porcelain(out);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "new.rs");
}

#[test]
fn status_porcelain_malformed_short_line_skipped() {
    assert_eq!(parse_status_porcelain("?\n").len(), 0);
    assert_eq!(parse_status_porcelain("\n").len(), 0);
}

#[test]
fn patch_single_hunk_common_case() {
    let diff = "diff --git a/f.rs b/f.rs\n\
index 111..222 100644\n\
--- a/f.rs\n\
+++ b/f.rs\n\
@@ -1,3 +1,3 @@ fn main\n\
 unchanged\n\
-old\n\
+new\n\
 tail\n";
    let patch = parse_patch(diff);
    let Patch::Text {
        hunks,
        no_newline_at_eof,
    } = patch
    else {
        panic!("expected Text patch");
    };
    assert!(!no_newline_at_eof);
    assert_eq!(hunks.len(), 1);
    let h = &hunks[0];
    assert_eq!(h.old_start, 1);
    assert_eq!(h.new_start, 1);
    assert_eq!(h.header, "fn main");
    assert_eq!(h.lines.len(), 4);
    assert_eq!(h.lines[0].kind, LineKind::Context);
    assert_eq!(h.lines[1].kind, LineKind::Del);
    assert_eq!(h.lines[1].text, "old");
    assert_eq!(h.lines[1].old_no, Some(2));
    assert_eq!(h.lines[1].new_no, None);
    assert_eq!(h.lines[2].kind, LineKind::Add);
    assert_eq!(h.lines[2].new_no, Some(2));
}

#[test]
fn patch_no_header_after_at_at() {
    let diff = "@@ -1,1 +1,1 @@\n-a\n+b\n";
    let Patch::Text { hunks, .. } = parse_patch(diff) else {
        panic!("expected Text patch");
    };
    assert_eq!(hunks[0].header, "");
}

#[test]
fn patch_multi_hunk() {
    let diff = "--- a/f.rs\n\
+++ b/f.rs\n\
@@ -1,1 +1,1 @@\n\
-a\n\
+b\n\
@@ -10,1 +10,1 @@\n\
-c\n\
+d\n";
    let Patch::Text { hunks, .. } = parse_patch(diff) else {
        panic!("expected Text patch");
    };
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].old_start, 1);
    assert_eq!(hunks[1].old_start, 10);
    assert_eq!(hunks[1].lines[0].text, "c");
}

#[test]
fn patch_no_newline_at_eof() {
    let diff = "--- a/f.rs\n\
+++ b/f.rs\n\
@@ -1,1 +1,1 @@\n\
-old\n\
\\ No newline at end of file\n\
+new\n\
\\ No newline at end of file\n";
    let Patch::Text {
        no_newline_at_eof, ..
    } = parse_patch(diff)
    else {
        panic!("expected Text patch");
    };
    assert!(no_newline_at_eof);
}

#[test]
fn patch_binary_marker() {
    let diff = "diff --git a/x.png b/x.png\n\
Binary files a/x.png and b/x.png differ\n";
    assert_eq!(parse_patch(diff), Patch::Binary);
}

#[test]
fn patch_malformed_hunk_header_skips_hunk() {
    let diff = "@@ garbage @@\n-a\n+b\n";
    let Patch::Text { hunks, .. } = parse_patch(diff) else {
        panic!("expected Text patch");
    };
    assert_eq!(hunks.len(), 0);
}

#[test]
fn patch_overflowing_line_numbers_skips_hunk() {
    let diff = "@@ -99999999999999,1 +1,1 @@\n-a\n+b\n";
    let Patch::Text { hunks, .. } = parse_patch(diff) else {
        panic!("expected Text patch");
    };
    assert_eq!(hunks.len(), 0);
}

#[test]
fn patch_empty_input() {
    let Patch::Text {
        hunks,
        no_newline_at_eof,
    } = parse_patch("")
    else {
        panic!("expected Text patch");
    };
    assert_eq!(hunks.len(), 0);
    assert!(!no_newline_at_eof);
}

#[test]
fn synthesize_added_patch_all_lines_added() {
    let Patch::Text { hunks, .. } = synthesize_added_patch("one\ntwo\nthree\n") else {
        panic!("expected Text patch");
    };
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0].lines.len(), 3);
    assert!(hunks[0].lines.iter().all(|l| l.kind == LineKind::Add));
    assert_eq!(hunks[0].lines[0].new_no, Some(1));
    assert_eq!(hunks[0].lines[2].new_no, Some(3));
}

fn line(kind: LineKind, text: &str, old_no: Option<u32>, new_no: Option<u32>) -> Line {
    Line {
        kind,
        text: text.to_string(),
        old_no,
        new_no,
    }
}

#[test]
fn unified_rows_orders_header_then_lines() {
    let patch = Patch::Text {
        hunks: vec![Hunk {
            old_start: 1,
            new_start: 1,
            header: "h".into(),
            lines: vec![
                line(LineKind::Context, "ctx", Some(1), Some(1)),
                line(LineKind::Del, "old", Some(2), None),
                line(LineKind::Add, "new", None, Some(2)),
            ],
        }],
        no_newline_at_eof: false,
    };
    let rows = patch.unified_rows();
    assert_eq!(rows.len(), 4);
    assert!(matches!(&rows[0], UnifiedRow::HunkHeader(h) if h == "h"));
    assert!(matches!(&rows[1], UnifiedRow::Line(l) if l.kind == LineKind::Context));
    assert!(matches!(&rows[2], UnifiedRow::Line(l) if l.kind == LineKind::Del));
    assert!(matches!(&rows[3], UnifiedRow::Line(l) if l.kind == LineKind::Add));
}

#[test]
fn unified_rows_empty_for_too_large_and_binary() {
    assert_eq!(
        Patch::TooLarge {
            added: 1,
            removed: 1
        }
        .unified_rows()
        .len(),
        0
    );
    assert_eq!(Patch::Binary.unified_rows().len(), 0);
}

#[test]
fn paired_rows_context_pairs_with_itself() {
    let patch = Patch::Text {
        hunks: vec![Hunk {
            old_start: 1,
            new_start: 1,
            header: String::new(),
            lines: vec![line(LineKind::Context, "ctx", Some(1), Some(1))],
        }],
        no_newline_at_eof: false,
    };
    let rows = patch.paired_rows();
    assert_eq!(rows.len(), 2); // header + one paired row
    let PairedRow::Lines { old, new } = &rows[1] else {
        panic!("expected Lines row");
    };
    assert_eq!(old.as_ref().unwrap().text, "ctx");
    assert_eq!(new.as_ref().unwrap().text, "ctx");
}

#[test]
fn paired_rows_equal_run_lengths_pair_positionally() {
    let patch = Patch::Text {
        hunks: vec![Hunk {
            old_start: 1,
            new_start: 1,
            header: String::new(),
            lines: vec![
                line(LineKind::Del, "d1", Some(1), None),
                line(LineKind::Del, "d2", Some(2), None),
                line(LineKind::Add, "a1", None, Some(1)),
                line(LineKind::Add, "a2", None, Some(2)),
            ],
        }],
        no_newline_at_eof: false,
    };
    let rows = patch.paired_rows();
    assert_eq!(rows.len(), 3); // header + 2 paired rows
    let PairedRow::Lines { old, new } = &rows[1] else {
        panic!()
    };
    assert_eq!(old.as_ref().unwrap().text, "d1");
    assert_eq!(new.as_ref().unwrap().text, "a1");
    let PairedRow::Lines { old, new } = &rows[2] else {
        panic!()
    };
    assert_eq!(old.as_ref().unwrap().text, "d2");
    assert_eq!(new.as_ref().unwrap().text, "a2");
}

#[test]
fn paired_rows_leftover_dels_become_half_empty_rows() {
    let patch = Patch::Text {
        hunks: vec![Hunk {
            old_start: 1,
            new_start: 1,
            header: String::new(),
            lines: vec![
                line(LineKind::Del, "d1", Some(1), None),
                line(LineKind::Del, "d2", Some(2), None),
                line(LineKind::Del, "d3", Some(3), None),
                line(LineKind::Add, "a1", None, Some(1)),
            ],
        }],
        no_newline_at_eof: false,
    };
    let rows = patch.paired_rows();
    assert_eq!(rows.len(), 4); // header + 3 rows (max(3,1))
    let PairedRow::Lines { old, new } = &rows[1] else {
        panic!()
    };
    assert_eq!(old.as_ref().unwrap().text, "d1");
    assert_eq!(new.as_ref().unwrap().text, "a1");
    let PairedRow::Lines { old, new } = &rows[2] else {
        panic!()
    };
    assert_eq!(old.as_ref().unwrap().text, "d2");
    assert!(new.is_none());
    let PairedRow::Lines { old, new } = &rows[3] else {
        panic!()
    };
    assert_eq!(old.as_ref().unwrap().text, "d3");
    assert!(new.is_none());
}

#[test]
fn paired_rows_leftover_adds_become_half_empty_rows() {
    let patch = Patch::Text {
        hunks: vec![Hunk {
            old_start: 1,
            new_start: 1,
            header: String::new(),
            lines: vec![
                line(LineKind::Add, "a1", None, Some(1)),
                line(LineKind::Add, "a2", None, Some(2)),
            ],
        }],
        no_newline_at_eof: false,
    };
    let rows = patch.paired_rows();
    assert_eq!(rows.len(), 3);
    let PairedRow::Lines { old, new } = &rows[1] else {
        panic!()
    };
    assert!(old.is_none());
    assert_eq!(new.as_ref().unwrap().text, "a1");
}

fn reconstruct(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

#[test]
fn word_runs_identical_lines_are_one_unchanged_run_each() {
    let (old, new) = word_runs("let x = 1;", "let x = 1;");
    assert_eq!(old, new);
    assert_eq!(old.len(), 1);
    assert!(!old[0].changed);
    assert_eq!(reconstruct(&old), "let x = 1;");
}

#[test]
fn word_runs_one_token_changed_in_the_middle() {
    let (old, new) = word_runs("let x = 1;", "let x = 2;");
    assert_eq!(reconstruct(&old), "let x = 1;");
    assert_eq!(reconstruct(&new), "let x = 2;");
    assert!(old.iter().any(|r| r.changed && r.text == "1"));
    assert!(new.iter().any(|r| r.changed && r.text == "2"));
    assert!(old.iter().any(|r| !r.changed && r.text.starts_with("let")));
}

#[test]
fn word_runs_insertion_at_start() {
    let (old, new) = word_runs("world", "hello world");
    assert_eq!(reconstruct(&old), "world");
    assert_eq!(reconstruct(&new), "hello world");
    assert!(old.iter().all(|r| !r.changed));
    assert!(new[0].changed && new[0].text == "hello ");
    assert!(!new.last().unwrap().changed);
}

#[test]
fn word_runs_insertion_at_end() {
    let (old, new) = word_runs("hello", "hello world");
    assert_eq!(reconstruct(&old), "hello");
    assert_eq!(reconstruct(&new), "hello world");
    assert!(old.iter().all(|r| !r.changed));
    assert!(new.last().unwrap().changed);
    assert!(!new[0].changed);
}

#[test]
fn word_runs_whole_line_replacement_no_common_affix() {
    let (old, new) = word_runs("abc", "xyz");
    assert_eq!(reconstruct(&old), "abc");
    assert_eq!(reconstruct(&new), "xyz");
    assert_eq!(old.len(), 1);
    assert_eq!(new.len(), 1);
    assert!(old[0].changed);
    assert!(new[0].changed);
}

#[test]
fn word_runs_empty_old() {
    let (old, new) = word_runs("", "new text");
    assert!(old.is_empty());
    assert_eq!(reconstruct(&new), "new text");
    assert!(new.iter().all(|r| r.changed));
}

#[test]
fn word_runs_empty_new() {
    let (old, new) = word_runs("old text", "");
    assert_eq!(reconstruct(&old), "old text");
    assert!(new.is_empty());
    assert!(old.iter().all(|r| r.changed));
}

#[test]
fn word_runs_both_empty() {
    let (old, new) = word_runs("", "");
    assert!(old.is_empty());
    assert!(new.is_empty());
}

#[test]
fn word_runs_no_empty_text_runs_anywhere() {
    for (a, b) in [
        ("", ""),
        ("", "x"),
        ("x", ""),
        ("abc", "abc"),
        ("abc", "xyz"),
        ("let x = 1;", "let x = 2;"),
    ] {
        let (old, new) = word_runs(a, b);
        assert!(old.iter().all(|r| !r.text.is_empty()));
        assert!(new.iter().all(|r| !r.text.is_empty()));
    }
}

#[test]
fn word_runs_cjk_multibyte_does_not_panic_and_reconstructs() {
    let (old, new) = word_runs("日本語のテキスト", "日本語のテキストです");
    assert_eq!(reconstruct(&old), "日本語のテキスト");
    assert_eq!(reconstruct(&new), "日本語のテキストです");
    assert!(old.iter().all(|r| r.changed));
    assert!(new.iter().all(|r| r.changed));
}

#[test]
fn word_runs_emoji_and_accented_chars_do_not_panic_and_reconstruct() {
    let old = "café 🎉 party";
    let new = "café 🎊 party time";
    let (old_runs, new_runs) = word_runs(old, new);
    assert_eq!(reconstruct(&old_runs), old);
    assert_eq!(reconstruct(&new_runs), new);
}

#[test]
fn word_runs_marks_land_on_word_boundaries_not_mid_character() {
    let (old, new) = word_runs("changed", "chunged");
    assert_eq!(reconstruct(&old), "changed");
    assert_eq!(reconstruct(&new), "chunged");
    assert_eq!(old.len(), 1);
    assert_eq!(new.len(), 1);
    assert!(old[0].changed && old[0].text == "changed");
    assert!(new[0].changed && new[0].text == "chunged");
}

fn fc(path: &str) -> FileChange {
    FileChange {
        path: path.to_string(),
        status: Status::Modified,
        added: 1,
        removed: 1,
    }
}

#[test]
fn reconcile_keeps_selection_on_a_path_that_dropped_out() {
    let old = vec![fc("a.rs"), fc("b.rs")];
    let new = vec![fc("a.rs")];
    let r = reconcile(&old, &new, Some("b.rs"));
    assert_eq!(r.selected.as_deref(), Some("b.rs"));
    assert_eq!(r.evicted, vec!["b.rs".to_string()]);
}

#[test]
fn reconcile_a_newly_appeared_file_does_not_steal_selection() {
    let old = vec![fc("a.rs")];
    let new = vec![fc("a.rs"), fc("new.rs")];
    let r = reconcile(&old, &new, Some("a.rs"));
    assert_eq!(r.selected.as_deref(), Some("a.rs"));
    assert!(r.evicted.is_empty());
}

#[test]
fn reconcile_selects_first_file_when_nothing_was_selected() {
    let old: Vec<FileChange> = vec![];
    let new = vec![fc("a.rs"), fc("b.rs")];
    let r = reconcile(&old, &new, None);
    assert_eq!(r.selected.as_deref(), Some("a.rs"));
}

#[test]
fn reconcile_none_selected_and_no_files_stays_none() {
    let r = reconcile(&[], &[], None);
    assert_eq!(r.selected, None);
    assert!(r.evicted.is_empty());
}

#[test]
fn reconcile_evicts_every_path_that_dropped_out_not_just_the_selected_one() {
    let old = vec![fc("a.rs"), fc("b.rs"), fc("c.rs")];
    let new = vec![fc("b.rs")];
    let r = reconcile(&old, &new, Some("b.rs"));
    let mut evicted = r.evicted;
    evicted.sort();
    assert_eq!(evicted, vec!["a.rs".to_string(), "c.rs".to_string()]);
}

#[test]
fn reconcile_untouched_list_evicts_nothing() {
    let old = vec![fc("a.rs")];
    let new = vec![fc("a.rs")];
    let r = reconcile(&old, &new, Some("a.rs"));
    assert!(r.evicted.is_empty());
    assert_eq!(r.selected.as_deref(), Some("a.rs"));
}

fn dirs(rows: &[TreeNode]) -> Vec<(&str, usize, bool)> {
    rows.iter()
        .filter_map(|n| match n {
            TreeNode::Dir {
                path,
                depth,
                expanded,
                ..
            } => Some((path.as_str(), *depth, *expanded)),
            TreeNode::File { .. } => None,
        })
        .collect()
}

fn files(rows: &[TreeNode]) -> Vec<&str> {
    rows.iter()
        .filter_map(|n| match n {
            TreeNode::File { file, .. } => Some(file.path.as_str()),
            TreeNode::Dir { .. } => None,
        })
        .collect()
}

#[test]
fn flatten_flat_files_have_no_dir_rows() {
    let files_in = vec![fc("a.rs"), fc("b.rs")];
    let rows = flatten_file_tree(&files_in, &HashSet::new());
    assert!(dirs(&rows).is_empty());
    assert_eq!(files(&rows), vec!["a.rs", "b.rs"]);
}

#[test]
fn flatten_nested_dirs_emit_one_dir_row_per_level() {
    let files_in = vec![fc("src/views/rows.rs")];
    let rows = flatten_file_tree(&files_in, &HashSet::new());
    assert_eq!(dirs(&rows), vec![("src", 0, true), ("src/views", 1, true)]);
    assert_eq!(files(&rows), vec!["src/views/rows.rs"]);
}

#[test]
fn flatten_single_deep_path() {
    let files_in = vec![fc("a/b/c/d/e.rs")];
    let rows = flatten_file_tree(&files_in, &HashSet::new());
    assert_eq!(
        dirs(&rows),
        vec![
            ("a", 0, true),
            ("a/b", 1, true),
            ("a/b/c", 2, true),
            ("a/b/c/d", 3, true)
        ]
    );
    assert_eq!(files(&rows), vec!["a/b/c/d/e.rs"]);
}

#[test]
fn flatten_sibling_files_at_multiple_levels() {
    let files_in = vec![fc("a.rs"), fc("src/b.rs"), fc("src/nested/c.rs")];
    let rows = flatten_file_tree(&files_in, &HashSet::new());
    assert_eq!(files(&rows), vec!["a.rs", "src/b.rs", "src/nested/c.rs"]);
    assert_eq!(dirs(&rows), vec![("src", 0, true), ("src/nested", 1, true)]);
}

#[test]
fn flatten_a_collapsed_dir_hides_its_children() {
    let files_in = vec![fc("src/a.rs"), fc("src/nested/b.rs"), fc("top.rs")];
    let mut collapsed = HashSet::new();
    collapsed.insert("src".to_string());
    let rows = flatten_file_tree(&files_in, &collapsed);
    // "src" itself still renders (so it can be reopened); its children do not.
    assert_eq!(dirs(&rows), vec![("src", 0, false)]);
    assert_eq!(files(&rows), vec!["top.rs"]);
}

#[test]
fn flatten_collapsing_a_shared_ancestor_reuses_the_stack_correctly() {
    let files_in = vec![fc("a/one.rs"), fc("a/two.rs"), fc("b/three.rs")];
    let mut collapsed = HashSet::new();
    collapsed.insert("a".to_string());
    let rows = flatten_file_tree(&files_in, &collapsed);
    assert_eq!(dirs(&rows), vec![("a", 0, false), ("b", 0, true)]);
    assert_eq!(files(&rows), vec!["b/three.rs"]);
}

use super::import::markdown_links;
use super::*;
use crate::provider::SavedSession;
use std::path::Path;
#[test]
fn native_presentation_is_bounded_immutable_and_included_in_copy_and_approval() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = PlanStore::default();
    let mut value = serde_json::json!({"schema_version":1,"plan_id":"native","revision_id":"r1","title":"Native overview","markdown":"## Implementation\nComplete technical details.","presentation":{"outcome":"Keep the draft intact.","flow":[{"title":"Choose","detail":"Select agent"}],"included":["Account login"],"deferred":["API keys"],"risks":["Verify provider status"],"success_criteria":["Cancel preserves draft"],"revision_changes":[]}});
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    import(&mut store, dir.path()).unwrap();
    let copy = store.revisions[0].document.markdown_body();
    for text in [
        "Keep the draft intact.",
        "Account login",
        "API keys",
        "Verify provider status",
        "Cancel preserves draft",
        "Complete technical details.",
    ] {
        assert!(copy.contains(text), "{copy}");
    }
    let (_, approval) = store.prepare_approval("native", "r1").unwrap().unwrap();
    assert!(approval.contains("Keep the draft intact."));
    assert!(approval.contains("presentation"));
    value["presentation"]["outcome"] = "A changed scope".into();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("immutable"));
    for (key, invalid) in [
        ("outcome", serde_json::json!("")),
        (
            "flow",
            serde_json::json!([{"title":"","detail":"Missing label"}]),
        ),
        ("risks", serde_json::json!([])),
        ("success_criteria", serde_json::json!(["x".repeat(1001)])),
    ] {
        let mut invalid_value = value.clone();
        invalid_value["revision_id"] = "invalid".into();
        invalid_value["presentation"][key] = invalid;
        fs_err::write(
            dir.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&invalid_value).unwrap(),
        )
        .unwrap();
        assert!(import(&mut store, dir.path()).is_err(), "{key}");
    }
}
#[test]
fn behavior_tree_presentation_preserves_exhibits_scope_and_finalization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.grove-plan.json");
    let mut store = PlanStore::default();
    let mut value = serde_json::json!({
        "schema_version":1,"plan_id":"behavior","revision_id":"r1","title":"Native behavior",
        "presentation":{"outcome":"Keep execution on the host.","included":["Native pairing"],
            "deferred":["Browser clients"],"risks":["Full host control"],
            "success_criteria":["Verify Codex and Claude separately"]},
        "claims":[{"id":"pair","text":"You can pair another desktop.",
            "exhibit":{"kind":"code","language":"rust","text":"let host = connect();"},
            "detail":"Grant one revocable device invitation.",
            "decisions":[{"id":"transport","prompt":"How do desktops connect?",
                "options":[{"id":"direct","label":"Direct connection","description":"Needs a reachable host."}],
                "default":"direct"}],
            "children":[{"id":"identity","text":"The host verifies device identity.","detail":"Reject an expired invitation."}]}],
        "scope":["Repository sync is deferred."]
    });
    fs_err::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    import(&mut store, dir.path()).unwrap();
    assert_eq!(store.revisions[0].unanswered_decisions(), vec!["transport"]);
    let copy = store.revisions[0].document.markdown_body();
    for expected in [
        "Keep execution on the host.",
        "Native pairing",
        "Browser clients",
        "Full host control",
        "Verify Codex and Claude separately",
        "let host = connect();",
        "Grant one revocable device invitation.",
        "Reject an expired invitation.",
        "Needs a reachable host.",
        "Repository sync is deferred.",
    ] {
        assert!(copy.contains(expected), "Missing {expected}: {copy}");
    }
    assert!(store.prepare_approval("behavior", "r1").is_err());
    store.revisions[0]
        .review
        .answers
        .insert("transport".into(), "direct".into());
    let (delivery, _) = store.prepare_delivery("behavior", "r1").unwrap().unwrap();
    store.confirm_delivery(delivery);
    value["revision_id"] = "r2".into();
    value["claims"][0]["decisions"] = serde_json::json!([]);
    value["finalized_from"] =
        serde_json::json!({"revision_id":"r1","answers":{"transport":"Direct connection"}});
    // Removing decision definitions or editing overview metadata does not incorporate answers.
    value["presentation"]["outcome"] = "The desktop uses a direct connection.".into();
    fs_err::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("must update"));
    value["claims"][0]["detail"] =
        "Connect directly to a reachable host and grant one revocable invitation.".into();
    fs_err::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    import(&mut store, dir.path()).unwrap();
    let (_, approval) = store.prepare_approval("behavior", "r2").unwrap().unwrap();
    assert!(approval.contains("Full host control"));
    assert!(approval.contains("Connect directly to a reachable host"));
    let mut restored: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    restored.recover_delivery();
    assert!(restored
        .prepare_approval("behavior", "r2")
        .unwrap()
        .is_none());
    // Complete Markdown and structured claims are mutually exclusive.
    value["revision_id"] = "invalid".into();
    value["markdown"] = "A second body".into();
    fs_err::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("not both"));
}

#[test]
fn exhibit_markdown_keeps_code_fences_safe_and_asset_paths_inert() {
    let code = Exhibit::Code {
        language: "rust\n```html".into(),
        text: "```\n<script>literal</script>".into(),
        source: None,
    };
    let rendered = exhibit_markdown(&code);
    assert!(rendered.starts_with("\n````rusthtml\n"));
    assert!(rendered.ends_with("\n````\n"));
    let asset = Exhibit::Image {
        path: "unsafe](javascript:run)".into(),
        alt: "A saved preview".into(),
    };
    assert_eq!(exhibit_markdown(&asset), "\nA saved preview\n");
}

#[test]
fn oversized_files_retain_the_affected_path_and_limit_and_can_be_reimported() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("plan.grove-plan.json");
    let mut store = PlanStore::default();
    fs_err::write(&plan, vec![b' '; 1024 * 1024 + 37]).unwrap();
    assert!(import(&mut store, dir.path()).is_err());
    let size = store.errors[0].size_limit.as_ref().unwrap();
    assert_eq!(size.path, plan.canonicalize().unwrap());
    assert_eq!(size.bytes, 1024 * 1024 + 37);
    assert_eq!(size.limit, 1024 * 1024);
    for (asset, limit, exhibit) in [
        ("comparison.html", 1024 * 1024, "html_link"),
        ("artwork.png", 8 * 1024 * 1024, "image"),
    ] {
        let path = dir.path().join(asset);
        fs_err::write(&path, vec![b'x'; limit + 7]).unwrap();
        fs_err::write(&plan, serde_json::to_vec(&serde_json::json!({
            "schema_version":1, "plan_id":"size-qa", "revision_id":"r1", "title":"Size QA",
            "claims":[{"id":"claim", "text":"Evidence", "exhibit":{"kind":exhibit, "path":asset, "alt":"Art", "label":"Comparison"}}]
        })).unwrap()).unwrap();
        assert!(import(&mut store, dir.path()).is_err());
        let size = store.errors[0].size_limit.as_ref().unwrap();
        assert_eq!(size.path, path.canonicalize().unwrap());
        assert_eq!(size.bytes, limit as u64 + 7);
        assert_eq!(size.limit, limit as u64);
    }
    fs_err::write(&plan, r#"{"schema_version":1,"plan_id":"size-qa","revision_id":"r2","title":"Fixed","claims":[{"id":"fixed","text":"Fixed"}]}"#).unwrap();
    import(&mut store, dir.path()).unwrap();
    assert!(store.errors.is_empty());
    assert_eq!(store.revisions[0].document.revision_id, "r2");
}

#[test]
fn restore_refreshes_legacy_size_errors_through_the_guarded_importer_once() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("plan.grove-plan.json");
    fs_err::write(&plan, vec![b' '; 1024 * 1024 + 19]).unwrap();
    let mut saved: SavedSession = serde_json::from_value(serde_json::json!({
        "key":"size-restore", "cwd":dir.path(), "project":"test", "label":"test", "roots":[], "items":[],
        "rich_plans":{"errors":[{"message_id":"m", "source":"plan.grove-plan.json", "error":"Plan file exceeds size limit"}]}
    })).unwrap();
    import_completed(&mut saved);
    assert_eq!(
        saved.rich_plans.errors[0]
            .size_limit
            .as_ref()
            .unwrap()
            .bytes,
        1024 * 1024 + 19
    );
    // A subsequent provider update retains the captured failure, without
    // re-reading a file that changed behind an immutable transcript entry.
    fs_err::write(&plan, "changed").unwrap();
    import_completed(&mut saved);
    assert!(saved.rich_plans.errors[0].size_limit.is_some());
}

#[test]
fn persisted_generic_import_errors_do_not_become_size_failures() {
    let error: ImportError = serde_json::from_str(r#"{"message_id":"m","source":"plan.grove-plan.json","error":"Plan file exceeds size limit"}"#).unwrap();
    assert!(error.size_limit.is_none());
}

#[test]
fn injected_authoring_example_imports_with_complete_v1_shapes() {
    let example = GUIDANCE
        .split_once("```json\n")
        .expect("authoring guidance must include a self-contained JSON example")
        .1
        .split_once("\n```")
        .unwrap()
        .0;
    let dir = tempfile::tempdir().unwrap();
    fs_err::write(dir.path().join("plan.grove-plan.json"), example).unwrap();
    let mut store = PlanStore::default();
    import(&mut store, dir.path()).unwrap();
    let doc = &store.revisions[0].document;
    assert!(doc.claims.is_empty());
    assert!(doc.markdown_body().starts_with("# Outcome"));
    assert!(doc.markdown_body().contains("verification"));
    assert!(store.errors.is_empty());
}
#[test]
fn markdown_plans_preserve_exact_body_and_immutable_revision() {
    let dir = tempfile::tempdir().unwrap();
    let body =
        "# Search\n\n| Metric | Target |\n|---|---|\n| p95 | 50 ms |\n\n```rust\nfn main() {}\n```";
    let mut doc = serde_json::json!({"schema_version":1,"plan_id":"markdown","revision_id":"r1","title":"Search","markdown":body});
    let path = dir.path().join("plan.grove-plan.json");
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let mut store = PlanStore::default();
    import(&mut store, dir.path()).unwrap();
    assert_eq!(store.revisions[0].document.markdown_body(), body);
    assert!(store.revisions[0]
        .approval_text()
        .unwrap()
        .contains("Search"));
    doc["markdown"] = "Changed".into();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("immutable"));
    assert_eq!(store.revisions[0].document.markdown_body(), body);
    doc["revision_id"] = "r2".into();
    doc["markdown"] = "   ".into();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path()).is_err());
}

fn approval_store() -> PlanStore {
    let mut store = PlanStore::default();
    store.revisions.push(PlanRevision {
        approval_digest: None,
        message_id: "m".into(),
        source: "plan.grove-plan.json".into(),
        document: document(),
        assets: vec![],
        review: ReviewState::default(),
    });
    store.revisions[0].document.claims[0].decisions.clear();
    store
}
#[test]
fn plan_request_display_preserves_payload_and_literal_custom_answers() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    let custom = "Use inline\n```json\n{custom}\n```\n[link](https://example.test)";
    store.revisions[0]
        .review
        .own_answers
        .insert("choice".into(), custom.into());
    let raw = store.revisions[0].response_text();
    assert!(
        store.response_display_text(&raw).is_none(),
        "unjournaled text stays visible"
    );
    let (_, payload) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    let saved = serde_json::to_vec(&store).unwrap();
    let display = store.response_display_text(&payload).unwrap();
    assert!(display.contains("Update plan with decisions"));
    assert!(display.contains("Where?"));
    assert!(display.contains("Use inline"));
    assert!(!display.contains("finalized_from"));
    assert!(!display.contains("```json"));
    assert!(display.contains("\\[link\\]"));
    assert!(payload.contains("finalized_from"));
    assert_eq!(serde_json::to_vec(&store).unwrap(), saved);
    assert!(store
        .response_display_text(&(payload.clone() + " extra feedback"))
        .is_none());
    let restored: PlanStore = serde_json::from_slice(&saved).unwrap();
    assert_eq!(restored.response_display_text(&payload).unwrap(), display);
    let (before, rest) = payload.split_once("\n```json\n").unwrap();
    let (_, after) = rest.split_once("\n```").unwrap();
    let reordered = format!(
        "{before}\n```json\n{{\"answers\":{{\"choice\":{}}},\"revision_id\":\"r1\"}}\n```{after}",
        serde_json::to_string(custom).unwrap()
    );
    assert_eq!(restored.response_display_text(&reordered).unwrap(), display);
    assert!(restored
        .response_display_text(&reordered.replace("Use inline", "Changed answer"))
        .is_none());
}

#[test]
fn approval_requires_resolved_explicit_decisions_and_latest_revision() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    assert!(store.prepare_approval("plan", "r1").is_err());
    assert!(store.prepare_delivery("plan", "r1").is_err());
    store.revisions[0]
        .review
        .own_answers
        .insert("choice".into(), "Use inline".into());
    assert!(store
        .prepare_approval("plan", "r1")
        .unwrap_err()
        .contains("finalized"));
    let (delivery, text) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    assert!(text.contains("finalized_from"));
    assert!(text.contains("Use inline"));
    let restored: PlanStore = serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    assert_eq!(restored.revisions[0].response_text(), text);
    store.confirm_delivery(delivery);
    let finalized = serde_json::json!({"schema_version":1,"plan_id":"plan","revision_id":"r2","title":"Review me","markdown":"# Final plan\nUse inline in the approach and verify inline behavior.","finalized_from":{"revision_id":"r1","answers":{"choice":"Use inline"}}});
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&finalized).unwrap(),
    )
    .unwrap();
    import(&mut store, dir.path()).unwrap();
    let (id, text) = store.prepare_approval("plan", "r2").unwrap().unwrap();
    assert!(text.contains("The user explicitly approved this immutable plan revision"));
    assert!(text.contains("Revision: r2"));
    assert!(text.contains("Use inline"));
    assert!(store.prepare_approval("plan", "r2").unwrap().is_none());
    let mut next = store.revisions[1].clone();
    next.document.revision_id = "r3".into();
    next.document.finalized_from = None;
    store.revisions.push(next);
    assert!(store.prepare_approval("plan", "r2").is_err());
    assert!(store.validate_dispatch(id).is_err());
    assert!(matches!(
        store.revisions[1].review.approval,
        ResponseDelivery::Failed { .. }
    ));
}
#[test]
fn approval_receipt_requires_journaled_exact_payload_and_survives_restore() {
    let mut store = approval_store();
    store.revisions[0]
        .review
        .own_answers
        .insert("choice".into(), "Use inline".into());
    let canonical = store.revisions[0].approval_text().unwrap();
    assert!(store.approval_for_payload(&canonical).is_none());
    let (id, text) = store.prepare_approval("plan", "r1").unwrap().unwrap();
    assert_eq!(
        text, canonical,
        "receipt provenance must not change the provider payload"
    );
    assert_eq!(
        store
            .approval_for_payload(&text)
            .unwrap()
            .document
            .revision_id,
        "r1"
    );
    assert!(store
        .approval_for_payload("# Explicit plan approval: Review me")
        .is_none());
    assert!(store
        .approval_for_payload(&(text.clone() + " altered"))
        .is_none());
    store.confirm_delivery(id);
    let mut restored: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    assert!(restored.approval_for_payload(&text).is_some());
    // Historical sessions predate digest provenance but retain locked approval journals.
    restored.revisions[0].approval_digest = None;
    assert!(restored.approval_for_payload(&text).is_some());
    restored.revisions[0].review.approval = ResponseDelivery::Draft;
    assert!(restored.approval_for_payload(&text).is_none());
}
#[test]
fn approval_size_failure_never_journals_and_review_cannot_forge_approval() {
    let mut store = approval_store();
    store.revisions[0].document.claims[0].decisions.clear();
    store.revisions[0].document.claims[0].detail = Some("x".repeat(512 * 1024));
    assert!(store
        .prepare_approval("plan", "r1")
        .unwrap_err()
        .contains("smaller plan revision"));
    assert!(matches!(
        store.revisions[0].review.approval,
        ResponseDelivery::Draft
    ));
    let review = ReviewState {
        approval: ResponseDelivery::Sent { id: 9 },
        ..Default::default()
    };
    store.update_review("plan", "r1", review).unwrap();
    assert!(matches!(
        store.revisions[0].review.approval,
        ResponseDelivery::Draft
    ));
}
#[test]
fn approval_is_independent_of_feedback_and_recovers_without_replay() {
    let mut store = approval_store();
    let (feedback, _) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    assert!(store.prepare_approval("plan", "r1").is_err());
    store.confirm_delivery(feedback);
    let mut review = ReviewState::default();
    review.answers.insert("choice".into(), "inline".into());
    store.update_review("plan", "r1", review).unwrap();
    let (approval, _) = store.prepare_approval("plan", "r1").unwrap().unwrap();
    assert_ne!(feedback, approval);
    let mut restored: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    restored.recover_delivery();
    assert!(matches!(
        restored.revisions[0].review.approval,
        ResponseDelivery::Uncertain { .. }
    ));
    assert!(matches!(
        restored.revisions[0].review.delivery,
        ResponseDelivery::Sent { .. }
    ));
    assert!(restored.prepare_approval("plan", "r1").unwrap().is_none());
    restored.confirm_delivery(approval);
    assert!(matches!(
        restored.revisions[0].review.approval,
        ResponseDelivery::Sent { .. }
    ));
    store.fail_delivery(approval, "disk full");
    assert_eq!(
        store.prepare_approval("plan", "r1").unwrap().unwrap().0,
        approval
    );
    store.cancel_delivery(approval);
    assert!(matches!(
        store.revisions[0].review.approval,
        ResponseDelivery::Draft
    ));
    let old: ReviewState = serde_json::from_str("{}").unwrap();
    assert!(matches!(old.approval, ResponseDelivery::Draft));
}
#[test]
fn editorial_fields_are_optional_and_round_trip_without_changing_feedback() {
    let old = document();
    assert!(old.summary.is_none());
    assert!(old.claims[0].detail.is_none());
    let original = PlanRevision {
        approval_digest: None,
        message_id: "m".into(),
        source: "plan.grove-plan.json".into(),
        document: old.clone(),
        assets: vec![],
        review: ReviewState::default(),
    };
    let mut updated = original.clone();
    updated.document.summary = Some("A readable outcome".into());
    updated.document.claims[0].detail = Some("An explanation of this claim".into());
    let restored: PlanDocument =
        serde_json::from_value(serde_json::to_value(&updated.document).unwrap()).unwrap();
    assert_eq!(restored, updated.document);
    assert_eq!(original.response_text(), updated.response_text());
}

#[test]
fn contextual_decisions_preserve_valid_answers_and_reset_changed_questions() {
    let (dir, mut store) = fixture();
    let mut doc = serde_json::json!({"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Plan","markdown":"# Plan\n## Rollout\nChoose a rollout.","decisions":[{"id":"rollout","section":"Rollout","prompt":"Who first?","options":[{"id":"pilot","label":"Pilot"}],"default":"pilot"},{"id":"custom","prompt":"Your policy?","options":[]}]});
    let path = dir.path().join("plan.grove-plan.json");
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    import(&mut store, dir.path()).unwrap();
    assert_eq!(store.revisions[0].unanswered_decisions().len(), 2);
    store.revisions[0]
        .review
        .answers
        .insert("rollout".into(), "pilot".into());
    store.revisions[0].review.own_answers.insert(
        "custom".into(),
        "Owners and leads\nKeep this literal <answer>".into(),
    );
    store.revisions[0]
        .review
        .answer_drafts
        .insert("custom".into(), "Unsent newer draft".into());
    assert_eq!(store.revisions[0].resolved_answers()["rollout"], "Pilot");
    let (id, text) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    assert!(!text.contains("Unsent newer draft"));
    store.confirm_delivery(id);
    let mut spoof = store.revisions[0].review.clone();
    spoof.submitted_answers = Some(HashMap::from([("custom".into(), "spoof".into())]));
    spoof
        .own_answers
        .insert("custom".into(), "edited behind delivery".into());
    store.update_review("plan", "r1", spoof).unwrap();
    assert!(store.revisions[0]
        .review
        .submitted_answers
        .as_ref()
        .unwrap()["custom"]
        .contains("Owners and leads"));
    doc["revision_id"] = "r2".into();
    doc["decisions"][0]["prompt"] = "Which workspaces first?".into();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    import(&mut store, dir.path()).unwrap();
    assert_eq!(store.revisions[1].unanswered_decisions(), vec!["rollout"]);
    assert!(store.revisions[1].resolved_answers()["custom"].contains("Owners and leads"));
    assert!(store.revisions[1].review.submitted_answers.is_none());
}

#[test]
fn finalization_rejects_omission_spoofing_unchanged_markdown_and_restored_bypass() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    let path = dir.path().join("plan.grove-plan.json");
    let mut doc = serde_json::json!({"schema_version":1,"plan_id":"plan","revision_id":"r2","title":"Final","markdown":"# Final\nUse inline."});
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("finalized_from"));
    doc["finalized_from"] =
        serde_json::json!({"revision_id":"r1","answers":{"choice":"Inline card"}});
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("delivered"));
    store.revisions[0]
        .review
        .answers
        .insert("choice".into(), "inline".into());
    let (id, _) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    store.confirm_delivery(id);
    doc["finalized_from"]["answers"]["choice"] = "Wrong answer".into();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("match"));
    doc["finalized_from"]["answers"]["choice"] = "Inline card".into();
    doc["markdown"] = store.revisions[0].document.markdown_body().into();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("update the Markdown"));
    // A new overview alone cannot substitute for incorporating answers in Markdown.
    doc["presentation"] = serde_json::json!({"outcome":"Now choose inline","risks":["Preserve the decision boundary"],"success_criteria":["Inline is verified"]});
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut store, dir.path())
        .unwrap_err()
        .contains("update the Markdown"));
    doc.as_object_mut().unwrap().remove("presentation");
    doc["markdown"] = "# Final\nUse inline.".into();
    let mut fake = store.revisions[0].clone();
    fake.document = serde_json::from_value(doc.clone()).unwrap();
    fake.document.finalized_from = None;
    fake.review = ReviewState::default();
    store.revisions.push(fake);
    let mut restored: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    assert!(restored.prepare_approval("plan", "r2").is_err());
    restored.revisions.pop();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    import(&mut restored, dir.path()).unwrap();
    restored.revisions[0].message_id = "old".into();
    restored.revisions[1].message_id = "final".into();
    restored.references.clear();
    restored.retain_messages(&[serde_json::json!({"id":"final"})]);
    assert_eq!(
        restored.revisions.len(),
        2,
        "finalization retains its journal lineage"
    );
    assert!(restored.prepare_approval("plan", "r2").unwrap().is_some());
}

#[test]
fn changing_finalized_decisions_blocks_approval_until_a_new_decision_cycle() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    store.revisions[0]
        .review
        .own_answers
        .insert("choice".into(), "Use inline".into());
    let (id, _) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    store.confirm_delivery(id);
    let mut doc = serde_json::json!({"schema_version":1,"plan_id":"plan","revision_id":"r2","title":"Final","markdown":"# Final\nUse inline in the approach.","finalized_from":{"revision_id":"r1","answers":{"choice":"Use inline"}}});
    let path = dir.path().join("plan.grove-plan.json");
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    import(&mut store, dir.path()).unwrap();
    assert!(store.approval_error("plan", "r2").is_none());
    let mut editorial = store.clone();
    let mut followup = doc.clone();
    followup["revision_id"] = "r3".into();
    followup["markdown"] = "# Final\nUse inline; clarify the verification steps.".into();
    fs_err::write(&path, serde_json::to_vec(&followup).unwrap()).unwrap();
    import(&mut editorial, dir.path()).unwrap();
    assert!(editorial.approval_error("plan", "r3").is_none());
    let mut already_approved = store.clone();
    already_approved.prepare_approval("plan", "r2").unwrap();
    let request = ReviewState {
        request_decision_revision: true,
        ..Default::default()
    };
    already_approved
        .update_review("plan", "r2", request.clone())
        .unwrap();
    assert!(
        !already_approved.revisions[1]
            .review
            .request_decision_revision
    );
    store.update_review("plan", "r2", request).unwrap();
    assert!(store.prepare_approval("plan", "r2").is_err());
    store
        .update_review("plan", "r2", ReviewState::default())
        .unwrap();
    assert!(
        store.revisions[1].review.request_decision_revision,
        "a stale UI snapshot cannot retract the change request"
    );
    let (id, payload) = store.prepare_delivery("plan", "r2").unwrap().unwrap();
    assert!(payload.contains("Where?"));
    assert!(payload.contains("Use inline"));
    assert!(payload.contains("Do not implement"));
    let display = store.response_display_text(&payload).unwrap();
    assert!(display.contains("Change plan decisions"));
    assert!(display.contains("Previous answers"));
    assert!(display.contains("Where?"));
    assert!(display.contains("Use inline"));
    assert!(!display.contains("```json"));
    assert!(!display.contains("Original decision definitions"));
    assert!(payload.contains("Original decision definitions"));
    store.confirm_delivery(id);
    assert_eq!(store.response_display_text(&payload).unwrap(), display);
    store
        .update_review("plan", "r2", ReviewState::default())
        .unwrap();
    assert!(store.prepare_approval("plan", "r2").is_err());
    let mut restored: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    assert!(restored.prepare_approval("plan", "r2").is_err());
    doc["revision_id"] = "r3".into();
    doc.as_object_mut().unwrap().remove("finalized_from");
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    assert!(import(&mut restored, dir.path())
        .unwrap_err()
        .contains("publish a decision revision"));
    let mut unchanged = restored.clone();
    doc["decisions"] =
        serde_json::to_value(unchanged.revisions[0].document.all_decisions()).unwrap();
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    import(&mut unchanged, dir.path()).unwrap();
    assert_eq!(
        unchanged.revisions[2].resolved_answers()["choice"],
        "Use inline"
    );
    assert!(unchanged.revisions[2].unanswered_decisions().is_empty());
    assert!(
        unchanged.prepare_approval("plan", "r3").is_err(),
        "retained explicit answers still require a new finalized revision"
    );
    doc["decisions"] = serde_json::json!([{"id":"choice","prompt":"Where should it appear now?","options":[{"id":"inline","label":"Inline card"}]}]);
    fs_err::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    import(&mut restored, dir.path()).unwrap();
    assert_eq!(restored.revisions[2].unanswered_decisions(), vec!["choice"]);
    assert!(restored.prepare_approval("plan", "r3").is_err());
    assert!(!restored.revisions[2].review.request_decision_revision);
}

fn document() -> PlanDocument {
    serde_json::from_value(serde_json::json!({"schema_version":1,"plan_id":"plan","revision_id":"r1","title":"Review me","claims":[{"id":"claim","text":"A claim","decisions":[{"id":"choice","prompt":"Where?","options":[{"id":"inline","label":"Inline card"}],"default":"inline"}]}]})).unwrap()
}
fn fixture() -> (tempfile::TempDir, PlanStore) {
    let dir = tempfile::tempdir().unwrap();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&document()).unwrap(),
    )
    .unwrap();
    (dir, PlanStore::default())
}
fn import(store: &mut PlanStore, dir: &Path) -> Result<(), String> {
    store.import(
        "message",
        "plan.grove-plan.json",
        dir.to_str().unwrap(),
        &[dir.to_owned()],
    )
}
#[test]
fn encoded_destinations_decode_once_before_guarded_import() {
    let (dir, mut store) = fixture();
    let root = dir.path();
    for name in ["space name", "space%20name", "é+%", "nested"] {
        fs_err::create_dir(root.join(name)).unwrap();
        let mut doc = document();
        doc.plan_id = name.into();
        fs_err::write(
            root.join(name).join("plan.grove-plan.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
    }
    for (source, expected) in [
        ("space%20name", "space name"),
        ("space%2520name", "space%20name"),
        ("%C3%A9+%25", "é+%"),
        ("nested%2F", "nested"),
    ] {
        let target = format!("{source}/plan.grove-plan.json");
        store
            .import(source, &target, root.to_str().unwrap(), &[root.into()])
            .unwrap();
        assert!(store
            .revisions
            .iter()
            .any(|r| r.document.plan_id == expected));
    }
    let absolute = format!("{}/space%20name/plan.grove-plan.json", root.display());
    store
        .import(
            "absolute",
            &absolute,
            root.to_str().unwrap(),
            &[root.into()],
        )
        .unwrap();
    for source in [
        "%",
        "%2",
        "%GG",
        "%FF",
        "%00",
        "%2e%2e",
        "nested%2F..%2F..",
        "https%3A%2F%2Fexample.org",
        "mailto:person",
        "file:plan",
        "%2F%2Fserver",
        "missing",
    ] {
        let target = format!("{source}/plan.grove-plan.json");
        assert!(
            store
                .import(source, &target, root.to_str().unwrap(), &[root.into()])
                .is_err(),
            "{target}"
        );
    }
    // Raw JSON asset paths must not be URL-decoded.
    fs_err::write(root.join("asset%20name.txt"), "literal").unwrap();
    fs_err::write(root.join("asset name.txt"), "decoded").unwrap();
    assert_eq!(
        fs_err::read_to_string(safe_path(root, "asset%20name.txt", &[root.into()]).unwrap())
            .unwrap(),
        "literal"
    );
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs_err::write(
            outside.path().join("plan.grove-plan.json"),
            serde_json::to_vec(&document()).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("escape here")).unwrap();
        assert!(store
            .import(
                "escape",
                "escape%20here/plan.grove-plan.json",
                root.to_str().unwrap(),
                &[root.into()]
            )
            .is_err());
    }
}

#[test]
fn persisted_encoded_error_retries_without_changing_source_identity() {
    let (dir, _) = fixture();
    let nested = dir.path().join("Projects Reworked");
    fs_err::create_dir(&nested).unwrap();
    let mut doc = document();
    let mut decision = doc.claims[0].decisions[0].clone();
    decision.id = "second".into();
    doc.claims[0].decisions.push(decision);
    fs_err::write(
        nested.join("plan.grove-plan.json"),
        serde_json::to_vec(&doc).unwrap(),
    )
    .unwrap();
    let source = format!(
        "{}/Projects%20Reworked/plan.grove-plan.json",
        dir.path().display()
    );
    for provider in ["codex", "claude"] {
        let mut saved: SavedSession = serde_json::from_value(serde_json::json!({
            "key":"encoded-retry", "provider":provider, "project":"Test", "cwd":dir.path(), "label":"Test", "roots":[], "thread_id":null,
            "items":[{"id":"answer","type":"agentMessage","status":"completed","text":format!("[Plan](<{source}>)"),"grovePlanImported":true}],
            "rich_plans":{"errors":[{"message_id":"answer","source":source,"error":"No such file or directory (os error 2)"}]}
        })).unwrap();
        import_completed(&mut saved);
        assert!(saved.rich_plans.revisions.is_empty());
        retry_import(&mut saved, "answer", &source).unwrap();
        assert!(saved.rich_plans.errors.is_empty());
        assert_eq!(saved.rich_plans.revisions[0].source, source);
        assert_eq!(
            saved.rich_plans.revisions[0].document.all_decisions().len(),
            2
        );
        retry_import(&mut saved, "answer", &source).unwrap_err();
        let mut restored: SavedSession =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        fs_err::remove_file(nested.join("plan.grove-plan.json")).unwrap();
        import_completed(&mut restored);
        assert_eq!(restored.rich_plans.revisions.len(), 1);
        assert_eq!(restored.rich_plans.revisions[0].document, doc);
        fs_err::write(
            nested.join("plan.grove-plan.json"),
            serde_json::to_vec(&doc).unwrap(),
        )
        .unwrap();
    }
    let relative = "Projects%20Reworked/plan.grove-plan.json";
    for text in [
        format!("[Plan](<{relative}>)"),
        format!("[Plan][p]\n\n[p]: {relative}"),
    ] {
        assert_eq!(markdown_links(&text), [relative]);
    }
}

#[test]
fn imports_only_real_markdown_links_with_full_fence_and_path_rules() {
    let (dir, mut store) = fixture();
    let nested = dir.path().join("checkout (copy)");
    fs_err::create_dir(&nested).unwrap();
    fs_err::write(
        nested.join("plan.grove-plan.json"),
        serde_json::to_vec(&document()).unwrap(),
    )
    .unwrap();
    let text="````text\n```\n[Fake](missing.grove-plan.json)\n````\n\n`[Inline code](missing-too.grove-plan.json)`\n\n[Review](<checkout (copy)/plan.grove-plan.json>)";
    let links = markdown_links(text);
    assert_eq!(links, ["checkout (copy)/plan.grove-plan.json"]);
    store
        .import(
            "real",
            &links[0],
            dir.path().to_str().unwrap(),
            &[dir.path().to_owned()],
        )
        .unwrap();
    assert_eq!(store.revisions.len(), 1);
    assert!(store.errors.is_empty());
    assert_eq!(
        markdown_links("[Review][plan]\n\n[plan]: <checkout (copy)/plan.grove-plan.json>"),
        links
    );
    let mut many = String::new();
    for index in 0..30 {
        writeln!(many, "[Plan{index}](p{index}.grove-plan.json)").unwrap();
    }
    let bounded = markdown_links(&many);
    assert_eq!(bounded.len(), 20);
    assert_eq!(bounded[19], "p19.grove-plan.json");
}

#[test]
fn snapshots_and_reviews_survive_source_removal_and_revision_conflicts() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    let mut review = ReviewState::default();
    review.comments.insert("claim".into(), "Keep this".into());
    store.update_review("plan", "r1", review).unwrap();
    let mut conflicting = document();
    conflicting.title = "Changed same identity".into();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&conflicting).unwrap(),
    )
    .unwrap();
    assert!(import(&mut store, dir.path()).is_err());
    assert_eq!(store.revisions.len(), 1);
    assert_eq!(store.revisions[0].document.title, "Review me");
    assert_eq!(store.errors.len(), 1);
    conflicting.revision_id = "r2".into();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&conflicting).unwrap(),
    )
    .unwrap();
    import(&mut store, dir.path()).unwrap();
    fs_err::remove_file(dir.path().join("plan.grove-plan.json")).unwrap();
    let restored: PlanStore = serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    assert_eq!(restored.revisions[0].review.comments["claim"], "Keep this");
    assert!(restored.revisions[1].review.comments.is_empty());
    assert_eq!(
        restored.revisions[1].document.title,
        "Changed same identity"
    );
}
#[test]
fn delivery_journal_suppresses_duplicate_click_restart_and_confirmed_retry() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    let mut review = ReviewState::default();
    review.answers.insert("choice".into(), "inline".into());
    store.update_review("plan", "r1", review).unwrap();
    let (id, text) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    assert!(text.contains("Where?"));
    assert!(text.contains("Inline card"));
    assert!(store.prepare_delivery("plan", "r1").unwrap().is_none());
    let mut spoof = ReviewState::default();
    spoof
        .comments
        .insert("claim".into(), "changed after send".into());
    store.update_review("plan", "r1", spoof).unwrap();
    assert_eq!(store.revisions[0].response_text(), text);
    let mut restart: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    restart.recover_delivery();
    assert!(
        matches!(restart.revisions[0].review.delivery,ResponseDelivery::Uncertain{id:held} if held==id)
    );
    assert!(restart.prepare_delivery("plan", "r1").unwrap().is_none());
    restart.confirm_delivery(id);
    assert!(restart.prepare_delivery("plan", "r1").unwrap().is_none());
    store.fail_delivery(id, "definitive rejection before side effect");
    assert_eq!(store.prepare_delivery("plan", "r1").unwrap().unwrap().0, id);
}
#[test]
fn own_answers_unread_and_viewed_remain_distinct() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    let r = &mut store.revisions[0];
    assert!(r.response_text().contains("Unread [choice]"));
    r.review.viewed_decisions.push("choice".into());
    assert!(r.response_text().contains("Viewed, unanswered"));
    r.review
        .own_answers
        .insert("choice".into(), "inline".into());
    let text = r.response_text();
    assert!(text.contains("Own answer [choice] Where?:\n> inline"));
    assert!(!text.contains("Inline card"));
    r.review.answers.insert("choice".into(), "inline".into());
    let override_text = r.response_text();
    assert!(override_text.contains("Own answer [choice]"));
    assert!(!override_text.contains("\nAnswer [choice]"));
    r.review.own_answers.clear();
    let restored = r.response_text();
    assert!(restored.contains("Answer [choice] Where?:\n> Inline card"));
    assert!(!restored.contains("Own answer [choice]"));
}
#[test]
fn rejects_traversal_symlink_escape_invalid_version_and_duplicate_ids() {
    let (dir, mut store) = fixture();
    assert!(store
        .import(
            "m",
            "../plan.grove-plan.json",
            dir.path().to_str().unwrap(),
            &[dir.path().to_owned()]
        )
        .is_err());
    let mut doc = document();
    doc.schema_version = 2;
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&doc).unwrap(),
    )
    .unwrap();
    assert!(import(&mut store, dir.path()).is_err());
    doc.schema_version = 1;
    doc.claims.push(doc.claims[0].clone());
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&doc).unwrap(),
    )
    .unwrap();
    assert!(import(&mut store, dir.path()).is_err());
    assert!(store.revisions.is_empty());
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs_err::write(
            outside.path().join("outside.grove-plan.json"),
            serde_json::to_vec(&document()).unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.grove-plan.json"),
            dir.path().join("escape.grove-plan.json"),
        )
        .unwrap();
        assert!(store
            .import(
                "m",
                "escape.grove-plan.json",
                dir.path().to_str().unwrap(),
                &[dir.path().to_owned()]
            )
            .is_err());
    }
}
#[test]
fn malformed_images_never_become_snapshots_and_html_is_immutable() {
    let (dir, mut store) = fixture();
    let mut doc = document();
    doc.claims[0].exhibit = Some(Exhibit::Image {
        path: "bad.png".into(),
        alt: "bad".into(),
    });
    fs_err::write(dir.path().join("bad.png"), b"\x89PNG\r\n\x1a\n").unwrap();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&doc).unwrap(),
    )
    .unwrap();
    assert!(import(&mut store, dir.path()).is_err());
    assert!(store.revisions.is_empty());
    doc.claims[0].exhibit = Some(Exhibit::HtmlLink {
        path: "exhibit.html".into(),
        label: "Open".into(),
    });
    fs_err::write(dir.path().join("exhibit.html"), b"<p>saved</p>").unwrap();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&doc).unwrap(),
    )
    .unwrap();
    import(&mut store, dir.path()).unwrap();
    fs_err::write(dir.path().join("exhibit.html"), b"<p>changed</p>").unwrap();
    assert!(import(&mut store, dir.path()).is_err());
    assert_eq!(
        store.revisions[0].assets[0].bytes.as_slice(),
        b"<p>saved</p>"
    );
}
#[test]
fn imported_links_only_come_from_completed_assistant_messages() {
    let (dir, _) = fixture();
    let mut saved:SavedSession=serde_json::from_value(serde_json::json!({"key":"test","cwd":dir.path(),"project":"test","label":"test","roots":[],"thread_id":null,"items":[{"id":"user","type":"userMessage","text":"[plan](plan.grove-plan.json)"},{"id":"stream","type":"agentMessage","status":"inProgress","text":"[plan](plan.grove-plan.json)"}]})).unwrap();
    import_completed(&mut saved);
    assert!(saved.rich_plans.revisions.is_empty());
    saved.items[1]["status"] = serde_json::json!("completed");
    import_completed(&mut saved);
    assert_eq!(saved.rich_plans.revisions.len(), 1);
    assert_eq!(saved.rich_plans.revisions[0].message_id, "stream");
    import_completed(&mut saved);
    assert_eq!(saved.rich_plans.revisions.len(), 1);
}
#[test]
fn canceled_queue_feedback_can_be_reviewed_and_sent_again_without_old_payload() {
    let (dir, mut store) = fixture();
    import(&mut store, dir.path()).unwrap();
    store.revisions[0]
        .review
        .answers
        .insert("choice".into(), "inline".into());
    let (id, _) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    assert!(store.is_delivery(id));
    store.cancel_delivery(id);
    assert!(!store.is_delivery(id));
    let mut review = ReviewState::default();
    review.answers.insert("choice".into(), "inline".into());
    review
        .comments
        .insert("claim".into(), "Changed after cancellation".into());
    store.update_review("plan", "r1", review).unwrap();
    let (new_id, text) = store.prepare_delivery("plan", "r1").unwrap().unwrap();
    assert_eq!(id, new_id);
    assert!(text.contains("Changed after cancellation"));
}
#[test]
fn repeated_revision_links_are_not_reopened_and_fenced_links_are_not_imported() {
    let (dir, _) = fixture();
    let mut saved:SavedSession=serde_json::from_value(serde_json::json!({"key":"test","cwd":dir.path(),"project":"test","label":"test","roots":[],"thread_id":null,"items":[{"id":"m1","type":"agentMessage","text":"[plan](plan.grove-plan.json)"},{"id":"fenced","type":"agentMessage","text":"```json\n[plan](missing.grove-plan.json)\n```"}]})).unwrap();
    import_completed(&mut saved);
    assert_eq!(saved.rich_plans.revisions.len(), 1);
    assert!(saved.rich_plans.errors.is_empty());
    saved.items.push(
        serde_json::json!({"id":"m2","type":"agentMessage","text":"[same](plan.grove-plan.json)"}),
    );
    import_completed(&mut saved);
    fs_err::remove_file(dir.path().join("plan.grove-plan.json")).unwrap();
    import_completed(&mut saved);
    assert!(saved.rich_plans.errors.is_empty());
    assert_eq!(saved.rich_plans.revisions.len(), 1);
    saved.items.retain(|i| i["id"] != "m1");
    saved.rich_plans.retain_messages(&saved.items);
    assert_eq!(saved.rich_plans.revisions.len(), 1);
    assert_eq!(saved.rich_plans.references.len(), 1);
    assert_eq!(saved.rich_plans.references[0].message_id, "m2");
}
#[test]
fn repeated_old_revision_links_resolve_exact_snapshot_after_source_deletion_and_restart() {
    let (dir, mut store) = fixture();
    let root = dir.path().to_str().unwrap();
    let roots = [dir.path().to_owned()];
    store
        .import("m1", "plan.grove-plan.json", root, &roots)
        .unwrap();
    let mut r2 = document();
    r2.revision_id = "r2".into();
    r2.title = "New revision".into();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&r2).unwrap(),
    )
    .unwrap();
    store
        .import("m2", "plan.grove-plan.json", root, &roots)
        .unwrap();
    fs_err::write(
        dir.path().join("plan.grove-plan.json"),
        serde_json::to_vec(&document()).unwrap(),
    )
    .unwrap();
    store
        .import("m3", "plan.grove-plan.json", root, &roots)
        .unwrap();
    fs_err::remove_file(dir.path().join("plan.grove-plan.json")).unwrap();
    let mut restored: PlanStore =
        serde_json::from_slice(&serde_json::to_vec(&store).unwrap()).unwrap();
    restored.recover_delivery();
    assert_eq!(restored.revisions.len(), 2);
    assert_eq!(restored.references.len(), 3);
    for (message, revision, title) in [
        ("m1", "r1", "Review me"),
        ("m2", "r2", "New revision"),
        ("m3", "r1", "Review me"),
    ] {
        let reference = restored.reference(message, "plan.grove-plan.json").unwrap();
        assert_eq!(reference.revision_id, revision);
        assert_eq!(
            restored
                .revision(&reference.plan_id, &reference.revision_id)
                .unwrap()
                .document
                .title,
            title
        );
    }
    restored.retain_messages(&[serde_json::json!({"id":"m3"})]);
    assert_eq!(restored.references.len(), 1);
    assert_eq!(restored.revisions.len(), 1);
    assert_eq!(restored.revisions[0].document.revision_id, "r1");
    restored.recover_delivery();
    assert_eq!(restored.references.len(), 1);
}

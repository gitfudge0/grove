//! Prepared response boundaries and transcript change identity.

use super::*;

/// Process-local change identity. TranscriptItems invalidates its revision before
/// every mutable borrow, including equal-length edits to opaque image payloads.
/// No provider payload is traversed to decide whether bottom-follow should run.
pub(super) fn transcript_content_signature(
    items: &codex::TranscriptItems,
    thread: Option<&str>,
) -> [u64; 2] {
    let mut hasher = rustc_hash::FxHasher::default();
    thread.hash(&mut hasher);
    [items.revision(), hasher.finish()]
}

/// Value's Hash implementation canonicalizes objects using an allocated sorted
/// vector when serde_json preserves insertion order. Render comparison only needs
/// the same ordered structure as JSON serialization, so visit entries directly.
pub(super) fn hash_transcript_value(value: &serde_json::Value, hasher: &mut impl Hasher) {
    std::mem::discriminant(value).hash(hasher);
    match value {
        serde_json::Value::Null => {}
        serde_json::Value::Bool(value) => value.hash(hasher),
        serde_json::Value::Number(value) => value.hash(hasher),
        serde_json::Value::String(value) => value.hash(hasher),
        serde_json::Value::Array(values) => {
            values.len().hash(hasher);
            for value in values {
                hash_transcript_value(value, hasher);
            }
        }
        serde_json::Value::Object(values) => {
            values.len().hash(hasher);
            for (key, value) in values {
                key.hash(hasher);
                hash_transcript_value(value, hasher);
            }
        }
    }
}

/// Presentation-only response boundaries; provider work periods never split a response.
#[derive(Clone, Debug)]
pub(super) struct ResponseGroup {
    pub(super) key: String,
    pub(super) end: usize,
    pub(super) prose: Vec<usize>,
    pub(super) source_keys: Vec<String>,
    pub(super) copy_text: std::cell::OnceCell<String>,
    pub(super) copy_sources: Vec<gpui::SharedString>,
    pub(super) anchor_index: Option<usize>,
    pub(super) received_at: Option<u64>,
}

impl ResponseGroup {
    pub(super) fn copy_text(&self) -> String {
        self.copy_text
            .get_or_init(|| {
                self.copy_sources
                    .iter()
                    .map(std::convert::AsRef::as_ref)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .clone()
    }
}

#[cfg(test)]
pub(super) fn response_groups<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
    thread_id: Option<&str>,
) -> Vec<ResponseGroup> {
    response_groups_from(items, thread_id, 0, None)
}

pub(super) fn response_groups_from<T: std::borrow::Borrow<serde_json::Value>>(
    items: &[T],
    thread_id: Option<&str>,
    restart: usize,
    prepared: Option<&[Rc<PreparedItem>]>,
) -> Vec<ResponseGroup> {
    let mut groups = Vec::new();
    let mut start = restart;
    // Compute the last usable fork anchor once, including idless trailing items.
    let mut anchors = Vec::with_capacity(items.len() + 1);
    let mut anchor = None;
    anchors.push(anchor);
    for (index, item) in items.iter().enumerate() {
        if item.borrow()["id"].is_string() {
            anchor = Some(index);
        }
        anchors.push(anchor);
    }
    for end in (restart..=items.len())
        .filter(|index| *index == items.len() || items[*index].borrow()["type"] == "userMessage")
    {
        let prose = (start..end)
            .filter(|index| {
                items[*index].borrow()["type"] == "agentMessage"
                    && !prepared_item_text(items, prepared, *index)
                        .trim()
                        .is_empty()
            })
            .collect::<Vec<_>>();
        if let Some(last) = prose.last() {
            let first = (start..end)
                .find(|index| items[*index].borrow()["type"] == "agentMessage")
                .unwrap_or(*last);
            let source_id = |index: usize| {
                items[index].borrow()["id"]
                    .as_str()
                    .map_or_else(|| index.to_string(), str::to_owned)
            };
            let key = format!(
                "{}:response:{}",
                thread_id.unwrap_or_default(),
                source_id(first)
            );
            groups.push(ResponseGroup {
                anchor_index: anchors[end],
                source_keys: prose
                    .iter()
                    .map(|index| format!("{key}:source:{}", source_id(*index)))
                    .collect(),
                copy_text: std::cell::OnceCell::default(),
                copy_sources: prose
                    .iter()
                    .map(|index| {
                        prepared.map_or_else(
                            || transcript_item_text(items[*index].borrow()).into(),
                            |prepared| prepared[*index].text.clone(),
                        )
                    })
                    .collect(),
                received_at: items[*last].borrow()["groveReceivedAt"].as_u64(),
                key,
                end,
                prose,
            });
        }
        start = end + 1;
    }
    groups
}

/// Explicit message phases win; older providers keep their last response outside work.
#[cfg(test)]
#[cfg(test)]
mod transcript_signature_tests {
    use super::super::transcript_content_signature;
    use serde_json::json;

    #[test]
    fn content_signature_tracks_equal_length_edits_optimistic_items_images_and_threads() {
        let mut items: grove_core::codex::TranscriptItems =
            vec![json!({"id":"answer", "type":"agentMessage", "text":"one"})].into();
        let original = transcript_content_signature(&items, Some("thread-a"));
        assert_eq!(
            original,
            transcript_content_signature(&items, Some("thread-a"))
        );
        items[0]["text"] = json!("two");
        let replaced = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(original, replaced);
        items.push(json!({"id":"optimistic", "type":"userMessage", "text":"next"}));
        let optimistic = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(replaced, optimistic);
        items[1]["images"] = json!(["data:image/png;base64,AAAA"]);
        let image = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(optimistic, image);
        items[1]["images"] = json!(["data:image/png;base64,BBBB"]);
        let edited_image = transcript_content_signature(&items, Some("thread-a"));
        assert_ne!(image, edited_image);
        assert_ne!(
            edited_image,
            transcript_content_signature(&items, Some("thread-b"))
        );
    }
}

//! Locally submitted messages and their reconciliation with provider items.

use super::{codex, Cell, CodexView, HashSet, Rc};
use crate::views::tokens::MOTION_SLOW_MS;
use gpui::Context;

/// Pending inputs keep their submission position while provider events stream.
/// This is only a render projection; saved items and their work ranges stay intact.
pub(super) fn project_pending_bubbles(
    state: &mut codex::Snapshot,
    optimistic: &[serde_json::Value],
) {
    let mut positions = optimistic
        .iter()
        .enumerate()
        .map(|(order, item)| {
            let position = match item.get("localAfterItem") {
                Some(anchor) => anchor
                    .as_str()
                    .and_then(|id| {
                        state
                            .saved
                            .items
                            .iter()
                            .position(|old| old["id"] == id)
                            .map(|at| at + 1)
                    })
                    .unwrap_or(0), // Compaction may remove the anchor; never move input below its reply.
                None => state.saved.items.len(),
            };
            (position, order, item)
        })
        .collect::<Vec<_>>();
    // Reverse insertion keeps sends sharing an anchor in submission order.
    positions.sort_by_key(|(position, order, _)| (*position, *order));
    for (position, _, item) in positions.into_iter().rev() {
        state.saved.items.insert(position, item.clone().into());
        for period in &mut state.saved.work_periods {
            if period.start_item > position {
                period.start_item += 1;
            }
            if let Some(end) = &mut period.end_item {
                if *end > position {
                    *end += 1;
                }
            }
        }
    }
}

/// A local send can be acknowledged before its transcript item arrives.
pub(super) struct BubbleEntrance {
    pub(super) submission: u64,
    pub(super) existing: HashSet<String>,
    pub(super) thread: Option<String>,
    pub(super) text: String,
    pub(super) optimistic: Option<serde_json::Value>,
}

impl CodexView {
    pub(super) fn sync_bubble_entrances(
        &mut self,
        state: &codex::Snapshot,
        reduced_motion: bool,
        now: std::time::Instant,
    ) {
        self.animating_bubbles.retain(|_, (started, finished)| {
            !finished.get()
                && !reduced_motion
                && now.duration_since(*started) < std::time::Duration::from_millis(MOTION_SLOW_MS)
        });
        let reserved: HashSet<_> = self
            .bubble_entrances
            .iter()
            .filter_map(|pending| state.submission_item_id(pending.submission))
            .map(str::to_owned)
            .collect();
        let mut consumed = HashSet::new();
        self.bubble_entrances.retain_mut(|pending| {
            if pending
                .thread
                .as_ref()
                .is_some_and(|thread| state.saved.thread_id.as_ref() != Some(thread))
                || state.rejected_submission == Some(pending.submission)
                || state.status == codex::Status::Disconnected
            {
                return false;
            }
            let steering = pending
                .optimistic
                .as_ref()
                .is_some_and(|item| item["localSteering"] == true);
            let queued_steering = pending
                .optimistic
                .as_ref()
                .is_some_and(|item| item["localQueuedSteering"] == true);
            let steering_pending = steering
                && (state
                    .queued_prompts
                    .iter()
                    .any(|prompt| prompt.id == pending.submission)
                    || (!queued_steering && !state.submission_accepted(pending.submission)));
            if let Some(optimistic) = &mut pending.optimistic {
                optimistic["localSteering"] = steering_pending.into();
            }
            pending.existing.extend(consumed.iter().cloned());
            let owned_id = state.submission_item_id(pending.submission);
            let id = state.saved.items.iter().find_map(|item| {
                let id = item["id"].as_str()?;
                (item["type"] == "userMessage"
                    && !consumed.contains(id)
                    && (owned_id.map_or_else(
                        || {
                            !pending.existing.contains(id)
                                && !reserved.contains(id)
                                && codex::item_text(item) == pending.text
                                && pending.optimistic.as_ref().is_none_or(|optimistic| {
                                    let images = |message: &serde_json::Value| {
                                        message["content"]
                                            .as_array()
                                            .into_iter()
                                            .flatten()
                                            .filter_map(|part| {
                                                part["url"]
                                                    .as_str()
                                                    .or_else(|| part["image_url"].as_str())
                                            })
                                            .map(str::to_owned)
                                            .collect::<Vec<_>>()
                                    };
                                    images(item) == images(optimistic)
                                })
                        },
                        |owned_id| owned_id == id,
                    )))
                .then_some((id, item))
            });
            if let Some((id, item)) = id {
                consumed.insert(id.to_owned());
                if let Some(optimistic) = &pending.optimistic {
                    if let Some(animation) = self
                        .animating_bubbles
                        .remove(optimistic["id"].as_str().unwrap_or_default())
                    {
                        self.animating_bubbles.insert(id.to_owned(), animation);
                    }
                } else if !reduced_motion {
                    self.animating_bubbles
                        .insert(id.to_owned(), (now, Rc::new(Cell::new(false))));
                }
                if steering_pending {
                    let mut echo = item.clone();
                    echo["localSteering"] = true.into();
                    echo["localQueuedSteering"] = queued_steering.into();
                    pending.optimistic = Some(echo.into_value());
                    true
                } else {
                    false
                }
            } else {
                true
            }
        });
    }
    pub(super) fn record_submission_bubble(
        &mut self,
        submission: u64,
        text: String,
        images: Vec<String>,
        steering: bool,
        queued_steering: bool,
        cx: &mut Context<Self>,
    ) {
        let id = format!("local-submission-{submission}");
        if !cx.reduce_motion() {
            self.animating_bubbles.insert(
                id.clone(),
                (cx.background_executor().now(), Rc::new(Cell::new(false))),
            );
        }
        let mut content = vec![serde_json::json!({"type":"text", "text":text})];
        content.extend(
            images
                .iter()
                .map(|url| serde_json::json!({"type":"image", "url":url})),
        );
        let state = &self.session.read(cx).state;
        self.bubble_entrances.push(BubbleEntrance {
            submission,
            existing: state.saved.items.iter().filter_map(|item| item["id"].as_str().map(str::to_owned)).collect(),
            thread: state.saved.thread_id.clone(),
            text,
            optimistic: Some(serde_json::json!({"id":id, "type":"userMessage", "content":content, "localSteering":steering, "localQueuedSteering":queued_steering, "localAfterItem":state.saved.items.last().and_then(|item| item["id"].as_str()), "groveReceivedAt":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64})),
        });
        self.follow_after_submit = true;
    }
}

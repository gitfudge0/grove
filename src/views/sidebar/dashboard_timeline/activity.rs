//! Projects session events into bounded Dashboard activity summaries.

use grove_core::{
    codex::Status,
    tasks::{Task, TaskList},
};
use serde_json::Value;
use std::sync::Arc;

pub(super) const ACTIVITY_HISTORY_LIMIT: usize = 64;
pub(super) const ACTIVITY_TEXT_LIMIT: usize = 120;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::views::sidebar) struct Activity {
    pub index: usize,
    pub text: String,
    pub observed_active: Option<bool>,
    pub task: Option<Task>,
    pub explicit_status: Option<grove_core::activity::StepStatus>,
    pub turn_id: Option<String>,
}
#[derive(Clone, Default, Debug, PartialEq)]
pub(in crate::views::sidebar) struct TimelineData {
    pub list: Option<Arc<TaskList>>,
    pub activity: Vec<Activity>,
    pub task_scope: Arc<grove_core::tasks::TaskScope>,
    pub current_turn: Option<String>,
}

impl TimelineData {
    pub(super) fn current_action(&self, status: Status) -> Option<&str> {
        let index = shimmering_activity(&self.activity, status)?;
        let activity = self
            .activity
            .iter()
            .find(|activity| activity.index == index)?;
        (activity.explicit_status == Some(grove_core::activity::StepStatus::InProgress)
            && self.current_turn.is_some()
            && activity.turn_id == self.current_turn)
            .then_some(activity.text.as_str())
    }
}
/// A short excerpt of the provider's own progress prose, never a command or reasoning.
pub(super) fn activity_excerpt(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if line.starts_with("```") {
        return None;
    }
    let line = line.trim_start_matches(['#', '*', '-', ' ']);
    let sentence_end = line.char_indices().find_map(|(index, ch)| {
        (matches!(ch, '.' | '!' | '?')
            && line[index + ch.len_utf8()..].starts_with(char::is_whitespace))
        .then_some(index + ch.len_utf8())
    });
    let sentence = &line[..sentence_end.unwrap_or(line.len())];
    let mut excerpt = sentence
        .chars()
        .take(ACTIVITY_TEXT_LIMIT)
        .collect::<String>();
    if sentence.chars().count() > ACTIVITY_TEXT_LIMIT {
        if let Some(space) = excerpt.rfind(' ') {
            excerpt.truncate(space);
        }
        excerpt.push('…');
    }
    (!excerpt.is_empty()).then_some(excerpt)
}
/// History is bounded by summaries, so a long tool phase cannot erase its current step.
pub(in crate::views::sidebar) fn recent_activity<T: std::borrow::Borrow<Value>>(
    items: &[T],
) -> Vec<Activity> {
    let explicit = grove_core::activity::steps(items);
    if !explicit.is_empty()
        || items
            .iter()
            .any(|item| grove_core::activity::is_internal(item.borrow()))
    {
        return explicit
            .into_iter()
            .map(|step| Activity {
                index: step.index,
                text: step.label,
                observed_active: (step.status != grove_core::activity::StepStatus::InProgress
                    || !step.current_turn)
                    .then_some(false),
                task: step.task,
                explicit_status: Some(step.status),
                turn_id: step.turn_id,
            })
            .collect();
    }
    let latest_prompt = items
        .iter()
        .rposition(|item| item.borrow()["type"] == "userMessage");
    items
        .iter()
        .enumerate()
        .rev()
        .filter_map(|(index, item)| {
            let item = item.borrow();
            if item["type"] != "agentMessage" {
                return None;
            }
            let text = activity_excerpt(item["text"].as_str()?)?;
            Some(Activity {
                index,
                text,
                explicit_status: None,
                turn_id: None,
                task: grove_core::tasks::activity_owner(items, index),
                // Finishing a text stream does not finish the work described by it.
                // A final answer or a later user prompt does end that activity.
                observed_active: (item["phase"] == "final_answer"
                    || latest_prompt.is_some_and(|prompt| index < prompt))
                .then_some(false),
            })
        })
        .take(ACTIVITY_HISTORY_LIMIT)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

pub(super) fn shimmering_activity(items: &[Activity], status: Status) -> Option<usize> {
    let latest = if items.iter().any(|item| item.explicit_status.is_some()) {
        items.iter().rev().find(|item| {
            item.explicit_status == Some(grove_core::activity::StepStatus::InProgress)
                && item.observed_active != Some(false)
        })?
    } else {
        items.last()?
    };
    (matches!(status, Status::Starting | Status::Working) && latest.observed_active != Some(false))
        .then_some(latest.index)
}

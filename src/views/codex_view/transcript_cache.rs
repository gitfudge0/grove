//! Snapshot and item projections reused across transcript frames.

use super::*;

/// Immutable preparation shared by scroll/animation frames. Session notifications
/// replace the source snapshot; local mutations fork it synchronously.
/// Optimistic bubbles are a separate cache input.
pub(super) struct TranscriptRenderCache {
    pub(super) rich_messages: Rc<RichMessageIndex>,
    pub(super) visual_items: HashSet<String>,
    pub(super) work: Rc<WorkIndex>,
    pub(super) review_display: Rc<std::cell::RefCell<HashMap<usize, Option<String>>>>,
    pub(super) review_overview: Rc<std::cell::OnceCell<Option<String>>>,
    pub(super) source: Rc<codex::Snapshot>,
    pub(super) optimistic: Vec<serde_json::Value>,
    pub(super) state: Rc<codex::Snapshot>,
    pub(super) prepared: Rc<PreparedTranscript>,
}

/// Rich attachments can change without changing transcript item identity. Keep
/// their lookup beside the snapshot cache, not in item-only preparation.
#[derive(Default)]
pub(super) struct RichMessageIndex {
    pub(super) plans: HashMap<String, Vec<usize>>,
    pub(super) errors: HashMap<String, Vec<usize>>,
}
impl RichMessageIndex {
    pub(super) fn new(store: &grove_core::rich_plan::PlanStore) -> Self {
        let mut result = Self::default();
        for (index, plan) in store.revisions.iter().enumerate() {
            result
                .plans
                .entry(plan.message_id.clone())
                .or_default()
                .push(index);
        }
        for (index, error) in store.errors.iter().enumerate() {
            result
                .errors
                .entry(error.message_id.clone())
                .or_default()
                .push(index);
        }
        result
    }
    pub(super) fn contains(&self, message: &str) -> bool {
        self.plans.contains_key(message) || self.errors.contains_key(message)
    }
}

/// Derived solely from transcript items and thread identity; metadata-only
/// provider updates can share this without keeping their old snapshot visible.
pub(super) struct PreparedTranscript {
    pub(super) content: [u64; 2],
    pub(super) id_index: rustc_hash::FxHashMap<String, usize>,
    pub(super) item_data: Vec<Rc<PreparedItem>>,
    pub(super) commentary: Vec<bool>,
    activity_prefix: Vec<usize>,
    pub(super) task_key: u64,
    pub(super) has_timestamp: bool,
    pub(super) response_groups: Vec<Rc<ResponseGroup>>,
    pub(super) response_ends: Vec<Option<usize>>,
    pub(super) response_prose: Vec<Option<usize>>,
    pub(super) minimap_entries: Vec<MessageMinimapEntry>,
    nearest_user: Vec<usize>,
    work_keys: rustc_hash::FxHashMap<usize, String>,
    next_nonuser: Vec<usize>,
}

/// Item-local payloads survive tail appends and metadata-only snapshots.
pub(super) struct PreparedItem {
    pub(super) source: codex::TranscriptItem,
    pub(super) text: gpui::SharedString,
    pub(super) tool: bool,
    pub(super) detail: std::cell::OnceCell<gpui::SharedString>,
    pub(super) detail_markdown: std::cell::OnceCell<gpui::SharedString>,
    pub(super) snippet_source_digest: std::cell::OnceCell<u64>,
    pub(super) label_width:
        std::cell::RefCell<Option<(String, gpui::TextStyle, gpui::Pixels, gpui::Pixels)>>,
}
impl PreparedItem {
    pub(super) fn new(source: codex::TranscriptItem) -> Self {
        Self {
            text: transcript_item_text(&source).into(),
            tool: is_tool_item(&source),
            source,
            detail: std::cell::OnceCell::default(),
            detail_markdown: std::cell::OnceCell::default(),
            snippet_source_digest: std::cell::OnceCell::default(),
            label_width: std::cell::RefCell::default(),
        }
    }
    pub(super) fn snippet_source_digest(&self, source: &str) -> u64 {
        if std::ptr::eq(source, self.text.as_ref()) || source == self.text.as_ref() {
            *self
                .snippet_source_digest
                .get_or_init(|| snippets::source_digest(&self.text))
        } else {
            // Report scopes can transform the same provider item for display.
            // Their actual source must retain its own command identity.
            snippets::source_digest(source)
        }
    }
    pub(super) fn detail(&self) -> gpui::SharedString {
        self.detail
            .get_or_init(|| tool_detail(&self.source).into())
            .clone()
    }
    pub(super) fn detail_markdown(&self) -> gpui::SharedString {
        self.detail_markdown
            .get_or_init(|| {
                format!("```text\n{}\n```", self.detail().replace("```", "` ` `")).into()
            })
            .clone()
    }
}

/// Snapshot metadata is independent of item identity (work may finish without
/// changing an item). Interval priority follows the original reverse-vector lookup,
/// including overlapping/out-of-order persisted periods.
pub(super) struct WorkIndex {
    pub(super) period: Vec<Option<usize>>,
    pub(super) active: Vec<bool>,
    pub(super) active_start: usize,
    latest_tool: Option<usize>,
    pub(super) parent_activity: Vec<bool>,
}
impl WorkIndex {
    pub(super) fn new(state: &codex::Snapshot, prepared: &PreparedTranscript) -> Self {
        let n = state.saved.items.len();
        let active_start = active_work_start(state);
        let latest_tool = state
            .saved
            .items
            .iter()
            .enumerate()
            .rev()
            .find(|(index, item)| *index >= active_start && is_tool_item(item))
            .map(|(index, _)| index);
        let mut starts: Vec<_> = state
            .saved
            .work_periods
            .iter()
            .enumerate()
            .map(|(index, period)| (period.start_item, index))
            .collect();
        starts.sort_unstable();
        let mut pending = starts.into_iter().peekable();
        let mut candidates = std::collections::BinaryHeap::new();
        let mut latest: Option<usize> = None;
        let mut period = Vec::with_capacity(n + 1);
        let mut active = Vec::with_capacity(n + 1);
        for index in 0..=n {
            while pending.peek().is_some_and(|(start, _)| *start <= index) {
                let Some((_, ordinal)) = pending.next() else {
                    break;
                };
                candidates.push(ordinal);
                latest = Some(latest.map_or(ordinal, |old| old.max(ordinal)));
            }
            while candidates.peek().is_some_and(|ordinal| {
                state.saved.work_periods[*ordinal]
                    .end_item
                    .is_some_and(|end| index >= end)
            }) {
                candidates.pop();
            }
            period.push(candidates.peek().copied());
            active.push(
                state.status.busy()
                    && index >= active_start
                    && latest
                        .is_none_or(|ordinal| state.saved.work_periods[ordinal].end_item.is_none()),
            );
        }
        debug_assert_eq!(prepared.nearest_user.len(), n + 1);
        let parent_activity = (0..=n)
            .map(|index| {
                let work = period[index].map(|ordinal| &state.saved.work_periods[ordinal]);
                // The legacy lookup excludes the current user when no period owns it.
                let start = work
                    .map_or_else(
                        || {
                            if index > 0
                                && state
                                    .saved
                                    .items
                                    .get(index)
                                    .is_some_and(|item| item["type"] == "userMessage")
                            {
                                prepared.nearest_user[index - 1]
                            } else {
                                prepared.nearest_user[index]
                            }
                        },
                        |work| work.start_item,
                    )
                    .min(n);
                let end = work.and_then(|work| work.end_item).unwrap_or(n).min(n);
                start < end && prepared.activity_prefix[end] > prepared.activity_prefix[start]
            })
            .collect();
        Self {
            period,
            active,
            active_start,
            latest_tool,
            parent_activity,
        }
    }
    pub(super) fn key(prepared: &PreparedTranscript, index: usize) -> &str {
        &prepared.work_keys[&prepared.nearest_user[index]]
    }
    pub(super) fn tool_visible(&self, state: &codex::Snapshot, index: usize) -> bool {
        !state.status.busy() || index < self.active_start || self.latest_tool == Some(index)
    }
    pub(super) fn header(
        &self,
        state: &codex::Snapshot,
        prepared: &PreparedTranscript,
        index: usize,
    ) -> Option<(String, String, bool)> {
        if state
            .saved
            .items
            .get(index)
            .is_some_and(|item| item["type"] == "userMessage")
        {
            return None;
        }
        let period = self.period[index].map(|ordinal| &state.saved.work_periods[ordinal]);
        let start = period.map_or(prepared.nearest_user[index], |period| period.start_item);
        let end = period
            .and_then(|period| period.end_item)
            .unwrap_or(state.saved.items.len());
        let first = prepared
            .next_nonuser
            .get(start)
            .copied()
            .filter(|first| *first < end)
            .or_else(|| self.active[index].then_some(state.saved.items.len()))?;
        if first != index {
            return None;
        }
        let duration = if self.active[index] {
            state.work_started.map(|start| start.elapsed().as_secs())
        } else {
            period.and_then(|period| period.duration_ms.map(|ms| ms / 1000))
        };
        Some((
            Self::key(prepared, index).to_owned(),
            work_duration_label(self.active[index], duration),
            self.active[index],
        ))
    }
}

impl std::ops::Deref for TranscriptRenderCache {
    type Target = PreparedTranscript;

    fn deref(&self) -> &Self::Target {
        &self.prepared
    }
}

impl TranscriptRenderCache {
    pub(super) fn matches(
        &self,
        source: &Rc<codex::Snapshot>,
        optimistic: &[serde_json::Value],
    ) -> bool {
        Rc::ptr_eq(&self.source, source) && self.optimistic == optimistic
    }

    #[cfg(test)]
    pub(super) fn new(source: Rc<codex::Snapshot>, optimistic: Vec<serde_json::Value>) -> Self {
        Self::refreshed(source, optimistic, None)
    }

    pub(super) fn refreshed(
        source: Rc<codex::Snapshot>,
        optimistic: Vec<serde_json::Value>,
        previous: Option<&Self>,
    ) -> Self {
        let prepared = previous
            .filter(|previous| {
                previous.source.saved.items.revision() == source.saved.items.revision()
                    && previous.source.saved.thread_id == source.saved.thread_id
                    && previous.optimistic == optimistic
            })
            .map(|previous| previous.prepared.clone());
        let mut state = source.clone();
        if !optimistic.is_empty() {
            project_pending_bubbles(Rc::make_mut(&mut state), &optimistic);
        }
        let prepared = prepared.unwrap_or_else(|| {
            let mut content = transcript_content_signature(
                &source.saved.items,
                source.saved.thread_id.as_deref(),
            );
            // Pending bubbles are usually tiny and are the only payloads that
            // need hashing; persisted transcript bytes are never revisited.
            if !optimistic.is_empty() {
                let mut hasher = rustc_hash::FxHasher::default();
                content[1].hash(&mut hasher);
                for item in &optimistic {
                    hash_transcript_value(item, &mut hasher);
                }
                content[1] = hasher.finish();
            }
            Rc::new(Self::prepare_reusing(
                &state,
                content,
                previous
                    .filter(|old| old.state.saved.thread_id == state.saved.thread_id)
                    .map(|old| old.prepared.as_ref()),
            ))
        });
        let work = previous
            .filter(|old| {
                Rc::ptr_eq(&old.prepared, &prepared)
                    && old.state.status.busy() == state.status.busy()
                    && old.state.saved.work_periods.len() == state.saved.work_periods.len()
                    && old
                        .state
                        .saved
                        .work_periods
                        .iter()
                        .zip(&state.saved.work_periods)
                        .all(|(a, b)| a.start_item == b.start_item && a.end_item == b.end_item)
            })
            .map_or_else(
                || Rc::new(WorkIndex::new(&state, &prepared)),
                |old| old.work.clone(),
            );
        let review_overview = previous
            .filter(|old| {
                let a = &old.state;
                let b = &state;
                a.saved.items.revision() == b.saved.items.revision()
                    && a.saved.cwd == b.saved.cwd
                    && a.saved.session_review.reviewed_through
                        == b.saved.session_review.reviewed_through
                    && a.saved.session_review.summary == b.saved.session_review.summary
                    && a.review_completion() == b.review_completion()
                    && a.saved
                        .session_review
                        .pending
                        .as_ref()
                        .and_then(|r| a.saved.work_periods.get(r.period))
                        .map(|p| (p.start_item, p.end_item))
                        == b.saved
                            .session_review
                            .pending
                            .as_ref()
                            .and_then(|r| b.saved.work_periods.get(r.period))
                            .map(|p| (p.start_item, p.end_item))
                    && a.saved
                        .session_review
                        .pending
                        .as_ref()
                        .map(|r| (r.id, r.period))
                        == b.saved
                            .session_review
                            .pending
                            .as_ref()
                            .map(|r| (r.id, r.period))
            })
            .map_or_else(Rc::default, |old| old.review_overview.clone());
        Self {
            review_overview,
            work,
            rich_messages: Rc::new(RichMessageIndex::new(&source.saved.rich_plans)),
            visual_items: source
                .saved
                .visuals
                .iter()
                .map(|visual| visual.item_id.clone())
                .collect(),
            review_display: Rc::default(),
            source,
            optimistic,
            state,
            prepared,
        }
    }

    fn prepare_reusing(
        state: &codex::Snapshot,
        content: [u64; 2],
        previous: Option<&PreparedTranscript>,
    ) -> PreparedTranscript {
        let items = &state.saved.items;
        let item_data: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                previous
                    .and_then(|old| old.item_data.get(index))
                    .filter(|old| std::ptr::eq(old.source.as_ref(), item.as_ref()))
                    .cloned()
                    .unwrap_or_else(|| Rc::new(PreparedItem::new(item.clone())))
            })
            .collect();
        let mut id_index = rustc_hash::FxHashMap::default();
        for (index, item) in items.iter().enumerate() {
            if let Some(id) = item["id"].as_str() {
                id_index.entry(id.to_owned()).or_insert(index);
            }
        }
        let mut commentary = vec![false; items.len()];
        let mut following_activity = false;
        for index in (0..items.len()).rev() {
            let item = &items[index];
            if item["type"] == "userMessage" {
                following_activity = false;
            }
            if item["type"] == "agentMessage" {
                commentary[index] = match item["phase"].as_str() {
                    Some("commentary") => true,
                    Some("final_answer") => false,
                    _ => following_activity,
                };
                following_activity = true;
            } else if item_data[index].tool {
                following_activity = true;
            }
        }
        let mut activity_prefix = Vec::with_capacity(items.len() + 1);
        activity_prefix.push(0);
        for (index, item) in items.iter().enumerate() {
            activity_prefix.push(
                activity_prefix[index]
                    + usize::from(
                        (item_data[index].tool && !tasks::is_checklist(item)) || commentary[index],
                    ),
            );
        }
        let task_key = task_cache_key(items);
        let has_timestamp = items.iter().any(|item| item["groveReceivedAt"].is_u64());
        let unchanged = previous.map_or(0, |old| {
            old.item_data
                .iter()
                .zip(&item_data)
                .take_while(|(a, b)| Rc::ptr_eq(a, b))
                .count()
        });
        // Rebuild the entire containing turn, including phase/final-answer selection.
        let restart = items[..unchanged.min(items.len())]
            .iter()
            .rposition(|item| item["type"] == "userMessage")
            .unwrap_or(0);
        let mut groups = previous.map_or_else(Vec::new, |old| {
            old.response_groups
                .iter()
                .filter(|group| group.end <= restart)
                .cloned()
                .collect()
        });
        let tail_groups = response_groups_from(
            items,
            state.saved.thread_id.as_deref(),
            restart,
            Some(&item_data),
        );
        groups.extend(tail_groups.into_iter().map(Rc::new));
        let response_groups = groups;
        let mut response_ends = vec![None; items.len() + 1];
        let mut response_prose = vec![None; items.len()];
        for (group_index, group) in response_groups.iter().enumerate() {
            response_ends[group.end] = Some(group_index);
            for index in &group.prose {
                response_prose[*index] = Some(group_index);
            }
        }
        let mut minimap_entries = previous.map_or_else(Vec::new, |old| {
            old.minimap_entries
                .iter()
                .filter(|entry| entry.item_index < restart)
                .cloned()
                .collect()
        });
        let tail_entries = message_minimap_entries_from(items, restart, Some(&item_data));
        minimap_entries.extend(tail_entries);
        let mut nearest_user = Vec::with_capacity(items.len() + 1);
        let mut latest_user = 0;
        for (index, item) in items.iter().enumerate() {
            if item["type"] == "userMessage" {
                latest_user = index;
            }
            nearest_user.push(latest_user);
        }
        nearest_user.push(latest_user);
        let mut work_keys = rustc_hash::FxHashMap::default();
        for &user in &nearest_user {
            work_keys
                .entry(user)
                .or_insert_with(|| format!("work-{user}"));
        }
        let mut next_nonuser = vec![items.len(); items.len() + 1];
        for index in (0..items.len()).rev() {
            next_nonuser[index] = if items[index]["type"] == "userMessage" {
                next_nonuser[index + 1]
            } else {
                index
            };
        }
        PreparedTranscript {
            content,
            id_index,
            item_data,
            commentary,
            activity_prefix,
            task_key,
            has_timestamp,
            response_groups,
            response_ends,
            response_prose,
            minimap_entries,
            nearest_user,
            work_keys,
            next_nonuser,
        }
    }
}

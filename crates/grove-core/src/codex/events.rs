//! Codex app-server responses, notifications, and child transcript polling.

use super::{Actor, Pending};
use crate::provider::{
    prompt_title, restore_task_snapshots, stamp_message_received, QuestionRequest, ReferenceDraft,
    Status, SubagentStatus, COMPACTION_ITEM,
};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

impl Actor {
    fn interaction_owner(
        &self,
        p: &Value,
        thread_bound: bool,
    ) -> Option<crate::interaction::InteractionOwner> {
        let thread = p["threadId"].as_str()?;
        if Some(thread) == self.state.saved.thread_id.as_deref() {
            let turn = p["turnId"].as_str();
            if !(thread_bound && turn.is_none())
                && (self.state.turn_id.is_none() || turn != self.state.turn_id.as_deref())
            {
                return None;
            }
            return Some(crate::interaction::InteractionOwner {
                thread: thread.into(),
                turn: turn.map(str::to_owned),
                agent: None,
            });
        }
        if !thread_bound && p["turnId"].as_str().is_none() {
            return None;
        }
        // Provider child approvals and questions belong to one active child
        // turn. Never attach an unscoped request to whichever turn is current.
        p["turnId"].as_str()?;
        self.interaction_children
            .owner(thread, p["turnId"].as_str())
    }
    pub(super) fn message(&mut self, message: Value) -> Result<(), String> {
        if message.get("method").is_some() {
            if message.get("id").is_some() {
                return self.server_request(message);
            }
            return self.notification(&message);
        }
        let Some(id) = message["id"].as_u64() else {
            return Ok(());
        };
        let Some(request) = self.pending.remove(&id) else {
            return Ok(());
        };
        if let Some(error) = message.get("error") {
            return self.response_error(request.kind, error);
        }
        self.response(request.kind, &message["result"])
    }

    fn server_request(&mut self, message: Value) -> Result<(), String> {
        let id = &message["id"];
        if self.side_chat {
            return self.write(&json!({"id":id,"error":{"code":-32601,"message":"Side chat does not expose host tools, approvals or permission changes."}}));
        }
        if self
            .state
            .approvals
            .iter()
            .any(|approval| approval["id"] == *id)
        {
            return Ok(());
        }
        let method = message["method"].as_str().unwrap_or_default();
        let params = &message["params"];
        if method == "item/tool/requestUserInput" {
            if self.state.questions.iter().any(|q| q.id == *id) {
                return Ok(());
            }
            let owner = self.interaction_owner(params, false);
            let current = owner.is_some();
            match QuestionRequest::parse(id.clone(), params, false) {
                Ok(request) if current => {
                    if let Some(owner) = owner {
                        self.state.interaction_owners.insert(id.to_string(), owner);
                    }
                    self.state.questions.push(request);
                    self.state.status = Status::Waiting;
                }
                _ => self.write(&json!({"id":id,"error":{"code":-32602,"message":"Invalid or expired question request"}}))?,
            }
            return Ok(());
        }
        let owner = self.interaction_owner(params, false);
        let active = owner.is_some();
        match method {
            "mcpServer/elicitation/request" => {
                if self.state.elicitations.iter().any(|request| request.id == *id) {
                    return Ok(());
                }
                let elicitation_owner = self.interaction_owner(params, true);
                match (
                    crate::elicitation::ElicitationRequest::parse_codex(id.clone(), params),
                    elicitation_owner,
                ) {
                    (Ok(request), Some(owner)) => {
                        self.state.interaction_owners.insert(id.to_string(), owner);
                        self.state.elicitations.push(request);
                        self.state.status = Status::Waiting;
                    }
                    _ => self.write(&json!({"id":id,"error":{"code":-32602,"message":"Invalid, unsupported or expired elicitation"}}))?,
                }
            }
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval" if active => {
                if method == "item/commandExecution/requestApproval"
                    && !params["kind"].is_null()
                    && !matches!(params["kind"].as_str(), Some("command" | "writeStdin"))
                {
                    self.write(&json!({"id":id,"error":{"code":-32602,"message":"Unknown command approval kind"}}))?;
                } else {
                    let mut approval = message.clone();
                    if let Some(owner) = owner {
                        if let Some(agent) = &owner.agent {
                            approval["groveAgent"] = json!(agent);
                        }
                        self.state.interaction_owners.insert(id.to_string(), owner);
                    }
                    self.state.approvals.push(approval);
                    self.state.status = Status::Waiting;
                }
            }
            "execCommandApproval" | "applyPatchApproval"
                if self.state.turn_id.is_some()
                    && params["conversationId"].as_str() == self.state.saved.thread_id.as_deref() =>
            {
                self.state.approvals.push(message);
                self.state.status = Status::Waiting;
            }
            "item/tool/call" => self.write(&json!({"id":id,"result":{"success":false,"contentItems":[{"type":"inputText","text":"Grove has no registered executor for this client-side dynamic tool. Configure a supported MCP tool instead."}]}}))?,
            "account/chatgptAuthTokens/refresh" | "attestation/generate" => {
                self.write(&json!({"id":id,"error":{"code":-32601,"message":"Grove uses CLI-managed authentication and cannot issue host credentials or attestation. Sign in with the agent CLI and reconnect."}}))?;
                self.state.error = Some("Codex requested host-managed credentials. Sign in with the Codex CLI and reconnect.".into());
            }
            _ => {
                self.write(&json!({"id":id,"error":{"code":-32601,"message":"Unsupported or expired Grove request"}}))?;
                self.state.error = Some(format!("Codex requested unsupported interaction: {method}. Interrupt or use a terminal session."));
            }
        }
        Ok(())
    }

    fn notification(&mut self, m: &Value) -> Result<(), String> {
        let active = self.state.turn_id.clone();
        let completed = m["method"] == "turn/completed"
            && active.is_some()
            && m["params"]["turn"]["id"].as_str() == active.as_deref()
            && m["params"]["threadId"].as_str() == self.state.saved.thread_id.as_deref();
        if let Some(root) = self.state.saved.thread_id.as_deref() {
            if let Some(expired) = self.interaction_children.codex_event(
                root,
                self.state.turn_id.as_deref().or_else(|| {
                    self.stop_tracking
                        .stopping
                        .then_some(self.stop_tracking.root.as_str())
                }),
                m,
            ) {
                self.state.expire_interactions(&expired);
            }
        }
        self.observe_stop_work(m)?;
        if m["method"] == "serverRequest/resolved" {
            let id = &m["params"]["requestId"];
            if self
                .state
                .interaction_owners
                .get(&id.to_string())
                .is_some_and(|owner| {
                    Some(owner.thread.as_str()) == m["params"]["threadId"].as_str()
                })
            {
                self.state.approvals.retain(|a| a["id"] != *id);
                self.state.questions.retain(|q| q.id != *id);
                self.state.elicitations.retain(|r| r.id != *id);
                self.state.retire_interaction(id);
            }
            self.state.refresh_interaction_status();
            return Ok(());
        }
        self.state.apply(m);
        if m["method"] == "turn/started"
            && m["params"]["threadId"].as_str() == self.state.saved.thread_id.as_deref()
            && m["params"]["turn"]["id"].as_str() == self.state.turn_id.as_deref()
        {
            if let Some(compaction) = &mut self.compaction_queue {
                // Bind once; an unrelated or stale turn cannot replace the owner.
                if compaction.turn.is_none() {
                    compaction.turn.clone_from(&self.state.turn_id);
                }
            }
        }
        if self.stop_tracking.stopping {
            if let Some(error) = &self.stop_tracking.error {
                self.state.error = Some(error.clone());
            }
        }
        self.state.refresh_interaction_status();
        if completed && m["params"]["turn"]["status"] == "completed" {
            crate::rich_plan::import_completed(&mut self.state.saved);
        }
        if completed {
            if let Some(compaction) = self.compaction_queue.take() {
                if compaction.turn.as_deref() == active.as_deref()
                    && m["params"]["turn"]["status"] == "completed"
                    && !self.state.queued_prompts.is_empty()
                    && self
                        .state
                        .queued_prompts
                        .iter()
                        .all(|p| compaction.submissions.contains(&p.id))
                    && self.state.queue_uncertain.is_empty()
                    && self.state.queue_sending.is_empty()
                {
                    self.queue_paused = false;
                }
            }
            self.queue_ready = m["params"]["turn"]["status"] == "completed";
            if !self.queue_ready {
                self.compaction_queue = None;
                self.queue_paused = true;
            }
            self.dispatch_queue()?;
        }
        if self.state.status == Status::Failed {
            self.compaction_queue = None;
            self.queue_paused = true;
            self.queue_ready = false;
        }
        Ok(())
    }

    fn response_error(&mut self, kind: Pending, error: &Value) -> Result<(), String> {
        if let Pending::StopList { root, thread, .. } = &kind {
            if self.stop_tracking.root == *root && self.stop_tracking.stopping {
                self.stop_error(format!("Stop could not inspect Codex background work for {thread}: {}. Check running work manually.", error["message"].as_str().unwrap_or("unsupported by provider")));
            }
            return Ok(());
        }
        if let Pending::StopChild { root, thread, turn }
        | Pending::StopProcess {
            root,
            thread,
            process: turn,
        } = &kind
        {
            if self.stop_tracking.root == *root && self.stop_tracking.stopping {
                self.stop_error(format!("Stop could not cancel Codex work ({thread}, {turn}): {}. Check running work manually.", error["message"].as_str().unwrap_or("provider does not support cancellation")));
            }
            return Ok(());
        }
        if matches!(kind, Pending::ChildInterrupt) {
            self.state.error = Some(
                error["message"]
                    .as_str()
                    .unwrap_or("Could not interrupt the child turn")
                    .into(),
            );
            return Ok(());
        }
        if let Pending::ChildRead {
            thread, generation, ..
        } = &kind
        {
            if self.state.subagent_observation_generation != *generation {
                return Ok(());
            }
            if let Some(a) = self.state.subagent_activity.get_mut(thread) {
                a.refreshing = false;
                a.refresh_error = true;
            }
            let failures = self.child_read_failures.entry(thread.clone()).or_default();
            *failures = failures.saturating_add(1);
            return Ok(());
        }
        // Optional catalogs must never fail or pause an otherwise healthy session.
        if matches!(
            kind,
            Pending::Models | Pending::Skills | Pending::ChildRead { .. }
        ) {
            return Ok(());
        }

        if matches!(kind, Pending::EditMessage) {
            self.recover_failed_edit()?;
            self.state.error = Some(
                error["message"]
                    .as_str()
                    .unwrap_or("Codex could not restore the conversation")
                    .into(),
            );
            return Ok(());
        }
        if let Pending::Turn { text, images, .. } = &kind {
            // A queued UI clear may arrive before the provider rejects the
            // start. Recover the submitted draft, preserving newer edits
            // and independently owned quote state.
            if self.state.saved.draft.is_empty()
                && self.state.saved.draft_images.is_empty()
                && self.state.saved.draft_reference.is_none()
                && !ReferenceDraft::is_canonical_prompt(text)
            {
                self.state.saved.draft.clone_from(text);
                self.state.saved.draft_images.clone_from(images);
            }
        }
        if let Pending::Turn {
            submission: Some(id),
            ..
        } = &kind
        {
            self.state.rejected_submission = Some(*id);
        }
        if matches!(kind, Pending::Compact) {
            self.compaction_queue = None;
            self.queue_paused = true;
            self.queue_ready = false;
            self.state.compact_requested = false;
            self.state.error = Some(
                error["message"]
                    .as_str()
                    .unwrap_or("Codex could not compact the conversation")
                    .into(),
            );
            return Ok(());
        }
        if let Pending::QueuedTurn { id } = &kind {
            let rich = self.state.saved.rich_plans.is_delivery(*id);
            if rich {
                self.state.saved.rich_plans.fail_delivery(
                    *id,
                    error["message"]
                        .as_str()
                        .unwrap_or("Provider rejected plan feedback"),
                );
                self.state.queued_prompts.retain(|p| p.id != *id);
            }
            self.state.queue_sending.retain(|sending| sending != id);
        }
        if let Pending::Steer {
            submission,
            from_queue,
            ..
        } = kind
        {
            self.state.queue_sending.retain(|id| *id != submission);
            self.state.queue_steering.retain(|id| *id != submission);
            if !from_queue {
                self.state.queued_prompts.retain(|p| p.id != submission);
            }
            self.compaction_queue = None;
            self.queue_paused = true;
            self.queue_ready = false;
            self.state.rejected_submission = Some(submission);
            self.state.error = Some(
                error["message"]
                    .as_str()
                    .unwrap_or("Codex rejected steering")
                    .into(),
            );
            return Ok(());
        }
        if matches!(kind, Pending::AsyncAnswer { .. }) {
            self.state.error = Some(
                error["message"]
                    .as_str()
                    .unwrap_or("Codex rejected the answer")
                    .into(),
            );
            self.state.status = Status::Waiting;
            return Ok(());
        }
        self.compaction_queue = None;
        self.queue_paused = true;
        self.queue_ready = false;
        let message = error["message"]
            .as_str()
            .unwrap_or("Codex request failed")
            .to_owned();
        if let Pending::Interrupt { attempt, turn } = kind {
            if self.state.turn_id.as_deref() != Some(&turn) {
                return Ok(());
            }
            if attempt < 4
                && message.contains("no active turn to interrupt")
                && self.state.turn_id.as_deref() == Some(&turn)
            {
                self.interrupt_retry = Some((
                    Instant::now() + Duration::from_millis(150),
                    attempt + 1,
                    turn,
                ));
                return Ok(());
            }
        }
        self.state.error = Some(message);
        if !matches!(self.state.status, Status::Done | Status::Interrupted) {
            self.state.status = Status::Failed;
            self.state.reconnecting = false;
        }
        Ok(())
    }

    fn response(&mut self, kind: Pending, result: &Value) -> Result<(), String> {
        match kind {
            Pending::Initialize => {
                self.write(&json!({"method":"initialized","params":{}}))?;
                if !self.side_chat && self.state.models.is_empty() {
                    self.request("model/list", json!({"limit":100}), Pending::Models)?;
                } else if self.state.resolved_model.is_none() {
                    if let Some(model) = self
                        .state
                        .models
                        .iter()
                        .find(|model| model["isDefault"] == true)
                    {
                        self.state.resolved_model = model["model"].as_str().map(str::to_owned);
                        self.state.resolved_effort =
                            model["defaultReasoningEffort"].as_str().map(str::to_owned);
                    }
                }
                if !self.side_chat {
                    self.request(
                        "skills/list",
                        json!({"cwds":[self.state.saved.cwd]}),
                        Pending::Skills,
                    )?;
                }
                self.request(
                    "config/read",
                    json!({"cwd":self.state.saved.cwd,"includeLayers":false}),
                    Pending::Config,
                )?;
            }
            Pending::Models => {
                self.state
                    .models
                    .extend(result["data"].as_array().cloned().unwrap_or_default());
                if self.state.resolved_model.is_none() {
                    if let Some(model) = self
                        .state
                        .models
                        .iter()
                        .find(|model| model["isDefault"] == true)
                    {
                        self.state.resolved_model = model["model"].as_str().map(str::to_owned);
                        self.state.resolved_effort =
                            model["defaultReasoningEffort"].as_str().map(str::to_owned);
                    }
                }
                if let Some(cursor) = result["nextCursor"].as_str() {
                    self.request(
                        "model/list",
                        json!({"limit":100,"cursor":cursor}),
                        Pending::Models,
                    )?;
                } else if let Some(key) = &self.model_catalog_key {
                    crate::model_catalog::remember(key, &self.state.models);
                }
            }
            Pending::Skills => {
                self.state.skills = (result["data"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry["skills"].as_array())
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>())
                .into();
            }
            Pending::Config => {
                if self.side_chat {
                    // Read the effective config only to disable inherited MCP registrations.
                    if let Some(servers) = result["config"]["mcp_servers"].as_object() {
                        for name in servers.keys() {
                            self.side_mcp_servers.push(name.clone());
                        }
                    }
                }
                result["config"]["developer_instructions"]
                    .as_str()
                    .unwrap_or_default()
                    .clone_into(&mut self.developer_instructions);
                if let Some(model) = result["config"]["model"].as_str() {
                    self.state.resolved_model = Some(model.into());
                }
                if let Some(effort) = result["config"]["model_reasoning_effort"].as_str() {
                    self.state.resolved_effort = Some(effort.into());
                }
                if let Some(model) = &self.state.resolved_model {
                    self.configured_turn["model"] = json!(model);
                }
                if let Some(effort) = &self.state.resolved_effort {
                    self.configured_turn["effort"] = json!(effort);
                }

                if let Some(policy) = result["config"].get("approval_policy") {
                    if !policy.is_null() {
                        self.configured_turn["approvalPolicy"] = policy.clone();
                    }
                }
                let configured_sandbox = match result["config"]["sandbox_mode"].as_str() {
                    Some("read-only") => Some(json!({"type":"readOnly"})),
                    Some("danger-full-access") => Some(json!({"type":"dangerFullAccess"})),
                    Some("workspace-write") => Some(json!({"type":"workspaceWrite",
                        "writableRoots":self.state.saved.roots.iter().map(|root| &root.wt_path).collect::<Vec<_>>(),
                        "networkAccess":result["config"]["sandbox_workspace_write"]["network_access"].as_bool().unwrap_or(false),
                        "excludeTmpdirEnvVar":result["config"]["sandbox_workspace_write"]["exclude_tmpdir_env_var"].as_bool().unwrap_or(false),
                        "excludeSlashTmp":result["config"]["sandbox_workspace_write"]["exclude_slash_tmp"].as_bool().unwrap_or(false)})),
                    _ => None,
                };
                if let Some(policy) = configured_sandbox {
                    self.configured_turn["sandboxPolicy"] = policy;
                }
                let mut roots = result["config"]["sandbox_workspace_write"]["writable_roots"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                for root in &self.state.saved.roots {
                    if root.wt_path != self.state.saved.cwd
                        && !roots
                            .iter()
                            .any(|value| value.as_str() == Some(&root.wt_path))
                    {
                        roots.push(json!(root.wt_path));
                    }
                }
                if self.configured_turn["sandboxPolicy"]["type"] == "workspaceWrite" {
                    self.configured_turn["sandboxPolicy"]["writableRoots"] = json!(roots);
                }
                if self.discovery_only {
                    self.state.sandbox = self.configured_turn.get("sandboxPolicy").cloned();
                    self.state.approval_policy =
                        self.configured_turn.get("approvalPolicy").cloned();
                    self.state.resolved_access =
                        match self.configured_turn["sandboxPolicy"]["type"].as_str() {
                            Some("dangerFullAccess") => Some("full-access".into()),
                            Some("readOnly") => Some("read-only".into()),
                            Some("workspaceWrite") => Some("workspace-write".into()),
                            _ => None,
                        };
                } else {
                    self.start_thread(Some(roots))?;
                }
            }
            Pending::Thread => {
                if let Some(model) = result["model"].as_str() {
                    self.state.resolved_model = Some(model.into());
                    if self.configured_turn.get("model").is_none() {
                        self.configured_turn["model"] = json!(model);
                    }
                }
                if let Some(effort) = result["reasoningEffort"].as_str() {
                    self.state.resolved_effort = Some(effort.into());
                    if self.configured_turn.get("effort").is_none() {
                        self.configured_turn["effort"] = json!(effort);
                    }
                }
                self.state.resolved_access = match result["sandbox"]["type"].as_str() {
                    Some("dangerFullAccess") => Some("full-access".into()),
                    Some("readOnly") => Some("read-only".into()),
                    Some("workspaceWrite") => Some("workspace-write".into()),
                    _ => None,
                };
                if let Some(sandbox) = result.get("sandbox") {
                    if !sandbox.is_null() {
                        self.configured_turn["sandboxPolicy"] = sandbox.clone();
                    }
                }
                if let Some(approval) = result.get("approvalPolicy") {
                    if !approval.is_null() {
                        self.configured_turn["approvalPolicy"] = approval.clone();
                    }
                }
                self.state.sandbox = result.get("sandbox").cloned();
                self.state.approval_policy = result.get("approvalPolicy").cloned();
                if result["sandbox"]["type"] == "workspaceWrite" {
                    for root in &self.state.saved.roots {
                        if root.wt_path != self.state.saved.cwd
                            && !result["sandbox"]["writableRoots"]
                                .as_array()
                                .is_some_and(|roots| {
                                    roots
                                        .iter()
                                        .any(|value| value.as_str() == Some(&root.wt_path))
                                })
                        {
                            return Err(format!(
                                "Codex did not honor writable context root {}. No prompt was submitted.",
                                root.wt_path
                            ));
                        }
                    }
                }
                self.state.edit_supported = result["thread"]["historyMode"] == "paginated";
                if let Some(id) = result["thread"]["id"].as_str() {
                    if let Some(ownership) =
                        self.process.as_mut().and_then(|p| p.ownership.as_mut())
                    {
                        ownership.claim(id)?;
                    }
                }
                self.state.saved.thread_id = result["thread"]["id"].as_str().map(str::to_owned);
                if self.state.saved.thread_id.is_none() {
                    return Err("Codex returned no thread identity".into());
                }
                if let Some(origin) = self.state.saved.fork_origin.as_mut() {
                    origin.source_thread = None;
                    self.persistence.flush(&self.state.saved)?;
                }
                if self.state.saved.pending_edit.is_some() {
                    self.request("thread/turns/list", json!({"threadId":self.state.saved.thread_id,"itemsView":"notLoaded","limit":100,"sortDirection":"desc"}), Pending::RecoverEdit)?;
                    self.state.status = Status::Starting;
                    return Ok(());
                }
                if let Some(turns) = result["thread"]["turns"].as_array() {
                    let items = turns
                        .iter()
                        .filter_map(|t| t["items"].as_array())
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>();
                    if !items.is_empty() {
                        // History carries no Grove metadata; keep known compaction figures.
                        let items = items
                            .into_iter()
                            .map(|mut item| {
                                if let Some(old) = self
                                    .state
                                    .saved
                                    .items
                                    .iter()
                                    .find(|old| old["id"] == item["id"])
                                {
                                    stamp_message_received(&mut item, Some(old));
                                    for marker in ["groveQuestionResolved", "groveQuestionPending"]
                                    {
                                        if old[marker] == true {
                                            item[marker] = json!(true);
                                        }
                                    }
                                }
                                if item["type"] != COMPACTION_ITEM {
                                    return item;
                                }
                                let mut known = self
                                    .state
                                    .saved
                                    .items
                                    .iter()
                                    .find(|old| old["id"] == item["id"])
                                    .map(|old| old.as_ref().clone())
                                    .unwrap_or(item);
                                known["status"] = json!("completed");
                                known
                            })
                            .collect();
                        // Transcript offsets remain valid only when restored items keep
                        // the saved prefix. A provider rewrite must not misattribute timing.
                        let items: Vec<Value> =
                            restore_task_snapshots(&self.state.saved.items, items);
                        if self.state.saved.items.len() > items.len()
                            || self
                                .state
                                .saved
                                .items
                                .iter()
                                .zip(&items)
                                .any(|(old, new)| old["id"] != new["id"])
                        {
                            self.state.saved.work_periods.clear();
                            self.state.saved.session_review.pending = None;
                            self.state.saved.session_review.last_completed_period = None;
                            self.state.saved.session_review.reviewed_through = 0;
                            self.state.saved.session_review.summary.clear();
                            self.state.saved.edit_checkpoints.clear();
                            self.state.work_started = None;
                        }
                        self.state.saved.items = items.into();
                    }
                }
                self.state
                    .set_provider_title(result["thread"]["name"].as_str());
                self.state.status = Status::Idle;
                self.state.reconnecting = false;
                if let Some(turns) = result["thread"]["turns"].as_array() {
                    if let Some(turn) = turns
                        .iter()
                        .rev()
                        .find(|turn| turn["status"] == "inProgress")
                    {
                        if result["thread"]["status"]["type"] == "active" {
                            self.state.turn_id = turn["id"].as_str().map(str::to_owned);
                            self.state.status = Status::Working;
                            self.state.begin_work();
                        } else {
                            self.state.turn_id = None;
                            if self.side_chat {
                                // A stopped read-only side turn has no workspace recovery.
                                // Reopening must permit the user's explicit retry.
                                self.state.finish_work();
                                self.state.status = Status::Idle;
                            } else {
                                self.state.status = Status::Interrupted;
                                self.state.error = Some("The previous turn stopped when Codex disconnected. Inspect the conversation before sending another prompt.".into());
                            }
                        }
                    }
                }
                self.state.restore_async_questions();
                if let Some(prompt) = self.initial.take() {
                    self.prompt(prompt)?;
                }
            }
            Pending::AsyncAnswer {
                question_id,
                request_instance,
                mut ack,
            } => {
                // A cancelled or expired request cannot become an answered receipt.
                if self
                    .state
                    .questions
                    .iter()
                    .any(|request| request.instance == request_instance)
                {
                    ack.sequence = self
                        .state
                        .question_answer_acks
                        .last()
                        .map_or(1, |old| old.sequence + 1);
                    self.state.question_answer_acks.push(ack);
                    self.state.resolve_async_question(&question_id);
                }
                if let Some(turn) = result["turn"]["id"]
                    .as_str()
                    .filter(|id| !self.state.settled_turns.iter().any(|old| old == id))
                {
                    self.state.turn_id = Some(turn.to_owned());
                }
                self.state.status = if !self.state.questions.is_empty()
                    || !self.state.approvals.is_empty()
                    || !self.state.elicitations.is_empty()
                {
                    Status::Waiting
                } else if self.state.turn_id.is_some() {
                    Status::Working
                } else {
                    Status::Done
                };
            }
            Pending::RecoverEdit => {
                let edit = self
                    .state
                    .saved
                    .pending_edit
                    .clone()
                    .ok_or("Missing edit recovery journal")?;
                let turns = result["data"].as_array().ok_or(
                    "Codex returned no history page for edit recovery; reconnect to retry.",
                )?;
                if turns
                    .iter()
                    .any(|turn| turn["id"].as_str() == Some(&edit.turn_id))
                {
                    self.recover_failed_edit()?;
                    self.state.error = Some("The interrupted edit was cancelled; the previous workspace and conversation were recovered.".into());
                } else if let Some(cursor) = result["nextCursor"].as_str() {
                    self.request("thread/turns/list", json!({"threadId":self.state.saved.thread_id,"itemsView":"notLoaded","limit":100,"sortDirection":"desc","cursor":cursor}), Pending::RecoverEdit)?;
                } else {
                    let record = self
                        .state
                        .saved
                        .edit_checkpoints
                        .iter()
                        .find(|record| record.turn_id.as_deref() == Some(&edit.turn_id))
                        .ok_or("The edit checkpoint is missing; recovery remains blocked.")?;
                    if edit.recovery.is_some() {
                        record
                            .files
                            .as_ref()
                            .ok_or(
                                "The workspace checkpoint is missing; recovery remains blocked.",
                            )?
                            .restore()?;
                    }
                    self.finish_edit()?;
                    self.state.error = Some(
                        "The interrupted edit completed; the restored message is ready to edit."
                            .into(),
                    );
                }
            }
            Pending::EditMessage => {
                if result["thread"]["id"].as_str() != self.state.saved.thread_id.as_deref() {
                    return Err("Codex returned an unexpected edit acknowledgement. Reconnect to reconcile the conversation; the workspace recovery was preserved.".into());
                }
                self.finish_edit()?;
            }
            Pending::InlineTurn => {
                self.bind_edit_checkpoint(result);
                self.state.accepted_prompts = self.state.accepted_prompts.saturating_add(1);
                if self.state.status == Status::Working {
                    self.state.turn_id = result["turn"]["id"].as_str().map(str::to_owned);
                }
            }
            Pending::Turn {
                submission,
                text,
                images,
            } => {
                self.bind_edit_checkpoint(result);
                if let Some(submission) = submission {
                    self.state.accept_submission(submission);
                    if let Some(turn) = result["turn"]["id"].as_str() {
                        self.state.submission_turns.insert(submission, turn.into());
                    }
                }
                self.state.accepted_prompts = self.state.accepted_prompts.saturating_add(1);
                if self.state.saved.title.is_none() {
                    self.state.saved.title = prompt_title(&text);
                }
                self.state.saved.clear_submitted_draft(&text, &images);
                if self.state.status == Status::Working {
                    self.state.turn_id = result["turn"]["id"].as_str().map(str::to_owned);
                }
            }
            Pending::Steer {
                submission,
                turn,
                draft,
                from_queue,
            } => {
                if result["turnId"].as_str() != Some(turn.as_str()) {
                    self.state.mark_queue_uncertain();
                    self.compaction_queue = None;
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.rejected_submission = Some(submission);
                    self.state.error = Some("Codex returned an unexpected steering turn".into());
                } else {
                    self.state.accept_submission(submission);
                    if !from_queue {
                        self.state
                            .saved
                            .clear_submitted_draft(&draft.text, &draft.images);
                        self.state.accept_submission(submission);
                    }
                    self.state.queued_prompts.retain(|p| p.id != submission);
                    self.state.queue_sending.retain(|id| *id != submission);
                    self.state.queue_steering.retain(|id| *id != submission);
                    self.state.queue_uncertain.retain(|id| *id != submission);
                }
            }
            Pending::QueuedTurn { id } => {
                self.bind_edit_checkpoint(result);
                let Some(turn) = result["turn"]["id"].as_str().filter(|id| !id.is_empty()) else {
                    self.compaction_queue = None;
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.mark_queue_uncertain();
                    self.state.error = Some("Codex returned no queued turn identity; inspect the conversation before resuming".into());
                    if self.state.turn_id.is_none() {
                        self.state.status = Status::Failed;
                        self.state.reconnecting = false;
                    }
                    return Ok(());
                };
                if self
                    .state
                    .turn_id
                    .as_deref()
                    .is_some_and(|active| active != turn)
                {
                    self.compaction_queue = None;
                    self.queue_paused = true;
                    self.queue_ready = false;
                    self.state.mark_queue_uncertain();
                    self.state.error =
                        Some("Codex returned an unexpected queued turn identity".into());
                    return Ok(());
                }
                self.state.submission_turns.insert(id, turn.into());
                self.state.saved.rich_plans.confirm_delivery(id);
                self.state.queued_prompts.retain(|p| p.id != id);
                self.state.queue_sending.retain(|sending| *sending != id);
                self.state.queue_uncertain.retain(|sending| *sending != id);
                if self.state.status == Status::Working {
                    self.state.turn_id = result["turn"]["id"]
                        .as_str()
                        .filter(|id| !self.state.settled_turns.iter().any(|old| old == id))
                        .map(str::to_owned);
                }
            }
            Pending::ChildRead {
                thread,
                period_start,
                generation,
            } => {
                if self.state.subagent_observation_generation != generation {
                    return Ok(());
                }
                if let Some(a) = self.state.subagent_activity.get_mut(&thread) {
                    a.refreshing = false;
                    a.refresh_error = false;
                }
                self.child_read_failures.remove(&thread);
                let revision = self
                    .state
                    .subagent_activity
                    .get(&thread)
                    .map(|activity| activity.items.revision());
                self.state
                    .apply_subagent_read(&thread, period_start, &result["thread"]);
                let unchanged = revision
                    == self
                        .state
                        .subagent_activity
                        .get(&thread)
                        .map(|activity| activity.items.revision());
                let attempts = if unchanged {
                    self.child_read_backoff
                        .get(&thread)
                        .filter(|(old, _, _)| *old == generation)
                        .map_or(1, |(_, attempts, _)| attempts.saturating_add(1))
                } else {
                    0
                };
                let delay = Duration::from_secs((2_u64 << attempts.min(4)).min(30));
                self.child_read_backoff
                    .insert(thread, (generation, attempts, Instant::now() + delay));
            }
            Pending::StopList {
                root,
                thread,
                version,
            } => {
                if self.stop_tracking.root != root || !self.stop_tracking.stopping {
                    return Ok(());
                }
                let Some(data) = result["data"].as_array() else {
                    self.stop_error("Stop could not inspect Codex background terminals. Check running work manually.".into());
                    return Ok(());
                };
                for entry in data {
                    let (Some(item), Some(process)) =
                        (entry["itemId"].as_str(), entry["processId"].as_str())
                    else {
                        continue;
                    };
                    if self
                        .stop_tracking
                        .processes
                        .get(&(thread.clone(), item.into()))
                        .is_some_and(|id| id == process)
                        && self
                            .stop_tracking
                            .sent_processes
                            .insert((thread.clone(), process.into()))
                    {
                        self.request(
                            "thread/backgroundTerminals/terminate",
                            json!({"threadId":thread,"processId":process}),
                            Pending::StopProcess {
                                root: root.clone(),
                                thread: thread.clone(),
                                process: process.into(),
                            },
                        )?;
                    }
                }
                if let Some(cursor) = result["nextCursor"].as_str() {
                    self.request(
                        "thread/backgroundTerminals/list",
                        json!({"threadId":thread,"cursor":cursor}),
                        Pending::StopList {
                            root,
                            thread,
                            version,
                        },
                    )?;
                } else {
                    self.stop_tracking.listed.insert(thread, version);
                    self.stop_descendants()?;
                }
            }
            Pending::StopChild { .. } => {} // ACK is not a completed turn.
            Pending::StopProcess { root, .. } => {
                if self.stop_tracking.root == root
                    && self.stop_tracking.stopping
                    && result["terminated"] != true
                {
                    self.stop_error("Stop could not confirm a Codex background terminal was terminated. Check running work manually.".into());
                }
            }
            Pending::Interrupt { .. } | Pending::Compact | Pending::ChildInterrupt => {}
        }
        self.dispatch_queue()?;
        Ok(())
    }

    pub(super) fn expire_subagent_reads(&mut self) {
        let expired: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, request)| match &request.kind {
                Pending::ChildRead { thread, .. } if request.deadline < Instant::now() => Some((
                    *id,
                    thread.clone(),
                    match &request.kind {
                        Pending::ChildRead { generation, .. } => *generation,
                        _ => 0,
                    },
                )),
                _ => None,
            })
            .collect();
        for (id, thread, generation) in expired {
            self.pending.remove(&id);
            if self.state.subagent_observation_generation != generation {
                continue;
            }
            if let Some(a) = self.state.subagent_activity.get_mut(&thread) {
                a.refreshing = false;
                a.refresh_error = true;
            }
            let failures = self.child_read_failures.entry(thread).or_default();
            *failures = failures.saturating_add(1);
        }
    }
    pub(super) fn poll_subagents(&mut self) {
        if self.process.is_none() || self.last_child_poll.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_child_poll = Instant::now();
        // Optional reads must never hold up the parent or accumulate timed-out requests.

        let children: Vec<_> = self
            .state
            .saved
            .work_periods
            .iter()
            .flat_map(|period| {
                period
                    .subagents
                    .iter()
                    .filter(|agent| {
                        matches!(
                            agent.status,
                            SubagentStatus::Working | SubagentStatus::Unknown
                        )
                    })
                    .map(|agent| (agent.id.clone(), period.start_item))
            })
            .collect();
        let active: HashSet<&str> = children.iter().map(|(thread, _)| thread.as_str()).collect();
        self.child_read_backoff
            .retain(|thread, _| active.contains(thread.as_str()));
        for (thread, period_start) in children {
            if self
                .child_read_backoff
                .get(&thread)
                .is_some_and(|(generation, _, due)| {
                    *generation == self.state.subagent_observation_generation
                        && Instant::now() < *due
                })
            {
                continue;
            }
            if thread.len() > 1024 {
                continue;
            }
            if self.child_read_failures.get(&thread).copied().unwrap_or(0) >= 3 {
                continue;
            }
            if self
                .state
                .subagent_period(&thread)
                .is_none_or(|index| self.state.saved.work_periods[index].start_item != period_start)
            {
                continue;
            }
            if self.pending.values().any(|request| matches!(&request.kind, Pending::ChildRead { thread: pending, .. } if pending == &thread)) { continue; }
            let generation = self.state.subagent_observation_generation;
            self.state.ensure_subagent_activity(&thread);
            let Some(activity) = self.state.subagent_activity.get_mut(&thread) else {
                continue;
            };
            activity.refreshing = true;
            let _ = self.request(
                "thread/read",
                json!({"threadId":thread,"includeTurns":true}),
                Pending::ChildRead {
                    thread,
                    period_start,
                    generation,
                },
            );
        }
    }
}

//! Claude stream frames, control responses, and primary transcript updates.

use super::{context::context_measurement, nonce, Actor};
use crate::provider::{self, QuestionRequest, Status};
use serde_json::{json, Value};

impl Actor {
    pub(super) fn respond(&mut self, id: Value, response: Value) -> Result<(), String> {
        self.write(json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}))
    }
    pub(super) fn upsert(&mut self, mut item: Value) {
        let previous = self
            .state
            .saved
            .items
            .iter()
            .find(|old| old["id"] == item["id"]);
        provider::stamp_message_received(&mut item, previous.map(AsRef::as_ref));
        if let Some(existing) = self
            .state
            .saved
            .items
            .iter_mut()
            .find(|old| old["id"] == item["id"])
        {
            *existing = item.into();
        } else {
            self.state.saved.items.push(item);
        }
    }
    fn control_owner(&self, value: &Value) -> Option<crate::interaction::InteractionOwner> {
        let agent = value["request"]["agent_id"].as_str();
        let parent = value["parent_tool_use_id"].as_str();
        if let Some(id) = agent.or(parent) {
            let owner = self.interaction_children.owner(id, None)?;
            if parent
                .is_some_and(|parent| self.interaction_children.canonical(parent) != owner.thread)
            {
                return None;
            }
            return Some(owner);
        }
        if !self.state.status.busy() || self.interrupting {
            return None;
        }
        Some(crate::interaction::InteractionOwner {
            thread: self.state.saved.thread_id.clone().unwrap_or_default(),
            turn: self.state.turn_id.clone(),
            agent: None,
        })
    }
    fn effective_permission_mode(&mut self, mode: &str) {
        self.permission_mode_revision += 1;
        self.state.resolved_access = Some(mode.into());
        if self.state.access.is_some() {
            self.state.access = Some(mode.into());
            self.state.saved.composer.access = Some(mode.into());
        }
    }
    pub(super) fn message(&mut self, value: Value) -> Result<(), String> {
        if let Some(session) = value["session_id"].as_str().filter(|id| !id.is_empty()) {
            if self.state.saved.pending_edit.is_some() {
                // A fork's new identity is committed together with its truncated
                // transcript only after initialize succeeds.
            } else if let Some(primary) = &self.state.saved.thread_id {
                if primary != session {
                    return Ok(());
                }
            } else {
                if let Some(ownership) = self.process.as_mut().and_then(|p| p.ownership.as_mut()) {
                    ownership.claim(session)?;
                }
                self.state.saved.thread_id = Some(session.into());
            }
        }
        if !matches!(
            value["type"].as_str(),
            Some("control_request" | "control_cancel_request")
        ) {
            if let Some(parent) = value["parent_tool_use_id"].as_str() {
                self.subagent_message(parent, &value);
                return Ok(());
            }
        }
        // These fields describe actual provider state, unlike a settings ACK.
        if let Some(speed) = value["fast_mode_state"].as_str() {
            self.state.reported_speed = Some(speed.into());
            self.state.speed_disabled_reason = value["fast_mode_disabled_reason"]
                .as_str()
                .map(str::to_owned);
        }
        self.subagent_system_message(&value);
        match value["type"].as_str().unwrap_or_default() {
            "control_response" => return self.control_response(&value["response"]),
            "system" if value["subtype"] == "session_state_changed" => {
                self.tracks_session_state = true;
                self.session_idle = value["state"] == "idle";
                match value["state"].as_str() {
                    Some("idle") if !self.configuring => {
                        if let Some(status) = self.pending_completion.take() {
                            self.settle_root_items(status);
                            self.state.finish_work();
                            self.state.status = status;
                            if status == Status::Done {
                                self.state.completed_for_review();
                            }
                            self.state.turn_id = None;
                            let root = self.state.saved.thread_id.clone().unwrap_or_default();
                            self.state.expire_interactions(&root);
                            self.interrupting = false;
                        } else if self.state.turn_id.is_none() {
                            self.state.status = Status::Idle;
                        }
                        // Current SDK may publish idle before result: retain an
                        // active foreground turn until its result also arrives.
                    }
                    Some("running") if self.initialized && !self.configuring => {
                        self.state.status = if self.state.approvals.is_empty()
                            && self.state.questions.is_empty()
                            && self.state.elicitations.is_empty()
                        {
                            Status::Working
                        } else {
                            Status::Waiting
                        };
                    }
                    Some("requires_action")
                        if !self.state.approvals.is_empty()
                            || !self.state.questions.is_empty()
                            || !self.state.elicitations.is_empty() =>
                    {
                        self.state.status = Status::Waiting;
                    }
                    _ => {}
                }
            }
            "system" if value["subtype"] == "status" => {
                self.state.compacting = value["status"] == "compacting";
                if self.state.compacting {
                    self.state.context_measurement_live = false;
                    self.state.saved.context_details = None;
                    self.state.context_revision = self.state.context_revision.wrapping_add(1);
                }
                if let Some(mode) = value["permissionMode"].as_str() {
                    // A later provider status supersedes even an in-flight ACK.
                    self.effective_permission_mode(mode);
                }
                if value["compact_result"]
                    .as_str()
                    .is_some_and(|result| result != "success")
                {
                    self.state.error = Some("Claude Code could not compact the context.".into());
                }
            }
            "system" if value["subtype"] == "compact_boundary" => {
                self.state.compacting = false;
                let metadata = &value["compact_metadata"];
                let id = value["uuid"]
                    .as_str()
                    .map_or_else(|| format!("compact-{}", nonce()), str::to_owned);
                let trigger = match metadata["trigger"].as_str() {
                    Some("manual") => "manual",
                    _ => "auto",
                };
                self.state.upsert_compaction(
                    &json!(id),
                    false,
                    Some(trigger),
                    metadata["pre_tokens"].as_u64(),
                );
                // The boundary's own post figure counts only the summary, not the
                // system prompt and tools, so measure the real post-compaction context.
                if self.process.is_some() {
                    let _ = self.control(json!({"subtype":"get_context_usage"}));
                }
            }
            "system" if value["subtype"] == "session_title_changed" => {
                self.state.set_provider_title(value["title"].as_str());
            }
            "system" if value["subtype"] == "init" => {
                self.observe_mcp_servers(&value["mcp_servers"]);
                let model = value["model"].as_str().map(str::to_owned);
                if self.state.resolved_model != model {
                    self.state.context_measurement_live = false;
                    self.state.context_revision = self.state.context_revision.wrapping_add(1);
                    self.state.saved.context_details = None;
                }
                self.state.resolved_model = model;
                if let Some(mode) = value["permissionMode"].as_str() {
                    self.effective_permission_mode(mode);
                }
                self.state.resolved_effort = self.state.effort.clone();
                if self.state.skills.is_empty() {
                    if let Some(commands) = value["slash_commands"].as_array() {
                        self.state.skills = (commands
                            .iter()
                            .map(|v| json!({"name":v,"description":"Claude Code command"}))
                            .collect::<Vec<_>>())
                        .into();
                    }
                }
            }
            "control_request" => return self.control_request(&value),
            "control_cancel_request" => {
                if let Some(parent) = value["parent_tool_use_id"]
                    .as_str()
                    .or(value["agent_id"].as_str())
                {
                    if self
                        .state
                        .interaction_owners
                        .get(&value["request_id"].to_string())
                        .is_none_or(|owner| {
                            self.interaction_children.canonical(parent) != owner.thread
                        })
                    {
                        return Ok(());
                    }
                }
                self.state
                    .interaction_owners
                    .remove(&value["request_id"].to_string());
                self.state.questions.retain(|q| q.id != value["request_id"]);
                self.state
                    .elicitations
                    .retain(|q| q.id != value["request_id"]);
                self.state
                    .approvals
                    .retain(|a| a["id"] != value["request_id"]);
                if self.state.approvals.is_empty()
                    && self.state.questions.is_empty()
                    && self.state.elicitations.is_empty()
                    && self.state.status == Status::Waiting
                {
                    self.state.status = Status::Working;
                }
            }
            "rate_limit_event" => {
                let info = &value["rate_limit_info"];
                self.state.account_usage =
                    crate::account_usage::claude(info, crate::account_usage::now());
                match info["status"].as_str() {
                    Some("rejected") => self.state.set_provider_error(
                        "Claude",
                        "Claude usage limit reached.".into(),
                        false,
                        Some(
                            if matches!(
                                info["rateLimitType"].as_str(),
                                Some(
                                    "five_hour"
                                        | "seven_day"
                                        | "seven_day_opus"
                                        | "seven_day_sonnet"
                                        | "overage"
                                )
                            ) {
                                "usage_limit_reached"
                            } else {
                                "rate_limit_error"
                            },
                        ),
                        info["resetsAt"].as_i64().or(info["resets_at"].as_i64()),
                    ),
                    Some("allowed_warning") => self.state.provider_warning(
                        "Approaching usage limit",
                        "Claude reports that usage is approaching a limit. Work can continue.",
                    ),
                    Some("allowed")
                        if self.state.effective_notice().is_some_and(|notice| {
                            notice.provider == "Claude"
                                && (notice.kind == provider::SessionNoticeKind::UsageLimit
                                    || notice.reset_at.is_some())
                        }) =>
                    {
                        self.state.clear_error();
                    }
                    _ => {}
                }
            }
            "stream_event" if value["parent_tool_use_id"].is_null() => self.stream(&value["event"]),
            "assistant" if value["parent_tool_use_id"].is_null() => {
                let message = &value["message"];
                let id = message["id"].as_str().unwrap_or("assistant");
                if let Some(content) = message["content"].as_array() {
                    let mut claimed = Vec::new();
                    for (index, block) in content.iter().enumerate() {
                        // SDK assistant frames can contain only a completed
                        // block, so their array index is not its stream index.
                        let item_id = self.assistant_block_id(id, index, block, &claimed);
                        self.block(&item_id, block);
                        if block["type"] == "text" {
                            if let Some(item) = self
                                .state
                                .saved
                                .items
                                .iter_mut()
                                .find(|item| item["id"] == item_id)
                            {
                                item["groveCompleted"] = json!(true);
                            }
                        }
                        claimed.push(item_id);
                    }
                }
                if let Some(error) = value["error"].as_str() {
                    self.state.set_provider_error(
                        "Claude",
                        format!("Claude Code: {error}"),
                        false,
                        Some(error),
                        None,
                    );
                }
                if let Some(uuid) = value["uuid"].as_str() {
                    self.last_assistant_uuid = Some(uuid.to_owned());
                    if let Some(item) = self.state.saved.items.last_mut() {
                        item["claudeAssistantUuid"] = json!(uuid);
                    }
                }
                let usage = &message["usage"];
                let used = [
                    "input_tokens",
                    "cache_creation_input_tokens",
                    "cache_read_input_tokens",
                    "output_tokens",
                ]
                .iter()
                .filter_map(|key| usage[*key].as_u64())
                .sum::<u64>();
                let model = message["model"].as_str().unwrap_or_default();
                if used > 0 && model != "<synthetic>" {
                    if !model.is_empty() {
                        if self
                            .context_model
                            .as_deref()
                            .is_some_and(|current| current != model)
                        {
                            self.state.saved.context = None;
                        }
                        self.context_model = Some(model.to_owned());
                        self.state.resolved_model = Some(model.to_owned());
                    }
                    self.state.record_context(Some(used), None);
                }
            }
            "user" => {
                self.acknowledge_input(&value["uuid"]);
                if let Some(content) = value["message"]["content"].as_array() {
                    for block in content {
                        if block["type"] == "tool_result" {
                            self.tool_result_with_structured(block, value.get("tool_use_result"));
                        }
                    }
                }
            }
            "tool_progress" => {
                if let Some(item) = self
                    .state
                    .saved
                    .items
                    .iter_mut()
                    .find(|item| item["id"] == value["tool_use_id"])
                {
                    item["progress"] =
                        json!(format!("Running for {}s", value["elapsed_time_seconds"]));
                }
            }
            "result" => return self.turn_result(&value),
            _ => {}
        }
        self.finish_message()
    }

    fn control_response(&mut self, response: &Value) -> Result<(), String> {
        let permission_change = response["request_id"]
            .as_str()
            .and_then(|id| self.permission_changes.remove(id));
        let pending = response["request_id"]
            .as_str()
            .and_then(|id| self.pending.remove(id));
        if pending.is_none() {
            return Ok(());
        }
        if pending.as_deref() == Some("mcp_status") {
            if response["subtype"] == "success" {
                self.observe_mcp_servers(&response["response"]["mcpServers"]);
            } else {
                self.mcp_status_supported = false;
            }
            return Ok(());
        }
        if let Some(revision) = pending
            .as_deref()
            .and_then(|value| value.strip_prefix("get_context_usage:"))
        {
            self.state.context_query_pending = self
                .pending
                .values()
                .any(|kind| kind.starts_with("get_context_usage:"));
            let current = format!(
                "{}:{}",
                self.state.context_revision,
                self.state.resolved_model.as_deref().unwrap_or_default()
            );
            if revision != current {
                return Ok(());
            }
            if response["subtype"] == "success" {
                let data = &response["response"];
                if let Some((usage, details)) = context_measurement(data) {
                    self.state
                        .record_context(Some(usage.used), Some(usage.window));
                    self.state.saved.context_details = Some(details);
                    self.state.context_query_error = None;
                } else {
                    self.state.context_query_error =
                        Some("Claude did not return a usable context measurement.".into());
                }
            } else {
                self.state.context_query_error = Some(
                    response["error"]
                        .as_str()
                        .unwrap_or("Context details are unavailable with this Claude version.")
                        .into(),
                );
            }
        } else if response["subtype"] == "error"
            && pending.as_deref() == Some("set_permission_mode")
        {
            // A startup selection is sent before the launch prompt is
            // drained into `deferred`. Cancel it on rejection as well;
            // it must never run under the previous permission mode.
            self.initial = None;
            if let Some(prompt) = self.deferred.take() {
                self.state.rejected_submission = Some(prompt.id);
                self.state.queue_sending.retain(|id| *id != prompt.id);
            }
            self.queue_ready = false;
            let error = response["error"]
                .as_str()
                .unwrap_or("Claude Code rejected the permission mode")
                .to_owned();
            self.state.error = Some(error.clone());
            self.permission_error = Some(error);
        } else if response["subtype"] == "error" {
            return Err(response["error"]
                .as_str()
                .unwrap_or("Claude Code rejected a control request")
                .into());
        }
        if response["subtype"] == "success" && pending.as_deref() == Some("set_permission_mode") {
            if let Some((requested, revision)) = permission_change {
                // Status can precede its control ACK. Keep its authoritative
                // effective mode while persisting the acknowledged explicit choice.
                let access = if revision == self.permission_mode_revision {
                    requested
                } else {
                    self.state.resolved_access.clone().unwrap_or(requested)
                };
                self.state.resolved_access = Some(access.clone());
                self.state.access = Some(access.clone());
                self.state.saved.composer.access = Some(access);
                if self
                    .permission_error
                    .take()
                    .is_some_and(|error| self.state.error.as_ref() == Some(&error))
                {
                    self.state.clear_error();
                }
            }
        }
        if self.configuring
            && !self
                .pending
                .values()
                .any(|kind| kind != "mcp_status" && !kind.starts_with("get_context_usage:"))
        {
            self.configuring = false;
            self.state.status = Status::Idle;
            if self.state.model.is_some() {
                self.state.resolved_model = self.state.model.clone();
            }

            self.state.resolved_effort = self.state.effort.clone();
        }
        if pending.as_deref() == Some("initialize") {
            self.finish_edit()?;
            self.initialized = true;
            self.state.reconnecting = false;
            tracing::info!(
                stage = "initialized",
                elapsed_ms = self.started.elapsed().as_millis() as u64,
                "Claude Code startup"
            );
            self.state.status = Status::Idle;
            let data = &response["response"];
            self.tracks_session_state = data.get("session_state").is_some();
            self.session_idle = data["session_state"] == "idle";
            if let Some(mode) = data["current_permission_mode"].as_str() {
                self.effective_permission_mode(mode);
            }
            self.state.resolved_model = data["models"]
                .as_array()
                .and_then(|models| models.iter().find(|m| m["value"] == "default"))
                .and_then(|m| m["resolvedModel"].as_str())
                .map(str::to_owned);

            if let Some(models) = data["models"].as_array() {
                self.state.models=(models.iter().map(|m| json!({"model":m["value"],"resolvedModel":m["resolvedModel"],"displayName":m["displayName"],"description":m["description"],"supportsFastMode":m["supportsFastMode"],"supportedReasoningEfforts":m["supportedEffortLevels"].as_array().map(|values| values.iter().map(|v| json!({"reasoningEffort":v})).collect::<Vec<_>>()).unwrap_or_default()})).collect::<Vec<_>>()).into();
            }
            if let Some(key) = &self.model_catalog_key {
                crate::model_catalog::remember(key, &self.state.models);
            }
            if self.state.saved.composer.speed == Some(provider::Speed::Fast) {
                self.reconfigure = true;
            }
            if let Some(speed) = data["fast_mode_state"].as_str() {
                self.state.reported_speed = Some(speed.into());
                self.state.speed_disabled_reason = data["fast_mode_disabled_reason"]
                    .as_str()
                    .map(str::to_owned);
            }
            if let Some(commands) = data["commands"].as_array() {
                self.state.skills = (commands
                    .iter()
                    .map(|v| json!({"name":v["name"],"description":v["description"]}))
                    .collect::<Vec<_>>())
                .into();
            }
        }
        self.finish_message()
    }

    fn control_request(&mut self, value: &Value) -> Result<(), String> {
        let id = value["request_id"].clone();
        let request = &value["request"];
        if self.side_chat {
            if request["subtype"] == "can_use_tool" {
                let allowed = matches!(
                    request["tool_name"].as_str(),
                    Some("Read" | "Glob" | "Grep")
                );
                return self.respond(id, if allowed {
                    json!({"behavior":"allow","updatedInput":request["input"]})
                } else {
                    json!({"behavior":"deny","message":"Side chat only supports Read, Glob and Grep."})
                });
            }
            return self.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Side chat does not expose host interactions."}}));
        }
        let owner = self.control_owner(value);
        if owner.is_none()
            && matches!(
                request["subtype"].as_str(),
                Some("can_use_tool" | "elicitation")
            )
        {
            return self.respond(id, if request["subtype"] == "elicitation" {json!({"action":"cancel"})} else {json!({"behavior":"deny","message":"This request has expired or its agent is not part of this session."})});
        }
        if request["subtype"] == "elicitation" {
            if self.state.elicitations.iter().any(|r| r.id == id) {
                return Ok(());
            }
            match crate::elicitation::ElicitationRequest::parse_claude(id.clone(), request) {
                Ok(r) if owner.is_some() && !self.interrupting => {
                    if let Some(owner) = owner {
                        self.state.interaction_owners.insert(id.to_string(), owner);
                    }
                    self.state.elicitations.push(r);
                    self.state.status = Status::Waiting;
                }
                _ => {
                    self.respond(id, json!({"action":"cancel"}))?;
                    self.state.error=Some("Claude requested an invalid or unsupported MCP form. Use the terminal for this request.".into());
                }
            }
            return Ok(());
        }
        if request["subtype"] != "can_use_tool" {
            let subtype = request["subtype"].as_str().unwrap_or("unknown");
            self.state.error = Some(format!(
                "Claude requested unsupported host interaction: {subtype}. Use a terminal session for this capability."
            ));
            if subtype == "request_user_dialog" {
                self.state.error = Some(format!(
                    "Claude requested unsupported dialog: {}. Grove does not advertise this dialog kind; use a terminal session.",
                    request["dialog_kind"].as_str().unwrap_or("unknown")
                ));
                // Undeclared dialogs can belong to another attached client.
                // A cancellation would falsely settle that client's question.
                // Claude ignores error-shaped responses and keeps it pending.
                return self.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Grove does not advertise this dialog kind."}}));
            }
            return self.write(json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Grove does not register this host capability. Use the Claude CLI for this interaction."}}));
        }
        if self.state.approvals.iter().any(|a| a["id"] == id) {
            return Ok(());
        }
        if owner.is_none() || self.interrupting {
            return self.respond(
                id,
                json!({"behavior":"deny","message":"This request has expired."}),
            );
        }
        // This exact private tool only records UI metadata and cannot
        // access files, commands or permissions. Other tools retain the
        // user's existing permission decisions.
        if request["tool_name"] == "mcp__grove_activity__report_activity"
            && crate::activity::validate_report(&request["input"]).is_ok()
        {
            return self.respond(
                id,
                json!({"behavior":"allow","updatedInput":request["input"]}),
            );
        }
        // Private visual tools enforce host-read permissions and root ownership
        // in their own session-bound endpoint; they grant no provider capability.
        if matches!(
            request["tool_name"].as_str(),
            Some("mcp__grove_visuals__render_html" | "mcp__grove_visuals__preview_html")
        ) && serde_json::from_value::<crate::visuals::Input>(request["input"].clone()).is_ok()
        {
            return self.respond(
                id,
                json!({"behavior":"allow","updatedInput":request["input"]}),
            );
        }
        if request["tool_name"] == "AskUserQuestion" {
            if self.state.questions.iter().any(|q| q.id == id) {
                return Ok(());
            }
            match QuestionRequest::parse(id.clone(), &request["input"], true) {
                Ok(question) if owner.is_some() && !self.interrupting => {
                    if let Some(owner) = owner {
                        self.state.interaction_owners.insert(id.to_string(), owner);
                    }
                    self.state.questions.push(question);
                    self.state.status = Status::Waiting;
                    return Ok(());
                }
                _ => {
                    return self.respond(
                        id,
                        json!({"behavior":"deny","message":"Invalid or expired question request."}),
                    )
                }
            }
        }
        let agent = owner.as_ref().and_then(|owner| owner.agent.clone());
        if let Some(owner) = owner {
            self.state.interaction_owners.insert(id.to_string(), owner);
        }
        self.state.approvals.push(json!({"groveAgent":agent,"id":id,"method":"item/commandExecution/requestApproval","claudeInput":request["input"],"claudeRequest":request,"params":{"command":format!("{} {}", request["tool_name"].as_str().unwrap_or("Tool"), request["input"]),"cwd":self.state.saved.cwd,"reason":request["decision_reason"].as_str().unwrap_or("Claude Code requests permission for this action."),"itemId":request["tool_use_id"]}}));
        self.state.status = Status::Waiting;
        self.finish_message()
    }

    fn turn_result(&mut self, value: &Value) -> Result<(), String> {
        let result_failed = value["is_error"] == true || value["terminal_reason"] == "api_error";
        if let Some(models) = value["modelUsage"].as_object() {
            let window = self
                .context_model
                .as_deref()
                .and_then(|model| models.get(model))
                .and_then(|entry| entry["contextWindow"].as_u64())
                .or_else(|| {
                    // Multiple entries can include subagents; never borrow a
                    // larger child's capacity for the main conversation.
                    if models.len() == 1 {
                        models
                            .values()
                            .next()
                            .and_then(|entry| entry["contextWindow"].as_u64())
                    } else {
                        None
                    }
                });
            // A full query can report a lower effective compaction window.
            // A result's model capacity must not replace that denominator.
            if !self
                .state
                .saved
                .context_details
                .as_ref()
                .is_some_and(|details| details.source == provider::ContextMeasurement::Full)
            {
                self.state.record_context(None, window);
            }
        }
        let aborted = value["terminal_reason"]
            .as_str()
            .is_some_and(|reason| reason.starts_with("aborted"));
        // Claude Code reports queued_turn_count 0 even while a steer
        // waits, so an unechoed steer is the reliable signal that this
        // result closes an intermediate turn and the steer's turn follows.
        let steer_unacknowledged =
            self.redirect_pending && self.in_flight.is_some() && !self.interrupting;
        let queued = value["queued_turn_count"].as_u64().unwrap_or(0) > 0
            && (self.redirect_pending || value["terminal_reason"] == "aborted_tools");
        if self.state.turn_id.is_some() && (steer_unacknowledged || queued) {
            for item in &mut self.state.saved.items {
                if item["status"] == "inProgress" {
                    item["status"] = json!(if aborted { "interrupted" } else { "completed" });
                }
            }
            self.state.approvals.clear();
            self.state.questions.clear();
            self.state.elicitations.clear();
            self.state.status = Status::Working;
            return Ok(());
        }
        self.redirect_pending = false;
        if self.state.turn_id.is_none() {
            return Ok(());
        }
        self.state.compacting = false;
        if self.in_flight.is_some() {
            if !self.interrupting {
                return Err("Claude Code finished before acknowledging the submitted message; the draft was preserved.".into());
            }
            // An immediate interrupt can finish a turn before the SDK
            // echoes its input. Settle without acknowledging delivery;
            // retain queued input and any composer edits for retry.
            self.state.mark_queue_uncertain();
            if let Some(prompt) = self.in_flight.take() {
                self.state.rejected_submission = Some(prompt.id);
                if !self
                    .state
                    .queued_prompts
                    .iter()
                    .any(|queued| queued.id == prompt.id)
                    && self.state.saved.draft.is_empty()
                    && self.state.saved.draft_images.is_empty()
                    && self.state.saved.draft_reference.is_none()
                {
                    self.state.saved.draft = prompt.text;
                    self.state.saved.draft_images = prompt.images;
                }
            }
            self.input_uuid = None;
            self.inline_uuid = None;
            self.queue_ready = false;
        }
        self.state.turn_id = None;
        let root = self.state.saved.thread_id.clone().unwrap_or_default();
        let owners = &self.state.interaction_owners;
        let child = |id: &Value| {
            owners
                .get(&id.to_string())
                .is_some_and(|owner| owner.thread != root)
        };
        self.state.approvals.retain(|a| child(&a["id"]));
        self.state.questions.retain(|q| child(&q.id));
        self.state.elicitations.retain(|r| child(&r.id));
        self.state
            .interaction_owners
            .retain(|_, owner| owner.thread != root);
        self.state.status = if self.interrupting {
            Status::Interrupted
        } else if result_failed {
            Status::Failed
        } else {
            Status::Done
        };
        if result_failed && !self.interrupting {
            // The terminal result often has a generic subtype. Retain
            // a preceding explicit rejection's category/reset metadata.
            let usage_notice = self
                .state
                .effective_notice()
                .filter(|notice| notice.kind == provider::SessionNoticeKind::UsageLimit)
                .cloned();
            self.state.error = Some(
                value["errors"]
                    .as_array()
                    .map(|errors| {
                        errors
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        value["result"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    })
                    .or_else(|| self.state.error.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "Claude Code ended with {}",
                            value["terminal_reason"]
                                .as_str()
                                .or_else(|| value["subtype"].as_str())
                                .unwrap_or("an unknown error")
                        )
                    }),
            );
            if let Some(body) = self.state.error.clone() {
                self.state.set_provider_error(
                    "Claude",
                    body,
                    false,
                    if usage_notice.is_some() {
                        Some("usage_limit_reached")
                    } else {
                        value["subtype"].as_str()
                    },
                    usage_notice.and_then(|notice| notice.reset_at),
                );
            }
            self.queue_ready = false;
        } else if !result_failed && self.state.effective_notice().is_some() {
            self.state.clear_error();
        }
        if !result_failed && !self.interrupting {
            crate::rich_plan::import_completed(&mut self.state.saved);
        }
        if self.tracks_session_state && !self.session_idle {
            self.pending_completion = Some(self.state.status);
            self.state.status = Status::Working;
            // result can be flushed before background continuations finish;
            // session_state_changed idle is the authoritative boundary.
            self.state.turn_id = Some("awaiting-session-idle".into());
        } else {
            self.settle_root_items(self.state.status);
            self.state.finish_work();
            if self.state.status == Status::Done {
                self.state.completed_for_review();
            }
            self.pending_completion = None;
            self.interrupting = false;
        }
        self.finish_message()
    }

    fn finish_message(&mut self) -> Result<(), String> {
        self.state.refresh_interaction_status();
        self.configure_pending_access()
    }
    /// Only an authoritative terminal boundary may settle still-open root tools.
    /// Real tool results arriving before idle retain their own status and output.
    fn settle_root_items(&mut self, status: Status) {
        let terminal = match status {
            Status::Interrupted => "interrupted",
            Status::Failed => "failed",
            _ => "completed",
        };
        for item in &mut self.state.saved.items {
            if item["status"] == "inProgress" {
                item["status"] = json!(terminal);
            }
        }
    }
    pub(super) fn assistant_block_id(
        &self,
        message: &str,
        index: usize,
        block: &Value,
        claimed: &[String],
    ) -> String {
        let fallback = format!("{message}-{index}");
        let kind = match block["type"].as_str() {
            Some("text") => "agentMessage",
            Some("thinking") => "reasoning",
            _ => return fallback,
        };
        if self.message_id != message {
            return fallback;
        }
        let text = block[if kind == "reasoning" {
            "thinking"
        } else {
            "text"
        }]
        .as_str()
        .unwrap_or_default();
        let mut candidates = self
            .blocks
            .iter()
            .filter_map(|(stream_index, id)| {
                if claimed.contains(id) {
                    return None;
                }
                self.state
                    .saved
                    .items
                    .iter()
                    .find(|item| item["id"] == *id && item["type"] == kind)
                    .map(|item| (*stream_index, id, item))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(stream_index, _, _)| *stream_index);
        let content = |item: &Value| -> String {
            if kind == "reasoning" {
                item["content"][0].as_str().unwrap_or_default().to_owned()
            } else {
                item["text"].as_str().unwrap_or_default().to_owned()
            }
        };
        if let Some((_, id, _)) = candidates.iter().find(|(_, _, item)| content(item) == text) {
            return (*id).clone();
        }
        if let Some((_, id, _)) = candidates.iter().find(|(_, _, item)| {
            let streamed = content(item);
            !streamed.is_empty() && (text.starts_with(&streamed) || streamed.starts_with(text))
        }) {
            return (*id).clone();
        }
        if let Some((_, id, _)) = candidates
            .iter()
            .find(|(stream_index, _, _)| *stream_index == index as u64)
        {
            return (*id).clone();
        }
        if candidates.len() == 1 {
            return candidates[0].1.clone();
        }
        fallback
    }
    pub(super) fn block(&mut self, id: &str, block: &Value) {
        match block["type"].as_str().unwrap_or_default() {
            "text" => self.upsert(json!({"id":id,"type":"agentMessage","text":block["text"]})),
            "thinking" => self.upsert(
                json!({"id":id,"type":"reasoning","summary":[],"content":[block["thinking"]]}),
            ),
            "tool_use" => {
                self.register_subagent(block);
                let tool_id = block["id"].as_str().unwrap_or(id);
                let old = self
                    .state
                    .saved
                    .items
                    .iter()
                    .find(|i| i["id"] == tool_id)
                    .cloned();
                let mut item = if block["name"] == "Bash" {
                    json!({"id":tool_id,"type":"commandExecution","command":block["input"]["command"],"cwd":self.state.saved.cwd,"status":"inProgress","aggregatedOutput":""})
                } else {
                    json!({"id":tool_id,"type":"mcpToolCall","server":"Claude Code","tool":block["name"],"arguments":block["input"],"status":"inProgress"})
                };
                if let Some(old) = old {
                    for key in [
                        "status",
                        "result",
                        "error",
                        "aggregatedOutput",
                        "progress",
                        "structuredResult",
                        "plan",
                        "text",
                        "detached",
                    ] {
                        if let Some(value) = old.get(key) {
                            item[key] = value.clone();
                        }
                    }
                }
                self.transcript.normalize_tool_item(&mut item);
                self.upsert(item);
            }
            _ => {}
        }
    }
    pub(super) fn stream(&mut self, event: &Value) {
        let index = event["index"].as_u64().unwrap_or(0);
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => {
                self.message_id = event["message"]["id"].as_str().unwrap_or("stream").into();
                self.blocks.clear();
            }
            "content_block_start" => {
                let block = &event["content_block"];
                let id = format!("{}-{index}", self.message_id);
                let item_id = if block["type"] == "tool_use" {
                    block["id"].as_str().unwrap_or(&id).to_owned()
                } else {
                    id.clone()
                };
                self.block(&id, block);
                self.blocks.insert(index, item_id);
            }
            "content_block_stop" => {
                if let Some(id) = self.blocks.get(&index) {
                    if let Some(item) = self
                        .state
                        .saved
                        .items
                        .iter_mut()
                        .find(|item| item["id"] == *id && item["type"] == "agentMessage")
                    {
                        item["groveCompleted"] = json!(true);
                    }
                }
            }
            "content_block_delta" => {
                let Some(id) = self.blocks.get(&index) else {
                    return;
                };
                let Some(item) = self
                    .state
                    .saved
                    .items
                    .iter_mut()
                    .rev()
                    .find(|item| item["id"] == *id)
                else {
                    return;
                };
                let delta = &event["delta"];
                if let Some(text) = delta["text"].as_str() {
                    provider::append_json_text(&mut item["text"], text);
                }
                if let Some(text) = delta["thinking"].as_str() {
                    if !item["content"].is_array()
                        || item["content"].as_array().is_some_and(Vec::is_empty)
                    {
                        item["content"] = json!([""]);
                    }
                    provider::append_json_text(&mut item["content"][0], text);
                }
                // Final assistant messages carry parsed tool input. Do not show
                // partial JSON as a command that could be mistaken for complete.
            }
            _ => {}
        }
    }
    #[cfg(test)]
    pub(super) fn tool_result(&mut self, block: &Value) {
        self.tool_result_with_structured(block, None);
    }
    fn tool_result_with_structured(&mut self, block: &Value, structured: Option<&Value>) {
        self.subagent_tool_result(block, structured);
        let Some(item) = self
            .state
            .saved
            .items
            .iter_mut()
            .find(|item| item["id"] == block["tool_use_id"])
        else {
            return;
        };
        let content = if let Some(text) = block["content"].as_str() {
            vec![json!({"type":"text","text":text})]
        } else {
            block["content"].as_array().cloned().unwrap_or_default()
        };
        self.transcript.apply_tool_result(
            item,
            &json!(content),
            structured,
            block["is_error"] == true,
        );
        if item["type"] == "commandExecution" {
            item["aggregatedOutput"] = json!(content
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"));
        }
    }
}

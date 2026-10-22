//! Provider-offered approval choices. Never manufacture policy grants from UI data.
use serde_json::{json, Value};
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub struct ApprovalChoice {
    pub id: String,
    pub label: String,
}
fn choice(id: &str, label: &str) -> ApprovalChoice {
    ApprovalChoice {
        id: id.into(),
        label: label.into(),
    }
}
pub fn approval_title(a: &Value) -> String {
    let title = approval_action_title(a);
    a["groveAgent"]
        .as_str()
        .map_or(title.clone(), |agent| format!("{agent}: {title}"))
}
fn approval_action_title(a: &Value) -> String {
    if let Some(name) = a["claudeRequest"]["tool_name"].as_str() {
        if name == "ExitPlanMode" {
            return "Approve plan and continue".into();
        }
        return format!(
            "Allow {}?",
            a["claudeRequest"]["display_name"].as_str().unwrap_or(name)
        );
    }
    match a["method"].as_str().unwrap_or_default() {
        "item/permissions/requestApproval" => "Additional permissions".into(),
        "item/fileChange/requestApproval" | "applyPatchApproval" => "Allow file changes?".into(),
        _ if a["params"]["kind"] == "writeStdin" => "Allow terminal input?".into(),
        _ => "Allow command?".into(),
    }
}
pub fn approval_details(a: &Value) -> String {
    let p = &a["params"];
    let mut lines = Vec::new();
    if let Some(agent) = a["groveAgent"].as_str() {
        lines.push(format!("Agent: {agent}"));
        lines.push(if a.get("claudeInput").is_some() {"Permission updates keep the provider’s destination; session grants also apply to the main conversation."} else {"Turn and session grants apply to the requesting agent’s provider thread."}.into());
    }
    for key in ["reason", "command", "cwd", "grantRoot"] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            lines.push(format!(
                "{key}: {}",
                v.as_str().map_or_else(|| v.to_string(), str::to_owned)
            ));
        }
    }
    for key in [
        "permissions",
        "additionalPermissions",
        "networkApprovalContext",
        "fileChanges",
        "proposedExecpolicyAmendment",
        "proposedNetworkPolicyAmendments",
    ] {
        if let Some(v) = p.get(key).filter(|v| !v.is_null()) {
            lines.push(format!(
                "{key}: {}",
                serde_json::to_string_pretty(v).unwrap_or_default()
            ));
        }
    }
    if a.get("claudeRequest").is_some() {
        let r = &a["claudeRequest"];
        for key in ["title", "description", "blocked_path", "decision_reason"] {
            if let Some(s) = r[key].as_str() {
                lines.push(format!("{key}: {s}"));
            }
        }
        lines.push(serde_json::to_string_pretty(&a["claudeInput"]).unwrap_or_default());
        if let Some(suggestions) = r["permission_suggestions"].as_array() {
            for (i, update) in suggestions.iter().enumerate() {
                lines.push(format!(
                    "Permission update {}:\n{}",
                    i + 1,
                    serde_json::to_string_pretty(update).unwrap_or_default()
                ));
            }
        }
    }
    lines.join("\n")
}
/// Structured command data is presentation only; policy responses always use the original request.
pub fn approval_command(a: &Value) -> Option<&str> {
    if a["claudeRequest"]["tool_name"] == "Bash" {
        a["claudeInput"]["command"].as_str()
    } else if a.get("claudeInput").is_none()
        && matches!(
            a["method"].as_str(),
            Some("item/commandExecution/requestApproval" | "execCommandApproval")
        )
    {
        a["params"]["command"].as_str()
    } else {
        None
    }
}

/// Keep unfamiliar fields visible without repeating structured command fields or policy JSON.
pub fn approval_command_extra(a: &Value) -> String {
    let mut remainder = a.clone();
    if let Some(p) = remainder.get_mut("params").and_then(Value::as_object_mut) {
        for key in ["reason", "command", "cwd"] {
            if p.get(key).is_some_and(Value::is_string) {
                p.remove(key);
            }
        }
        if approval_choices(a)
            .iter()
            .any(|choice| choice.id == "execpolicy")
        {
            p.remove("proposedExecpolicyAmendment");
        }
    }
    if a["claudeRequest"]["tool_name"] == "Bash" {
        if let Some(input) = remainder
            .get_mut("claudeInput")
            .and_then(Value::as_object_mut)
        {
            input.remove("command");
        }
    }
    let mut details = approval_details(&remainder);
    // Provider routing metadata is already represented by the tray identity.
    if let Some(p) = remainder["params"].as_object() {
        for (key, value) in p {
            if !matches!(
                key.as_str(),
                "permissions"
                    | "additionalPermissions"
                    | "networkApprovalContext"
                    | "fileChanges"
                    | "proposedExecpolicyAmendment"
                    | "proposedNetworkPolicyAmendments"
                    | "grantRoot"
                    | "availableDecisions"
                    | "threadId"
                    | "turnId"
                    | "itemId"
                    | "callId"
            ) && !value.is_null()
            {
                let _ = write!(details, "\n{key}: {value}");
            }
        }
    }
    details.trim().to_owned()
}

pub fn approval_choices(a: &Value) -> Vec<ApprovalChoice> {
    let mut c = Vec::new();
    if a.get("claudeInput").is_some() {
        c.push(choice("allow_once", "Allow once"));
        if a["claudeRequest"]["suppress_always_allow_rule"] != true {
            if let Some(s) = a["claudeRequest"]["permission_suggestions"].as_array() {
                for (i, _) in s.iter().enumerate() {
                    c.push(choice(
                        &format!("claude_suggestion_{i}"),
                        &format!("Allow with permission update {}", i + 1),
                    ));
                }
            }
        }
    } else if a["method"] == "item/permissions/requestApproval" {
        c.push(choice("permission_turn", "Allow for this turn"));
        c.push(choice("permission_session", "Allow for this session"));
    } else {
        c.push(choice("allow_once", "Allow once"));
        c.push(choice("allow_session", "Allow for this session"));
        if a["method"] == "item/commandExecution/requestApproval" {
            let p = &a["params"];
            if p["proposedExecpolicyAmendment"]
                .as_array()
                .is_some_and(|v| !v.is_empty())
            {
                c.push(choice("execpolicy", "Allow matching command prefix"));
            }
            if let Some(rules) = p["proposedNetworkPolicyAmendments"].as_array() {
                for (i, r) in rules.iter().enumerate() {
                    if r["host"].is_string()
                        && matches!(r["action"].as_str(), Some("allow" | "deny"))
                    {
                        c.push(choice(
                            &format!("network_{i}"),
                            &format!("Apply network rule {}", i + 1),
                        ));
                    }
                }
            }
        }
    }
    c.push(choice("deny", "Decline and continue"));
    if !(a["groveAgent"].is_string() && a.get("claudeInput").is_some()) {
        c.push(choice("cancel", "Decline and stop"));
    }
    if let Some(available) = a["params"]["availableDecisions"].as_array() {
        c.retain(|choice| {
            let value = codex_decision(&a["params"], &choice.id);
            available.contains(&value)
        });
    }
    c
}
fn codex_decision(p: &Value, id: &str) -> Value {
    match id {
        "allow_once"=>json!("accept"),"allow_session"=>json!("acceptForSession"),"deny"=>json!("decline"),"cancel"=>json!("cancel"),
        "execpolicy"=>json!({"acceptWithExecpolicyAmendment":{"execpolicy_amendment":p["proposedExecpolicyAmendment"]}}),
        n if n.starts_with("network_")=>n[8..].parse::<usize>().ok().map_or(Value::Null,|i|json!({"applyNetworkPolicyAmendment":{"network_policy_amendment":p["proposedNetworkPolicyAmendments"][i]}})),
        _=>Value::Null,
    }
}
/// Returns the exact provider response for an offered choice; caller routes the envelope.
pub fn approval_response(
    a: &Value,
    id: &str,
    input: Option<Value>,
    feedback: Option<String>,
) -> Result<Value, String> {
    if !approval_choices(a).iter().any(|c| c.id == id) {
        return Err("This approval choice is not offered".into());
    }
    if a.get("claudeInput").is_some() {
        if matches!(id, "deny" | "cancel") {
            return Ok(
                json!({"behavior":"deny","message":feedback.filter(|s|!s.trim().is_empty()).unwrap_or_else(||"The user declined this action.".into()),"interrupt":id=="cancel"}),
            );
        }
        let input = input.unwrap_or_else(|| a["claudeInput"].clone());
        if !input.is_object() {
            return Err("Tool input must be a JSON object".into());
        }
        let mut result = json!({"behavior":"allow","updatedInput":input});
        if let Some(n) = id
            .strip_prefix("claude_suggestion_")
            .and_then(|n| n.parse::<usize>().ok())
        {
            result["updatedPermissions"] =
                json!([a["claudeRequest"]["permission_suggestions"][n].clone()]);
        }
        return Ok(result);
    }
    if input.is_some() || feedback.as_ref().is_some_and(|s| !s.is_empty()) {
        return Err("This provider does not support edited approval input or feedback".into());
    }
    let p = &a["params"];
    if a["method"] == "item/permissions/requestApproval" {
        return Ok(if matches!(id, "permission_turn" | "permission_session") {
            json!({"permissions":p["permissions"],"scope":if id=="permission_session"{"session"}else{"turn"}})
        } else {
            json!({"permissions":{},"scope":"turn"})
        });
    }
    let decision = match id {
        "allow_once" => json!("accept"),
        "allow_session" => json!("acceptForSession"),
        "deny" => json!("decline"),
        "cancel" => json!("cancel"),
        "execpolicy" => {
            json!({"acceptWithExecpolicyAmendment":{"execpolicy_amendment":p["proposedExecpolicyAmendment"]}})
        }
        n if n.starts_with("network_") => {
            let n = n[8..]
                .parse::<usize>()
                .map_err(|_| "Invalid network rule")?;
            json!({"applyNetworkPolicyAmendment":{"network_policy_amendment":p["proposedNetworkPolicyAmendments"][n]}})
        }
        _ => return Err("Invalid approval choice".into()),
    };
    if matches!(
        a["method"].as_str(),
        Some("execCommandApproval" | "applyPatchApproval")
    ) {
        if id == "deny" {
            return Ok(
                json!({"decision":{"denied":{"rejection":"The user declined this action."}}}),
            );
        }
        let legacy = match id {
            "allow_once" => "approved",
            "allow_session" => "approved_for_session",
            "cancel" => "abort",
            _ => return Err("Invalid legacy choice".into()),
        };
        return Ok(json!({"decision":legacy}));
    }
    Ok(json!({"decision":decision}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn structured_command_preserves_unfamiliar_and_unoffered_policy_data() {
        let a = json!({"method":"item/commandExecution/requestApproval","params":{
            "command":"cargo test <literal>","cwd":"/tmp/project","reason":"Run tests",
            "threadId":"routing-only","customContext":{"important":true},
            "proposedExecpolicyAmendment":["cargo","test"],"availableDecisions":["accept","decline"]
        }});
        assert_eq!(approval_command(&a), Some("cargo test <literal>"));
        assert!(approval_command(&json!({"method":"item/commandExecution/requestApproval","claudeInput":{"path":"/tmp"},"claudeRequest":{"tool_name":"Read"},"params":{"command":"Read {path}"}})).is_none());
        let extra = approval_command_extra(&a);
        assert!(extra.contains("customContext"));
        assert!(extra.contains("proposedExecpolicyAmendment"));
        assert!(!extra.contains("routing-only"));
        assert!(!extra.contains("Run tests"));
        let mut offered = a.clone();
        offered["params"]
            .as_object_mut()
            .unwrap()
            .remove("availableDecisions");
        assert!(!approval_command_extra(&offered).contains("proposedExecpolicyAmendment"));
        offered["params"]["reason"] = json!({"future":"shape"});
        assert!(approval_command_extra(&offered).contains("future"));
    }

    #[test]
    fn command_decisions_use_exact_offered_policies_and_reject_forgery() {
        let a = json!({"method":"item/commandExecution/requestApproval","params":{"kind":"writeStdin","proposedExecpolicyAmendment":["cargo","test"],"proposedNetworkPolicyAmendments":[{"host":"example.com","action":"allow"}]}});
        assert_eq!(approval_title(&a), "Allow terminal input?");
        assert_eq!(
            approval_response(&a, "execpolicy", None, None).unwrap(),
            json!({"decision":{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":["cargo","test"]}}})
        );
        assert_eq!(
            approval_response(&a, "network_0", None, None).unwrap(),
            json!({"decision":{"applyNetworkPolicyAmendment":{"network_policy_amendment":{"host":"example.com","action":"allow"}}}})
        );
        assert!(approval_response(&a, "network_1", None, None).is_err());
        let mut restricted = a.clone();
        restricted["params"]["availableDecisions"] = json!(["accept", "decline"]);
        assert_eq!(
            approval_choices(&restricted)
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["allow_once", "deny"]
        );
        assert!(approval_response(&restricted, "allow_session", None, None).is_err());
    }
    #[test]
    fn file_permissions_and_legacy_have_distinct_wire_contracts() {
        let file = json!({"method":"item/fileChange/requestApproval","params":{"grantRoot":"/tmp/project"}});
        assert!(approval_details(&file).contains("/tmp/project"));
        assert_eq!(
            approval_response(&file, "allow_session", None, None).unwrap(),
            json!({"decision":"acceptForSession"})
        );
        let requested = json!({"network":{"enabled":true},"fileSystem":{"read":["/tmp/read"],"write":["/tmp/write"]}});
        let permission =
            json!({"method":"item/permissions/requestApproval","params":{"permissions":requested}});
        assert_eq!(
            approval_response(&permission, "permission_turn", None, None).unwrap(),
            json!({"permissions":requested,"scope":"turn"})
        );
        assert_eq!(
            approval_response(&permission, "permission_session", None, None).unwrap()["scope"],
            "session"
        );
        assert_eq!(
            approval_response(&permission, "deny", None, None).unwrap(),
            json!({"permissions":{},"scope":"turn"})
        );
        assert!(approval_response(&permission, "allow_session", None, None).is_err());
        let legacy = json!({"method":"execCommandApproval","params":{}});
        assert_eq!(
            approval_response(&legacy, "allow_once", None, None).unwrap(),
            json!({"decision":"approved"})
        );
        assert_eq!(
            approval_response(&legacy, "deny", None, None).unwrap(),
            json!({"decision":{"denied":{"rejection":"The user declined this action."}}})
        );
    }
    #[test]
    fn claude_edits_feedback_and_permissions_are_explicit() {
        let a = json!({"claudeInput":{"plan":"initial"},"claudeRequest":{"tool_name":"ExitPlanMode","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}});
        assert_eq!(approval_title(&a), "Approve plan and continue");
        assert_eq!(
            approval_response(&a, "allow_once", Some(json!({"plan":"edited"})), None).unwrap(),
            json!({"behavior":"allow","updatedInput":{"plan":"edited"}})
        );
        assert_eq!(
            approval_response(&a, "claude_suggestion_0", None, None).unwrap()["updatedPermissions"],
            json!([{"type":"setMode","mode":"acceptEdits","destination":"session"}])
        );
        assert_eq!(
            approval_response(&a, "cancel", None, Some("Revise it".into())).unwrap(),
            json!({"behavior":"deny","message":"Revise it","interrupt":true})
        );
        assert!(approval_response(&a, "allow_once", Some(json!("not an object")), None).is_err());
        let mut suppressed = a;
        suppressed["claudeRequest"]["suppress_always_allow_rule"] = json!(true);
        assert!(approval_response(&suppressed, "claude_suggestion_0", None, None).is_err());
    }
}

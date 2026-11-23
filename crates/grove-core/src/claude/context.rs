//! Normalization of optional Claude context-window diagnostics.

use crate::provider;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// Normalize the optional Claude diagnostic payload without treating absent data as zero.
pub(super) fn context_measurement(
    data: &Value,
) -> Option<(provider::ContextUsage, provider::ContextDetails)> {
    use provider::{
        ContextCategory, ContextCategoryKind, ContextDetails, ContextEntry, ContextMeasurement,
        ContextUsage,
    };
    let usage = ContextUsage {
        used: data["totalTokens"].as_u64()?,
        window: data["maxTokens"].as_u64().filter(|window| *window > 0)?,
    };
    let entries = |values: &Value, label: &str, loaded: Option<bool>| -> Vec<ContextEntry> {
        values
            .as_array()
            .into_iter()
            .flatten()
            .filter(|entry| {
                loaded.is_none_or(|loaded| entry["isLoaded"].as_bool().unwrap_or(true) == loaded)
            })
            .filter_map(|entry| {
                let tokens = entry["tokens"].as_u64()?;
                let name = entry[label].as_str()?;
                let name = entry["serverName"]
                    .as_str()
                    .filter(|server| *server != name)
                    .map_or_else(|| name.to_owned(), |server| format!("{server} / {name}"));
                Some(ContextEntry {
                    label: name,
                    tokens,
                })
            })
            .collect()
    };
    let categories = data["categories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let label = row["name"].as_str()?.to_owned();
            let tokens = row["tokens"].as_u64()?;
            let id = row["id"].as_str().map_or_else(
                || label.to_ascii_lowercase().replace([' ', '-'], "_"),
                str::to_owned,
            );
            let kind = match row["kind"].as_str() {
                Some("used") => ContextCategoryKind::Used,
                Some("free") => ContextCategoryKind::Free,
                Some("buffer") => ContextCategoryKind::Buffer,
                Some("deferred") => ContextCategoryKind::Deferred,
                _ if row["isDeferred"] == true => ContextCategoryKind::Deferred,
                _ if id == "free_space" => ContextCategoryKind::Free,
                _ if matches!(
                    id.as_str(),
                    "autocompact_buffer" | "auto_compact_buffer" | "compact_buffer"
                ) =>
                {
                    ContextCategoryKind::Buffer
                }
                _ if matches!(
                    id.as_str(),
                    "system_prompt"
                        | "system_tools"
                        | "mcp_tools"
                        | "mcp_instructions"
                        | "custom_agents"
                        | "memory_files"
                        | "skills"
                        | "messages"
                ) =>
                {
                    ContextCategoryKind::Used
                }
                _ => ContextCategoryKind::Unknown,
            };
            let mut children = match id.as_str() {
                "mcp_tools_deferred" => entries(&data["mcpTools"], "name", Some(false)),
                "system_tools_deferred" => {
                    entries(&data["deferredBuiltinTools"], "name", Some(false))
                }
                "memory_files" => entries(&data["memoryFiles"], "path", None),
                "mcp_tools" => entries(&data["mcpTools"], "name", Some(true)),
                "mcp_instructions" => entries(&data["mcpInstructions"], "serverName", None),
                "system_tools" => {
                    let mut tools = entries(&data["systemTools"], "name", None);
                    tools.extend(entries(&data["deferredBuiltinTools"], "name", Some(true)));
                    tools
                }
                "system_prompt" => entries(&data["systemPromptSections"], "name", None),
                "custom_agents" => entries(&data["agents"], "agentType", None),
                "skills" => entries(&data["skills"]["skillFrontmatter"], "name", None),
                _ => Vec::new(),
            };
            if id == "messages" {
                for (key, label) in [
                    ("toolCallTokens", "Tool calls"),
                    ("toolResultTokens", "Tool results"),
                    ("attachmentTokens", "Attachments"),
                    ("assistantMessageTokens", "Assistant messages"),
                    ("userMessageTokens", "User messages"),
                    ("redirectedContextTokens", "Redirected context"),
                    ("unattributedTokens", "Unattributed"),
                ] {
                    if let Some(tokens) = data["messageBreakdown"][key].as_u64() {
                        children.push(ContextEntry {
                            label: label.into(),
                            tokens,
                        });
                    }
                }
            }
            if id == "messages" {
                for entry in data["messageBreakdown"]["toolCallsByType"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    if let Some(name) = entry["name"].as_str() {
                        for (key, suffix) in [("callTokens", "calls"), ("resultTokens", "results")]
                        {
                            if let Some(tokens) = entry[key].as_u64() {
                                children.push(ContextEntry {
                                    label: format!("By tool / {name} / {suffix}"),
                                    tokens,
                                });
                            }
                        }
                    }
                }
                for mut entry in
                    entries(&data["messageBreakdown"]["attachmentsByType"], "name", None)
                {
                    entry.label = format!("By attachment / {}", entry.label);
                    children.push(entry);
                }
            }
            Some(ContextCategory {
                id,
                label,
                tokens,
                kind,
                entries: children,
            })
        })
        .collect();
    Some((
        usage,
        ContextDetails {
            accounting: None,
            model: data["model"].as_str().map(str::to_owned),
            measured_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |duration| duration.as_secs()),
            source: ContextMeasurement::Full,
            categories,
            auto_compact_enabled: data["isAutoCompactEnabled"].as_bool(),
            auto_compact_threshold: data["autoCompactThreshold"].as_u64(),
        },
    ))
}

//! Connection-local provenance for provider-owned descendant interactions.
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct InteractionOwner {
    pub thread: String,
    pub turn: Option<String>,
    pub agent: Option<String>,
}
#[derive(Clone, Debug)]
struct Child {
    parent: String,
    turn: Option<String>,
    settled: Vec<String>,
    active: bool,
    name: String,
}
#[derive(Default)]
pub(crate) struct Children {
    children: HashMap<String, Child>,
    aliases: HashMap<String, String>,
}
impl Children {
    pub fn known(&self, id: &str) -> bool {
        self.children.contains_key(self.canonical(id))
    }
    pub fn canonical<'a>(&'a self, id: &'a str) -> &'a str {
        self.aliases.get(id).map_or(id, String::as_str)
    }
    pub fn register(
        &mut self,
        root: &str,
        parent: &str,
        id: &str,
        name: Option<&str>,
        alias: Option<&str>,
    ) -> bool {
        if id == root
            || self.canonical(id) != id
            || self.canonical(parent) == id
            || (parent != root && !self.known(parent))
            || alias.is_some_and(|alias| {
                alias == root
                    || (alias != id && self.children.contains_key(alias))
                    || self.aliases.get(alias).is_some_and(|target| target != id)
            })
        {
            return false;
        }
        let parent = self.canonical(parent).to_owned();
        if let Some(child) = self.children.get(id) {
            if child.parent != parent {
                return false;
            }
        }
        let child = self.children.entry(id.into()).or_insert_with(|| Child {
            parent,
            turn: None,
            settled: Vec::new(),
            active: false,
            name: name.unwrap_or(id).into(),
        });
        if let Some(name) = name {
            child.name = name.into();
        }
        if let Some(alias) = alias {
            self.aliases
                .entry(alias.into())
                .or_insert_with(|| id.into());
        }
        true
    }
    /// The runtime's task_started joins its task id to the observed Agent tool
    /// invocation. Keep that canonical identity stable so existing requests survive.
    pub fn claude_task(
        &mut self,
        root: &str,
        parent: &str,
        task: &str,
        tool: Option<&str>,
        name: Option<&str>,
    ) -> bool {
        let parent = self.canonical(parent).to_owned();
        if parent != root && !self.known(&parent) {
            return false;
        }
        if task == root || tool == Some(root) {
            return false;
        }
        let canonical = if self.known(task) {
            self.canonical(task).to_owned()
        } else if let Some(tool) = tool.filter(|tool| self.known(tool)) {
            self.canonical(tool).to_owned()
        } else {
            task.to_owned()
        };
        for identity in [Some(task), tool].into_iter().flatten() {
            if self.known(identity) && self.canonical(identity) != canonical {
                return false;
            }
            if identity != canonical
                && self
                    .aliases
                    .get(identity)
                    .is_some_and(|target| target != &canonical)
            {
                return false;
            }
            if let Some(child) = self.children.get(self.canonical(identity)) {
                if self.canonical(&child.parent) != parent {
                    return false;
                }
            }
        }
        if !self.children.contains_key(&canonical)
            && !self.register(root, &parent, &canonical, name, None)
        {
            return false;
        }
        if let Some(name) = name {
            if let Some(child) = self.children.get_mut(&canonical) {
                child.name = name.into();
            }
        }
        for identity in [Some(task), tool].into_iter().flatten() {
            if identity != canonical {
                self.aliases.insert(identity.into(), canonical.clone());
            }
        }
        true
    }
    pub fn start(&mut self, id: &str, turn: Option<&str>) -> bool {
        let id = self.canonical(id).to_owned();
        let Some(child) = self.children.get_mut(&id) else {
            return false;
        };
        if turn.is_some_and(|turn| child.settled.iter().any(|old| old == turn))
            || (child.active && child.turn.as_deref() != turn)
        {
            return false;
        }
        child.turn = turn.map(str::to_owned);
        child.active = true;
        true
    }
    pub fn finish(&mut self, id: &str, turn: Option<&str>) -> Option<String> {
        let id = self.canonical(id).to_owned();
        let child = self.children.get_mut(&id)?;
        if turn.is_some() && child.turn.as_deref() != turn {
            return None;
        }
        if let Some(turn) = child.turn.take() {
            child.settled.push(turn);
        }
        child.active = false;
        Some(id)
    }
    pub fn owner(&self, id: &str, turn: Option<&str>) -> Option<InteractionOwner> {
        let id = self.canonical(id);
        let child = self.children.get(id)?;
        if !child.active || (turn.is_some() && child.turn.as_deref() != turn) {
            return None;
        }
        Some(InteractionOwner {
            thread: id.into(),
            turn: child.turn.clone(),
            agent: Some(format!(
                "{} · {}",
                child.name,
                id.chars()
                    .rev()
                    .take(8)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            )),
        })
    }
    pub fn codex_event(
        &mut self,
        root: &str,
        root_turn: Option<&str>,
        message: &Value,
    ) -> Option<String> {
        let p = &message["params"];
        if message["method"] == "thread/started" {
            let t = &p["thread"];
            let source = &t["source"]["subAgent"]["thread_spawn"];
            if let (Some(id), Some(parent)) =
                (t["id"].as_str(), source["parent_thread_id"].as_str())
            {
                self.register(
                    root,
                    parent,
                    id,
                    t["agentNickname"]
                        .as_str()
                        .or(source["agent_nickname"].as_str()),
                    None,
                );
            }
        }
        let thread = p["threadId"].as_str()?;
        if matches!(
            message["method"].as_str(),
            Some("item/started" | "item/completed")
        ) && p["turnId"].as_str().is_some()
            && (if thread == root {
                root_turn == p["turnId"].as_str()
            } else {
                self.owner(thread, p["turnId"].as_str()).is_some()
            })
        {
            let item = &p["item"];
            if item["type"] == "subAgentActivity" && item["kind"] == "started" {
                if let Some(id) = item["agentThreadId"].as_str() {
                    self.register(
                        root,
                        thread,
                        id,
                        item["agentPath"]
                            .as_str()
                            .and_then(|path| path.rsplit('/').next()),
                        None,
                    );
                }
            }
            if item["type"] == "collabAgentToolCall"
                && item["senderThreadId"].as_str() == Some(thread)
                && item["tool"] == "spawnAgent"
            {
                for id in item["receiverThreadIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    self.register(root, thread, id, None, None);
                }
            }
        }
        if message["method"] == "turn/started" {
            self.start(thread, p["turn"]["id"].as_str());
        }
        if message["method"] == "turn/completed" {
            return self.finish(thread, p["turn"]["id"].as_str());
        }
        if matches!(
            message["method"].as_str(),
            Some("thread/closed" | "thread/unloaded")
        ) {
            return self.finish(thread, None);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_activity_registers_nested_child_and_expires_its_turn() {
        let mut children = Children::default();
        children.register("root", "root", "child", Some("Explorer"), None);
        children.start("child", Some("child-turn"));
        children.codex_event(
            "root",
            Some("root-turn"),
            &json!({"method":"item/started","params":{"threadId":"child","turnId":"child-turn","item":{"type":"subAgentActivity","kind":"started","agentThreadId":"nested","agentPath":"reviewer/security"}}}),
        );
        assert!(children.known("nested"));
        children.start("nested", Some("nested-turn"));
        assert!(children.owner("nested", Some("nested-turn")).is_some());
        assert_eq!(children.finish("nested", Some("stale-turn")), None);
        assert!(children.owner("nested", Some("nested-turn")).is_some());
        assert_eq!(
            children.finish("nested", Some("nested-turn")).as_deref(),
            Some("nested")
        );
        assert!(children.owner("nested", Some("nested-turn")).is_none());
    }

    #[test]
    fn claude_task_tool_alias_is_stable_and_cannot_hijack_sibling() {
        let mut children = Children::default();
        children.register("root", "root", "tool-a", Some("Agent A"), None);
        children.start("tool-a", None);
        children.register("root", "root", "tool-b", Some("Agent B"), None);
        children.start("tool-b", None);
        assert!(children.claude_task("root", "root", "task-a", Some("tool-a"), Some("A")));
        assert_eq!(children.canonical("task-a"), "tool-a");
        assert!(!children.claude_task("root", "root", "task-a", Some("tool-b"), Some("A")));
        assert_eq!(children.canonical("tool-b"), "tool-b");
        assert!(!children.claude_task("root", "root", "root", Some("tool-b"), None));
    }

    #[test]
    fn codex_activity_rejects_stale_parent_or_child_turns() {
        let mut children = Children::default();
        children.register("root", "root", "child", None, None);
        children.start("child", Some("child-turn"));
        for (thread, turn) in [("root", "stale-root-turn"), ("child", "stale-child-turn")] {
            children.codex_event(
                "root",
                Some("root-turn"),
                &json!({"method":"item/started","params":{"threadId":thread,"turnId":turn,"item":{"type":"subAgentActivity","kind":"started","agentThreadId":"foreign"}}}),
            );
        }
        assert!(!children.known("foreign"));
    }
}

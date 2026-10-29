//! App-owned provider maintenance. Install intent is always explicit and reserved
//! synchronously against new launches; existing sessions are preserved. Cached
//! network facts never grant install authority.
use super::session_registry::SessionRegistry;
use futures::StreamExt as _;
use gpui::{App, AppContext as _, Context, Entity, Task};
use grove_core::{
    agent::Agent,
    provider_updates::{self, ProviderStatus},
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

const PROVIDERS: [Agent; 3] = [Agent::Codex, Agent::Claude, Agent::OpenCode];
const DAY: i64 = 86400;

pub struct ProviderRow {
    pub status: ProviderStatus,
    pub checking: bool,
    pub updating: bool,
    pub updated: bool,
    pub update_verified: bool,
    pub last_success: Option<i64>,
    pub error: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    latest: HashMap<String, (String, i64)>,
    dismissed: HashMap<String, String>,
}

pub struct ProviderUpdates {
    rows: Vec<ProviderRow>,
    registry: Entity<SessionRegistry>,
    cache: Cache,
    last_attempt: Option<i64>,
    restoring: bool,
    _timer: Option<Task<()>>,
    _registry_observer: gpui::Subscription,
    check_task: Option<Task<()>>,
    update_task: Option<Task<()>>,
    stopping_task: Option<Task<()>>,
}

impl ProviderUpdates {
    pub fn new(registry: Entity<SessionRegistry>, cx: &mut Context<Self>) -> Self {
        let cache = if cfg!(test) {
            Cache::default()
        } else {
            cache_path()
                .and_then(|p| fs_err::read(p).ok())
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default()
        };
        let timer = if cfg!(test) {
            None
        } else {
            Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                loop {
                    if this
                        .update(cx, |this, cx| {
                            if this
                                .last_attempt
                                .is_none_or(|last| now().saturating_sub(last) >= DAY)
                            {
                                this.check(cx);
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                    cx.background_executor().timer(Duration::from_mins(1)).await;
                }
            }))
        };
        Self {
            rows: Vec::new(),
            _registry_observer: cx.observe(&registry, |this, _, cx| this.registry_changed(cx)),
            registry,
            cache,
            last_attempt: None,
            restoring: false,
            _timer: timer,
            check_task: None,
            update_task: None,
            stopping_task: None,
        }
    }
    fn registry_changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        if self.stopping_task.is_some() || !self.registry.read(cx).has_stopping_provider_sessions()
        {
            return;
        }
        self.stopping_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let pending = this.update(cx, |this, cx| {
                let pending = this.registry.read(cx).has_stopping_provider_sessions();
                if !pending {
                    this.stopping_task = None;
                    cx.notify();
                }
                pending
            });
            if !matches!(pending, Ok(true)) {
                return;
            }
        }));
    }
    pub fn set_restoring(&mut self, restoring: bool, cx: &mut Context<Self>) {
        self.restoring = restoring;
        cx.notify();
    }
    pub fn rows(&self) -> &[ProviderRow] {
        &self.rows
    }
    #[cfg(test)]
    pub(crate) fn set_rows_for_test(&mut self, rows: Vec<ProviderRow>, cx: &mut Context<Self>) {
        self.rows = rows;
        cx.notify();
    }
    pub fn checking(&self) -> bool {
        self.rows.iter().any(|r| r.checking) || self.check_task.is_some()
    }
    pub fn is_updating(&self, agent: Agent) -> bool {
        self.rows
            .iter()
            .any(|r| r.status.agent == agent && r.updating)
    }
    /// Conservatively protect every open provider process, even between turns.
    pub fn active_sessions(&self, agent: Agent, cx: &App) -> Vec<String> {
        if self.restoring {
            return vec!["Restoring saved sessions".into()];
        }
        if self.registry.read(cx).has_stopping_provider_sessions() {
            return vec!["Waiting for closed provider processes to exit".into()];
        }
        self.registry
            .read(cx)
            .all()
            .iter()
            .filter(|s| s.agent == agent)
            .map(|s| s.restored_title.clone().unwrap_or_else(|| s.label.clone()))
            .collect()
    }
    pub fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking() || self.rows.iter().any(|r| r.updating) {
            return;
        }
        self.last_attempt = Some(now());
        for row in &mut self.rows {
            row.checking = true;
            row.updated = false;
            row.update_verified = false;
        }
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let work = PROVIDERS
            .into_iter()
            .map(|agent| {
                let sender = sender.clone();
                cx.background_spawn(async move {
                    let mut status = provider_updates::discover(agent);
                    let _ = sender.unbounded_send((status.clone(), true));
                    match if matches!(status.installer, provider_updates::Installer::ClaudeNative) {
                        Ok(None)
                    } else {
                        provider_updates::latest(agent).map(Some)
                    } {
                        Ok(latest) => status.latest = latest,
                        Err(error) => {
                            status.error = Some(format!("Could not check for updates: {error}"));
                        }
                    }
                    let _ = sender.unbounded_send((status, false));
                })
            })
            .collect::<Vec<_>>();
        drop(sender);
        self.check_task = Some(cx.spawn(async move |this, cx| {
            // Keep all independently running local/network checks alive.
            let _work = work;
            while let Some((mut status, checking)) = receiver.next().await {
                if this
                    .update(cx, |this, cx| {
                        let key = cache_key(&status);
                        let error = status.error.clone();
                        if matches!(status.installer, provider_updates::Installer::ClaudeNative) {
                            status.latest = None;
                        }
                        if let Some(latest) = &status.latest {
                            this.cache
                                .latest
                                .insert(key.clone(), (latest.clone(), now()));
                        }
                        let last_success = this.cache.latest.get(&key).map(|(_, at)| *at);
                        if status.latest.is_none()
                            && !matches!(
                                status.installer,
                                provider_updates::Installer::ClaudeNative
                            )
                        {
                            status.latest = this.cache.latest.get(&key).map(|(v, _)| v.clone());
                        }
                        let row = ProviderRow {
                            status,
                            checking,
                            updating: false,
                            updated: false,
                            update_verified: false,
                            last_success,
                            error,
                        };
                        if let Some(existing) = this
                            .rows
                            .iter_mut()
                            .find(|r| r.status.agent == row.status.agent)
                        {
                            *existing = row;
                        } else {
                            this.rows.push(row);
                            this.rows.sort_by_key(|r| {
                                PROVIDERS.iter().position(|a| *a == r.status.agent)
                            });
                        }
                        this.persist();
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.check_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub fn start_update(&mut self, agent: Agent, cx: &mut Context<Self>) {
        if self.checking() || self.rows.iter().any(|r| r.updating) {
            return;
        }
        let Some(row) = self.rows.iter_mut().find(|r| r.status.agent == agent) else {
            return;
        };
        if !row.status.can_update() {
            return;
        }
        row.updating = true;
        row.updated = false;
        row.update_verified = false;
        row.error = None;
        let status = row.status.clone();
        let work = cx.background_spawn(async move {
            let result = provider_updates::update(&status);
            let detected = result
                .as_ref()
                .err()
                .map(|_| provider_updates::check(status.agent));
            (result, detected)
        });
        self.update_task = Some(cx.spawn(async move |this, cx| {
            let (result, detected) = work.await;
            let _ = this.update(cx, |this, cx| {
                let Some(row) = this.rows.iter_mut().find(|r| r.status.agent == agent) else {
                    return;
                };
                row.updating = false;
                match result {
                    Ok(status) => {
                        row.updated = status.installed != row.status.installed;
                        row.update_verified = true;
                        row.status = status;
                        row.error = None;
                    }
                    Err(error) => {
                        if let Some(mut detected) = detected {
                            if detected.latest.is_none()
                                && !matches!(
                                    detected.installer,
                                    provider_updates::Installer::ClaudeNative
                                )
                            {
                                detected.latest.clone_from(&row.status.latest);
                            }
                            row.status = detected;
                        }
                        row.error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub fn notice(&self) -> Option<String> {
        let names = self
            .rows
            .iter()
            .filter(|r| {
                !r.updating
                    && !r.checking
                    && r.error.is_none()
                    && r.status.update_available()
                    && self.cache.dismissed.get(r.status.agent.label()) != r.status.latest.as_ref()
            })
            .map(|r| match r.status.agent {
                Agent::Claude => "Claude Code",
                Agent::Codex => "Codex",
                _ => "OpenCode",
            })
            .collect::<Vec<_>>();
        (!names.is_empty()).then(|| format!("Updates available for {}", names.join(", ")))
    }
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        for row in &self.rows {
            if row.status.update_available() {
                if let Some(latest) = &row.status.latest {
                    self.cache
                        .dismissed
                        .insert(row.status.agent.label().into(), latest.clone());
                }
            }
        }
        self.persist();
        cx.notify();
    }
    fn persist(&self) {
        if cfg!(test) {
            return;
        }
        if let (Some(path), Ok(bytes)) = (cache_path(), serde_json::to_vec(&self.cache)) {
            let temp = path.with_extension("tmp");
            if let Err(error) =
                fs_err::write(&temp, bytes).and_then(|()| fs_err::rename(temp, path))
            {
                tracing::warn!(%error, "Could not save provider update cache");
            }
        }
    }
}
fn cache_key(status: &ProviderStatus) -> String {
    format!(
        "{}|{:?}|{:?}",
        status.agent.label(),
        status.executable,
        status.installer
    )
}
fn cache_path() -> Option<std::path::PathBuf> {
    grove_core::storage::config_dir()
        .ok()
        .map(|p| p.join("provider-updates.json"))
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext;
    fn status() -> ProviderStatus {
        ProviderStatus {
            agent: Agent::Codex,
            installed: Some("1.0.0".into()),
            latest: Some("1.1.0".into()),
            executable: Some("/fixture/codex".into()),
            installer: provider_updates::Installer::Npm {
                executable: "/fixture/npm".into(),
                prefix: "/fixture".into(),
                package: "@openai/codex".into(),
            },
            error: None,
        }
    }
    fn row() -> ProviderRow {
        ProviderRow {
            status: status(),
            checking: false,
            updating: false,
            updated: false,
            update_verified: false,
            last_success: Some(1),
            error: None,
        }
    }
    #[gpui::test]
    fn open_sessions_allow_updates_before_worker_dispatch(cx: &mut gpui::TestAppContext) {
        for agent in [Agent::Codex, Agent::Claude] {
            let registry = cx.new(|_| SessionRegistry::new());
            registry.update(cx, |registry, _| {
                registry.insert_meta("Project".into(), "/fixture".into(), agent);
            });
            let updates = cx.new(|cx| ProviderUpdates::new(registry, cx));
            updates.update(cx, |updates, cx| {
                let mut fixture = row();
                fixture.status.agent = agent;
                if agent == Agent::Claude {
                    fixture.status.installer = provider_updates::Installer::ClaudeNative;
                }
                fixture.error = Some("Earlier update failed".into());
                updates.rows.push(fixture);
                updates.set_restoring(true, cx);
                updates.start_update(agent, cx);
                assert!(updates.is_updating(agent));
                assert!(updates.update_task.is_some());
                assert!(updates.rows[0].error.is_none());
            });
        }
    }
    #[gpui::test]
    fn provider_stopping_process_completion_refreshes_maintenance_state(
        cx: &mut gpui::TestAppContext,
    ) {
        let registry = cx.new(|_| SessionRegistry::new());
        let updates = cx.new(|cx| ProviderUpdates::new(registry.clone(), cx));
        let (sender, receiver) = std::sync::mpsc::channel();
        registry.update(cx, |registry, cx| {
            registry.track_stopping_for_test(receiver);
            cx.notify();
        });
        cx.run_until_parked();
        updates.read_with(cx, |updates, cx| {
            assert!(updates.stopping_task.is_some());
            assert!(!updates.active_sessions(Agent::Codex, cx).is_empty());
        });
        sender.send(()).unwrap();
        drop(sender);
        cx.executor().advance_clock(Duration::from_millis(250));
        cx.run_until_parked();
        updates.read_with(cx, |updates, cx| {
            assert!(updates.stopping_task.is_none());
            assert!(updates.active_sessions(Agent::Codex, cx).is_empty());
        });
    }
    #[gpui::test]
    fn dismissal_is_per_release_and_install_reservation_is_per_provider(
        cx: &mut gpui::TestAppContext,
    ) {
        let registry = cx.new(|_| SessionRegistry::new());
        let updates = cx.new(|cx| ProviderUpdates::new(registry, cx));
        updates.update(cx, |updates, cx| {
            updates.rows.push(row());
            assert!(updates.notice().is_some());
            updates.dismiss(cx);
            assert!(updates.notice().is_none());
            updates.rows[0].status.latest = Some("1.2.0".into());
            assert!(updates.notice().is_some());
            updates.rows[0].updating = true;
            assert!(updates.is_updating(Agent::Codex));
            assert!(!updates.is_updating(Agent::Claude));
            assert!(updates.notice().is_none());
        });
    }
}

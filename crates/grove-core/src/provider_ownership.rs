//! Process-lifetime ownership of native threads within a Grove configuration.
//! CLI-owned threads outside Grove still rely on the native provider's refusal.
use crate::provider::{Provider, SavedSession};
use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
};

/// A pending fork writes a new target; an established fork resumes its current ID.
pub(crate) fn native_write_target(saved: &SavedSession) -> Option<&str> {
    match saved.provider {
        Provider::Codex
            if saved
                .fork_origin
                .as_ref()
                .is_some_and(|origin| origin.source_thread.is_some()) =>
        {
            None
        }
        Provider::Codex => saved.thread_id.as_deref(),
        Provider::Claude => saved
            .pending_edit
            .as_ref()
            .and_then(|edit| edit.claude_fork_id.as_deref())
            .or_else(|| {
                saved
                    .claude_resume
                    .as_ref()
                    .map(|resume| resume.fork_id.as_str())
            })
            .or(saved.thread_id.as_deref()),
    }
}

pub(crate) struct ThreadOwnership {
    directory: PathBuf,
    provider: Provider,
    held: Vec<(String, File)>,
}
impl ThreadOwnership {
    pub(crate) fn new(provider: Provider) -> Result<Self, String> {
        Ok(Self {
            directory: crate::storage::config_dir()
                .map_err(|e| e.to_string())?
                .join("provider-ownership"),
            provider,
            held: Vec::new(),
        })
    }
    pub(crate) fn claim(&mut self, id: &str) -> Result<(), String> {
        if self.held.iter().any(|(held, _)| held == id) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        // Native identifiers never become paths; namespaces keep providers independent.
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            format!("{}:{id}", self.provider.label()).as_bytes(),
        );
        let name: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(self.directory.join(name))
            .map_err(|e| e.to_string())?;
        file.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => format!("{} conversation is already open in another Grove process. Close its existing session, then reconnect. The conversation identity was preserved.", self.provider.label()),
            std::fs::TryLockError::Error(error) => format!("Could not establish {} conversation ownership: {error}", self.provider.label()),
        })?;
        self.held.push((id.into(), file));
        // Keep the lock file: unlinking a locked inode would permit a second owner.
        Ok(())
    }
}
impl Drop for ThreadOwnership {
    fn drop(&mut self) {
        // Release explicitly before close: another thread can fork briefly while
        // these descriptors are live, retaining the same OS lock until exec.
        for (_, file) in &self.held {
            let _ = file.unlock();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_refuses_duplicate_threads_and_releases_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        for provider in [Provider::Codex, Provider::Claude] {
            let owner = || ThreadOwnership {
                directory: dir.path().into(),
                provider,
                held: Vec::new(),
            };
            let mut first = owner();
            first.claim("../../native-thread").unwrap();
            first.claim("../../native-thread").unwrap();
            let mut second = owner();
            assert!(second
                .claim("../../native-thread")
                .unwrap_err()
                .contains("already open"));
            second.claim("different-thread").unwrap();
            drop(first);
            second.claim("../../native-thread").unwrap();
        }
    }
    #[test]
    fn ownership_child_process() {
        let Some(directory) = std::env::var_os("GROVE_OWNERSHIP_TEST_DIRECTORY") else {
            return;
        };
        let provider = if std::env::var("GROVE_OWNERSHIP_TEST_PROVIDER").unwrap() == "codex" {
            Provider::Codex
        } else {
            Provider::Claude
        };
        let mut owner = ThreadOwnership {
            directory: directory.into(),
            provider,
            held: Vec::new(),
        };
        let result = owner.claim("native-thread");
        if std::env::var_os("GROVE_OWNERSHIP_TEST_CONFLICT").is_some() {
            assert!(result.unwrap_err().contains("already open"));
        } else {
            result.unwrap();
        }
    }
    #[test]
    fn ownership_is_exclusive_across_processes() {
        let dir = tempfile::tempdir().unwrap();
        for (provider, name) in [(Provider::Codex, "codex"), (Provider::Claude, "claude")] {
            let mut owner = ThreadOwnership {
                directory: dir.path().into(),
                provider,
                held: Vec::new(),
            };
            owner.claim("native-thread").unwrap();
            let run = |conflict| {
                let mut child = std::process::Command::new(std::env::current_exe().unwrap());
                child
                    .args([
                        "--exact",
                        "provider_ownership::tests::ownership_child_process",
                    ])
                    .env("GROVE_OWNERSHIP_TEST_DIRECTORY", dir.path())
                    .env("GROVE_OWNERSHIP_TEST_PROVIDER", name)
                    .env_remove("GROVE_OWNERSHIP_TEST_CONFLICT");
                if conflict {
                    child.env("GROVE_OWNERSHIP_TEST_CONFLICT", "1");
                }
                let output = child.output().unwrap();
                assert!(
                    output.status.success(),
                    "{} {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            };
            run(true);
            drop(owner);
            run(false);
        }
    }
    #[test]
    fn providers_have_independent_native_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let mut codex = ThreadOwnership {
            directory: dir.path().into(),
            provider: Provider::Codex,
            held: Vec::new(),
        };
        let mut claude = ThreadOwnership {
            directory: dir.path().into(),
            provider: Provider::Claude,
            held: Vec::new(),
        };
        codex.claim("same-id").unwrap();
        claude.claim("same-id").unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}

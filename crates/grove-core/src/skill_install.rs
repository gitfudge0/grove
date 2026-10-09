//! Install the embedded operating skill in agent-discovered locations.
use serde::Serialize;
use std::{
    io,
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const SKILL: &str = include_str!("../../../skills/grove/SKILL.md");
const MANAGED: &str = "<!-- grove-managed-skill: v1 -->";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTarget {
    Codex,
    Claude,
    OpenCode,
}
impl AgentTarget {
    pub const ALL: [Self; 3] = [Self::Codex, Self::Claude, Self::OpenCode];
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::OpenCode => "OpenCode",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "root", rename_all = "snake_case")]
pub enum InstallScope {
    User,
    Project(PathBuf),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    Missing,
    Installed,
    UpdateAvailable,
    Conflict,
}
#[derive(Clone, Debug, Serialize)]
pub struct Installation {
    pub agent: AgentTarget,
    pub scope: InstallScope,
    pub path: PathBuf,
    pub status: InstallStatus,
}
#[derive(Debug, Error)]
pub enum InstallError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] crate::storage::StoreError),
    #[error("Cannot locate the user home directory")]
    NoHome,
    #[error("Project skill root must be an existing absolute directory: {0}")]
    InvalidProject(PathBuf),
    #[error("Existing skill is not managed by Grove; preserved at {0}")]
    Conflict(PathBuf),
    #[error("Refusing to install through a symbolic link: {0}")]
    Symlink(PathBuf),
}

fn target_path(agent: AgentTarget, scope: &InstallScope) -> Result<PathBuf, InstallError> {
    let home = || dirs::home_dir().ok_or(InstallError::NoHome);
    let base = match scope {
        InstallScope::User => match agent {
            AgentTarget::Codex => home()?.join(".agents/skills"),
            AgentTarget::Claude => home()?.join(".claude/skills"),
            AgentTarget::OpenCode => std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or(home()?.join(".config"))
                .join("opencode/skills"),
        },
        InstallScope::Project(root) => {
            if !root.is_absolute() || !root.is_dir() {
                return Err(InstallError::InvalidProject(root.clone()));
            }
            root.join(match agent {
                AgentTarget::Codex => ".agents/skills",
                AgentTarget::Claude => ".claude/skills",
                AgentTarget::OpenCode => ".opencode/skills",
            })
        }
    };
    // Resolve trusted scope ancestors (macOS /var is a system symlink),
    // while keeping agent skill descendants subject to symlink refusal.
    let (trusted, relative) = match scope {
        InstallScope::Project(root) => (
            root.canonicalize()?,
            base.strip_prefix(root)
                .map_err(|_| InstallError::InvalidProject(root.clone()))?
                .to_path_buf(),
        ),
        InstallScope::User => {
            let root = home()?;
            if let Ok(relative) = base.strip_prefix(&root) {
                (root.canonicalize()?, relative.to_path_buf())
            } else {
                // An explicit XDG path is itself the trusted configuration root.
                let root = std::env::var_os("XDG_CONFIG_HOME")
                    .map(PathBuf::from)
                    .ok_or(InstallError::NoHome)?;
                let mut ancestor = root.as_path();
                while !ancestor.exists() {
                    ancestor = ancestor.parent().ok_or(InstallError::NoHome)?;
                }
                reject_symlinks(ancestor)?;
                let relative = root
                    .strip_prefix(ancestor)
                    .map_err(|_| InstallError::NoHome)?;
                (ancestor.canonicalize()?, relative.join("opencode/skills"))
            }
        }
    };
    Ok(trusted.join(relative).join("grove/SKILL.md"))
}
fn reject_symlinks(path: &Path) -> Result<(), InstallError> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(InstallError::Symlink(ancestor.into()))
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn status_at(
    agent: AgentTarget,
    scope: &InstallScope,
    path: PathBuf,
) -> Result<Installation, InstallError> {
    reject_symlinks(&path)?;
    let status = match fs_err::read(&path) {
        Ok(bytes) if bytes == SKILL.as_bytes() => InstallStatus::Installed,
        Ok(bytes) if String::from_utf8_lossy(&bytes).contains(MANAGED) => {
            InstallStatus::UpdateAvailable
        }
        Ok(_) => InstallStatus::Conflict,
        Err(e) if e.kind() == io::ErrorKind::NotFound => InstallStatus::Missing,
        Err(e) => return Err(e.into()),
    };
    Ok(Installation {
        agent,
        scope: scope.clone(),
        path,
        status,
    })
}
pub fn status(agent: AgentTarget, scope: &InstallScope) -> Result<Installation, InstallError> {
    status_at(agent, scope, target_path(agent, scope)?)
}
pub fn install(
    agent: AgentTarget,
    scope: &InstallScope,
    overwrite: bool,
) -> Result<Installation, InstallError> {
    let current = status(agent, scope)?;
    if current.status == InstallStatus::Conflict && !overwrite {
        return Err(InstallError::Conflict(current.path));
    }
    if current.status != InstallStatus::Installed {
        if let Some(parent) = current.path.parent() {
            fs_err::create_dir_all(parent)?;
        }
        reject_symlinks(&current.path)?;
        crate::storage::write_atomic(&current.path, SKILL.as_bytes())?;
    }
    status(agent, scope)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn installs_all_project_targets_and_preserves_foreign_content() {
        let root = tempfile::tempdir().unwrap();
        let scope = InstallScope::Project(root.path().into());
        for agent in AgentTarget::ALL {
            assert_eq!(
                status(agent, &scope).unwrap().status,
                InstallStatus::Missing
            );
            let installed = install(agent, &scope, false).unwrap();
            assert_eq!(installed.status, InstallStatus::Installed);
            fs_err::write(&installed.path, "foreign skill").unwrap();
            assert!(matches!(
                install(agent, &scope, false),
                Err(InstallError::Conflict(_))
            ));
            assert_eq!(
                fs_err::read_to_string(&installed.path).unwrap(),
                "foreign skill"
            );
            assert_eq!(
                install(agent, &scope, true).unwrap().status,
                InstallStatus::Installed
            );
            fs_err::write(&installed.path, format!("{MANAGED}\nold version")).unwrap();
            assert_eq!(
                status(agent, &scope).unwrap().status,
                InstallStatus::UpdateAvailable
            );
            assert_eq!(
                install(agent, &scope, false).unwrap().status,
                InstallStatus::Installed
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_destination() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join(".agents")).unwrap();
        assert!(matches!(
            install(
                AgentTarget::Codex,
                &InstallScope::Project(root.path().into()),
                true
            ),
            Err(InstallError::Symlink(_))
        ));
        assert!(!outside.path().join("skills/grove/SKILL.md").exists());
    }
}

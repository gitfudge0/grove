//! Stable project symbols; independent of names, ordering, providers and themes.
use serde::{Deserialize, Serialize};

/// Stable indices: append new symbols; never reorder existing registrations.
pub const SYMBOLS: &[(&str, &str)] = &[
    ("project-leaf", "Leaf"),
    ("project-compass", "Compass"),
    ("project-layers", "Layers"),
    ("project-cube", "Cube"),
    ("project-globe", "Globe"),
    ("project-diamond", "Diamond"),
    ("project-orbit", "Orbit"),
    ("project-mountain", "Mountain"),
    ("project-wave", "Wave"),
    ("project-sun", "Sun"),
    ("project-feather", "Feather"),
    ("project-gem", "Gem"),
    ("project-flower", "Flower"),
    ("project-moon", "Moon"),
    ("project-bolt", "Bolt"),
    ("project-hexagon", "Hexagon"),
];
pub const COLOR_COUNT: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectIdentity {
    pub symbol: u8,
    pub color: u8,
}
impl ProjectIdentity {
    pub fn valid(self) -> bool {
        usize::from(self.symbol) < SYMBOLS.len() && self.color < COLOR_COUNT
    }
    /// Fixed FNV-1a, rather than Rust's unspecified DefaultHasher algorithm.
    pub fn for_path(path: &str) -> Self {
        let hash = path
            .trim_end_matches(['/', '\\'])
            .bytes()
            .fold(0xcbf29ce484222325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
            });
        Self {
            symbol: (hash as usize % SYMBOLS.len()) as u8,
            color: ((hash >> 32) % u64::from(COLOR_COUNT)) as u8,
        }
    }
    /// Pick a different symbol AND color without changing the saved automatic identity.
    pub fn randomized(self) -> Result<Self, &'static str> {
        use ring::rand::SecureRandom as _;
        let mut entropy = [0u8; 2];
        ring::rand::SystemRandom::new()
            .fill(&mut entropy)
            .map_err(|_| "Could not randomize appearance. Try again.")?;
        Ok(Self {
            symbol: ((usize::from(self.symbol) + 1 + usize::from(entropy[0]) % (SYMBOLS.len() - 1))
                % SYMBOLS.len()) as u8,
            color: (self.color % COLOR_COUNT + 1 + entropy[1] % (COLOR_COUNT - 1)) % COLOR_COUNT,
        })
    }

    pub fn assign(path: &str, occupied: &[Self]) -> Self {
        let preferred = Self::for_path(path);
        let start = usize::from(preferred.symbol);
        // Prefer an unused silhouette, then an unused pair, then the stable fallback.
        for offset in 0..SYMBOLS.len() {
            let candidate = Self {
                symbol: ((start + offset) % SYMBOLS.len()) as u8,
                ..preferred
            };
            if !occupied.iter().any(|m| m.symbol == candidate.symbol) {
                return candidate;
            }
        }
        for color_offset in 0..COLOR_COUNT {
            for offset in 0..SYMBOLS.len() {
                let candidate = Self {
                    symbol: ((start + offset) % SYMBOLS.len()) as u8,
                    color: (preferred.color + color_offset) % COLOR_COUNT,
                };
                if !occupied.contains(&candidate) {
                    return candidate;
                }
            }
        }
        preferred
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAppearance {
    pub automatic: ProjectIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<ProjectIdentity>,
}
impl ProjectAppearance {
    pub fn current(self) -> ProjectIdentity {
        self.custom.filter(|m| m.valid()).unwrap_or(self.automatic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Project, ProjectScripts, Store};

    fn project(name: &str, path: &str) -> Project {
        Project {
            name: name.into(),
            path: path.into(),
            scripts: ProjectScripts::default(),
            archived: false,
            worktree_dir: None,
        }
    }

    #[test]
    fn symbol_randomization_changes_both_parts_and_always_remains_valid() {
        let mut current = ProjectIdentity::for_path("/randomize-fixture");
        for _ in 0..256 {
            let next = current.randomized().unwrap();
            assert!(next.valid());
            assert_ne!(next.symbol, current.symbol);
            assert_ne!(next.color, current.color);
            current = next;
        }
    }

    #[test]
    fn symbol_assignments_exhaust_unused_shapes_then_pairs() {
        let mut assigned = Vec::new();
        for i in 0..SYMBOLS.len() * usize::from(COLOR_COUNT) {
            let value = ProjectIdentity::assign(&format!("/projects/{i}"), &assigned);
            assert!(value.valid());
            assert!(!assigned.contains(&value));
            if i < SYMBOLS.len() {
                assert!(!assigned
                    .iter()
                    .any(|m: &ProjectIdentity| m.symbol == value.symbol));
            }
            assigned.push(value);
        }
        assert!(ProjectIdentity::assign("/projects/overflow", &assigned).valid());
    }

    #[test]
    fn symbol_migration_is_order_independent_and_survives_rename_restart_and_custom_reset() {
        let mut a = Store {
            projects: vec![project("same", "/repos/a"), project("same", "/repos/b")],
            ..Default::default()
        };
        let mut b = a.clone();
        b.projects.reverse();
        a.normalize_workspaces();
        b.normalize_workspaces();
        assert_eq!(a.project_appearances, b.project_appearances);
        assert_ne!(
            a.project_appearance("/repos/a").current().symbol,
            a.project_appearance("/repos/b").current().symbol
        );
        let automatic = a.project_appearance("/repos/a");
        a.projects[0].name = "renamed".into();
        a.projects[0].archived = true;
        a.project_appearances.get_mut("/repos/a").unwrap().custom = Some(ProjectIdentity {
            symbol: 4,
            color: 4,
        });
        let mut restored: Store =
            serde_json::from_str(&serde_json::to_string(&a).unwrap()).unwrap();
        restored.normalize_workspaces();
        assert_eq!(
            restored.project_appearance("/repos/a").current(),
            ProjectIdentity {
                symbol: 4,
                color: 4
            }
        );
        assert_eq!(
            restored.project_appearance("/repos/a/child").current(),
            ProjectIdentity {
                symbol: 4,
                color: 4
            }
        );
        restored
            .project_appearances
            .get_mut("/repos/a")
            .unwrap()
            .custom = None;
        restored.normalize_workspaces();
        assert_eq!(restored.project_appearance("/repos/a"), automatic);
        restored.projects.push(project("new", "/repos/c"));
        restored.normalize_workspaces();
        assert_eq!(restored.project_appearance("/repos/a"), automatic);
    }

    #[test]
    fn symbol_invalid_saved_data_recovers_without_changing_valid_neighbors() {
        let mut store = Store {
            projects: vec![project("a", "/a"), project("b", "/b")],
            ..Default::default()
        };
        store.normalize_workspaces();
        let before = store.project_appearance("/b");
        store
            .project_appearances
            .get_mut("/a")
            .unwrap()
            .automatic
            .symbol = 255;
        store.project_appearances.get_mut("/b").unwrap().custom = Some(ProjectIdentity {
            symbol: 0,
            color: 255,
        });
        store.normalize_workspaces();
        assert!(store.project_appearance("/a").current().valid());
        assert_eq!(store.project_appearance("/b").current(), before.current());
    }
}

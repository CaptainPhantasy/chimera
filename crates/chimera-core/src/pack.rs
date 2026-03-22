use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Execution scope
// ---------------------------------------------------------------------------

/// Defines the boundaries within which a pack operates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionScope {
    /// Allowed path globs (e.g. "src/**", "tests/**").
    pub allowed_paths: Vec<String>,
    /// Denied path globs (e.g. ".env", "secrets/**").
    pub denied_paths: Vec<String>,
    /// Whether network access is permitted.
    pub network: bool,
    /// Whether commits are permitted.
    pub can_commit: bool,
    /// Whether external comms are permitted.
    pub can_message_external: bool,
    /// Maximum total token budget across all creatures.
    pub total_token_budget: Option<u64>,
    /// Maximum wall-clock time in seconds.
    pub timeout_secs: Option<u64>,
}

impl Default for ExecutionScope {
    fn default() -> Self {
        Self {
            allowed_paths: vec!["**".into()],
            denied_paths: vec![".env".into(), "secrets/**".into()],
            network: false,
            can_commit: false,
            can_message_external: false,
            total_token_budget: None,
            timeout_secs: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Pack specification
// ---------------------------------------------------------------------------

/// A predefined team of creatures with a shared scope and objective template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackSpec {
    /// Pack name (e.g. "ship-hotfix", "incident-response", "browser-lab").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Creature type names that make up this pack.
    pub creatures: Vec<String>,
    /// The execution scope for this pack.
    pub scope: ExecutionScope,
    /// Optional default objective template.
    pub objective_template: Option<String>,
    /// Optional skill names to pre-load.
    pub skills: Vec<String>,
}

// ---------------------------------------------------------------------------
// PackRegistry trait
// ---------------------------------------------------------------------------

/// Registry for predefined creature packs.
#[async_trait::async_trait]
pub trait PackRegistry: Send + Sync {
    /// List all registered pack names.
    async fn list(&self) -> anyhow::Result<Vec<String>>;

    /// Resolve a pack spec by name.
    async fn resolve(&self, name: &str) -> anyhow::Result<PackSpec>;

    /// Register a new pack spec.
    async fn register(&self, spec: PackSpec) -> anyhow::Result<()>;

    /// Remove a pack by name.
    async fn remove(&self, name: &str) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// In-memory mock
// ---------------------------------------------------------------------------

/// In-memory PackRegistry for testing.
pub struct InMemoryPackRegistry {
    packs: tokio::sync::RwLock<std::collections::HashMap<String, PackSpec>>,
}

impl InMemoryPackRegistry {
    pub fn new() -> Self {
        Self {
            packs: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Seed with builtin packs.
    pub async fn seed_builtins(&self) {
        let builtins = vec![
            PackSpec {
                name: "ship-hotfix".into(),
                description: "Rapid hotfix: recon, patch, verify, PR".into(),
                creatures: vec!["raven".into(), "mantis".into(), "hound".into(), "owl".into()],
                scope: ExecutionScope {
                    can_commit: true,
                    ..Default::default()
                },
                objective_template: Some("Fix: {objective}".into()),
                skills: vec!["ship-hotfix".into()],
            },
            PackSpec {
                name: "incident-response".into(),
                description: "Triage, diagnose, fix, verify, notify".into(),
                creatures: vec!["raven".into(), "hound".into(), "mantis".into(), "owl".into()],
                scope: ExecutionScope {
                    network: true,
                    can_commit: true,
                    can_message_external: true,
                    ..Default::default()
                },
                objective_template: None,
                skills: vec!["triage-incident".into()],
            },
            PackSpec {
                name: "browser-lab".into(),
                description: "Browser automation and visual regression testing".into(),
                creatures: vec!["raven".into(), "wasp".into(), "hound".into()],
                scope: ExecutionScope {
                    network: true,
                    ..Default::default()
                },
                objective_template: None,
                skills: vec!["browser-regression-check".into()],
            },
        ];

        let mut packs = self.packs.write().await;
        for pack in builtins {
            packs.insert(pack.name.clone(), pack);
        }
    }
}

impl Default for InMemoryPackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PackRegistry for InMemoryPackRegistry {
    async fn list(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.packs.read().await.keys().cloned().collect())
    }

    async fn resolve(&self, name: &str) -> anyhow::Result<PackSpec> {
        self.packs
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("pack not found: {}", name))
    }

    async fn register(&self, spec: PackSpec) -> anyhow::Result<()> {
        self.packs.write().await.insert(spec.name.clone(), spec);
        Ok(())
    }

    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        self.packs
            .write()
            .await
            .remove(name)
            .ok_or_else(|| anyhow::anyhow!("pack not found: {}", name))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_and_resolve() {
        let reg = InMemoryPackRegistry::new();
        let spec = PackSpec {
            name: "test-pack".into(),
            description: "a test".into(),
            creatures: vec!["raven".into(), "mantis".into()],
            scope: ExecutionScope::default(),
            objective_template: None,
            skills: vec![],
        };

        reg.register(spec).await.unwrap();
        let resolved = reg.resolve("test-pack").await.unwrap();
        assert_eq!(resolved.name, "test-pack");
        assert_eq!(resolved.creatures.len(), 2);
    }

    #[tokio::test]
    async fn list_returns_registered_names() {
        let reg = InMemoryPackRegistry::new();
        reg.seed_builtins().await;

        let mut names = reg.list().await.unwrap();
        names.sort();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"ship-hotfix".to_string()));
        assert!(names.contains(&"incident-response".to_string()));
        assert!(names.contains(&"browser-lab".to_string()));
    }

    #[tokio::test]
    async fn resolve_unknown_fails() {
        let reg = InMemoryPackRegistry::new();
        assert!(reg.resolve("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn remove_existing() {
        let reg = InMemoryPackRegistry::new();
        reg.seed_builtins().await;

        reg.remove("ship-hotfix").await.unwrap();
        assert!(reg.resolve("ship-hotfix").await.is_err());
    }

    #[tokio::test]
    async fn remove_unknown_fails() {
        let reg = InMemoryPackRegistry::new();
        assert!(reg.remove("ghost").await.is_err());
    }

    #[tokio::test]
    async fn pack_scope_defaults() {
        let scope = ExecutionScope::default();
        assert!(!scope.network);
        assert!(!scope.can_commit);
        assert!(!scope.can_message_external);
        assert!(scope.denied_paths.contains(&".env".to_string()));
    }
}

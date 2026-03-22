use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

pub type SkillId = Uuid;

// ---------------------------------------------------------------------------
// Skill specification
// ---------------------------------------------------------------------------

/// A reusable skill definition — a versioned workflow pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSpec {
    /// Unique skill name (e.g. "ship-hotfix", "browser-regression-check").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Version string.
    pub version: String,
    /// The objective template this skill addresses.
    pub objective_template: String,
    /// Creature types needed to execute this skill.
    pub required_creatures: Vec<String>,
    /// Tool names required.
    pub required_tools: Vec<String>,
    /// Pre-run hook commands.
    pub pre_hooks: Vec<String>,
    /// Post-run hook commands.
    pub post_hooks: Vec<String>,
}

// ---------------------------------------------------------------------------
// Captured skill
// ---------------------------------------------------------------------------

/// A skill captured from a successful run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapturedSkill {
    /// Generated skill ID.
    pub id: SkillId,
    /// The skill specification.
    pub spec: SkillSpec,
    /// Session ID that produced this skill.
    pub source_session: Uuid,
    /// When this skill was captured.
    pub captured_at: DateTime<Utc>,
    /// Confidence score (0.0 to 1.0) based on verification results.
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// SkillCapture trait
// ---------------------------------------------------------------------------

/// Captures, stores, and retrieves reusable skills.
#[async_trait::async_trait]
pub trait SkillCapture: Send + Sync {
    /// Capture a skill from a successful run.
    async fn capture(&self, skill: CapturedSkill) -> anyhow::Result<SkillId>;

    /// List all captured skill names.
    async fn list(&self) -> anyhow::Result<Vec<String>>;

    /// Resolve a skill spec by name.
    async fn resolve(&self, name: &str) -> anyhow::Result<CapturedSkill>;

    /// Remove a captured skill.
    async fn remove(&self, name: &str) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// In-memory mock
// ---------------------------------------------------------------------------

/// In-memory SkillCapture for testing.
pub struct MockSkillCapture {
    skills: tokio::sync::RwLock<std::collections::HashMap<String, CapturedSkill>>,
}

impl MockSkillCapture {
    pub fn new() -> Self {
        Self {
            skills: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MockSkillCapture {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SkillCapture for MockSkillCapture {
    async fn capture(&self, skill: CapturedSkill) -> anyhow::Result<SkillId> {
        let id = skill.id;
        self.skills.write().await.insert(skill.spec.name.clone(), skill);
        Ok(id)
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        Ok(self.skills.read().await.keys().cloned().collect())
    }

    async fn resolve(&self, name: &str) -> anyhow::Result<CapturedSkill> {
        self.skills
            .read()
            .await
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("skill not found: {}", name))
    }

    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        self.skills
            .write()
            .await
            .remove(name)
            .ok_or_else(|| anyhow::anyhow!("skill not found: {}", name))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skill(name: &str) -> CapturedSkill {
        CapturedSkill {
            id: Uuid::new_v4(),
            spec: SkillSpec {
                name: name.into(),
                description: format!("{} skill", name),
                version: "1.0.0".into(),
                objective_template: format!("execute {}", name),
                required_creatures: vec!["raven".into(), "mantis".into()],
                required_tools: vec!["bash".into(), "git".into()],
                pre_hooks: vec!["cargo check".into()],
                post_hooks: vec!["cargo test".into()],
            },
            source_session: Uuid::new_v4(),
            captured_at: Utc::now(),
            confidence: 0.92,
        }
    }

    #[tokio::test]
    async fn capture_and_resolve() {
        let store = MockSkillCapture::new();
        let skill = make_skill("ship-hotfix");
        let id = store.capture(skill).await.unwrap();

        let resolved = store.resolve("ship-hotfix").await.unwrap();
        assert_eq!(resolved.id, id);
        assert_eq!(resolved.spec.name, "ship-hotfix");
        assert_eq!(resolved.confidence, 0.92);
    }

    #[tokio::test]
    async fn list_captured_skills() {
        let store = MockSkillCapture::new();
        store.capture(make_skill("a")).await.unwrap();
        store.capture(make_skill("b")).await.unwrap();

        let mut names = store.list().await.unwrap();
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn resolve_unknown_fails() {
        let store = MockSkillCapture::new();
        assert!(store.resolve("nonexistent").await.is_err());
    }

    #[tokio::test]
    async fn remove_skill() {
        let store = MockSkillCapture::new();
        store.capture(make_skill("temp")).await.unwrap();

        store.remove("temp").await.unwrap();
        assert!(store.resolve("temp").await.is_err());
    }

    #[tokio::test]
    async fn remove_unknown_fails() {
        let store = MockSkillCapture::new();
        assert!(store.remove("ghost").await.is_err());
    }

    #[tokio::test]
    async fn skill_spec_serializes() {
        let skill = make_skill("test");
        let json = serde_json::to_string(&skill.spec).unwrap();
        assert!(json.contains("test"));
        assert!(json.contains("raven"));
        assert!(json.contains("cargo check"));
    }
}

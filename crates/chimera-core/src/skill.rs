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
        self.skills
            .write()
            .await
            .insert(skill.spec.name.clone(), skill);
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

#[derive(Default, Serialize, Deserialize)]
struct SkillRecords {
    skills: std::collections::BTreeMap<String, CapturedSkill>,
    archived: Vec<ArchivedSkill>,
}
#[derive(Serialize, Deserialize)]
struct ArchivedSkill {
    skill: CapturedSkill,
    reason: String,
    at: DateTime<Utc>,
}

/// Captured workflows persist in the current project; their source must be an
/// actual completed, saved session. Running a saved objective is a fresh run
/// under current permissions, never automatic replay of historic tool calls.
pub struct FileSkillCapture {
    records: chimera_memory::storage::ProjectRecords,
}
impl FileSkillCapture {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            records: chimera_memory::storage::ProjectRecords::new(root, "skills"),
        }
    }

    async fn successful_source(&self, id: Uuid) -> anyhow::Result<chimera_session::SessionState> {
        use chimera_session::SessionStore;
        let state = chimera_session::FileSessionStore::new(self.records.root())
            .load(id)
            .await?;
        let effective = state.effective_events()?;
        let outcome = effective.iter().rev().find_map(|event| match event {
            chimera_session::SessionEvent::Custom { key, payload } if key == "run_outcome" => {
                payload.get("outcome").and_then(|v| v.as_str())
            }
            _ => None,
        });
        if state.active || outcome != Some("completed") {
            anyhow::bail!("skills require a completed saved source session");
        }
        Ok(state)
    }

    pub async fn capture_from_session(
        &self,
        name: &str,
        description: &str,
        source_session: Uuid,
    ) -> anyhow::Result<SkillId> {
        let state = self.successful_source(source_session).await?;
        let mut tools = std::collections::BTreeSet::new();
        for event in state.effective_events()? {
            match event {
                chimera_session::SessionEvent::ToolCalled { tool, .. } => {
                    tools.insert(tool);
                }
                chimera_session::SessionEvent::Custom { key, payload } if key == "messages" => {
                    if let Some(messages) = payload.as_array() {
                        for message in messages {
                            if let Some(calls) =
                                message.get("tool_calls").and_then(|v| v.as_array())
                            {
                                for call in calls {
                                    if let Some(tool) =
                                        call.pointer("/function/name").and_then(|v| v.as_str())
                                    {
                                        tools.insert(tool.into());
                                    }
                                }
                            }
                        }
                    }
                }
                _ => (),
            }
        }
        let skill = CapturedSkill {
            id: Uuid::new_v4(),
            source_session,
            captured_at: Utc::now(),
            confidence: 0.0,
            spec: SkillSpec {
                name: name.into(),
                description: description.into(),
                version: "1".into(),
                objective_template: state.meta.objective,
                required_creatures: vec![],
                required_tools: tools.into_iter().collect(),
                pre_hooks: vec![],
                post_hooks: vec![],
            },
        };
        self.capture(skill).await
    }

    /// Prepare a fresh authorized objective. Hook commands stay visible and must
    /// pass through the same current permission checks as all other commands.
    pub async fn execution_objective(&self, name: &str) -> anyhow::Result<String> {
        let skill = self.resolve(name).await?;
        Ok(format!("Run the saved workflow '{}' (version {}). Apply current permissions and report actual results.\n\nObjective:\n{}\n\nRequired tools: {}\nPre-run checks (require current approval): {}\nPost-run checks (require current approval): {}",skill.spec.name,skill.spec.version,skill.spec.objective_template,skill.spec.required_tools.join(", "),skill.spec.pre_hooks.join("; "),skill.spec.post_hooks.join("; ")))
    }

    /// Every captured skill with its full specification, newest capture first.
    /// Interactive choosers use this to show details; `SkillCapture::list`
    /// stays name-only for scripts.
    pub async fn list_skills(&self) -> anyhow::Result<Vec<CapturedSkill>> {
        let records: SkillRecords = self.records.read("captured")?;
        let mut skills: Vec<CapturedSkill> = records.skills.into_values().collect();
        skills.sort_by_key(|skill| std::cmp::Reverse((skill.captured_at, skill.id)));
        Ok(skills)
    }
}

fn validate_skill(skill: &CapturedSkill) -> anyhow::Result<()> {
    if skill.spec.name.is_empty()
        || skill.spec.name.len() > 128
        || !skill
            .spec
            .name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        anyhow::bail!("skill name must contain only letters, digits, hyphens, or underscores");
    }
    if skill.spec.objective_template.trim().is_empty() || skill.spec.version.trim().is_empty() {
        anyhow::bail!("skill objective and version are required");
    }
    if !skill.confidence.is_finite() || !(0.0..=1.0).contains(&skill.confidence) {
        anyhow::bail!("skill confidence must be between 0 and 1");
    }
    if serde_json::to_vec(skill)?.len() > 1024 * 1024 {
        anyhow::bail!("skill exceeds 1 MiB limit");
    }
    Ok(())
}

#[async_trait::async_trait]
impl SkillCapture for FileSkillCapture {
    async fn capture(&self, skill: CapturedSkill) -> anyhow::Result<SkillId> {
        validate_skill(&skill)?;
        self.successful_source(skill.source_session).await?;
        self.records
            .update("captured", |records: &mut SkillRecords| {
                let id = skill.id;
                if let Some(old) = records.skills.insert(skill.spec.name.clone(), skill) {
                    records.archived.push(ArchivedSkill {
                        skill: old,
                        reason: "superseded version".into(),
                        at: Utc::now(),
                    });
                }
                Ok(id)
            })
    }
    async fn list(&self) -> anyhow::Result<Vec<String>> {
        let records: SkillRecords = self.records.read("captured")?;
        Ok(records.skills.into_keys().collect())
    }
    async fn resolve(&self, name: &str) -> anyhow::Result<CapturedSkill> {
        let records: SkillRecords = self.records.read("captured")?;
        records
            .skills
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("skill not found: {name}"))
    }
    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        self.records
            .update("captured", |records: &mut SkillRecords| {
                let skill = records
                    .skills
                    .remove(name)
                    .ok_or_else(|| anyhow::anyhow!("skill not found: {name}"))?;
                records.archived.push(ArchivedSkill {
                    skill,
                    reason: "removed by operator".into(),
                    at: Utc::now(),
                });
                Ok(())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

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
    async fn list_skills_returns_full_specs_newest_first() {
        use chimera_session::{SessionEvent, SessionStore};
        let root = root();
        let sessions = chimera_session::FileSessionStore::new(&root);
        let source = sessions
            .create(chimera_session::SessionMeta {
                label: "source".into(),
                objective: "inspect a sample".into(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        sessions
            .append_event_sync(
                source,
                SessionEvent::ToolCalled {
                    tool: "read_file".into(),
                    args: serde_json::json!({"path":"sample.txt"}),
                },
            )
            .unwrap();
        sessions.close(source, "completed").await.unwrap();
        let store = FileSkillCapture::new(&root);
        let mut older = make_skill("older-skill");
        older.source_session = source;
        older.captured_at = Utc.with_ymd_and_hms(2026, 3, 10, 8, 30, 0).unwrap();
        let mut newer = make_skill("newer-skill");
        newer.source_session = source;
        newer.captured_at = Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap();
        store.capture(older).await.unwrap();
        store.capture(newer).await.unwrap();
        let listed = store.list_skills().await.unwrap();
        let names: Vec<String> = listed.iter().map(|s| s.spec.name.clone()).collect();
        assert_eq!(names, vec!["newer-skill", "older-skill"]);
        assert_eq!(listed[0].spec.description, "newer-skill skill");
        assert_eq!(listed[0].spec.required_tools, vec!["bash", "git"]);
        assert_eq!(listed[0].source_session, source);
        // The plain name listing keeps working alongside the detailed one.
        assert_eq!(store.list().await.unwrap().len(), 2);
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
    fn root() -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("chimera-skill-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }
    async fn restored_source(root: &std::path::Path, before: &str, after: &str) -> Uuid {
        use chimera_session::{SessionEvent, SessionStore};
        let sessions = chimera_session::FileSessionStore::new(root);
        let id = sessions
            .create(chimera_session::SessionMeta {
                label: "restored source".into(),
                objective: "inspect a sample".into(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        sessions
            .append_event_sync(
                id,
                SessionEvent::ToolCalled {
                    tool: "read_file".into(),
                    args: serde_json::json!({"path":"sample.txt"}),
                },
            )
            .unwrap();
        sessions.close(id, before).await.unwrap();
        let checkpoint = sessions.snapshot(id).await.unwrap();
        sessions.reopen(id).await.unwrap();
        sessions
            .append_event_sync(
                id,
                SessionEvent::ToolCalled {
                    tool: "write_file".into(),
                    args: serde_json::json!({"path":"abandoned.txt"}),
                },
            )
            .unwrap();
        sessions.close(id, after).await.unwrap();
        sessions
            .restore_checkpoint(id, checkpoint, &[])
            .await
            .unwrap();
        id
    }
    #[tokio::test]
    async fn restored_failed_source_cannot_capture_abandoned_success() {
        let root = root();
        let id = restored_source(&root, "failed", "completed").await;
        let store = FileSkillCapture::new(&root);
        assert!(store
            .capture_from_session("restored", "failed checkpoint", id)
            .await
            .is_err());
        assert!(store.list().await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn restored_completed_source_captures_only_effective_success_and_tools() {
        let root = root();
        let id = restored_source(&root, "completed", "failed").await;
        let store = FileSkillCapture::new(&root);
        store
            .capture_from_session("restored", "successful checkpoint", id)
            .await
            .unwrap();
        let reopened = FileSkillCapture::new(&root);
        assert_eq!(
            reopened
                .resolve("restored")
                .await
                .unwrap()
                .spec
                .required_tools,
            vec!["read_file"]
        );
    }
    #[tokio::test]
    async fn durable_capture_requires_success_and_survives_restart() {
        use chimera_session::SessionStore;
        let root = root();
        let sessions = chimera_session::FileSessionStore::new(&root);
        let store = FileSkillCapture::new(&root);
        let id = sessions
            .create(chimera_session::SessionMeta {
                label: "test".into(),
                objective: "inspect the sample file".into(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        assert!(store
            .capture_from_session("inspect", "inspect sample", id)
            .await
            .is_err());
        sessions
            .append_event_sync(
                id,
                chimera_session::SessionEvent::ToolCalled {
                    tool: "read_file".into(),
                    args: serde_json::json!({"path":"sample.txt"}),
                },
            )
            .unwrap();
        sessions.close(id, "completed").await.unwrap();
        store
            .capture_from_session("inspect", "inspect sample", id)
            .await
            .unwrap();
        let reopened = FileSkillCapture::new(&root);
        let skill = reopened.resolve("inspect").await.unwrap();
        assert_eq!(skill.source_session, id);
        assert_eq!(skill.spec.required_tools, vec!["read_file"]);
        assert_eq!(skill.confidence, 0.0);
        assert!(reopened
            .execution_objective("inspect")
            .await
            .unwrap()
            .contains("current permissions"));
        reopened.remove("inspect").await.unwrap();
        assert!(reopened.resolve("inspect").await.is_err());
        let records: SkillRecords = reopened.records.read("captured").unwrap();
        assert_eq!(records.archived.len(), 1);
    }
    #[tokio::test]
    async fn durable_failed_source_and_other_projects_are_rejected() {
        use chimera_session::SessionStore;
        let first = root();
        let second = root();
        let sessions = chimera_session::FileSessionStore::new(&first);
        let id = sessions
            .create(chimera_session::SessionMeta {
                label: "bad".into(),
                objective: "failed task".into(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        sessions.close(id, "failed").await.unwrap();
        assert!(FileSkillCapture::new(&first)
            .capture_from_session("bad", "failed", id)
            .await
            .is_err());
        sessions.reopen(id).await.unwrap();
        sessions.close(id, "completed").await.unwrap();
        let store = FileSkillCapture::new(&first);
        store
            .capture_from_session("ok", "completed", id)
            .await
            .unwrap();
        let other = FileSkillCapture::new(&second);
        assert!(other.list().await.unwrap().is_empty());
        std::fs::copy(
            store.records.path("captured").unwrap(),
            other.records.path("captured").unwrap(),
        )
        .unwrap();
        assert!(other.list().await.is_err());
    }
}

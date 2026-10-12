//! Project-owned, single-use decisions. Model output cannot write these records.
use std::{collections::BTreeSet, path::PathBuf};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chimera_memory::storage::ProjectRecords;
use chimera_tools::ToolCall;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Decision { Pending, Approved, Denied, Used, Expired, Cancelled }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub id: Uuid, pub session: Uuid, pub call: ToolCall,
    pub created_at: DateTime<Utc>, pub expires_at: DateTime<Utc>,
    pub decision: Decision,
}
pub struct ApprovalStore { records: ProjectRecords }
impl ApprovalStore {
    pub fn new(root: PathBuf) -> Self { Self { records: ProjectRecords::new(root,"approvals") } }
    pub fn request(&self, session:Uuid, call:ToolCall, lifetime:std::time::Duration) -> Result<PendingAction> {
        anyhow::ensure!(lifetime.as_secs() <= 300,"approval lifetime cannot exceed five minutes");
        anyhow::ensure!(serde_json::to_vec(&call)?.len() <= 1_048_576,"action exceeds the approval size limit");
        let now=Utc::now();
        let action=PendingAction {id:Uuid::new_v4(),session,call,created_at:now,expires_at:now+chrono::Duration::from_std(lifetime)?,decision:Decision::Pending};
        self.records.update("actions",|rows:&mut Vec<PendingAction>| {
            anyhow::ensure!(rows.len()<10_000,"approval history reached its limit; archive the project state before continuing");
            rows.push(action.clone()); Ok(())
        })?;
        Ok(action)
    }
    pub fn list(&self) -> Result<Vec<PendingAction>> {
        let mut rows:Vec<PendingAction>=self.records.read("actions")?;
        for row in &mut rows { expire(row); }
        Ok(rows)
    }
    pub fn get(&self,id:Uuid)->Result<PendingAction> { self.list()?.into_iter().find(|r|r.id==id).context("pending action not found in this project") }
    /// Caller must display the stored action. Supplying an identifier cannot alter it.
    pub fn decide(&self,id:Uuid,approved:bool)->Result<PendingAction> {
        self.records.update("actions",|rows:&mut Vec<PendingAction>| {
            let row=rows.iter_mut().find(|r|r.id==id).context("pending action not found in this project")?;
            expire(row);
            anyhow::ensure!(row.decision==Decision::Pending,"action is no longer pending: {:?}",row.decision);
            row.decision=if approved {Decision::Approved} else {Decision::Denied}; Ok(row.clone())
        })
    }
    pub fn consume(&self,id:Uuid,session:Uuid,call:&ToolCall)->Result<()> {
        self.records.update("actions",|rows:&mut Vec<PendingAction>| {
            let row=rows.iter_mut().find(|r|r.id==id).context("pending action not found")?;
            expire(row);
            anyhow::ensure!(row.session==session && serde_json::to_value(&row.call)?==serde_json::to_value(call)?,"approval belongs to a different action or session");
            anyhow::ensure!(row.decision==Decision::Approved,"action was not approved or approval expired: {:?}",row.decision);
            row.decision=Decision::Used; Ok(())
        })
    }
    pub fn cancel(&self,id:Uuid)->Result<()> {
        self.records.update("actions",|rows:&mut Vec<PendingAction>| {
            if let Some(row)=rows.iter_mut().find(|r|r.id==id) {
                if matches!(row.decision,Decision::Pending|Decision::Approved) {row.decision=Decision::Cancelled;}
            } Ok(())
        })
    }
}
fn expire(row:&mut PendingAction) {
    if matches!(row.decision,Decision::Pending|Decision::Approved) && row.expires_at<=Utc::now() {row.decision=Decision::Expired;}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPolicy {
    pub read_only: bool,
    /// none disables tools, low reviews every action, medium reviews mutations.
    pub autonomy: String,
    pub disabled_tools: BTreeSet<String>,
}
impl Default for ProjectPolicy {
    fn default()->Self {Self {read_only:false,autonomy:"medium".into(),disabled_tools:BTreeSet::new()}}
}
pub struct PolicyStore {records:ProjectRecords}
impl PolicyStore {
    pub fn new(root:PathBuf)->Self {Self {records:ProjectRecords::new(root,"policy")}}
    pub fn get(&self)->Result<ProjectPolicy> {
        let policy:ProjectPolicy=self.records.read("settings")?; validate(&policy)?; Ok(policy)
    }
    pub fn change(&self,update:impl FnOnce(&mut ProjectPolicy)->Result<()>)->Result<ProjectPolicy> {
        self.records.update("settings",|p:&mut ProjectPolicy| { update(p)?; validate(p)?; Ok(p.clone()) })
    }
    pub fn check(&self,tool:&str,read_only:bool)->Result<bool> {
        let policy=self.get()?;
        anyhow::ensure!(policy.autonomy!="none","tools are stopped by autonomy none");
        anyhow::ensure!(!policy.disabled_tools.contains(tool),"tool is disabled by project policy: {tool}");
        anyhow::ensure!(!policy.read_only || read_only,"project read-only policy denies: {tool}");
        Ok(policy.autonomy=="low")
    }
}
fn validate(policy:&ProjectPolicy)->Result<()> {
    anyhow::ensure!(matches!(policy.autonomy.as_str(),"none"|"low"|"medium"),"autonomy must be none, low or medium; automatic approval of mutations is unavailable");
    anyhow::ensure!(policy.disabled_tools.len()<=256 && policy.disabled_tools.iter().all(|n|!n.is_empty()&&n.len()<=128),"invalid disabled tool list");
    Ok(())
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn display_masks_credentials_without_changing_exact_action() {
        let original=serde_json::json!({"tool":"mcp_server_start","args":{"server":{"command":"python3","args":["server.py"],"env":{"SERVICE_KEY":"fake-secret-marker","MODE":"private-mode"}},"token":"fake-token-marker","path":"answer.txt"}});
        let shown=display_value(original.clone());
        assert!(!shown.to_string().contains("fake-secret-marker"));
        assert!(!shown.to_string().contains("fake-token-marker"));
        assert!(!shown.to_string().contains("private-mode"));
        assert_eq!(shown["args"]["server"]["command"],"python3");
        assert_eq!(shown["args"]["path"],"answer.txt");
        assert_eq!(original["args"]["server"]["env"]["SERVICE_KEY"],"fake-secret-marker");
    }
    fn root()->PathBuf {let p=std::env::temp_dir().join(format!("chimera-approval-{}",Uuid::new_v4())); std::fs::create_dir(&p).unwrap();p}
    fn call()->ToolCall {ToolCall {tool:"write_file".into(),args:serde_json::json!({"path":"answer.txt","content":"yes"}),world:None,requested_by:Uuid::new_v4()}}
    #[test] fn restarted_store_consumes_only_the_exact_action_once() {
        let root=root();let session=Uuid::new_v4();let call=call();
        let store=ApprovalStore::new(root.clone());let row=store.request(session,call.clone(),std::time::Duration::from_secs(60)).unwrap();
        let other=ApprovalStore::new(root);other.decide(row.id,true).unwrap();
        let mut changed=call.clone();changed.args["content"]=serde_json::json!("changed");
        assert!(store.consume(row.id,session,&changed).is_err());
        assert!(store.consume(row.id,Uuid::new_v4(),&call).is_err());
        store.consume(row.id,session,&call).unwrap();
        assert!(other.consume(row.id,session,&call).is_err());assert!(other.decide(row.id,true).is_err());
    }
    #[test] fn denied_expired_cancelled_and_other_project_cannot_release() {
        let store=ApprovalStore::new(root());let session=Uuid::new_v4();let call=call();
        let expired=store.request(session,call.clone(),std::time::Duration::ZERO).unwrap();assert!(store.decide(expired.id,true).is_err());
        let denied=store.request(session,call.clone(),std::time::Duration::from_secs(60)).unwrap();store.decide(denied.id,false).unwrap();assert!(store.consume(denied.id,session,&call).is_err());
        let cancelled=store.request(session,call.clone(),std::time::Duration::from_secs(60)).unwrap();store.decide(cancelled.id,true).unwrap();store.cancel(cancelled.id).unwrap();assert!(store.consume(cancelled.id,session,&call).is_err());
        assert!(ApprovalStore::new(root()).decide(cancelled.id,true).is_err());
    }
    #[test] fn policy_changes_apply_to_already_open_instances() {
        let root=root();let first=PolicyStore::new(root.clone());let second=PolicyStore::new(root);
        assert!(!first.check("read_file",true).unwrap());
        second.change(|p|{p.autonomy="low".into();Ok(())}).unwrap();assert!(first.check("read_file",true).unwrap());
        second.change(|p|{p.disabled_tools.insert("read_file".into());Ok(())}).unwrap();assert!(first.check("read_file",true).is_err());
        second.change(|p|{p.read_only=true;Ok(())}).unwrap();assert!(first.check("write_file",false).is_err());
        assert!(second.change(|p|{p.autonomy="full".into();Ok(())}).is_err());assert_eq!(first.get().unwrap().autonomy,"low");
    }
}
/// Hide configured credentials on screens and in command output. Exact approval
/// binding continues to use the original action in private project storage.
pub fn display_value(mut value: serde_json::Value) -> serde_json::Value {
    fn visit(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => for (key, value) in fields {
                let key=key.to_ascii_lowercase();
                if key=="env" || key=="environment" {
                    if let Some(vars)=value.as_object_mut() {
                        for value in vars.values_mut() { *value=serde_json::json!("[private configured value]"); }
                    } else { *value=serde_json::json!("[private configured value]"); }
                } else if key.contains("password") || key.contains("secret") || key.contains("api_key") || key.contains("apikey") || key=="authorization" || key=="token" || key.ends_with("_token") {
                    *value=serde_json::json!("[private configured value]");
                } else { visit(value); }
            },
            serde_json::Value::Array(items) => for item in items {visit(item)},
            _=>(),
        }
    }
    visit(&mut value);
    value
}


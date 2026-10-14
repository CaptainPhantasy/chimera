use super::*;
use chimera_comms::{FileCommsBridge,Channel,Notification,Priority,DeliveryStatus};

/// Saved URLs can contain access tokens. Display the named destination instead.
fn without_urls(mut value:Value)->Value {
    fn walk(value:&mut Value) {
        match value {
            Value::Object(map)=>for (key,value) in map {if key=="url" {*value=json!("[private configured URL]");}else{walk(value)}},
            Value::Array(rows)=>for value in rows {walk(value)},
            _=>(),
        }
    }
    walk(&mut value);value
}
async fn send(bridge:&FileCommsBridge,id:uuid::Uuid,format:OutputFormat,profile:Option<&str>)->Result<()> {
    anyhow::ensure!(!policy(profile,false)?.read_only,"read-only policy denies message delivery");
    let row=bridge.get(id)?;
    let action=chimera_tools::ToolCall {tool:"deliver_notification".into(),args:without_urls(json!({"notification":row})),world:None,requested_by:uuid::Uuid::new_v4()};
    if let Err(error)=commands::approve(action,format).await {bridge.cancel(id)?;return Err(error);}
    let result=bridge.deliver(id).await?;
    let value=without_urls(json!(result));
    output(format,value.clone(),serde_json::to_string_pretty(&value)?)?;
    anyhow::ensure!(result.status==DeliveryStatus::Delivered,"message was not delivered: {}",result.error.as_deref().unwrap_or("expired, cancelled or unavailable"));
    Ok(())
}
fn prepare_approval_notification(bridge:&FileCommsBridge,action:&chimera_core::approval::PendingAction,channel:Channel)->Result<()> {
    let id=action.id;
    let shown=chimera_core::approval::display_value(serde_json::to_value(&action.call)?);
    bridge.prepare_approval(id,chimera_comms::CommsApprovalPrompt {channel,actor:action.session.to_string(),class:"exact_action".into(),action:serde_json::to_string(&shown)?,reason:format!("Review this exact action before {}. In an authenticated terminal for this project, use chimera approve action {id} or chimera deny {id}. Receiving this message grants no permission.",action.expires_at),touched_resources:Vec::new()},action.expires_at)?;
    Ok(())
}
pub(crate) async fn execute(cmd:NotifyCmd,format:OutputFormat,profile:Option<&str>)->Result<()> {
    let bridge=FileCommsBridge::new(workspace_root())?;
    match cmd {
        NotifyCmd::Destination {name,url,terminal}=> {
            anyhow::ensure!(terminal != url.is_some(),"choose exactly one of --terminal and --url");
            let channel=if terminal {Channel::Terminal} else {Channel::Webhook {url:url.unwrap()}};
            let row=bridge.configure_destination(&name,channel)?;
            output(format,without_urls(json!(row)),format!("Saved destination {name}. Each message still needs review."))
        }
        NotifyCmd::Destinations=> {let value=without_urls(json!({"destinations":bridge.destinations()?}));output(format,value.clone(),serde_json::to_string_pretty(&value)?)},
        NotifyCmd::Disable {name}=> {bridge.disable_destination(&name)?;output(format,json!({"disabled":name}),format!("Disabled {name}"))},
        NotifyCmd::Send {destination,subject,body}=> {
            let dest=bridge.destinations()?.into_iter().find(|d|d.name==destination).context("destination not found")?;
            let row=bridge.prepare(&destination,Notification {channel:dest.channel,priority:Priority::Normal,subject,body,metadata:None},std::time::Duration::from_secs(300))?;
            send(&bridge,row.id,format,profile).await
        }
        NotifyCmd::Approval {id,destination}=> {
            let id=uuid::Uuid::parse_str(&id)?;
            let action=chimera_core::approval::ApprovalStore::new(workspace_root()).get(id)?;
            anyhow::ensure!(action.decision==chimera_core::approval::Decision::Pending,"action is no longer pending");
            let dest=bridge.destinations()?.into_iter().find(|d|d.name==destination).context("destination not found")?;
            prepare_approval_notification(&bridge,&action,dest.channel)?;
            send(&bridge,id,format,profile).await
        }
        NotifyCmd::List=> {let value=without_urls(json!({"notifications":bridge.list()?}));output(format,value.clone(),serde_json::to_string_pretty(&value)?)},
        NotifyCmd::Inspect {id}=> {let value=without_urls(json!(bridge.get(uuid::Uuid::parse_str(&id)?)?));output(format,value.clone(),serde_json::to_string_pretty(&value)?)},
        NotifyCmd::Inbox=> {let value=without_urls(json!({"inbox":bridge.inbox()?}));output(format,value.clone(),serde_json::to_string_pretty(&value)?)},
        NotifyCmd::Acknowledge {id}=> {let value=without_urls(json!(bridge.acknowledge(uuid::Uuid::parse_str(&id)?)?));output(format,value.clone(),serde_json::to_string_pretty(&value)?)},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_notification_keeps_credentials_out_of_body_and_metadata() {
        let root=std::env::temp_dir().join(format!("chimera-notification-privacy-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let bridge=FileCommsBridge::new(root.clone()).unwrap();
        bridge.configure_destination("review",Channel::Terminal).unwrap();
        let store=chimera_core::approval::ApprovalStore::new(root);
        let actor=uuid::Uuid::new_v4();
        let call=chimera_tools::ToolCall {tool:"external_server".into(),args:json!({"command":"server.py","env":{"SERVICE_KEY":"fake-env-secret"},"api_key":"fake-key-secret"}),world:None,requested_by:actor};
        let action=store.request(actor,call.clone(),std::time::Duration::from_secs(300)).unwrap();
        prepare_approval_notification(&bridge,&action,Channel::Terminal).unwrap();
        let displayed=serde_json::to_string(&bridge.get(action.id).unwrap()).unwrap();
        assert!(!displayed.contains("fake-env-secret"),"approval message exposes an environment credential");
        assert!(!displayed.contains("fake-key-secret"),"approval message exposes an API credential");
        assert!(displayed.contains("server.py"),"the visible action must still identify the program");
        assert_eq!(serde_json::to_value(store.get(action.id).unwrap().call).unwrap(),serde_json::to_value(call.clone()).unwrap());
        // Sending a review request cannot approve the action or change its binding.
        assert_eq!(store.get(action.id).unwrap().decision,chimera_core::approval::Decision::Pending);
        store.decide(action.id,true).unwrap();
        store.consume(action.id,actor,&call).unwrap();
        assert!(store.consume(action.id,actor,&call).is_err());
    }
}

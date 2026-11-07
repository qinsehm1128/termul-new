//! Composition shared by desktop, standalone, MCP bridge and web adapters.
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use super::{
    host_capabilities::{HostCapabilityBackend, HostCapabilityError},
    host_policy::{profile_fingerprint, HostAccessPolicy, HostGrants, HostPolicyStore},
    workspace::WorkspaceCatalog,
};
use crate::{
    ssh::automation::{CallContext, ProfileSource, SshAutomationService},
    web::project_registry::ProjectRegistry,
};

pub struct HostCapabilityService {
    pub policy: Arc<HostPolicyStore>,
    pub profiles: Arc<dyn ProfileSource>,
    pub registry: Arc<ProjectRegistry>,
    ssh: Arc<SshAutomationService>,
    grants: Arc<HostGrants>,
    host_mode: &'static str,
}

impl HostCapabilityService {
    pub fn new(
        policy: Arc<HostPolicyStore>,
        profiles: Arc<dyn ProfileSource>,
        registry: Arc<ProjectRegistry>,
        ssh: Arc<SshAutomationService>,
        host_mode: &'static str,
    ) -> Self {
        let grants = Arc::new(HostGrants {
            store: policy.clone(),
            profiles: profiles.clone(),
            caller_id: "mcp-global".into(),
        });
        Self {
            policy,
            profiles,
            registry,
            ssh,
            grants,
            host_mode,
        }
    }

    /// Management adapters must authorize the operator before calling this.
    pub fn access_view(&self) -> Result<Value, HostCapabilityError> {
        let profiles = self.profiles.list_profiles().map_err(|_| {
            HostCapabilityError::new("PROFILES_UNAVAILABLE", "SSH profiles cannot be read")
        })?;
        let profiles: Vec<_> = profiles
            .iter()
            .map(|p| {
                json!({
                    "profileId":p.id,"name":p.name,"host":p.host,"port":p.port,
                    "username":p.username,"authMethod":p.auth_method,"tags":p.tags,
                    "fingerprint":profile_fingerprint(p)
                })
            })
            .collect();
        let projects: Vec<_> = self
            .registry
            .snapshot()
            .projects
            .iter()
            .map(|p| json!({"id":p.id,"name":p.name}))
            .collect();
        Ok(
            json!({"policy":self.policy.snapshot(),"profiles":profiles,"projects":projects,"hostMode":self.host_mode}),
        )
    }

    pub fn put_access(
        &self,
        expected_revision: u64,
        policy: HostAccessPolicy,
    ) -> Result<HostAccessPolicy, HostCapabilityError> {
        let profiles = self.profiles.list_profiles().map_err(|_| {
            HostCapabilityError::new("PROFILES_UNAVAILABLE", "SSH profiles cannot be read")
        })?;
        self.policy.put(expected_revision, policy, &profiles)
    }
}

#[async_trait]
impl HostCapabilityBackend for HostCapabilityService {
    async fn call(
        &self,
        provider: &str,
        tool: &str,
        arguments: Map<String, Value>,
        cancellation: CancellationToken,
    ) -> Result<Value, HostCapabilityError> {
        let policy = self.policy.snapshot();
        if !policy.enabled {
            return Err(HostCapabilityError::new(
                "PERMISSION_DENIED",
                "Host capabilities are not enabled",
            ));
        }
        if arguments
            .keys()
            .any(|key| matches!(key.as_str(), "callerId" | "grant" | "confirmed"))
        {
            return Err(HostCapabilityError::new(
                "INVALID_ARGUMENTS",
                "Caller authority cannot be supplied as arguments",
            ));
        }
        if cancellation.is_cancelled() {
            return Err(HostCapabilityError::new("CANCELLED", "Request cancelled"));
        }
        if provider == "ssh" && tool.starts_with("job_") && !cfg!(unix) {
            return Err(HostCapabilityError::new(
                "UNSUPPORTED_PLATFORM",
                "Durable SSH jobs require a Unix Se host",
            ));
        }
        match provider {
            "workspace" => WorkspaceCatalog::new(self.registry.clone()).call(
                tool,
                arguments,
                &policy.workspace,
            ),
            "ssh" => self
                .ssh
                .call_cancellable(
                    tool,
                    Value::Object(arguments),
                    CallContext {
                        caller_id: self.grants.caller_id.clone(),
                        grants: self.grants.clone(),
                    },
                    cancellation,
                )
                .await
                .map_err(|error| HostCapabilityError::new(&error.code, &error.message)),
            _ => Err(HostCapabilityError::new(
                "TOOL_NOT_FOUND",
                "Unknown host capability",
            )),
        }
    }
}

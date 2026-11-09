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

pub use crate::ssh::command_policy::CommandPolicyPreview;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandPolicyPreviewRequest {
    pub profile_id: String,
    pub operation: String,
    pub command: String,
    pub policy: Option<HostAccessPolicy>,
}

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

    /// Local management diagnostic only: a non-match is never an execution grant.
    pub fn preview(
        &self,
        request: CommandPolicyPreviewRequest,
    ) -> Result<CommandPolicyPreview, HostCapabilityError> {
        if !["execute", "job_start"].contains(&request.operation.as_str())
            || request.profile_id.is_empty()
            || request.profile_id.len() > 256
        {
            return Err(HostCapabilityError::new(
                "INVALID_ARGUMENTS",
                "Invalid preview request",
            ));
        }
        crate::ssh::command_policy::validate_command(&request.command).map_err(|_| {
            HostCapabilityError::new("INVALID_ARGUMENTS", "Invalid preview command")
        })?;
        let snapshot = self.policy.compiled_snapshot();
        let revision = snapshot.policy.revision;
        let draft = request.policy.is_some();
        let (policy, commands) = match request.policy {
            Some(policy) => {
                let commands = Arc::new(super::host_policy::compile(&policy)?);
                (policy, commands)
            }
            None => (snapshot.policy, snapshot.commands),
        };
        let profiles = self.profiles.list_profiles().map_err(|_| {
            HostCapabilityError::new("PROFILES_UNAVAILABLE", "SSH profiles cannot be read")
        })?;
        let profile = profiles.iter().find(|p| p.id == request.profile_id);
        let grant = policy.ssh.profiles.get(&request.profile_id);
        let blocked_by = if !policy.enabled
            || !grant.is_some_and(|g| g.operations.contains(&request.operation))
        {
            Some("permission_denied")
        } else if profile.is_none() {
            Some("profile_not_found")
        } else if !draft
            && policy.profile_fingerprints.get(&request.profile_id)
                != profile.map(profile_fingerprint).as_ref()
        {
            Some("profile_identity_changed")
        } else {
            None
        };
        let matches = commands
            .evaluate(&request.profile_id, &request.command)
            .map_err(|_| {
                HostCapabilityError::new("INVALID_ARGUMENTS", "Invalid preview command")
            })?;
        Ok(CommandPolicyPreview {
            outcome: if matches.is_empty() {
                "not_matched"
            } else {
                "matched"
            }
            .into(),
            matches,
            policy_revision: revision,
            draft,
            blocked_by: blocked_by.map(str::to_string),
        })
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

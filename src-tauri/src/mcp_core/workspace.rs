//! Read-only workspace catalog. Authorization precedes all derived metadata.
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::host_capabilities::HostCapabilityError;
use crate::web::project_registry::{ProjectRegistry, ProjectSummary};

type Result<T> = std::result::Result<T, HostCapabilityError>;

/// Host-owned grant, never accepted from tool arguments. None selects the full
/// catalog only when explicitly enabled; Some(empty) grants no projects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct WorkspaceGrant {
    pub enabled: bool,
    pub project_ids: Option<BTreeSet<String>>,
    pub include_paths: bool,
}

impl Default for WorkspaceGrant {
    fn default() -> Self {
        Self {
            enabled: false,
            project_ids: Some(BTreeSet::new()),
            include_paths: false,
        }
    }
}

pub struct WorkspaceCatalog {
    registry: Arc<ProjectRegistry>,
}

impl WorkspaceCatalog {
    pub fn new(registry: Arc<ProjectRegistry>) -> Self {
        Self { registry }
    }

    pub fn call(
        &self,
        tool: &str,
        args: Map<String, Value>,
        grant: &WorkspaceGrant,
    ) -> Result<Value> {
        if !grant.enabled {
            return Err(HostCapabilityError::new(
                "FORBIDDEN",
                "workspace access is disabled",
            ));
        }
        let allowed: &[&str] = match tool {
            "group_list" => &["query", "cursor", "limit", "includeEmpty"],
            "project_list" => &[
                "groupId",
                "ungrouped",
                "query",
                "includeArchived",
                "cursor",
                "limit",
            ],
            "project_get" => &["projectId"],
            _ => {
                return Err(HostCapabilityError::new(
                    "NOT_FOUND",
                    "workspace tool not found",
                ))
            }
        };
        if args.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(invalid());
        }
        // Parse all arguments before touching the registry, including optional
        // fields: JSON null and wrong types are not silently defaulted.
        let query = string(&args, "query")?.map(str::to_lowercase);
        let group_id = string(&args, "groupId")?;
        let project_id = string(&args, "projectId")?;
        let ungrouped = boolean(&args, "ungrouped", false)?;
        let archived = boolean(&args, "includeArchived", false)?;
        let include_empty = boolean(&args, "includeEmpty", true)?;
        if group_id.is_some() && ungrouped {
            return Err(invalid());
        }
        let limit = match args.get("limit") {
            None => 100,
            Some(v) => v
                .as_u64()
                .filter(|n| (1..=200).contains(n))
                .ok_or_else(invalid)? as usize,
        };
        let offset = match string(&args, "cursor")? {
            None => 0,
            Some(cursor) => {
                let raw = cursor.strip_prefix("offset:").ok_or_else(invalid)?;
                if raw.is_empty()
                    || !raw.bytes().all(|b| b.is_ascii_digit())
                    || (raw.len() > 1 && raw.starts_with('0'))
                {
                    return Err(invalid());
                }
                raw.parse::<usize>().map_err(|_| invalid())?
            }
        };
        if tool == "project_get" && project_id.is_none() {
            return Err(invalid());
        }
        let state = self.registry.snapshot_with_state();
        if !state.ready {
            return Err(HostCapabilityError::new(
                "REGISTRY_NOT_READY",
                "workspace registry is not synchronized",
            ));
        }
        let mut payload = state.payload;
        payload.projects.retain(|p| {
            grant
                .project_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&p.id))
        });
        let visible: HashSet<_> = payload.projects.iter().map(|p| p.id.as_str()).collect();
        payload.default_project_id = payload
            .default_project_id
            .filter(|id| visible.contains(id.as_str()));
        for group in &mut payload.groups {
            group.project_ids.retain(|id| visible.contains(id.as_str()));
            group.preferred_project_id = group
                .preferred_project_id
                .take()
                .filter(|id| group.project_ids.contains(id));
        }
        payload
            .groups
            .retain(|g| grant.project_ids.is_none() || !g.project_ids.is_empty());
        let grouped: HashSet<_> = payload
            .groups
            .iter()
            .flat_map(|g| g.project_ids.iter().map(String::as_str))
            .collect();
        let meta =
            json!({"source":"project_registry", "observedAt":state.observed_at, "ready":true});
        let project_value = |p: &ProjectSummary| {
            let group_ids: Vec<_> = payload
                .groups
                .iter()
                .filter(|g| g.project_ids.contains(&p.id))
                .map(|g| &g.id)
                .collect();
            let mut value = json!({"id":p.id,"name":p.name,"color":p.color,"isArchived":p.is_archived,
                "isDefault":payload.default_project_id.as_deref()==Some(p.id.as_str()),"groupIds":group_ids});
            if grant.include_paths {
                value["path"] = json!(p.path);
            }
            value
        };
        match tool {
            "project_get" => {
                let p = payload
                    .projects
                    .iter()
                    .find(|p| Some(p.id.as_str()) == project_id)
                    .ok_or_else(not_found)?;
                Ok(json!({"project":project_value(p),"meta":meta}))
            }
            "project_list" => {
                let selected_group = group_id
                    .map(|id| {
                        payload
                            .groups
                            .iter()
                            .find(|g| g.id == id)
                            .ok_or_else(not_found)
                    })
                    .transpose()?;
                let projects: Vec<_> = payload
                    .projects
                    .iter()
                    .filter(|p| archived || !p.is_archived)
                    .filter(|p| {
                        query
                            .as_ref()
                            .is_none_or(|q| p.name.to_lowercase().contains(q))
                    })
                    .filter(|p| selected_group.is_none_or(|g| g.project_ids.contains(&p.id)))
                    .filter(|p| !ungrouped || !grouped.contains(p.id.as_str()))
                    .collect();
                let (end, next) = page(projects.len(), offset, limit)?;
                let values: Vec<_> = projects[offset..end]
                    .iter()
                    .map(|p| project_value(p))
                    .collect();
                // Default metadata follows the same archive/query/group filters.
                let default = payload
                    .default_project_id
                    .as_ref()
                    .filter(|id| projects.iter().any(|p| &p.id == *id));
                Ok(
                    json!({"projects":values,"total":projects.len(),"nextCursor":next,"defaultProjectId":default,"meta":meta}),
                )
            }
            "group_list" => {
                let groups: Vec<_> = payload
                    .groups
                    .iter()
                    .filter(|g| include_empty || !g.project_ids.is_empty())
                    .filter(|g| {
                        query
                            .as_ref()
                            .is_none_or(|q| g.name.to_lowercase().contains(q))
                    })
                    .collect();
                let (end, next) = page(groups.len(), offset, limit)?;
                let values: Vec<_> = groups[offset..end].iter().map(|g| json!({"id":g.id,"name":g.name,"color":g.color,
                    "projectIds":g.project_ids,"projectCount":g.project_ids.len(),"preferredProjectId":g.preferred_project_id})).collect();
                let ungrouped_count = payload
                    .projects
                    .iter()
                    .filter(|p| !grouped.contains(p.id.as_str()))
                    .count();
                Ok(
                    json!({"groups":values,"total":groups.len(),"nextCursor":next,"ungroupedProjectCount":ungrouped_count,
                    "defaultProjectId":payload.default_project_id,"meta":meta}),
                )
            }
            _ => unreachable!(),
        }
    }
}

fn invalid() -> HostCapabilityError {
    HostCapabilityError::new("INVALID_ARGUMENTS", "invalid workspace arguments")
}
fn not_found() -> HostCapabilityError {
    HostCapabilityError::new("NOT_FOUND", "workspace item not found")
}
fn string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match args.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
            .map(Some)
            .ok_or_else(invalid),
    }
}
fn boolean(args: &Map<String, Value>, key: &str, default: bool) -> Result<bool> {
    args.get(key)
        .map_or(Ok(default), |v| v.as_bool().ok_or_else(invalid))
}
fn page(total: usize, offset: usize, limit: usize) -> Result<(usize, Option<String>)> {
    if offset > total {
        return Err(invalid());
    }
    let end = offset.saturating_add(limit).min(total);
    Ok((end, (end < total).then(|| format!("offset:{end}"))))
}

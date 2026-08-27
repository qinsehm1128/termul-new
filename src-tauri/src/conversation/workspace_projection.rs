//! Conservative projection of preserved project workspace manifests into one Conversation.
//!
//! Projection is permitted only when attribution is unique. Editor references and pane geometry are
//! portable; terminal references require an exact durable ConversationId binding. No raw claim or
//! live PTY ownership is inferred, and source bytes/checksums are never changed.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::acp::workspace_manifest::{PaneNode as LegacyPaneNode, WorkspaceManifestFile};
use crate::conversation::contracts::ConversationId;
use crate::conversation::migration::{
    RecoveryItemV1, RecoveryKind, RecoveryProvenanceV1, RecoverySeverity,
};
use crate::conversation::session_workspace::{
    SessionWorkspaceLeafNode, SessionWorkspacePaneNode, SessionWorkspaceProjectionState,
    SessionWorkspaceResourceDescriptor, SessionWorkspaceSplitNode, SessionWorkspaceV1,
    SESSION_WORKSPACE_SCHEMA_VERSION,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceProjectionReceiptV1 {
    pub source_path: String,
    pub source_sha256: String,
    pub conversation_id: ConversationId,
    pub projected_editor_count: usize,
    pub projected_terminal_count: usize,
    pub unresolved_terminal_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceProjectionOutcome {
    Projected {
        workspace: SessionWorkspaceV1,
        receipt: WorkspaceProjectionReceiptV1,
    },
    RecoveryRequired {
        item: RecoveryItemV1,
    },
}

pub struct LegacyWorkspaceProjector;

impl LegacyWorkspaceProjector {
    #[allow(clippy::too_many_arguments)]
    pub fn project(
        source_bytes: &[u8],
        source_path: &str,
        source_sha256: &str,
        target_conversation_id: ConversationId,
        candidate_conversation_ids: &[ConversationId],
        exact_terminal_bindings: &HashMap<String, ConversationId>,
        update_identity: Option<String>,
        updated_at_utc: String,
    ) -> WorkspaceProjectionOutcome {
        let mut candidates = candidate_conversation_ids.to_vec();
        candidates.sort_by_key(ToString::to_string);
        candidates.dedup();
        if candidates != [target_conversation_id] {
            return WorkspaceProjectionOutcome::RecoveryRequired {
                item: ambiguous_item(
                    source_path,
                    source_sha256,
                    candidates,
                    "legacy manifest attribution is not unique",
                ),
            };
        }
        let legacy: WorkspaceManifestFile =
            match serde_json::from_slice::<WorkspaceManifestFile>(source_bytes) {
                Ok(value) if value.schema_version == 1 => value,
                _ => {
                    return WorkspaceProjectionOutcome::RecoveryRequired {
                        item: ambiguous_item(
                            source_path,
                            source_sha256,
                            vec![target_conversation_id],
                            "legacy manifest is corrupt or unsupported",
                        ),
                    }
                }
            };
        let allowed_terminals = legacy
            .manifest
            .terminals
            .iter()
            .filter(|terminal| {
                exact_terminal_bindings.get(&terminal.terminal_id) == Some(&target_conversation_id)
            })
            .map(|terminal| terminal.terminal_id.clone())
            .collect::<HashSet<_>>();
        let all_terminal_ids = legacy
            .manifest
            .terminals
            .iter()
            .map(|terminal| terminal.terminal_id.clone())
            .collect::<HashSet<_>>();
        let editor_ids = legacy
            .manifest
            .editors
            .iter()
            .map(|editor| editor.editor_id.clone())
            .collect::<HashSet<_>>();
        let topology = legacy
            .manifest
            .topology
            .as_ref()
            .map(|node| project_node(node, &allowed_terminals, &editor_ids));
        let mut unresolved_terminal_ids = all_terminal_ids
            .difference(&allowed_terminals)
            .cloned()
            .collect::<Vec<_>>();
        unresolved_terminal_ids.sort();
        let mut resources = legacy
            .manifest
            .editors
            .iter()
            .map(|editor| SessionWorkspaceResourceDescriptor::Editor {
                editor_id: editor.editor_id.clone(),
                file_path: editor.file_path.clone(),
            })
            .collect::<Vec<_>>();
        let mut projected_terminal_ids = allowed_terminals.into_iter().collect::<Vec<_>>();
        projected_terminal_ids.sort();
        resources.extend(projected_terminal_ids.iter().map(|terminal_id| {
            SessionWorkspaceResourceDescriptor::Terminal {
                terminal_id: terminal_id.clone(),
                terminal_record_id: None,
                conversation_id: target_conversation_id,
            }
        }));
        let receipt = WorkspaceProjectionReceiptV1 {
            source_path: source_path.to_string(),
            source_sha256: source_sha256.to_string(),
            conversation_id: target_conversation_id,
            projected_editor_count: legacy.manifest.editors.len(),
            projected_terminal_count: projected_terminal_ids.len(),
            unresolved_terminal_ids,
        };
        let projected_resource_count = resources.len();
        let unresolved_resource_count = receipt.unresolved_terminal_ids.len();
        WorkspaceProjectionOutcome::Projected {
            workspace: SessionWorkspaceV1 {
                schema_version: SESSION_WORKSPACE_SCHEMA_VERSION,
                conversation_id: target_conversation_id,
                revision: 0,
                updated_at_utc,
                update_identity,
                topology,
                active_pane_id: legacy.manifest.active_pane_id,
                resources,
                projection_state: SessionWorkspaceProjectionState::Projected {
                    source_path: source_path.to_string(),
                    source_sha256: source_sha256.to_string(),
                    projected_resource_count,
                    unresolved_resource_count,
                },
            },
            receipt,
        }
    }
}

fn project_node(
    node: &LegacyPaneNode,
    allowed_terminals: &HashSet<String>,
    editor_ids: &HashSet<String>,
) -> SessionWorkspacePaneNode {
    match node {
        LegacyPaneNode::Split(split) => SessionWorkspacePaneNode::Split(SessionWorkspaceSplitNode {
            id: split.id.clone(),
            direction: match split.direction {
                crate::acp::workspace_manifest::PaneDirection::Horizontal => {
                    crate::conversation::session_workspace::SessionWorkspacePaneDirection::Horizontal
                }
                crate::acp::workspace_manifest::PaneDirection::Vertical => {
                    crate::conversation::session_workspace::SessionWorkspacePaneDirection::Vertical
                }
            },
            children: split
                .children
                .iter()
                .map(|child| project_node(child, allowed_terminals, editor_ids))
                .collect(),
            sizes: split.sizes.clone(),
        }),
        LegacyPaneNode::Leaf(leaf) => {
            let terminal_ids = leaf
                .terminal_ids
                .iter()
                .filter(|terminal_id| allowed_terminals.contains(*terminal_id))
                .cloned()
                .collect::<Vec<_>>();
            let editor_ids = leaf
                .editor_ids
                .iter()
                .filter(|editor_id| editor_ids.contains(*editor_id))
                .cloned()
                .collect::<Vec<_>>();
            let active_tab_id = leaf.active_tab_id.clone().filter(|active| {
                terminal_ids
                    .iter()
                    .any(|terminal_id| active == terminal_id || active == &format!("term-{terminal_id}"))
                    || editor_ids.contains(active)
            });
            SessionWorkspacePaneNode::Leaf(SessionWorkspaceLeafNode {
                id: leaf.id.clone(),
                terminal_ids,
                editor_ids,
                active_tab_id,
            })
        }
    }
}

fn ambiguous_item(
    source_path: &str,
    source_sha256: &str,
    conversation_ids: Vec<ConversationId>,
    reason: &str,
) -> RecoveryItemV1 {
    RecoveryItemV1::new(
        RecoveryKind::AmbiguousWorkspaceManifest,
        RecoverySeverity::Warning,
        vec![source_path.to_string()],
        conversation_ids,
        vec![source_sha256.to_string()],
        vec![json!({"reason":reason})],
        vec![RecoveryProvenanceV1 {
            source_kind: "legacy_workspace_manifests".to_string(),
            relative_path: source_path.to_string(),
            sha256: source_sha256.to_string(),
            preserved_read_only: true,
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::workspace_manifest::{
        EditorDescriptor, LeafNode, PaneNode, TerminalDescriptor, WorkspaceManifest,
        WorkspaceManifestFile,
    };
    use sha2::{Digest, Sha256};

    const ID: &str = "018f7a1c-1b4d-7c8a-9f01-0123456789ab";
    const OTHER: &str = "5f7a1c01-4d1b-4c8a-af01-0123456789ab";

    fn source() -> Vec<u8> {
        serde_json::to_vec(&WorkspaceManifestFile {
            schema_version: 1,
            manifest: WorkspaceManifest {
                project_id: "project-one".to_string(),
                revision: 7,
                update_identity: None,
                updated_at: 9,
                topology: Some(PaneNode::Leaf(LeafNode {
                    id: "leaf-one".to_string(),
                    terminal_ids: vec!["terminal-bound".to_string(), "terminal-shared".to_string()],
                    editor_ids: vec!["editor-one".to_string()],
                    active_tab_id: Some("editor-one".to_string()),
                })),
                active_pane_id: Some("leaf-one".to_string()),
                focused_session_id: Some("opaque-session".to_string()),
                terminals: vec![
                    TerminalDescriptor {
                        terminal_id: "terminal-bound".to_string(),
                        project_id: "project-one".to_string(),
                        shell: "bash".to_string(),
                        cwd: "/project".to_string(),
                        name: "bound".to_string(),
                        worktree_id: None,
                        claim_handle: None,
                    },
                    TerminalDescriptor {
                        terminal_id: "terminal-shared".to_string(),
                        project_id: "project-one".to_string(),
                        shell: "bash".to_string(),
                        cwd: "/project".to_string(),
                        name: "shared".to_string(),
                        worktree_id: None,
                        claim_handle: None,
                    },
                ],
                editors: vec![EditorDescriptor {
                    editor_id: "editor-one".to_string(),
                    file_path: "/project/src/main.ts".to_string(),
                }],
            },
        })
        .unwrap()
    }

    fn checksum(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn unique_projection_preserves_geometry_and_only_exact_terminal_bindings() {
        let bytes = source();
        let before = bytes.clone();
        let id = ConversationId::parse(ID).unwrap();
        let outcome = LegacyWorkspaceProjector::project(
            &bytes,
            "legacy_workspace_manifests/0/project-one.json",
            &checksum(&bytes),
            id,
            &[id],
            &HashMap::from([("terminal-bound".to_string(), id)]),
            Some("migration".to_string()),
            "2026-08-15T10:00:00.000Z".to_string(),
        );
        let WorkspaceProjectionOutcome::Projected { workspace, receipt } = outcome else {
            panic!("expected projection");
        };
        assert_eq!(receipt.projected_terminal_count, 1);
        assert_eq!(receipt.unresolved_terminal_ids, vec!["terminal-shared"]);
        assert_eq!(receipt.projected_editor_count, 1);
        assert_eq!(workspace.resources.len(), 2);
        let SessionWorkspacePaneNode::Leaf(leaf) = workspace.topology.unwrap() else {
            panic!("expected leaf");
        };
        assert_eq!(leaf.id, "leaf-one");
        assert_eq!(leaf.terminal_ids, vec!["terminal-bound"]);
        assert_eq!(leaf.editor_ids, vec!["editor-one"]);
        assert_eq!(leaf.active_tab_id.as_deref(), Some("editor-one"));
        assert_eq!(bytes, before);
        assert_eq!(checksum(&bytes), receipt.source_sha256);
    }

    #[test]
    fn ambiguous_attribution_emits_recovery_and_projects_no_resources() {
        let bytes = source();
        let id = ConversationId::parse(ID).unwrap();
        let other = ConversationId::parse(OTHER).unwrap();
        let outcome = LegacyWorkspaceProjector::project(
            &bytes,
            "legacy_workspace_manifests/0/shared.json",
            &checksum(&bytes),
            id,
            &[id, other],
            &HashMap::new(),
            None,
            "2026-08-15T10:00:00.000Z".to_string(),
        );
        let WorkspaceProjectionOutcome::RecoveryRequired { item } = outcome else {
            panic!("ambiguous source must not project");
        };
        assert_eq!(item.kind, RecoveryKind::AmbiguousWorkspaceManifest);
        assert_eq!(item.conversation_ids.len(), 2);
        assert_eq!(item.source_sha256, vec![checksum(&bytes)]);
        assert!(item.provenance[0].preserved_read_only);
    }

    #[test]
    fn unbound_terminal_refs_are_omitted_without_guessing_pty_ownership() {
        let bytes = source();
        let id = ConversationId::parse(ID).unwrap();
        let outcome = LegacyWorkspaceProjector::project(
            &bytes,
            "legacy_workspace_manifests/0/project-one.json",
            &checksum(&bytes),
            id,
            &[id],
            &HashMap::new(),
            None,
            "2026-08-15T10:00:00.000Z".to_string(),
        );
        let WorkspaceProjectionOutcome::Projected { workspace, receipt } = outcome else {
            panic!("expected projection");
        };
        assert_eq!(receipt.projected_terminal_count, 0);
        assert_eq!(receipt.unresolved_terminal_ids.len(), 2);
        assert!(workspace.resources.iter().all(|resource| !matches!(
            resource,
            SessionWorkspaceResourceDescriptor::Terminal { .. }
        )));
    }
}

//! In-memory workspace catalog regression and authorization coverage.
use se_manager_lib::{
    mcp_core::{
        host_capabilities::HostCapabilityError,
        workspace::{WorkspaceCatalog, WorkspaceGrant},
    },
    web::project_registry::{ProjectGroupSummary, ProjectRegistry, ProjectSummary},
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

fn grant(ids: Option<&[&str]>, paths: bool) -> WorkspaceGrant {
    WorkspaceGrant {
        enabled: true,
        project_ids: ids.map(|ids| ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>()),
        include_paths: paths,
    }
}
fn call(
    c: &WorkspaceCatalog,
    name: &str,
    args: Value,
    g: &WorkspaceGrant,
) -> Result<Value, HostCapabilityError> {
    c.call(name, args.as_object().unwrap().clone(), g)
}
fn fixture() -> (Arc<ProjectRegistry>, WorkspaceCatalog) {
    let r = Arc::new(ProjectRegistry::new());
    let projects = [
        ("a", "Alpha", false),
        ("b", "Beta secret", false),
        ("c", "Archive", true),
        ("u", "Ungrouped no memory", false),
    ]
    .into_iter()
    .map(|(id, name, archive)| ProjectSummary {
        id: id.into(),
        name: name.into(),
        path: Some(format!("/private/{id}")),
        terminal_screen_engine: Default::default(),
        live_terminal_count: 0,
        color: "blue".into(),
        is_archived: archive,
        is_default: false,
    })
    .collect();
    let groups = [
        ("g1", "Work", vec!["b", "a", "c"], Some("b")),
        ("g2", "Other", vec!["a"], Some("a")),
        ("empty", "Empty", vec![], None),
        ("hidden", "Hidden", vec!["b"], Some("b")),
    ]
    .into_iter()
    .map(|(id, name, ids, pref)| ProjectGroupSummary {
        id: id.into(),
        name: name.into(),
        project_ids: ids.into_iter().map(str::to_string).collect(),
        color: None,
        preferred_project_id: pref.map(str::to_string),
    })
    .collect();
    r.set_with_groups(projects, groups, Some("b".into()));
    let c = WorkspaceCatalog::new(r.clone());
    (r, c)
}
#[test]
fn ready_empty_is_distinct_and_snapshot_is_readonly() {
    let r = Arc::new(ProjectRegistry::new());
    let c = WorkspaceCatalog::new(r.clone());
    let g = grant(None, false);
    assert_eq!(
        call(&c, "project_list", json!({}), &g).unwrap_err().code,
        "REGISTRY_NOT_READY"
    );
    r.set(vec![], None);
    let before = r.snapshot_with_state();
    assert!(before.ready);
    assert!(before.observed_at.is_some());
    let v = call(&c, "project_list", json!({}), &g).unwrap();
    assert_eq!(v["total"], 0);
    assert_eq!(v["meta"]["ready"], true);
    chrono::DateTime::parse_from_rfc3339(v["meta"]["observedAt"].as_str().unwrap()).unwrap();
    assert!(v["meta"].get("revision").is_none());
    assert_eq!(before, r.snapshot_with_state());
    r.clear();
    assert!(!r.snapshot_with_state().ready);
    assert_eq!(
        call(&c, "group_list", json!({}), &g).unwrap_err().code,
        "REGISTRY_NOT_READY"
    );
}
#[test]
fn deny_default_paths_opt_in_and_no_memory_project() {
    let (_, c) = fixture();
    assert_eq!(
        call(&c, "project_list", json!({}), &WorkspaceGrant::default())
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    let g: WorkspaceGrant = serde_json::from_value(json!({})).unwrap();
    assert!(!g.enabled);
    assert_eq!(g.project_ids, Some(BTreeSet::new()));
    let omitted: WorkspaceGrant = serde_json::from_value(json!({"enabled":true})).unwrap();
    assert_eq!(
        call(&c, "project_list", json!({}), &omitted).unwrap()["total"],
        0
    );
    let explicit_all: WorkspaceGrant =
        serde_json::from_value(json!({"enabled":true,"projectIds":null})).unwrap();
    assert_eq!(
        call(&c, "project_list", json!({}), &explicit_all).unwrap()["total"],
        3
    );
    let v = call(
        &c,
        "project_get",
        json!({"projectId":"u"}),
        &grant(None, false),
    )
    .unwrap();
    assert_eq!(v["project"]["name"], "Ungrouped no memory");
    assert!(v["project"].get("path").is_none());
    assert_eq!(v["project"]["groupIds"], json!([]));
    let v = call(
        &c,
        "project_get",
        json!({"projectId":"u"}),
        &grant(None, true),
    )
    .unwrap();
    assert_eq!(v["project"]["path"], "/private/u");
}
#[test]
fn permission_filter_precedes_all_counts_and_metadata() {
    let (_, c) = fixture();
    let g = grant(Some(&["a", "u"]), false);
    let v = call(&c, "group_list", json!({"includeEmpty":true}), &g).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["ungroupedProjectCount"], 1);
    assert!(v["defaultProjectId"].is_null());
    assert_eq!(v["groups"][0]["projectIds"], json!(["a"]));
    assert_eq!(v["groups"][0]["projectCount"], 1);
    assert!(v["groups"][0]["preferredProjectId"].is_null());
    assert_eq!(v["groups"][1]["preferredProjectId"], "a");
    let raw = v.to_string();
    for secret in [
        "Beta secret",
        "hidden",
        "empty",
        "/private",
        "\"b\"",
        "\"c\"",
    ] {
        assert!(!raw.contains(secret), "{secret}");
    }
    let v = call(&c, "project_list", json!({"limit":1}), &g).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["nextCursor"], "offset:1");
    assert_eq!(v["projects"][0]["groupIds"], json!(["g1", "g2"]));
    assert_eq!(v["projects"][0]["isDefault"], false);
    let a = call(&c, "project_get", json!({"projectId":"b"}), &g).unwrap_err();
    let b = call(&c, "project_get", json!({"projectId":"missing"}), &g).unwrap_err();
    assert_eq!(a, b);
    assert_eq!(a.code, "NOT_FOUND");
    assert_eq!(
        call(&c, "project_list", json!({"groupId":"hidden"}), &g)
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
    let v = call(&c, "group_list", json!({}), &grant(Some(&[]), false)).unwrap();
    assert_eq!(v["total"], 0);
    assert_eq!(v["ungroupedProjectCount"], 0);
}
#[test]
fn filters_order_archive_ungrouped_and_page_bounds() {
    let (_, c) = fixture();
    let g = grant(None, false);
    let v = call(&c, "project_list", json!({"groupId":"g1"}), &g).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["projects"][0]["id"], "a");
    let v = call(
        &c,
        "project_list",
        json!({"groupId":"g1","includeArchived":true}),
        &g,
    )
    .unwrap();
    assert_eq!(v["total"], 3);
    let v = call(&c, "project_get", json!({"projectId":"c"}), &g).unwrap();
    assert_eq!(v["project"]["isArchived"], true);
    let v = call(&c, "project_list", json!({"ungrouped":true}), &g).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["projects"][0]["id"], "u");
    let v = call(&c, "project_list", json!({"query":"ALP"}), &g).unwrap();
    assert_eq!(v["total"], 1);
    let v = call(&c, "group_list", json!({"query":"wORk"}), &g).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["groups"][0]["projectCount"], 3);
    let v = call(&c, "group_list", json!({}), &g).unwrap();
    assert_eq!(v["total"], 4);
    let v = call(&c, "group_list", json!({"includeEmpty":false}), &g).unwrap();
    assert_eq!(v["total"], 3);
    let v = call(
        &c,
        "project_list",
        json!({"limit":1,"cursor":"offset:1"}),
        &g,
    )
    .unwrap();
    assert_eq!(v["projects"][0]["id"], "b");
    assert_eq!(v["nextCursor"], "offset:2");
    let v = call(&c, "project_list", json!({"cursor":"offset:3"}), &g).unwrap();
    assert_eq!(v["projects"], json!([]));
    assert!(v["nextCursor"].is_null());
    assert_eq!(
        call(
            &c,
            "project_list",
            json!({"cursor":"offset:3"}),
            &grant(Some(&["a"]), false)
        )
        .unwrap_err()
        .code,
        "INVALID_ARGUMENTS"
    );
}
#[test]
fn malformed_unknown_and_privilege_arguments_are_rejected() {
    let (_, c) = fixture();
    let g = grant(None, false);
    for args in [
        json!({"includePaths":true}),
        json!({"limit":0}),
        json!({"limit":201}),
        json!({"limit":1.5}),
        json!({"limit":"1"}),
        json!({"query":null}),
        json!({"query":""}),
        json!({"query":"x".repeat(4097)}),
        json!({"includeArchived":"true"}),
        json!({"ungrouped":null}),
        json!({"groupId":"g1","ungrouped":true}),
        json!({"cursor":"offset:01"}),
        json!({"cursor":"offset:-1"}),
        json!({"cursor":"secret-path"}),
        json!({"cursor":"offset:999"}),
    ] {
        assert_eq!(
            call(&c, "project_list", args.clone(), &g).unwrap_err().code,
            "INVALID_ARGUMENTS",
            "{args}"
        );
    }
    for args in [
        json!({}),
        json!({"projectId":null}),
        json!({"projectId":""}),
        json!({"projectId":"a","limit":1}),
    ] {
        assert_eq!(
            call(&c, "project_get", args, &g).unwrap_err().code,
            "INVALID_ARGUMENTS"
        );
    }
    assert_eq!(
        call(&c, "group_list", json!({"includeEmpty":null}), &g)
            .unwrap_err()
            .code,
        "INVALID_ARGUMENTS"
    );
    assert_eq!(
        call(&c, "unknown", json!({}), &g).unwrap_err().code,
        "NOT_FOUND"
    );
}

//! File-backed workspace groups and the public web mirror share one contract.
//! Every filesystem operation is confined to a disposable temporary fixture.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use se_manager_lib::web::project_registry::{
    seed_from_file, FileProjectRegistry, ProjectGroup, ProjectGroupSummary, ProjectRegistry,
    ProjectRegistryError, ProjectSummary, VfsRoot, SCHEMA_VERSION,
};
use serde_json::{json, Value};
use tempfile::TempDir;

fn root(dir: &Path, id: &str, archived: bool) -> VfsRoot {
    let path = dir.join(id);
    fs::create_dir(&path).expect("create fixture project directory");
    VfsRoot {
        id: id.to_string(),
        name: format!("Project {id}"),
        path: path.canonicalize().expect("canonical fixture root"),
        color: "blue".to_string(),
        is_archived: archived,
        terminal_screen_engine: Default::default(),
        mcp_servers: Vec::new(),
    }
}

fn roots(dir: &Path) -> Vec<VfsRoot> {
    vec![
        root(dir, "a", false),
        root(dir, "b", false),
        root(dir, "archived", true),
        root(dir, "loose", false),
    ]
}

fn group(id: &str, members: &[&str], preferred: Option<&str>) -> ProjectGroup {
    ProjectGroup {
        id: id.to_string(),
        name: format!("Group {id}"),
        project_ids: members.iter().map(|id| (*id).to_string()).collect(),
        color: Some("purple".to_string()),
        preferred_project_id: preferred.map(str::to_string),
    }
}

fn summaries(roots: &[VfsRoot]) -> Vec<ProjectSummary> {
    roots.iter().cloned().map(ProjectSummary::from).collect()
}

/// Includes every fixture file's bytes and every directory entry, so reads
/// cannot silently create migration/temp/backup files or change nested data.
fn directory_snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(base: &Path, dir: &Path, entries: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).expect("read fixture directory") {
            let entry = entry.expect("fixture directory entry");
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                entries.insert(relative, None);
                visit(base, &path, entries);
            } else {
                entries.insert(relative, Some(fs::read(path).expect("read fixture bytes")));
            }
        }
    }
    let mut entries = BTreeMap::new();
    visit(dir, dir, &mut entries);
    entries
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).expect("write fixture JSON");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("read fixture JSON")).unwrap()
}

fn assert_legacy_reads_are_pure(version: u32) {
    let dir = TempDir::new().unwrap();
    let roots = roots(dir.path());
    let path = dir.path().join("projects.json");
    let mut projects = serde_json::to_value(&roots).unwrap();
    if version == 1 {
        for project in projects.as_array_mut().unwrap() {
            project.as_object_mut().unwrap().remove("mcpServers");
            project
                .as_object_mut()
                .unwrap()
                .remove("terminalScreenEngine");
        }
    }
    let mut fixture = json!({ "schemaVersion": version, "projects": projects });
    fixture[if version < 3 {
        "activeProjectId"
    } else {
        "defaultProjectId"
    }] = json!("a");
    write_json(&path, &fixture);
    fs::write(roots[0].path.join("sentinel.txt"), b"do not rewrite").unwrap();
    let original_bytes = fs::read(&path).unwrap();
    let before = directory_snapshot(dir.path());

    let file = FileProjectRegistry::load(&path).expect("legacy schema loads in memory");
    assert_eq!(file.roots(), roots);
    assert_eq!(file.default_project_id(), Some("a"));
    assert!(file.groups().is_empty());
    assert!(file.groups_snapshot().is_empty());
    assert_eq!(file.resolve_path("a"), Some(roots[0].path.clone()));

    let mirror = ProjectRegistry::new();
    mirror.set_with_groups(
        summaries(&roots),
        vec![group("stale", &["b"], Some("b"))],
        Some("b".to_string()),
    );
    seed_from_file(&mirror, &file);
    let snapshot = mirror.snapshot();
    assert!(snapshot.groups.is_empty(), "seed must replace stale groups");
    assert_eq!(snapshot.default_project_id.as_deref(), Some("a"));
    assert!(snapshot.projects[0].is_default);
    assert!(!snapshot.projects[1].is_default);
    assert!(snapshot.projects[2].is_archived);
    assert_eq!(mirror.snapshot(), snapshot, "snapshot is a pure read");
    assert_eq!(
        serde_json::to_value(&snapshot).unwrap()["groups"],
        json!([])
    );
    assert_eq!(FileProjectRegistry::load(&path).unwrap(), file);
    assert_eq!(fs::read(&path).unwrap(), original_bytes);
    assert_eq!(directory_snapshot(dir.path()), before);
}

#[test]
fn v1_load_snapshots_and_seed_do_not_rewrite_or_create_files() {
    assert_legacy_reads_are_pure(1);
}

#[test]
fn v2_load_snapshots_and_seed_do_not_rewrite_or_create_files() {
    assert_legacy_reads_are_pure(2);
}

#[test]
fn v3_load_snapshots_and_seed_do_not_rewrite_or_create_files() {
    assert_legacy_reads_are_pure(3);
}

#[test]
fn v4_round_trip_and_default_mutation_preserve_groups_until_explicit_save() {
    assert_eq!(SCHEMA_VERSION, 4);
    let dir = TempDir::new().unwrap();
    let roots = roots(dir.path());
    let path = dir.path().join("projects.json");
    let groups = vec![
        group("first", &["b", "archived", "a"], Some("archived")),
        group("shared", &["a"], Some("a")),
        ProjectGroup {
            color: None,
            ..group("empty", &[], None)
        },
    ];
    // Keep the original constructor's groups-free behavior source compatible.
    let old_constructor = FileProjectRegistry::from_roots(roots.clone(), Some("a".into()));
    assert!(old_constructor.groups().is_empty());
    let file = FileProjectRegistry::from_roots_with_groups(
        roots.clone(),
        groups.clone(),
        Some("a".into()),
    );
    file.save_atomic(&path).unwrap();
    let saved = read_json(&path);
    assert_eq!(saved["schemaVersion"], 4);
    assert_eq!(saved["groups"], serde_json::to_value(&groups).unwrap());
    assert_eq!(saved["defaultProjectId"], "a");
    assert!(saved.get("activeProjectId").is_none());
    assert!(saved.get("ungrouped").is_none());
    let before = directory_snapshot(dir.path());
    let mut loaded = FileProjectRegistry::load(&path).unwrap();
    assert_eq!(loaded, file);

    let mut detached = loaded.groups_snapshot();
    detached[0].project_ids.clear();
    detached.clear();
    assert_eq!(
        loaded.groups(),
        groups,
        "snapshot owns an independent clone"
    );
    loaded.set_default_project("b").unwrap();
    assert_eq!(loaded.default_project_id(), Some("b"));
    for rejected in ["missing", "archived"] {
        assert!(loaded.set_default_project(rejected).is_err());
        assert_eq!(loaded.default_project_id(), Some("b"));
        assert_eq!(loaded.groups(), groups);
    }
    let mirror = ProjectRegistry::new();
    seed_from_file(&mirror, &loaded);
    assert_eq!(mirror.snapshot().groups, groups);
    assert_eq!(mirror.snapshot().default_project_id.as_deref(), Some("b"));
    assert!(mirror.set_default_project("a"));
    assert_eq!(mirror.snapshot().groups, groups);
    assert_eq!(loaded.default_project_id(), Some("b"));
    assert_eq!(directory_snapshot(dir.path()), before);
    assert_eq!(read_json(&path)["defaultProjectId"], "a");

    loaded.save_atomic(&path).unwrap();
    let reloaded = FileProjectRegistry::load(&path).unwrap();
    assert_eq!(reloaded, loaded);
    assert_eq!(reloaded.groups_snapshot(), groups);
    assert_eq!(read_json(&path)["defaultProjectId"], "b");
    assert_eq!(
        directory_snapshot(dir.path()).keys().collect::<Vec<_>>(),
        before.keys().collect::<Vec<_>>()
    );
}

#[test]
fn legacy_files_upgrade_to_v4_only_when_explicitly_saved() {
    for version in 1..=3 {
        let dir = TempDir::new().unwrap();
        let roots = roots(dir.path());
        let path = dir.path().join("projects.json");
        let mut fixture = json!({ "schemaVersion": version, "projects": roots });
        fixture[if version < 3 {
            "activeProjectId"
        } else {
            "defaultProjectId"
        }] = json!("a");
        write_json(&path, &fixture);
        let before = directory_snapshot(dir.path());
        let file = FileProjectRegistry::load(&path).unwrap();
        assert_eq!(directory_snapshot(dir.path()), before);
        file.save_atomic(&path).unwrap();
        let saved = read_json(&path);
        assert_eq!(saved["schemaVersion"], 4);
        assert_eq!(saved["groups"], json!([]));
        assert_eq!(saved["defaultProjectId"], "a");
        assert!(saved.get("activeProjectId").is_none());
        assert_eq!(FileProjectRegistry::load(&path).unwrap(), file);
    }
}

#[test]
fn file_load_constructor_and_web_mirror_normalize_groups_identically() {
    let dir = TempDir::new().unwrap();
    let roots = roots(dir.path());
    let path = dir.path().join("projects.json");
    let mut duplicate = group("first", &["loose"], Some("loose"));
    duplicate.name = "Later duplicate must not win".into();
    duplicate.color = Some("red".into());
    let raw = vec![
        group(
            "first",
            &["b", "missing", "a", "b", "archived", "a"],
            Some("archived"),
        ),
        duplicate,
        group("not-a-member", &["b", "b"], Some("a")),
        group("stale-preference", &["a"], Some("missing")),
        group("stale-members", &["missing", "missing"], Some("missing")),
        ProjectGroup {
            color: None,
            ..group("empty", &[], None)
        },
        group("shared", &["a", "b"], Some("b")),
    ];
    let expected = vec![
        group("first", &["b", "a", "archived"], Some("archived")),
        group("not-a-member", &["b"], None),
        group("stale-preference", &["a"], None),
        group("stale-members", &[], None),
        ProjectGroup {
            color: None,
            ..group("empty", &[], None)
        },
        group("shared", &["a", "b"], Some("b")),
    ];
    write_json(
        &path,
        &json!({
            "schemaVersion": 4, "projects": roots, "groups": raw,
            "defaultProjectId": "a"
        }),
    );
    let before = directory_snapshot(dir.path());
    let loaded = FileProjectRegistry::load(&path).unwrap();
    let constructed =
        FileProjectRegistry::from_roots_with_groups(roots.clone(), raw.clone(), Some("a".into()));
    assert_eq!(loaded, constructed);
    assert_eq!(loaded.groups(), expected);
    let mirror = ProjectRegistry::new();
    // The alias must be the same type, not a divergent lookalike DTO.
    let web_groups: Vec<ProjectGroupSummary> = raw;
    mirror.set_with_groups(summaries(&roots), web_groups, Some("a".into()));
    assert_eq!(mirror.snapshot().groups, expected);
    seed_from_file(&mirror, &loaded);
    assert_eq!(mirror.snapshot().groups, loaded.groups_snapshot());
    assert_eq!(
        directory_snapshot(dir.path()),
        before,
        "normalization is memory-only"
    );
    assert_eq!(read_json(&path)["groups"].as_array().unwrap().len(), 7);
    loaded.save_atomic(&path).unwrap();
    assert_eq!(
        read_json(&path)["groups"],
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(FileProjectRegistry::load(&path).unwrap(), loaded);
}

#[test]
fn removal_cleans_all_memberships_preferences_and_default_without_writing() {
    let dir = TempDir::new().unwrap();
    let roots = roots(dir.path());
    let path = dir.path().join("projects.json");
    let groups = vec![
        group("first", &["a", "archived", "b"], Some("b")),
        group("becomes-empty", &["a"], Some("a")),
        group("shared", &["b", "a"], Some("a")),
        group("already-empty", &[], None),
    ];
    let mut file =
        FileProjectRegistry::from_roots_with_groups(roots, groups.clone(), Some("a".into()));
    file.save_atomic(&path).unwrap();
    let original = file.clone();
    let before = directory_snapshot(dir.path());
    assert!(!file.remove_project("missing"));
    assert_eq!(file, original, "unknown removal is a no-op");
    assert!(file.remove_project("b"));
    assert_eq!(file.default_project_id(), Some("a"));
    assert_eq!(
        file.groups(),
        vec![
            group("first", &["a", "archived"], None),
            group("becomes-empty", &["a"], Some("a")),
            group("shared", &["a"], Some("a")),
            group("already-empty", &[], None),
        ]
    );
    assert!(file.remove_project("a"));
    assert!(!file.remove_project("a"), "removal is idempotent");
    assert_eq!(file.default_project_id(), None);
    let expected = vec![
        group("first", &["archived"], None),
        group("becomes-empty", &[], None),
        group("shared", &[], None),
        group("already-empty", &[], None),
    ];
    assert_eq!(file.groups_snapshot(), expected);
    assert_eq!(
        file.roots()
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        ["archived", "loose"]
    );
    assert!(file.roots()[0].is_archived);
    assert_eq!(file.resolve_path("archived"), None);
    let ungrouped: Vec<_> = file
        .roots()
        .iter()
        .filter(|root| {
            !file
                .groups()
                .iter()
                .any(|g| g.project_ids.contains(&root.id))
        })
        .map(|root| root.id.as_str())
        .collect();
    assert_eq!(
        ungrouped,
        ["loose"],
        "ungrouped is derived, not a stored group"
    );
    let mirror = ProjectRegistry::new();
    seed_from_file(&mirror, &file);
    assert_eq!(mirror.snapshot().groups, expected);
    assert_eq!(mirror.snapshot().projects.len(), 2);
    assert_eq!(mirror.snapshot().default_project_id, None);
    assert!(mirror.snapshot().projects.iter().all(|p| !p.is_default));
    assert_eq!(directory_snapshot(dir.path()), before);
    assert_eq!(FileProjectRegistry::load(&path).unwrap(), original);

    file.save_atomic(&path).unwrap();
    assert_eq!(FileProjectRegistry::load(&path).unwrap(), file);
    let saved = read_json(&path);
    assert!(saved.get("ungrouped").is_none());
    assert_eq!(saved["groups"], serde_json::to_value(&expected).unwrap());
}

#[test]
fn seed_preserves_groups_and_private_mcp_context_without_exposing_it_in_payload() {
    let dir = TempDir::new().unwrap();
    let mut roots = roots(dir.path());
    let private_command = dir.path().join("private-mcp-executable");
    roots[0].mcp_servers = serde_json::from_value(json!([{
        "name": "private-server-name",
        "command": private_command,
        "args": ["--private-argument", "private-token-value"],
        "env": [{"name": "PRIVATE_API_TOKEN", "value": "private-env-value"}]
    }]))
    .expect("valid private stdio MCP fixture");
    let groups = vec![
        group("navigation", &["archived", "a"], Some("a")),
        group("empty", &[], None),
    ];
    let path = dir.path().join("projects.json");
    FileProjectRegistry::from_roots_with_groups(roots.clone(), groups.clone(), Some("a".into()))
        .save_atomic(&path)
        .unwrap();
    let file = FileProjectRegistry::load(&path).unwrap();
    assert_eq!(file.roots()[0].mcp_servers, roots[0].mcp_servers);
    let before = directory_snapshot(dir.path());
    let mirror = ProjectRegistry::new();
    mirror.set_with_groups(summaries(&roots), vec![group("obsolete", &[], None)], None);
    seed_from_file(&mirror, &file);
    let payload = mirror.snapshot();
    assert_eq!(payload.groups, file.groups_snapshot());
    assert_eq!(payload.groups, groups);
    assert_eq!(payload.default_project_id.as_deref(), Some("a"));
    let private = mirror
        .switch_context("a")
        .expect("seeded private switch context");
    assert_eq!(private.cwd, roots[0].path.to_string_lossy());
    assert_eq!(private.mcp_servers, roots[0].mcp_servers);
    assert!(mirror.switch_context("archived").is_none());
    let public = serde_json::to_value(&payload).unwrap();
    assert_eq!(public["groups"], serde_json::to_value(&groups).unwrap());
    assert_eq!(public["groups"][0]["projectIds"], json!(["archived", "a"]));
    assert_eq!(public["groups"][0]["preferredProjectId"], "a");
    for project in public["projects"].as_array().unwrap() {
        for forbidden in ["mcpServers", "mcp_servers", "command", "args", "env"] {
            assert!(
                project.get(forbidden).is_none(),
                "public summary leaked {forbidden}"
            );
        }
    }
    let serialized = serde_json::to_string(&payload).unwrap();
    for secret in [
        private_command.to_str().unwrap(),
        "private-server-name",
        "--private-argument",
        "private-token-value",
        "PRIVATE_API_TOKEN",
        "private-env-value",
    ] {
        assert!(
            !serialized.contains(secret),
            "public payload leaked private MCP data"
        );
    }
    assert_eq!(directory_snapshot(dir.path()), before);
}

#[test]
fn corrupt_json_still_backs_up_exact_bytes_without_replacing_the_original() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("projects.json");
    let corrupt = b"{\"schemaVersion\":4,\"groups\":[ broken JSON\n";
    fs::write(&path, corrupt).unwrap();
    assert!(matches!(
        FileProjectRegistry::load(&path),
        Err(ProjectRegistryError::Parse(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    let entries = directory_snapshot(dir.path());
    let backups: Vec<_> = entries
        .iter()
        .filter(|(path, _)| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.starts_with("projects.json.corrupt-") && name.ends_with(".bak")
        })
        .collect();
    assert_eq!(backups.len(), 1, "one recoverable corruption backup");
    assert_eq!(backups[0].1.as_deref(), Some(corrupt.as_slice()));
    assert_eq!(entries.len(), 2, "only original and backup should exist");
}

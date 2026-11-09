use super::*;
use tempfile::TempDir;

fn home() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}
fn write(path: &Path, bytes: &[u8], mode: u32) {
    safe::replace(
        path,
        &safe::read(path).unwrap(),
        &FileState::file(bytes.to_vec(), mode),
    )
    .unwrap();
}

#[test]
fn transaction_failure_rolls_back_all_preoperation_bytes() {
    let (_dir, home) = home();
    custom(&home, "3.0.0");
    let settings = home.join(".example/settings.json");
    let original = b"{\"user\": true}\n";
    write(&settings, original, 0o640);
    let preview = preview_v3(&home, request(HookAction::Install)).unwrap();
    FAIL_REPLACE.with(|f| *f.borrow_mut() = Some(receipt_path(&home, "example-agent")));
    assert!(confirm(&home, &preview).is_err());
    assert_eq!(
        safe::read(&settings).unwrap(),
        FileState::file(original.to_vec(), 0o640)
    );
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
    assert!(!root(&home)
        .join("installed/example-agent/install.env")
        .exists());
    assert!(!receipt_path(&home, "example-agent").exists());
    assert!(!root(&home).join("transaction.json").exists());
}

#[test]
fn transaction_preflights_all_bytes_and_modes() {
    let (_dir, home) = home();
    let one = home.join("a");
    let two = home.join("b");
    write(&one, b"original", 0o600);
    write(&two, b"changed", 0o600);
    let changes = vec![
        Change {
            path: one.clone(),
            before: safe::read(&one).unwrap(),
            after: FileState::file(b"new".to_vec(), 0o700),
        },
        Change {
            path: two.clone(),
            before: FileState::file(b"stale".to_vec(), 0o600),
            after: FileState::absent(),
        },
    ];
    assert!(transaction(&home, &changes).is_err());
    assert_eq!(safe::read(&one).unwrap().bytes.unwrap(), b"original");
    assert!(!root(&home).join("transaction.json").exists());
    let changes = vec![Change {
        path: one.clone(),
        before: FileState::file(b"original".to_vec(), 0o700),
        after: FileState::absent(),
    }];
    assert!(transaction(&home, &changes).is_err());
}

#[test]
fn transaction_writes_and_restores_exact_bytes_permissions() {
    let (_dir, home) = home();
    let path = home.join("nested/config");
    let original = FileState::file(b"# exact\n\n[original]\nvalue = 1\n".to_vec(), 0o640);
    safe::replace(&path, &FileState::absent(), &original).unwrap();
    let installed = FileState::file(b"installed\n".to_vec(), 0o700);
    transaction(
        &home,
        &[Change {
            path: path.clone(),
            before: original.clone(),
            after: installed.clone(),
        }],
    )
    .unwrap();
    assert_eq!(safe::read(&path).unwrap(), installed);
    transaction(
        &home,
        &[Change {
            path: path.clone(),
            before: installed,
            after: original.clone(),
        }],
    )
    .unwrap();
    assert_eq!(safe::read(&path).unwrap(), original);
    assert!(!root(&home).join("transaction.json").exists());
}

#[cfg(unix)]
#[test]
fn no_follow_rejects_leaf_parent_and_lock_symlinks() {
    use std::os::unix::fs::symlink;
    let (_dir, home) = home();
    let outside = home.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("target"), b"untouched").unwrap();
    symlink(&outside, home.join("parent")).unwrap();
    assert!(safe::read(&home.join("parent/target")).is_err());
    assert!(safe::replace(
        &home.join("parent/new"),
        &FileState::absent(),
        &FileState::file(vec![], 0o600)
    )
    .is_err());
    symlink(outside.join("target"), home.join("leaf")).unwrap();
    assert!(safe::read(&home.join("leaf")).is_err());
    assert!(safe::lock(&home.join("leaf")).is_err());
    assert_eq!(fs::read(outside.join("target")).unwrap(), b"untouched");
}

#[test]
fn unresolved_journal_blocks_every_further_transaction() {
    let (_dir, home) = home();
    write(&root(&home).join("transaction.json"), b"recover me", 0o600);
    assert!(transaction(&home, &[]).is_err());
    assert_eq!(
        fs::read(root(&home).join("transaction.json")).unwrap(),
        b"recover me"
    );
}

#[test]
fn legacy_receipt_restores_first_bytes_and_recognizes_pi_v1_v2() {
    let (_dir, home) = home();
    for kind in [AgentKind::ClaudeCode, AgentKind::QinCode, AgentKind::Pi] {
        let adapter = Adapter::new(kind, home.clone(), false);
        let original = if kind == AgentKind::Pi {
            b"// original pi user extension\n".to_vec()
        } else if kind == AgentKind::ClaudeCode {
            b"{\n  \"user\": true\n}\n".to_vec()
        } else {
            b"# user formatting\ncustom = true\n".to_vec()
        };
        let installed = adapter
            .merge(
                if kind == AgentKind::Pi {
                    None
                } else {
                    Some(&original)
                },
                true,
            )
            .unwrap();
        write(&adapter.config(), &installed, 0o640);
        write(&adapter.script(), adapter.script_source().as_bytes(), 0o755);
        write(
            &adapter.receipt(),
            &serde_json::to_vec(&Receipt {
                original: Some(original.clone()),
                installed,
            })
            .unwrap(),
            0o600,
        );
        let changes = legacy_changes(&home, &adapter.name()).unwrap().unwrap();
        transaction(&home, &changes).unwrap();
        assert_eq!(fs::read(adapter.config()).unwrap(), original);
    }
    let adapter = Adapter::new(AgentKind::Pi, home.clone(), false);
    for bytes in [adapter.pi_v1_source(), adapter.pi_source()] {
        write(&adapter.config(), bytes.as_bytes(), 0o600);
        let changes = legacy_changes(&home, "pi").unwrap().unwrap();
        transaction(&home, &changes).unwrap();
        assert!(!adapter.config().exists());
    }
}

fn custom(home: &Path, version: &str) -> PathBuf {
    let directory = root(home).join("adapters/example-agent");
    fs::create_dir_all(directory.join("files")).unwrap();
    let manifest = format!(
        r#"schema_version = 1
id = "example-agent"
display_name = "Example Agent"
version = "{version}"
[cli]
binaries = ["se-hook-test-no-such-binary"]
version_args = ["--version"]
[cli.version_parser]
kind = "anchored-regex"
pattern = '^Example Agent (?P<version>[0-9]+\.[0-9]+\.[0-9]+)$'
[[config]]
id = "settings"
path = "home:.example/settings.json"
format = "json-merge"
[[config.ops]]
op = "append-unique"
key = ["hooks", "session_start"]
value_json = '{{"command":"{{{{script_path}}}}"}}'
event = "start"
[[config.ops]]
op = "append-unique"
key = ["hooks", "tool_start"]
value_json = '{{"command":"{{{{script_path}}}}"}}'
event = "tool"
[payload]
native_event = "/event"
session_id = "/session_id"
[[native_events]]
name = "start"
hook_kind = "session_start"
enabled_by_default = true
[[native_events]]
name = "tool"
hook_kind = "tool_start"
enabled_by_default = false
[[files]]
id = "hook"
source = "files/hook.sh.tmpl"
target = "install:example-hook.sh"
mode = "0700"
"#
    );
    fs::write(directory.join("adapter.toml"), manifest).unwrap();
    fs::write(
        directory.join("files/hook.sh.tmpl"),
        "#!/bin/sh\n# {{agent_id}} {{endpoint_file}}\n",
    )
    .unwrap();
    directory
}
fn request(action: HookAction) -> HookPreviewRequest {
    HookPreviewRequest {
        adapter_id: AdapterId::try_from("example-agent".to_owned()).unwrap(),
        action,
        event_options: BTreeMap::new(),
    }
}
fn confirm(home: &Path, preview: &HookPreviewDto) -> Result<HookMutationResult, HookErrorDto> {
    confirm_v3(
        home,
        HookConfirmRequest {
            adapter_id: preview.adapter_id.clone(),
            preview_id: preview.preview_id.clone(),
            fingerprint: preview.fingerprint.clone(),
        },
        Some(preview.action),
    )
}

#[test]
fn v3_preview_redacts_credentials_without_changing_recovery_bytes() {
    let (_dir, home) = home();
    custom(&home, "3.0.0");
    let settings = home.join(".example/settings.json");
    let original = b"{\n  \"api_token\": \"private-bearer-value\",\n  \"user\": true\n}\n";
    write(&settings, original, 0o600);
    let preview = preview_v3(&home, request(HookAction::Install)).unwrap();
    let display = serde_json::to_string(&preview).unwrap();
    assert!(!display.contains("private-bearer-value"));
    assert!(display.contains("<redacted>"));
    confirm(&home, &preview).unwrap();
    assert!(fs::read_to_string(&settings)
        .unwrap()
        .contains("private-bearer-value"));
    confirm(
        &home,
        &preview_v3(&home, request(HookAction::Uninstall)).unwrap(),
    )
    .unwrap();
    assert_eq!(fs::read(settings).unwrap(), original);
}

#[test]
fn v3_manual_preview_upgrade_uuid_original_bytes_and_exact_uninstall() {
    let (_dir, home) = home();
    let source_dir = custom(&home, "3.0.0");
    let settings = home.join(".example/settings.json");
    let original = b"{\n    \"user\": true\n}\n";
    write(&settings, original, 0o640);
    let before = fs::read(&settings).unwrap();
    let status = adapters_reload(&home).unwrap();
    assert_eq!(
        status
            .iter()
            .find(|s| s.id.as_str() == "example-agent")
            .unwrap()
            .state,
        AdapterStatus::NotInstalled
    );
    assert_eq!(fs::read(&settings).unwrap(), before);
    let preview = preview_v3(&home, request(HookAction::Install)).unwrap();
    assert_eq!(fs::read(&settings).unwrap(), before);
    assert!(!root(&home).join("installed").exists());
    assert!(preview
        .files
        .iter()
        .any(|f| f.path.ends_with("install.env") && f.mode.as_deref() == Some("0600")));
    assert!(preview
        .keys
        .iter()
        .any(|k| k.key == ["hooks", "session_start"]));
    assert!(!preview
        .files
        .iter()
        .any(|f| f.after.as_ref().is_some_and(|s| s.contains("tool_start"))));
    let installed = confirm(&home, &preview).unwrap();
    assert_eq!(installed.status.state, AdapterStatus::Installed);
    let first = installed_snapshot(&home, "example-agent").unwrap().unwrap();
    assert_eq!(first.revision, 1);
    let frozen_ref = snapshot_ref_for(&home, "example-agent", &first.install_id).unwrap();
    assert_eq!(
        frozen_ref,
        archive_path(&home, "example-agent", &first.install_id, 1)
    );
    assert!(load_resume_snapshot(
        &home,
        "example-agent",
        &first.install_id,
        frozen_ref.to_str().unwrap()
    )
    .is_ok());
    assert!(load_resume_snapshot(
        &home,
        "example-agent",
        &first.install_id,
        home.join("forged.json").to_str().unwrap()
    )
    .is_err());
    assert_eq!(
        safe::read(&root(&home).join("installed/example-agent/install.env")).unwrap(),
        FileState::file(
            format!("SE_AGENT_HOOK_INSTALL_ID={}\n", first.install_id).into_bytes(),
            0o600
        )
    );
    assert!(confirm(&home, &preview).is_err());
    custom(&home, "3.1.0");
    assert_eq!(
        status_one(&home, "example-agent").unwrap().state,
        AdapterStatus::Outdated
    );
    let mut upgrade = request(HookAction::Install);
    upgrade.event_options.insert("tool".into(), true);
    confirm(&home, &preview_v3(&home, upgrade).unwrap()).unwrap();
    let second = installed_snapshot(&home, "example-agent").unwrap().unwrap();
    assert_eq!(first.install_id, second.install_id);
    assert_eq!(second.revision, 2);
    assert!(fs::read_to_string(&settings)
        .unwrap()
        .contains("tool_start"));
    assert!(load_resume_snapshot(
        &home,
        "example-agent",
        &first.install_id,
        &first.snapshot_ref()
    )
    .is_ok());
    fs::remove_dir_all(source_dir).unwrap();
    assert_eq!(
        status_one(&home, "example-agent").unwrap().state,
        AdapterStatus::SourceMissing
    );
    assert!(normalize_installed(
        &home,
        "example-agent",
        &first.install_id,
        &serde_json::json!({"event":"start","session_id":"s"})
    )
    .is_ok());
    let uninstall = preview_v3(&home, request(HookAction::Uninstall)).unwrap();
    confirm(&home, &uninstall).unwrap();
    assert_eq!(
        safe::read(&settings).unwrap(),
        FileState::file(original.to_vec(), 0o640)
    );
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
    assert!(!root(&home)
        .join("installed/example-agent/install.env")
        .exists());
    assert!(archive_path(&home, "example-agent", &first.install_id, 1).exists());
    assert!(load_resume_snapshot(
        &home,
        "example-agent",
        &first.install_id,
        &first.snapshot_ref()
    )
    .is_err());
    custom(&home, "3.1.0");
    confirm(
        &home,
        &preview_v3(&home, request(HookAction::Install)).unwrap(),
    )
    .unwrap();
    assert_ne!(
        installed_snapshot(&home, "example-agent")
            .unwrap()
            .unwrap()
            .install_id,
        first.install_id
    );
}

#[test]
fn v3_cas_rejects_source_target_receipt_and_permission_edits() {
    let (_dir, home) = home();
    let directory = custom(&home, "3.0.0");
    let preview = preview_v3(&home, request(HookAction::Install)).unwrap();
    fs::write(directory.join("files/hook.sh.tmpl"), "# source changed").unwrap();
    assert!(confirm(&home, &preview).is_err());
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
    custom(&home, "3.0.0");
    let preview = preview_v3(&home, request(HookAction::Install)).unwrap();
    write(&home.join(".example/settings.json"), b"{}", 0o600);
    assert!(confirm(&home, &preview).is_err());
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
    confirm(
        &home,
        &preview_v3(&home, request(HookAction::Install)).unwrap(),
    )
    .unwrap();
    let preview = preview_v3(&home, request(HookAction::Uninstall)).unwrap();
    write(&receipt_path(&home, "example-agent"), b"{}", 0o600);
    assert!(confirm(&home, &preview).is_err());
    assert!(root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
}

#[test]
fn v3_rejects_unknown_version_identity_floor_and_unknown_event_option() {
    let (_dir, home) = home();
    let directory = custom(&home, "3.0.0");
    let mut req = request(HookAction::Install);
    req.event_options.insert("unknown".into(), true);
    assert!(preview_v3(&home, req).is_err());
    let manifest = fs::read_to_string(directory.join("adapter.toml"))
        .unwrap()
        .replace(
            "name = \"start\"",
            "name = \"start\"\nmin_schema_version = \"1.0.0\"",
        );
    fs::write(directory.join("adapter.toml"), manifest).unwrap();
    assert!(preview_v3(&home, request(HookAction::Install)).is_err());
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
}

#[cfg(unix)]
#[test]
fn v3_probe_subprocesses_have_output_and_time_budgets() {
    let mut bytes = 64 * 1024;
    assert_eq!(
        capture(
            Path::new("/usr/bin/yes"),
            &[],
            Duration::from_secs(2),
            &mut bytes
        ),
        Err("probe_output_limit")
    );
    assert_eq!(bytes, 0);
    let mut bytes = 64 * 1024;
    let start = Instant::now();
    assert_eq!(
        capture(
            Path::new("/bin/sleep"),
            &["2".into()],
            Duration::from_millis(30),
            &mut bytes
        ),
        Err("probe_timeout")
    );
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn v3_unsafe_home_status_reports_renderer_diagnostic_without_probe_or_writes() {
    let (_dir, base) = home();
    for character in ['\'', '"', '$', '`', '\\', '\n', '\t', '\u{7f}'] {
        let home = base.join(format!("home{character}path"));
        let directory = custom(&home, "3.0.0");
        let marker = base.join("unexpected-probe-marker");
        let manifest_path = directory.join("adapter.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("se-hook-test-no-such-binary", "touch")
            .replace(
                "version_args = [\"--version\"]",
                &format!("version_args = [\"{}\"]", marker.display()),
            );
        fs::write(&manifest_path, &manifest).unwrap();
        let template_path = directory.join("files/hook.sh.tmpl");
        let template = fs::read(&template_path).unwrap();
        for statuses in [
            adapters_status(&home).unwrap(),
            adapters_reload(&home).unwrap(),
        ] {
            assert!(statuses
                .iter()
                .any(|status| status.id.as_str() == "example-agent"));
            assert!(statuses.iter().all(|status| status
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "unsafe_render_path"
                    && diagnostic.message.contains("HOME"))));
        }
        let error = preview_v3(&home, request(HookAction::Install)).unwrap_err();
        assert_eq!(error.code, HookErrorCode::ValidationError);
        assert!(error.message.contains("HOME"));
        assert!(
            !marker.exists(),
            "read-only status and rejected preview must not probe"
        );
        assert!(!root(&home).join("installed").exists());
        assert!(!root(&home).join("transaction.json").exists());
        assert_eq!(fs::read_to_string(&manifest_path).unwrap(), manifest);
        assert_eq!(fs::read(template_path).unwrap(), template);
    }
    assert!(home_render_diagnostic(&base.join("home with spaces"), "example-agent").is_none());
}

#[test]
fn v3_status_reload_never_executes_cli_but_manual_preview_does() {
    let (_dir, home) = home();
    let directory = custom(&home, "3.0.0");
    let marker = home.join("probe-marker");
    let path = directory.join("adapter.toml");
    let content = fs::read_to_string(&path)
        .unwrap()
        .replace("se-hook-test-no-such-binary", "touch")
        .replace(
            "version_args = [\"--version\"]",
            &format!("version_args = [\"{}\"]", marker.display()),
        );
    fs::write(path, content).unwrap();
    adapters_status(&home).unwrap();
    adapters_reload(&home).unwrap();
    assert!(!marker.exists(), "status/reload must not spawn a CLI");
    preview_v3(&home, request(HookAction::Install)).unwrap();
    assert!(
        marker.exists(),
        "only explicit manual preview probes the CLI"
    );
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
}

#[cfg(unix)]
#[test]
fn v3_mode_edits_and_concurrent_confirms_fail_without_overwrite() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, home) = home();
    custom(&home, "3.0.0");
    let first = preview_v3(&home, request(HookAction::Install)).unwrap();
    let second = preview_v3(&home, request(HookAction::Install)).unwrap();
    confirm(&home, &first).unwrap();
    let before = fs::read(receipt_path(&home, "example-agent")).unwrap();
    assert!(confirm(&home, &second).is_err());
    assert_eq!(
        fs::read(receipt_path(&home, "example-agent")).unwrap(),
        before
    );
    let uninstall = preview_v3(&home, request(HookAction::Uninstall)).unwrap();
    let hook = root(&home).join("installed/example-agent/files/example-hook.sh");
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(confirm(&home, &uninstall).is_err());
    assert!(hook.exists());
    assert_eq!(
        fs::read(receipt_path(&home, "example-agent")).unwrap(),
        before
    );
}

#[cfg(unix)]
#[test]
fn v3_preview_then_parent_symlink_substitution_is_conflict() {
    use std::os::unix::fs::symlink;
    let (_dir, home) = home();
    custom(&home, "3.0.0");
    let preview = preview_v3(&home, request(HookAction::Install)).unwrap();
    fs::create_dir(home.join("outside")).unwrap();
    symlink(home.join("outside"), home.join(".example")).unwrap();
    assert!(confirm(&home, &preview).is_err());
    assert!(!home.join("outside/settings.json").exists());
    assert!(!root(&home)
        .join("installed/example-agent/files/example-hook.sh")
        .exists());
}

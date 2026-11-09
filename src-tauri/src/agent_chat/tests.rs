use super::*;
use serde_json::json;
use std::io::Write;

fn hook(kind: HookKind) -> AgentHookEvent {
    AgentHookEvent {
        sequence: 1,
        terminal_id: "terminal-fixture".into(),
        project_id: "project-fixture".into(),
        agent: AgentKind::ClaudeCode,
        session_id: "session-fixture".into(),
        transcript_path: None,
        kind,
        at: 1,
        tool_name: None,
        ok: None,
    }
}
fn writer() -> (Writer, Arc<Mutex<Vec<String>>>) {
    let writes = Arc::new(Mutex::new(vec![]));
    let sink = writes.clone();
    (
        Box::new(move |_, bytes| {
            sink.lock().unwrap().push(bytes.into());
            Ok(())
        }),
        writes,
    )
}
fn transcript() -> (tempfile::TempDir, PathBuf, Session) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chat.jsonl");
    std::fs::write(&path, "").unwrap();
    let mut session = Session::new(hook(HookKind::Idle));
    session.path = Some(path.clone());
    (dir, path, session)
}
fn append(path: &std::path::Path, value: Value) {
    writeln!(
        std::fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{value}"
    )
    .unwrap();
}
#[test]
fn sends_idle_as_one_bracketed_paste_and_enter_but_not_delivered() {
    let (writer, writes) = writer();
    let mut s = Session::new(hook(HookKind::Idle));
    let delivery = s.enqueue("hello\nworld".into(), None, &writer).unwrap();
    assert_eq!(delivery.status, "pending");
    assert_eq!(
        *writes.lock().unwrap(),
        vec!["\x1b[200~hello\nworld\x1b[201~\r"]
    );
}
#[test]
fn busy_queues_and_turn_done_flushes_one_without_resend() {
    let (writer, writes) = writer();
    let mut s = Session::new(hook(HookKind::ToolStart));
    assert_eq!(
        s.enqueue("one".into(), None, &writer).unwrap().status,
        "queued"
    );
    s.enqueue("two".into(), None, &writer).unwrap();
    assert!(writes.lock().unwrap().is_empty());
    s.hook.kind = HookKind::TurnDone;
    s.ready_credit = true;
    s.flush(&writer);
    s.flush(&writer);
    assert_eq!(writes.lock().unwrap().len(), 1);
    assert_eq!(s.pending[1].delivery.status, "queued");
}
#[test]
fn matching_new_user_echo_is_the_only_delivery_ack() {
    let (_dir, path, mut s) = transcript();
    let (writer, _) = writer();
    append(
        &path,
        json!({"type":"user","uuid":"old","message":{"role":"user","content":"hello"}}),
    );
    s.read();
    s.enqueue("hello".into(), None, &writer).unwrap();
    s.read();
    assert_eq!(s.pending[0].delivery.status, "pending");
    append(
        &path,
        json!({"type":"assistant","uuid":"answer","message":{"role":"assistant","content":"hello"}}),
    );
    s.read();
    assert_eq!(s.pending[0].delivery.status, "pending");
    append(
        &path,
        json!({"type":"user","uuid":"new","message":{"role":"user","content":"hello"}}),
    );
    s.read();
    assert_eq!(s.pending[0].delivery.status, "delivered");
}
#[test]
fn ack_timeout_is_unconfirmed_and_never_auto_resends() {
    let (writer, writes) = writer();
    let mut s = Session::new(hook(HookKind::Idle));
    s.enqueue("hello".into(), None, &writer).unwrap();
    s.expire(Instant::now() + ACK_TIMEOUT);
    s.ready_credit = true;
    s.flush(&writer);
    assert_eq!(s.pending[0].delivery.status, "unconfirmed");
    assert!(s.pending[0].raw.is_empty());
    assert_eq!(writes.lock().unwrap().len(), 1);
}
#[test]
fn scope_revoked_while_queued_prevents_flush() {
    let (writer, writes) = writer();
    let mut s = Session::new(hook(HookKind::ToolStart));
    s.enqueue("hello".into(), Some(Arc::new(|| false)), &writer)
        .unwrap();
    s.ready_credit = true;
    s.flush(&writer);
    assert!(writes.lock().unwrap().is_empty());
    assert_eq!(s.pending[0].delivery.status, "unconfirmed");
}
#[test]
fn injection_controls_in_text_are_rejected() {
    let (writer, writes) = writer();
    let mut s = Session::new(hook(HookKind::Idle));
    for text in ["\x1b[201~evil", "a\x03", "a\0"] {
        assert!(s.enqueue(text.into(), None, &writer).is_err());
    }
    assert!(writes.lock().unwrap().is_empty());
}
#[test]
fn incremental_tail_ignores_old_records_and_retains_partial_lines() {
    let (_dir, path, mut s) = transcript();
    append(
        &path,
        json!({"type":"user","uuid":"first","message":{"role":"user","content":"one"}}),
    );
    s.read();
    let cursor = s.cursor;
    assert_eq!(s.messages.len(), 1);
    let value =
        json!({"type":"assistant","uuid":"second","message":{"role":"assistant","content":"two"}})
            .to_string();
    write!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap(),
        "{value}"
    )
    .unwrap();
    s.read();
    assert_eq!(s.messages.len(), 1);
    writeln!(std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap())
    .unwrap();
    s.read();
    assert_eq!(s.messages.len(), 2);
    let tail = s.snapshot(Some(cursor));
    assert_eq!(tail.messages.len(), 1);
    assert!(!tail.reset);
    s.read();
    assert_eq!(s.messages.len(), 2);
}
#[test]
fn malformed_complete_record_degrades_and_advances_without_spinning() {
    let (_dir, path, mut s) = transcript();
    std::fs::write(&path, "{bad}\n").unwrap();
    s.read();
    let offset = s.offset;
    let cursor = s.cursor;
    assert_eq!(offset, 6);
    assert_eq!(s.snapshot(None).state, "degraded");
    assert!(s.reason.is_some());
    s.read();
    assert_eq!(s.cursor, cursor);
    assert_eq!(s.offset, offset);
}
#[test]
fn replacement_resets_snapshot_and_invalidates_pending_echo() {
    let (_dir, path, mut s) = transcript();
    let (writer, _) = writer();
    append(
        &path,
        json!({"type":"user","uuid":"old","message":{"role":"user","content":"long old text"}}),
    );
    s.read();
    s.enqueue("x".into(), None, &writer).unwrap();
    let before = s.cursor;
    std::fs::write(&path, "\n").unwrap();
    s.read();
    assert!(s.snapshot(Some(before)).reset);
    assert!(s.messages.is_empty());
    assert_eq!(s.pending[0].delivery.status, "unconfirmed");
}
#[test]
fn an_old_echo_written_between_scan_and_paste_cannot_ack() {
    let (_dir, path, mut s) = transcript();
    let (writer, _) = writer();
    s.read();
    append(
        &path,
        json!({"type":"user","uuid":"old-race","message":{"role":"user","content":"same"}}),
    );
    s.enqueue("same".into(), None, &writer).unwrap();
    s.read();
    assert_eq!(s.pending[0].delivery.status, "pending");
    append(
        &path,
        json!({"type":"user","uuid":"new-race","message":{"role":"user","content":"same"}}),
    );
    s.read();
    assert_eq!(s.pending[0].delivery.status, "delivered");
}
#[test]
fn later_tool_results_keep_the_original_invocation_metadata() {
    let (_dir, path, mut s) = transcript();
    append(
        &path,
        json!({"type":"assistant","uuid":"call-record","message":{"role":"assistant","content":[{"type":"tool_use","id":"call1","name":"Read","input":{"file_path":"fixture.txt"}}]}}),
    );
    s.read();
    let title = s.tools[0]["title"].clone();
    let kind = s.tools[0]["kind"].clone();
    append(
        &path,
        json!({"type":"user","uuid":"result-record","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call1","content":"done"}]}}),
    );
    s.read();
    assert_eq!(s.tools.len(), 1);
    assert_eq!(s.tools[0]["status"], "completed");
    assert_eq!(s.tools[0]["title"], title);
    assert_eq!(s.tools[0]["kind"], kind);
    assert_eq!(s.tools[0]["rawInput"]["file_path"], "fixture.txt");
}
#[test]
fn file_creation_recovers_a_transient_degraded_state() {
    let (dir, _path, mut s) = transcript();
    let path = dir.path().join("later.jsonl");
    s.path = Some(path.clone());
    s.read();
    assert_eq!(s.snapshot(None).state, "degraded");
    std::fs::write(path, "\n").unwrap();
    s.read();
    assert_eq!(s.snapshot(None).state, "ready");
}
#[test]
fn notify_wakes_the_host_for_transcript_appends() {
    let (_dir, path, mut s) = transcript();
    s.hook.transcript_path = Some(path.to_string_lossy().into());
    let (tx, rx) = mpsc::sync_channel(64);
    s.configure_path(&tx);
    assert!(s.watcher.is_some());
    append(
        &path,
        json!({"type":"user","uuid":"watch","message":{"role":"user","content":"new"}}),
    );
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(3)),
        Ok(Request::Wake)
    ));
    s.read();
    assert_eq!(s.messages.len(), 1);
}
#[test]
fn a_ready_edge_is_not_reissued_by_consecutive_idle_turn_done_hooks() {
    let (writer, writes) = writer();
    let mut s = Session::new(hook(HookKind::ToolStart));
    s.enqueue("one".into(), None, &writer).unwrap();
    s.enqueue("two".into(), None, &writer).unwrap();
    let mut ready = hook(HookKind::TurnDone);
    ready.sequence = 2;
    s.apply_hook(ready);
    s.flush(&writer);
    s.pending[0].delivery.status = "delivered".into();
    let mut duplicate = hook(HookKind::Idle);
    duplicate.sequence = 3;
    s.apply_hook(duplicate);
    s.flush(&writer);
    assert_eq!(writes.lock().unwrap().len(), 1);
    assert_eq!(s.pending[1].delivery.status, "queued");
    let mut busy = hook(HookKind::PromptSubmitted);
    busy.sequence = 4;
    s.apply_hook(busy);
    let mut done = hook(HookKind::TurnDone);
    done.sequence = 5;
    s.apply_hook(done);
    s.flush(&writer);
    assert_eq!(writes.lock().unwrap().len(), 2);
}
#[test]
fn write_failure_is_unconfirmed_and_never_retried() {
    let attempts = Arc::new(Mutex::new(0));
    let copy = attempts.clone();
    let writer: Writer = Box::new(move |_, _| {
        *copy.lock().unwrap() += 1;
        Err("fixture".into())
    });
    let mut s = Session::new(hook(HookKind::Idle));
    assert_eq!(
        s.enqueue("one".into(), None, &writer).unwrap().status,
        "unconfirmed"
    );
    s.ready_credit = true;
    s.flush(&writer);
    assert_eq!(*attempts.lock().unwrap(), 1);
}
#[test]
fn delivery_text_is_redacted_without_search_truncation() {
    let (writer, _) = writer();
    let mut s = Session::new(hook(HookKind::ToolStart));
    let text = format!(
        "{}\nAPI_KEY=super-secret-fixture",
        "normal prose ".repeat(4000)
    );
    let value = s.enqueue(text, None, &writer).unwrap();
    assert!(value.text.len() > 32 * 1024);
    assert!(!value.text.contains("super-secret-fixture"));
    assert!(!value.text.contains("truncated for search"));
}

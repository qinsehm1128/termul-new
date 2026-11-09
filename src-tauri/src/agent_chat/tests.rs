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
    assert_eq!(s.snapshot(None).messages.len(), 1);
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
    assert_eq!(s.snapshot(None).messages.len(), 1);
    writeln!(std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap())
    .unwrap();
    s.read();
    assert_eq!(s.snapshot(None).messages.len(), 2);
    let tail = s.snapshot(Some(cursor));
    assert_eq!(tail.messages.len(), 1);
    assert!(!tail.reset);
    s.read();
    assert_eq!(s.snapshot(None).messages.len(), 2);
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
    assert!(s.snapshot(None).messages.is_empty());
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
    let title = s.snapshot(None).tool_calls[0]["title"].clone();
    let kind = s.snapshot(None).tool_calls[0]["kind"].clone();
    append(
        &path,
        json!({"type":"user","uuid":"result-record","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call1","content":"done"}]}}),
    );
    s.read();
    assert_eq!(s.snapshot(None).tool_calls.len(), 1);
    assert_eq!(s.snapshot(None).tool_calls[0]["status"], "completed");
    assert_eq!(s.snapshot(None).tool_calls[0]["title"], title);
    assert_eq!(s.snapshot(None).tool_calls[0]["kind"], kind);
    assert_eq!(
        s.snapshot(None).tool_calls[0]["rawInput"]["file_path"],
        "fixture.txt"
    );
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
    assert_eq!(s.snapshot(None).messages.len(), 1);
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

#[test]
fn replacement_from_short_to_longer_history_returns_a_fenced_replacement() {
    let (_dir, path, mut s) = transcript();
    append(
        &path,
        json!({"type":"user","uuid":"old","message":{"role":"user","content":"old"}}),
    );
    s.read();
    let old = s.cursor;
    let mut next = hook(HookKind::SessionStart);
    next.session_id = "resumed".into();
    s.replace(next);
    s.path = Some(path.clone());
    let contents: String = (0..100).map(|i| format!("{}\n", json!({"type":"user","uuid":format!("new-{i}"),"message":{"role":"user","content":format!("new {i}")}}))).collect();
    std::fs::write(&path, contents).unwrap();
    s.read();
    let page = s.snapshot(Some(old));
    assert!(page.reset);
    assert!(page.cursor > old);
    assert_eq!(page.messages.len(), 100);
    assert_eq!(page.messages[0].id, "claude:new-0");
    let stale_page = s.page(None, Some(old));
    assert!(stale_page.reset);
    assert_eq!(stale_page.messages.len(), 100);
}

#[test]
fn same_inode_truncate_and_regrow_validates_the_previous_boundary() {
    let (_dir, path, mut s) = transcript();
    append(
        &path,
        json!({"type":"user","uuid":"stale","message":{"role":"user","content":"stale"}}),
    );
    s.read();
    let old = s.cursor;
    let identity = FileStamp::read(&path).unwrap().identity;
    std::fs::write(&path, format!("{}\n{}\n", json!({"type":"user","uuid":"fresh","message":{"role":"user","content":"replacement longer than the original record"}}), json!({"type":"user","uuid":"fresh-2","message":{"role":"user","content":"second"}}))).unwrap();
    assert_eq!(FileStamp::read(&path).unwrap().identity, identity);
    s.read();
    let page = s.snapshot(Some(old));
    assert!(page.reset);
    assert_eq!(page.messages.len(), 2);
    assert!(page.messages.iter().all(|m| m.id != "claude:stale"));
    let stale_page = s.page(None, Some(old));
    assert!(stale_page.reset);
    assert_eq!(stale_page.messages.len(), 2);
    assert!(stale_page.messages.iter().all(|m| m.id != "claude:stale"));
}

#[test]
fn session_end_and_pty_close_finalize_multiple_queued_messages_without_resend() {
    for closed in [false, true] {
        let (writer, writes) = writer();
        let mut s = Session::new(hook(HookKind::ToolStart));
        s.enqueue("one".into(), None, &writer).unwrap();
        s.enqueue("two".into(), None, &writer).unwrap();
        let before = s.cursor;
        if closed {
            let mut sessions = HashMap::from([("terminal-fixture".into(), s)]);
            retire_sessions(&mut sessions, Instant::now(), &|_| false);
            s = sessions.remove("terminal-fixture").unwrap();
        } else {
            let mut end = hook(HookKind::SessionEnd);
            end.sequence = 2;
            s.apply_hook(end);
        }
        assert!(s.cursor > before);
        assert!(s
            .pending
            .iter()
            .all(|p| p.delivery.status == "unconfirmed" && p.raw.is_empty() && p.guard.is_none()));
        s.flush(&writer);
        assert!(writes.lock().unwrap().is_empty());
    }
}

#[test]
fn more_than_128_sequential_closed_terminals_do_not_exhaust_the_service() {
    let mut sessions = HashMap::new();
    for i in 0..300 {
        assert!(make_room(&mut sessions));
        let mut event = hook(HookKind::SessionStart);
        event.terminal_id = format!("terminal-{i}");
        sessions.insert(event.terminal_id.clone(), Session::new(event));
        retire_sessions(&mut sessions, Instant::now(), &|_| false);
        assert!(sessions.len() <= MAX_SESSIONS);
    }
    assert!(sessions.contains_key("terminal-299"));
    retire_sessions(
        &mut sessions,
        Instant::now() + Duration::from_secs(31),
        &|_| false,
    );
    assert!(sessions.is_empty());
}

#[test]
fn large_transcript_has_bounded_batches_pages_indexed_dedupe_and_complete_older_text() {
    let (_dir, path, mut s) = transcript();
    let text = "full text, not index-truncated ".repeat(128);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    for i in 0..5000 {
        writeln!(file, "{}", json!({"type":"user","uuid":format!("large-{i}"),"message":{"role":"user","content":text}})).unwrap();
    }
    drop(file);
    let len = std::fs::metadata(&path).unwrap().len();
    let mut batches = 0;
    while s.offset < len {
        let old_offset = s.offset;
        s.read();
        batches += 1;
        assert!(s.offset > old_offset);
        assert!(
            s.offset - old_offset
                <= scan::MAX_SCAN_BYTES + crate::memory_index::types::MAX_RECORD_BYTES as u64
        );
        let page = s.snapshot(None);
        assert!(page.messages.len() <= history::PAGE_MESSAGES);
        assert!(
            serde_json::to_vec(&page.messages).unwrap().len()
                <= history::PAGE_BYTES + crate::memory_index::types::MAX_RECORD_BYTES
        );
        assert!(s.anchor.len() <= 256);
    }
    assert!(batches >= 5000 / scan::MAX_SCAN_RECORDS);
    let latest = s.snapshot(None);
    assert!(latest.older_cursor.is_some());
    let mut before = latest.older_cursor;
    let mut count = latest.messages.len();
    while let Some(cursor) = before {
        let page = s.page(None, Some(cursor));
        assert!(!page.reset);
        assert_eq!(page.cursor, latest.cursor);
        assert!(page.messages.iter().all(|m| m.seq < cursor));
        for m in &page.messages {
            assert_eq!(m.blocks[0]["text"], text);
        }
        count += page.messages.len();
        before = page.older_cursor;
    }
    assert_eq!(count, 5000);
    let cursor = s.cursor;
    s.apply_entry(
        0,
        adapters::parse_record(
            AgentKind::ClaudeCode,
            &json!({"type":"user","uuid":"large-0","message":{"role":"user","content":text}}),
            0,
        )
        .unwrap()
        .remove(0),
    );
    assert_eq!(s.cursor, cursor); // old, evicted-from-window ID still indexed on disk
}

#[test]
fn completed_receipts_are_bounded_and_a_lagging_delta_is_explicitly_reset() {
    let (writer, _) = writer();
    let mut s = Session::new(hook(HookKind::Idle));
    for _ in 0..200 {
        s.ready_credit = true;
        s.enqueue("receipt".into(), None, &writer).unwrap();
        s.expire(Instant::now() + ACK_TIMEOUT + Duration::from_secs(1));
    }
    assert!(s.pending.len() <= MAX_RECEIPTS);
    let before = s.cursor;
    for i in 0..300 {
        s.apply_entry(i, adapters::parse_record(AgentKind::ClaudeCode, &json!({"type":"user","uuid":format!("gap-{i}"),"message":{"role":"user","content":"entry"}}), i).unwrap().remove(0));
    }
    let page = s.snapshot(Some(before));
    assert!(page.reset);
    assert!(page.older_cursor.is_some());
    assert_eq!(page.messages.len(), history::PAGE_MESSAGES);
}

// Called by the executable managed-observer fixture: events are captured from
// production generated TypeScript + shell, not a test-seeded synthetic Idle.
pub(crate) fn assert_real_pi_observer_startup_can_send(events: Vec<Value>) {
    assert_eq!(events[0]["event"], "session_start");
    assert_eq!(events[1]["event"], "idle");
    let mut start = hook(HookKind::SessionStart);
    start.agent = AgentKind::Pi;
    start.session_id = events[0]["session_id"].as_str().unwrap().into();
    let mut s = Session::new(start.clone());
    let (writer, writes) = writer();
    s.enqueue("first pi message".into(), None, &writer).unwrap();
    assert!(writes.lock().unwrap().is_empty());
    // Decode the real observer event only after enqueue, exactly like the host.
    let mut ready = start;
    ready.kind = serde_json::from_value(json!(events[1]["event"])).unwrap();
    ready.sequence = 2;
    s.apply_hook(ready);
    s.flush(&writer);
    assert_eq!(writes.lock().unwrap().len(), 1);
    assert_eq!(s.pending[0].delivery.status, "pending");
    s.flush(&writer);
    assert_eq!(writes.lock().unwrap().len(), 1);
}

#[test]
fn pty_close_without_receipts_advances_final_state_cursor() {
    let mut s = Session::new(hook(HookKind::Idle));
    let before = s.cursor;
    s.finish();
    assert!(s.cursor > before);
    assert_eq!(s.snapshot(Some(before)).hook_state, "session_end");
}

#[test]
fn tool_groups_and_large_tool_outputs_are_fully_pageable() {
    let mut s = Session::new(hook(HookKind::Idle));
    let calls: Vec<Value> = (0..140).map(|i| json!({"type":"tool_use","id":format!("tool-{i}"),"name":"Read","input":{"path":format!("file-{i}")}})).collect();
    let entry = adapters::parse_record(AgentKind::ClaudeCode, &json!({"type":"assistant","uuid":"many-tools","message":{"role":"assistant","content":calls}}), 0).unwrap().remove(0);
    s.apply_entry(0, entry);
    for i in 0..4 {
        s.apply_entry(i + 1, adapters::parse_record(AgentKind::ClaudeCode, &json!({"type":"user","uuid":format!("result-{i}"),"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":format!("tool-{i}"),"content":"complete tool output ".repeat(60000)}]}}), i + 1).unwrap().remove(0));
    }
    let mut tools = std::collections::HashMap::new();
    let mut page = s.snapshot(None);
    loop {
        assert!(page.messages.len() <= history::PAGE_MESSAGES);
        for tool in page.tool_calls {
            tools.insert(tool["toolCallId"].as_str().unwrap().to_string(), tool);
        }
        let Some(before) = page.older_cursor else {
            break;
        };
        page = s.page(None, Some(before));
    }
    assert_eq!(tools.len(), 140);
    assert_eq!(
        tools["tool-0"]["rawOutput"],
        "complete tool output ".repeat(60000)
    );
}

#[test]
fn oversized_unterminated_lines_are_discarded_in_bounded_resumable_turns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversized.jsonl");
    std::fs::write(
        &path,
        vec![b'x'; crate::memory_index::types::MAX_RECORD_BYTES * 3],
    )
    .unwrap();
    let mut state = scan::ScanState::default();
    let mut offset = 0;
    let len = std::fs::metadata(&path).unwrap().len();
    let mut turns = 0;
    while offset < len {
        let result = scan::scan_file(
            std::fs::File::open(&path).unwrap(),
            offset,
            &mut state,
            |_| panic!("oversized record must not parse"),
        );
        assert!(result.resume_offset > offset);
        assert!(
            result.resume_offset - offset
                <= crate::memory_index::types::MAX_RECORD_BYTES as u64 + 8192
        );
        offset = result.resume_offset;
        turns += 1;
    }
    assert!(turns > 2);
    assert!(state.discarding);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "\n{}", json!({"valid":true})).unwrap();
    let mut seen = 0;
    scan::scan_file(
        std::fs::File::open(&path).unwrap(),
        offset,
        &mut state,
        |_| seen += 1,
    );
    assert_eq!(seen, 1);
    assert!(!state.discarding);
    let mut count = 0;
    let result = scan::scan(&path, len + 1, |_| count += 1);
    assert_eq!(count, 1);
    assert!(result.issues.is_empty());
}

#[test]
fn receipt_byte_budget_reclaims_finished_items_without_becoming_a_lifetime_limit() {
    let (writer, _) = writer();
    let mut s = Session::new(hook(HookKind::Idle));
    for _ in 0..40 {
        s.ready_credit = true;
        s.enqueue("plain conversation sentence ".repeat(2500), None, &writer)
            .unwrap();
        assert!(s.pending.back().unwrap().delivery.text.len() > 60 * 1024);
        s.expire(Instant::now() + ACK_TIMEOUT + Duration::from_secs(1));
        assert!(
            s.pending
                .iter()
                .map(|p| p.raw.len() + p.delivery.text.len())
                .sum::<usize>()
                <= MAX_PENDING_BYTES
        );
        assert!(s
            .pending
            .iter()
            .all(|p| p.raw.is_empty() && p.guard.is_none()));
    }
    assert!(s.pending.len() < 40);
}

#[test]
fn canonical_session_start_requires_force_and_real_idle_gets_a_distinct_revision() {
    use crate::agent_hooks::{
        contract::*,
        state::{StateInput, StateStore},
    };
    for adapter in ["claude-code", "pi", "qin-code"] {
        let mut store = StateStore::new("host".into());
        let identity = HookIdentityDto {
            adapter_id: adapter.to_owned().try_into().unwrap(),
            agent_session_id: "session".into(),
            transcript_path: None,
            cwd: None,
            source: HookIdentitySource::Hook,
            descriptor_id: None,
            resume_argv: None,
        };
        let input = StateInput {
            terminal_id: "canonical-cold-start".into(),
            generation: 0,
            adapter_install_id: "install".into(),
            project_id: "project".into(),
            identity,
            kind: HookKind::SessionStart,
        };
        let start = store.upsert(input.clone(), Instant::now()).unwrap();
        assert!(!start.ready);
        let mut session = Session::new(derived_chat_hook(start.clone()).unwrap());
        let (writer, writes) = writer();
        let queued = session
            .enqueue("queued once".into(), None, &writer)
            .unwrap();
        assert_eq!(queued.status, "queued");
        assert!(session.snapshot(None).can_force_send);
        assert!(writes.lock().unwrap().is_empty());
        session.force_send(&queued.id, None, &writer).unwrap();
        session.flush(&writer);
        assert_eq!(
            &*writes.lock().unwrap(),
            &["\x1b[200~queued once\x1b[201~\r"]
        );
        assert!(session.force_send(&queued.id, None, &writer).is_err());
        let idle = store
            .upsert(
                StateInput {
                    kind: HookKind::Idle,
                    ..input.clone()
                },
                Instant::now(),
            )
            .unwrap();
        assert!(idle.ready);
        assert!(idle.revision > start.revision);
        session.apply_hook(derived_chat_hook(idle).unwrap());
        session.flush(&writer);
        assert_eq!(writes.lock().unwrap().len(), 1);
        let repeated_start = store.upsert(input, Instant::now()).unwrap();
        assert!(!repeated_start.ready);
        assert_eq!(
            derived_chat_hook(repeated_start).unwrap().kind,
            HookKind::SessionStart
        );
    }
}

#[test]
fn cold_start_without_idle_requires_explicit_force_and_only_sends_the_selected_receipt() {
    for agent in [AgentKind::ClaudeCode, AgentKind::Pi, AgentKind::QinCode] {
        let mut start = hook(HookKind::SessionStart);
        start.agent = agent;
        let mut s = Session::new(start);
        let (writer, writes) = writer();
        let first = s.enqueue("first queued".into(), None, &writer).unwrap();
        let second = s.enqueue("explicit second".into(), None, &writer).unwrap();
        assert!(writes.lock().unwrap().is_empty());
        assert!(s.snapshot(None).can_force_send);
        let forced = s
            .force_send(&second.id, Some(Arc::new(|| true)), &writer)
            .unwrap();
        assert_eq!(forced.id, second.id);
        assert_eq!(forced.status, "pending");
        assert_eq!(s.pending.len(), 2);
        assert_eq!(s.pending[0].delivery.id, first.id);
        assert_eq!(s.pending[0].delivery.status, "queued");
        assert_eq!(
            &*writes.lock().unwrap(),
            &["\x1b[200~explicit second\x1b[201~\r"]
        );
        assert!(!s.snapshot(None).can_force_send);
        assert!(s.force_send(&first.id, None, &writer).is_err());
        assert!(s.force_send(&second.id, None, &writer).is_err());
        s.expire(Instant::now() + ACK_TIMEOUT + Duration::from_secs(1));
        s.flush(&writer);
        assert_eq!(writes.lock().unwrap().len(), 1); // never retries a forced write
    }
}

#[test]
fn known_busy_or_previously_ready_sessions_reject_the_force_override() {
    for kind in [
        HookKind::PromptSubmitted,
        HookKind::ToolStart,
        HookKind::ToolEnd,
        HookKind::NeedsApproval,
        HookKind::Blocked,
        HookKind::Error,
        HookKind::SessionEnd,
    ] {
        let mut s = Session::new(hook(HookKind::SessionStart));
        let (writer, writes) = writer();
        let queued = s.enqueue("queued".into(), None, &writer).unwrap();
        let mut busy = hook(kind);
        busy.sequence = 2;
        s.apply_hook(busy);
        assert!(!s.snapshot(None).can_force_send);
        assert!(s.force_send(&queued.id, None, &writer).is_err());
        assert!(writes.lock().unwrap().is_empty());
    }
    let mut s = Session::new(hook(HookKind::Idle));
    let mut identity = hook(HookKind::SessionStart);
    identity.sequence = 2;
    s.apply_hook(identity);
    let (writer, writes) = writer();
    let queued = s.enqueue("queued".into(), None, &writer).unwrap();
    assert!(!s.can_force_send());
    assert!(s.force_send(&queued.id, None, &writer).is_err());
    assert!(writes.lock().unwrap().is_empty());
}

#[test]
fn explicit_force_rechecks_input_authorization_and_confirms_only_from_transcript_echo() {
    let (_dir, path, _) = transcript();
    let mut s = Session::new(hook(HookKind::SessionStart));
    s.path = Some(path.clone());
    let (writer, writes) = writer();
    let raw = "password=fixture-force-secret";
    let queued = s.enqueue(raw.into(), None, &writer).unwrap();
    assert!(s
        .force_send(&queued.id, Some(Arc::new(|| false)), &writer)
        .is_err());
    assert!(writes.lock().unwrap().is_empty());
    assert_eq!(s.pending[0].delivery.status, "queued");
    let sent = s
        .force_send(&queued.id, Some(Arc::new(|| true)), &writer)
        .unwrap();
    assert_eq!(sent.status, "pending");
    assert_eq!(
        writes.lock().unwrap()[0],
        format!("\x1b[200~{raw}\x1b[201~\r")
    );
    append(
        &path,
        json!({"type":"user","uuid":"forced-echo","message":{"role":"user","content":raw}}),
    );
    s.read();
    assert_eq!(s.pending[0].delivery.status, "delivered");
    assert!(s.pending[0].raw.is_empty());
    assert!(s.pending[0].guard.is_none());
    assert!(!s.snapshot(None).can_force_send);
    s.flush(&writer);
    assert_eq!(writes.lock().unwrap().len(), 1);
}

#[test]
fn failed_forced_write_is_final_and_never_resent() {
    let mut s = Session::new(hook(HookKind::SessionStart));
    let (quiet, _) = writer();
    let receipt = s.enqueue("queued".into(), None, &quiet).unwrap();
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = attempts.clone();
    let failed: Writer = Box::new(move |_, _| {
        captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err("partial write".into())
    });
    let result = s.force_send(&receipt.id, None, &failed).unwrap();
    assert_eq!(result.status, "unconfirmed");
    assert!(s.pending[0].raw.is_empty());
    assert!(!s.can_force_send());
    assert!(s.force_send(&receipt.id, None, &failed).is_err());
    s.flush(&failed);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

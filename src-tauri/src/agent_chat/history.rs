//! Ephemeral, disk-backed redacted transcript window. SQLite owns the dedupe index;
//! its page cache is bounded, and the temporary database is deleted on close.
use super::ChatMessage;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub(super) const PAGE_MESSAGES: usize = 128;
pub(super) const PAGE_BYTES: usize = 2 * 1024 * 1024;
pub(super) struct History(Connection);
pub(super) struct Page {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<Value>,
    pub older: Option<u64>,
}
impl History {
    pub fn new() -> rusqlite::Result<Self> {
        let db = Connection::open("")?;
        db.execute_batch("PRAGMA cache_size=-256; PRAGMA temp_store=FILE; PRAGMA journal_mode=OFF;
            CREATE TABLE messages(id TEXT PRIMARY KEY, seq INTEGER NOT NULL UNIQUE, body TEXT NOT NULL);
            CREATE TABLE tools(id TEXT PRIMARY KEY, seq INTEGER NOT NULL, body TEXT NOT NULL);
            CREATE INDEX tools_seq ON tools(seq);
            CREATE TABLE tool_events(seq INTEGER PRIMARY KEY, id TEXT NOT NULL);")?;
        Ok(Self(db))
    }
    pub fn clear(&self) -> rusqlite::Result<()> {
        self.0
            .execute_batch("DELETE FROM messages; DELETE FROM tools; DELETE FROM tool_events;")
    }
    pub fn contains(&self, id: &str) -> rusqlite::Result<bool> {
        self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?1)",
            [id],
            |r| r.get(0),
        )
    }
    pub fn has_between(&self, after: u64, before: u64) -> rusqlite::Result<bool> {
        self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM messages WHERE seq>?1 AND seq<?2)",
            params![after, before],
            |r| r.get(0),
        )
    }
    pub fn put_message(&self, message: &ChatMessage) -> rusqlite::Result<()> {
        let body = serde_json::to_string(message).map_err(|_| rusqlite::Error::InvalidQuery)?;
        self.0.execute("INSERT INTO messages VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq,body=excluded.body", params![message.id, message.seq, body])?;
        Ok(())
    }
    pub fn put_tool(&self, seq: u64, mut tool: Value) -> rusqlite::Result<()> {
        let Some(id) = tool["toolCallId"].as_str().map(str::to_owned) else {
            return Ok(());
        };
        let previous: Option<String> = self
            .0
            .query_row("SELECT body FROM tools WHERE id=?1", [&id], |r| r.get(0))
            .optional()?;
        if let Some(previous) = previous.and_then(|p| serde_json::from_str::<Value>(&p).ok()) {
            if let (Some(target), Some(update)) = (previous.as_object(), tool.as_object_mut()) {
                for (key, value) in target {
                    if (key == "title" && update.get(key).is_some_and(|v| v == "Tool result"))
                        || (key == "kind" && update.get(key).is_some_and(|v| v == "other"))
                    {
                        update.insert(key.clone(), value.clone());
                    } else {
                        update.entry(key.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }
        self.0.execute("INSERT INTO tools VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET seq=excluded.seq,body=excluded.body", params![id, seq, tool.to_string()])?;
        self.0.execute(
            "INSERT OR REPLACE INTO tool_events VALUES(?1,?2)",
            params![seq, id],
        )?;
        Ok(())
    }
    pub fn page(&self, after: u64, before: Option<u64>) -> rusqlite::Result<Page> {
        // Select newest-first so a first snapshot never scans/allocates the full journal.
        let mut statement = self.0.prepare("SELECT m.body, t.body FROM messages m LEFT JOIN tool_events e ON e.seq=m.seq LEFT JOIN tools t ON t.id=e.id WHERE m.seq>?1 AND m.seq<?2 ORDER BY m.seq DESC LIMIT ?3")?;
        let mut rows = statement.query(params![
            after,
            before.unwrap_or(i64::MAX as u64),
            PAGE_MESSAGES as u64
        ])?;
        let mut messages = Vec::new();
        let mut bytes = 0;
        let mut tools = Vec::new();
        let mut tool_ids = std::collections::HashSet::new();
        while let Some(row) = rows.next()? {
            let body: String = row.get(0)?;
            let tool: Option<String> = row.get(1)?;
            let row_bytes = body.len() + tool.as_ref().map_or(0, String::len);
            // A message/tool pair is atomic for paging. One complete pair may
            // exceed the target, but no independent tool cap can lose history.
            if !messages.is_empty() && bytes + row_bytes > PAGE_BYTES {
                break;
            }
            bytes += row_bytes;
            if let Some(tool) = tool {
                let tool: Value =
                    serde_json::from_str(&tool).map_err(|_| rusqlite::Error::InvalidQuery)?;
                if tool_ids.insert(tool["toolCallId"].as_str().unwrap_or("").to_owned()) {
                    tools.push(tool);
                }
            }
            messages.push(
                serde_json::from_str::<ChatMessage>(&body)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            );
        }
        messages.reverse();
        let first = messages.first().map(|m| m.seq);
        let older = match first {
            Some(seq)
                if self.0.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE seq<?1)",
                    [seq],
                    |r| r.get::<_, bool>(0),
                )? =>
            {
                Some(seq)
            }
            _ => None,
        };
        tools.reverse();
        Ok(Page {
            messages,
            tools,
            older,
        })
    }
}

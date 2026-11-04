//! Generation-owned web sessions; no browser/runtime dependencies.
use crate::validation::{error, io};
use se_plugin_protocol::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, PluginError>;
#[derive(Clone)]
pub(crate) struct Session {
    pub reply: WebOpenReply,
    pub owner: String,
    root: PathBuf,
    inventory: BTreeMap<String, PackageFile>,
    touched: Instant,
    events: VecDeque<WebReadEvent>,
    sequence: u64,
    bytes: usize,
    pending: BTreeSet<String>,
    seen: BTreeSet<String>,
}
#[derive(Default)]
pub(crate) struct WebSessions {
    sessions: BTreeMap<String, Session>,
    changed: bool,
    themes: BTreeMap<String, ThemeSnapshot>,
    rates: BTreeMap<(String, String), (f64, Instant)>,
}
impl WebSessions {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &mut self,
        boot: &str,
        owner: &str,
        manifest: &Manifest,
        generation: &str,
        package: (PathBuf, PackageDescriptor),
        digest: &str,
        view: &str,
    ) -> Result<WebOpenReply> {
        self.expire();
        let definition = manifest
            .contributes
            .views
            .iter()
            .find(|v| v.id == view && v.kind == Some(ViewKind::Web))
            .ok_or_else(|| error(PLUGIN_NOT_SUPPORTED, "view is not a web view"))?;
        if self.sessions.len() >= MAX_WEB_SESSIONS
            || self
                .sessions
                .values()
                .filter(|s| s.reply.plugin_id == manifest.id)
                .count()
                >= MAX_WEB_SESSIONS_PER_PLUGIN
            || self
                .sessions
                .values()
                .filter(|s| s.reply.plugin_id == manifest.id && s.reply.view_id == view)
                .count()
                >= MAX_WEB_SESSIONS_PER_VIEW
        {
            return Err(error(PLUGIN_BUSY, "web session limit reached"));
        }
        let (root, descriptor) = package;
        let entry = definition
            .entry
            .clone()
            .ok_or_else(|| error(PLUGIN_INVALID_MANIFEST, "web entry missing"))?;
        if !descriptor
            .files
            .iter()
            .any(|f| f.path == format!("ui/{entry}"))
        {
            return Err(error(
                PLUGIN_INVALID_PACKAGE,
                "web entry missing from inventory",
            ));
        }
        let reply = WebOpenReply {
            boot_id: boot.into(),
            plugin_id: manifest.id.clone(),
            view_id: view.into(),
            generation: generation.into(),
            session_id: uuid::Uuid::new_v4().to_string(),
            nonce: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            package_digest: digest.into(),
            entry,
            allow: definition.allow.clone().unwrap_or_default(),
            shim_version: WEB_SHIM_VERSION,
        };
        validate_web_open_reply(&reply)?;
        self.sessions.insert(
            reply.session_id.clone(),
            Session {
                reply: reply.clone(),
                owner: owner.to_owned(),
                root,
                inventory: descriptor
                    .files
                    .into_iter()
                    .map(|f| (f.path.clone(), f))
                    .collect(),
                touched: Instant::now(),
                events: VecDeque::new(),
                sequence: 0,
                bytes: 0,
                pending: BTreeSet::new(),
                seen: BTreeSet::new(),
            },
        );
        Ok(reply)
    }
    fn expire(&mut self) {
        self.sessions
            .retain(|_, s| s.touched.elapsed() < Duration::from_millis(WEB_SESSION_LEASE_MS));
    }
    pub fn get(&mut self, id: &str) -> Result<Session> {
        self.expire();
        let session = self.sessions.get_mut(id).ok_or_else(|| {
            error(
                PLUGIN_UI_SESSION_EXPIRED,
                "web session is closed or expired",
            )
        })?;
        session.touched = Instant::now();
        Ok(session.clone())
    }
    pub fn admit(&mut self, request: &WebRequest) -> Result<Session> {
        validate_web_request(request)?;
        let session = self.get(&request.session_id)?;
        let needed = match request.method.as_str() {
            "invoke" => Some(WebAllow::Commands),
            "view.dispatch" => Some(WebAllow::ViewDispatch),
            "config.get" => Some(WebAllow::ConfigRead),
            "config.set" => Some(WebAllow::ConfigWrite),
            "context.snapshot" => Some(WebAllow::ContextRead),
            "storage.get" => Some(WebAllow::StorageRead),
            "storage.set" | "storage.delete" => Some(WebAllow::StorageWrite),
            "feedback.notification.upsert"
            | "feedback.notification.close"
            | "feedback.progress.upsert" => Some(WebAllow::Feedback),
            "theme.get" => None,
            _ => return Err(error(PLUGIN_NOT_SUPPORTED, "unknown web method")),
        };
        if needed.is_some_and(|allow| !session.reply.allow.contains(&allow)) {
            return Err(error(PLUGIN_UI_FORBIDDEN, "web method not allowed by view"));
        }
        let rate = self
            .rates
            .entry((
                session.reply.plugin_id.clone(),
                session.reply.generation.clone(),
            ))
            .or_insert((WEB_RATE_BURST as f64, Instant::now()));
        let now = Instant::now();
        rate.0 = (rate.0 + now.duration_since(rate.1).as_secs_f64() * WEB_RATE_PER_SECOND as f64)
            .min(WEB_RATE_BURST as f64);
        rate.1 = now;
        let live = self
            .sessions
            .get_mut(&request.session_id)
            .expect("session exists");
        if live.pending.len() >= MAX_WEB_PENDING_REQUESTS
            || live.seen.len() >= 4096
            || live.seen.contains(&request.request_id)
            || rate.0 < 1.0
        {
            return Err(error(
                PLUGIN_BUSY,
                "web request duplicate or quota exceeded",
            ));
        }
        rate.0 -= 1.0;
        live.pending.insert(request.request_id.clone());
        live.seen.insert(request.request_id.clone());
        Ok(session)
    }
    pub fn complete(&mut self, id: &str, request: &str) {
        if let Some(session) = self.sessions.get_mut(id) {
            session.pending.remove(request);
        }
    }
    pub fn close(&mut self, id: &str) -> bool {
        self.sessions.remove(id).is_some()
    }
    pub fn revoke(&mut self, id: &str, generation: &str) {
        self.sessions
            .retain(|_, s| s.reply.plugin_id != id || s.reply.generation != generation);
        self.rates.remove(&(id.into(), generation.into()));
    }
    pub fn detach(&mut self, owner: &str) {
        self.sessions.retain(|_, s| s.owner != owner);
        self.themes.remove(owner);
    }
    pub fn post(
        &mut self,
        id: &str,
        generation: &str,
        view: &str,
        kind: WebEventType,
        payload: Value,
    ) -> Result<u32> {
        self.expire();
        let bytes = serde_json::to_vec(&payload).map_err(io)?.len();
        if bytes > MAX_WEB_PAYLOAD_BYTES {
            return Err(error(PLUGIN_INVALID_REQUEST, "web message too large"));
        }
        validate_web_read_reply(&WebReadReply {
            sequence: 9_007_199_254_740_991,
            resync_required: true,
            events: vec![WebReadEvent {
                sequence: 9_007_199_254_740_991,
                event_type: kind,
                payload: payload.clone(),
            }],
        })?;
        let mut delivered = 0;
        for session in self.sessions.values_mut().filter(|s| {
            s.reply.plugin_id == id
                && s.reply.generation == generation
                && (view.is_empty() || s.reply.view_id == view)
        }) {
            while session.events.len() >= MAX_WEB_QUEUED_EVENTS
                || session.bytes + bytes > MAX_WEB_QUEUED_BYTES
            {
                if let Some(event) = session.events.pop_front() {
                    session.bytes = session
                        .bytes
                        .saturating_sub(serde_json::to_vec(&event.payload).map_err(io)?.len());
                } else {
                    break;
                }
            }
            session.sequence += 1;
            session.events.push_back(WebReadEvent {
                sequence: session.sequence,
                event_type: kind,
                payload: payload.clone(),
            });
            session.bytes += bytes;
            delivered += 1;
        }
        self.changed |= delivered > 0;
        Ok(delivered)
    }
    pub fn read(&mut self, request: &WebReadRequest) -> Result<WebReadReply> {
        validate_web_read_request(request)?;
        let session = self.get(&request.session_id)?;
        if request.after_sequence > session.sequence {
            return Err(error(PLUGIN_INVALID_REQUEST, "invalid web cursor"));
        }
        let resync_required = session
            .events
            .front()
            .is_some_and(|e| e.sequence > request.after_sequence.saturating_add(1));
        let mut reply = WebReadReply {
            sequence: session.sequence,
            resync_required,
            events: Vec::new(),
        };
        for event in session
            .events
            .iter()
            .filter(|e| e.sequence > request.after_sequence)
            .take(MAX_WEB_READ_EVENTS)
        {
            reply.events.push(event.clone());
            reply.sequence = event.sequence;
            if validate_web_read_reply(&reply).is_err() {
                reply.events.pop();
                break;
            }
        }
        reply.sequence = reply
            .events
            .last()
            .map_or(session.sequence, |event| event.sequence);
        validate_web_read_reply(&reply)?;
        Ok(reply)
    }
    pub fn asset(session: &Session, request: &WebAssetRequest) -> Result<WebAssetReply> {
        use base64::Engine;
        validate_web_asset_request(request)?;
        let path = if request.path.starts_with("ui/") {
            request.path.clone()
        } else {
            format!("ui/{}", request.path)
        };
        if !is_package_path(&path) || !path.starts_with("ui/") {
            return Err(error(PLUGIN_INVALID_REQUEST, "invalid asset path"));
        }
        let item = session
            .inventory
            .get(&path)
            .ok_or_else(|| error(PLUGIN_INVALID_REQUEST, "asset not in inventory"))?;
        let ui = session.root.join("ui").canonicalize().map_err(io)?;
        let resolved = session.root.join(&path).canonicalize().map_err(io)?;
        let root = session.root.canonicalize().map_err(io)?;
        if !ui.starts_with(&root)
            || ui != root.join("ui")
            || !resolved.starts_with(&ui)
            || !resolved.is_file()
        {
            return Err(error(PLUGIN_INVALID_PACKAGE, "asset escapes package"));
        }
        use std::io::Read;
        let mut bytes = Vec::new();
        fs::File::open(&resolved)
            .map_err(io)?
            .take(item.size.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(io)?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        if bytes.len() as u64 != item.size || sha256 != item.sha256 {
            return Err(error(PLUGIN_INVALID_PACKAGE, "asset SHA-256 mismatch"));
        }
        let extension = resolved.extension().and_then(|v| v.to_str()).unwrap_or("");
        let mime = match extension {
            "html" => "text/html",
            "js" | "mjs" => "text/javascript",
            "css" => "text/css",
            "json" => "application/json",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "wasm" => "application/wasm",
            _ => "application/octet-stream",
        };
        let reply = WebAssetReply {
            mime: mime.into(),
            size: bytes.len() as u64,
            sha256,
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        validate_web_asset_reply(&reply)?;
        Ok(reply)
    }
    pub fn identities(&self) -> Vec<(String, String, String, String, Vec<WebAllow>)> {
        self.sessions
            .values()
            .map(|s| {
                (
                    s.reply.session_id.clone(),
                    s.reply.plugin_id.clone(),
                    s.reply.generation.clone(),
                    s.owner.clone(),
                    s.reply.allow.clone(),
                )
            })
            .collect()
    }
    pub fn theme(&self, owner: &str) -> ThemeSnapshot {
        self.themes.get(owner).cloned().unwrap_or(ThemeSnapshot {
            revision: 0,
            appearance: ThemeAppearance::Dark,
            variables: BTreeMap::new(),
            font_family: "system-ui".into(),
            font_size: "13px".into(),
        })
    }
    pub fn publish_theme(&mut self, owner: &str, theme: ThemeSnapshot) -> Result<()> {
        validate_theme_snapshot(&theme)?;
        if self
            .themes
            .get(owner)
            .is_some_and(|old| old.revision >= theme.revision)
        {
            return Err(error(PLUGIN_CONFIG_CONFLICT, "stale theme revision"));
        }
        if !self.themes.contains_key(owner) && self.themes.len() >= MAX_WEB_SESSIONS {
            return Err(error(PLUGIN_BUSY, "theme owner limit"));
        }
        self.themes.insert(owner.into(), theme.clone());
        let ids: Vec<_> = self
            .sessions
            .values()
            .filter(|s| s.owner == owner)
            .map(|s| s.reply.session_id.clone())
            .collect();
        for id in ids {
            self.post_one(
                &id,
                WebEventType::ThemeChanged,
                serde_json::to_value(&theme).map_err(io)?,
            )?;
        }
        Ok(())
    }
    pub fn post_one(&mut self, session_id: &str, kind: WebEventType, payload: Value) -> Result<()> {
        self.expire();
        let size = serde_json::to_vec(&payload).map_err(io)?.len();
        if size > MAX_WEB_PAYLOAD_BYTES {
            return Err(error(PLUGIN_INVALID_REQUEST, "web event too large"));
        }
        validate_web_read_reply(&WebReadReply {
            sequence: 9_007_199_254_740_991,
            resync_required: true,
            events: vec![WebReadEvent {
                sequence: 9_007_199_254_740_991,
                event_type: kind,
                payload: payload.clone(),
            }],
        })?;
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| error(PLUGIN_UI_SESSION_EXPIRED, "session closed"))?;
        while session.events.len() >= MAX_WEB_QUEUED_EVENTS
            || session.bytes + size > MAX_WEB_QUEUED_BYTES
        {
            if let Some(event) = session.events.pop_front() {
                session.bytes = session
                    .bytes
                    .saturating_sub(serde_json::to_vec(&event.payload).map_err(io)?.len());
            } else {
                break;
            }
        }
        session.sequence += 1;
        session.bytes += size;
        session.events.push_back(WebReadEvent {
            sequence: session.sequence,
            event_type: kind,
            payload,
        });
        self.changed = true;
        Ok(())
    }
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_hub() -> (WebSessions, String) {
        let mut hub = WebSessions::default();
        let id = uuid::Uuid::new_v4().to_string();
        let reply = WebOpenReply {
            boot_id: uuid::Uuid::new_v4().to_string(),
            plugin_id: "org.test.web".into(),
            view_id: "panel".into(),
            generation: uuid::Uuid::new_v4().to_string(),
            session_id: id.clone(),
            nonce: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            package_digest: "a".repeat(64),
            entry: "index.html".into(),
            allow: vec![WebAllow::ViewDispatch],
            shim_version: WEB_SHIM_VERSION,
        };
        hub.sessions.insert(
            id.clone(),
            Session {
                reply,
                owner: "owner".into(),
                root: PathBuf::new(),
                inventory: BTreeMap::new(),
                touched: Instant::now(),
                events: VecDeque::new(),
                sequence: 0,
                bytes: 0,
                pending: BTreeSet::new(),
                seen: BTreeSet::new(),
            },
        );
        (hub, id)
    }
    #[test]
    fn web_open_and_core_assets_match_frozen_contract() {
        let root = std::env::temp_dir().join(format!("web-assets-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("ui")).unwrap();
        let manifest:Manifest=serde_json::from_value(serde_json::json!({"schemaVersion":1,"id":"org.test.web","name":"Web","version":"1.0.0","abi":{"min":1,"max":1},"contributes":{"commands":[],"configuration":{"title":"Settings","properties":{}},"views":[{"id":"panel","title":"Panel","location":"panel","kind":"web","entry":"index.html","allow":["view.dispatch"]}]},"services":{"provides":[],"requires":[]}})).unwrap();
        let mut files = Vec::new();
        for (name, bytes) in [
            ("index.html", b"<html></html>".as_slice()),
            ("app.js", b"void 0".as_slice()),
            ("app.css", b"body{}".as_slice()),
        ] {
            fs::write(root.join("ui").join(name), bytes).unwrap();
            files.push(PackageFile {
                path: format!("ui/{name}"),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            });
        }
        let descriptor = PackageDescriptor {
            ui: "ui".into(),
            schema_version: 1,
            files,
            native: BTreeMap::new(),
        };
        let mut hub = WebSessions::default();
        let reply = hub
            .open(
                &uuid::Uuid::new_v4().to_string(),
                "owner",
                &manifest,
                &uuid::Uuid::new_v4().to_string(),
                (root.clone(), descriptor),
                &"a".repeat(64),
                "panel",
            )
            .unwrap();
        validate_web_open_reply(&reply).unwrap();
        let session = hub.get(&reply.session_id).unwrap();
        for (name, mime) in [
            ("index.html", "text/html"),
            ("app.js", "text/javascript"),
            ("app.css", "text/css"),
        ] {
            let asset = WebSessions::asset(
                &session,
                &WebAssetRequest {
                    session_id: reply.session_id.clone(),
                    path: format!("ui/{name}"),
                },
            )
            .unwrap();
            assert_eq!(asset.mime, mime);
            validate_web_asset_reply(&asset).unwrap();
        }
        fs::write(root.join("ui/index.html"), "tampered").unwrap();
        assert!(WebSessions::asset(
            &session,
            &WebAssetRequest {
                session_id: reply.session_id,
                path: "ui/index.html".into()
            }
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn web_session_revokes_with_generation() {
        let (mut hub, id) = make_hub();
        let session = hub.get(&id).unwrap();
        hub.revoke(&session.reply.plugin_id, &uuid::Uuid::new_v4().to_string());
        assert!(hub.get(&id).is_ok());
        hub.revoke(&session.reply.plugin_id, &session.reply.generation);
        assert_eq!(hub.get(&id).err().unwrap().code, PLUGIN_UI_SESSION_EXPIRED);
        let (mut hub, id) = make_hub();
        hub.detach("owner");
        assert!(hub.get(&id).is_err());
    }
    #[test]
    fn web_bridge_enforces_allow_list() {
        let (mut hub, id) = make_hub();
        let mut request = WebRequest {
            session_id: id.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            method: "invoke".into(),
            params: serde_json::json!({"commandId":"go"}),
        };
        assert!(hub.admit(&request).is_err());
        request.method = "view.dispatch".into();
        request.params = serde_json::json!({"action":"go"});
        assert!(hub.admit(&request).is_ok());
        assert!(hub.admit(&request).is_err());
        hub.complete(&id, &request.request_id);
        assert!(hub.admit(&request).is_err());
        let generation = hub.get(&id).unwrap().reply.generation;
        hub.revoke("org.test.web", &generation);
        assert!(hub.admit(&request).is_err());
    }
    #[test]
    fn host_ui_messages_are_bounded() {
        let (mut hub, id) = make_hub();
        let session = hub.get(&id).unwrap();
        for n in 0..MAX_WEB_QUEUED_EVENTS + 20 {
            assert_eq!(
                hub.post(
                    "org.test.web",
                    &session.reply.generation,
                    "panel",
                    WebEventType::Message,
                    serde_json::json!({"n":n})
                )
                .unwrap(),
                1
            );
        }
        assert_eq!(hub.sessions[&id].events.len(), MAX_WEB_QUEUED_EVENTS);
        assert!(hub.sessions[&id].bytes <= MAX_WEB_QUEUED_BYTES);
        let read = hub
            .read(&WebReadRequest {
                session_id: id.clone(),
                after_sequence: 0,
            })
            .unwrap();
        assert!(read.resync_required);
        assert_eq!(
            read.events.len(),
            MAX_WEB_QUEUED_EVENTS.min(MAX_WEB_READ_EVENTS)
        );
        assert!(hub
            .post(
                "org.test.web",
                &session.reply.generation,
                "panel",
                WebEventType::Message,
                Value::String("x".repeat(MAX_WEB_PAYLOAD_BYTES))
            )
            .is_err());
        let (mut large, large_id) = make_hub();
        let generation = large.get(&large_id).unwrap().reply.generation;
        for _ in 0..2 {
            large
                .post(
                    "org.test.web",
                    &generation,
                    "panel",
                    WebEventType::Message,
                    Value::String("a".repeat(131062)),
                )
                .unwrap();
        }
        let batch = large
            .read(&WebReadRequest {
                session_id: large_id.clone(),
                after_sequence: 0,
            })
            .unwrap();
        validate_web_read_reply(&batch).unwrap();
        assert_eq!(batch.events.len(), 1);
        let rest = large
            .read(&WebReadRequest {
                session_id: large_id,
                after_sequence: batch.sequence,
            })
            .unwrap();
        assert_eq!(rest.events.len(), 1);
        validate_web_read_reply(&rest).unwrap();
        hub.close(&id);
        assert_eq!(
            hub.post(
                "org.test.web",
                &session.reply.generation,
                "panel",
                WebEventType::Message,
                Value::Null
            )
            .unwrap(),
            0
        );
    }
}

//! GUI-only, filesystem-free serving for owner-scoped Plugin Core web sessions.
use crate::core::PluginServiceHandle;
use base64::Engine;
use se_plugin_protocol::{
    is_package_path, validate_web_close_request, PluginError, WebAssetReply, WebAssetRequest,
    WebCloseRequest, WebOpenReply, WEB_UI_CSP,
};
use std::{collections::VecDeque, sync::Mutex};
use tauri::http::{Method, Request, Response, StatusCode, Uri};

/// Reserved within each session root; package bytes cannot replace the trusted bridge.
pub const SHIM_PATH: &str = "__se__/se-plugin-ui.js";
/// Web (browser) transport prefix for the same session tuples.
pub const WEB_UI_PREFIX: &str = "/plugin-ui/";
/// Extra CSP appended for browser delivery so a top-level navigation to
/// `/plugin-ui/...` is still an opaque origin and can never touch the app origin.
pub const WEB_SANDBOX_CSP: &str =
    "sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads";
const WEB_REFERRER_POLICY: &str = "no-referrer";
const SHIM: &str = include_str!("../../src/renderer/features/plugins/web/shim/se-plugin-ui.js");
const MAX_TRACKED_SESSIONS: usize = 64;

#[derive(Clone)]
struct Session {
    plugin_id: String,
    generation: String,
    session_id: String,
    entry: String,
}

#[derive(Default)]
pub struct PluginUiSessions(Mutex<VecDeque<Session>>);

impl PluginUiSessions {
    pub fn insert(&self, reply: &WebOpenReply) -> Result<(), PluginError> {
        let mut sessions = self.0.lock().map_err(|_| unavailable())?;
        sessions.retain(|s| s.session_id != reply.session_id);
        // Core permits only 16 live sessions per owner. Bound stale reconnect metadata as well.
        while sessions.len() >= MAX_TRACKED_SESSIONS {
            sessions.pop_front();
        }
        sessions.push_back(Session {
            plugin_id: reply.plugin_id.clone(),
            generation: reply.generation.clone(),
            session_id: reply.session_id.clone(),
            entry: package_entry(&reply.entry),
        });
        Ok(())
    }

    pub fn remove(&self, session_id: &str) {
        if let Ok(mut sessions) = self.0.lock() {
            sessions.retain(|s| s.session_id != session_id);
        }
    }

    fn find(&self, route: &Route) -> Option<Session> {
        self.0
            .lock()
            .ok()?
            .iter()
            .find(|s| {
                s.plugin_id == route.plugin_id
                    && s.generation == route.generation
                    && s.session_id == route.session_id
            })
            .cloned()
    }
}

fn unavailable() -> PluginError {
    PluginError::new(
        se_plugin_protocol::PLUGIN_CORE_UNAVAILABLE,
        "plugin UI unavailable",
    )
}

/// Manifest web entries are relative to the package `ui/` directory; scheme URLs and Core
/// asset requests use full package paths so `../__se__/se-plugin-ui.js` resolves to the shim.
fn package_entry(entry: &str) -> String {
    if entry.starts_with("ui/") {
        entry.to_owned()
    } else {
        format!("ui/{entry}")
    }
}

/// W4 consumes this URL rather than constructing a platform-specific scheme URL.
pub fn session_url(reply: &WebOpenReply) -> String {
    let origin = if cfg!(target_os = "windows") {
        "http://se-plugin.localhost"
    } else {
        "se-plugin://localhost"
    };
    let mut url = url::Url::parse(origin).expect("static plugin origin");
    url.path_segments_mut()
        .expect("static hierarchical origin")
        .extend([&reply.plugin_id, &reply.generation, &reply.session_id]);
    url.path_segments_mut()
        .expect("static hierarchical origin")
        .extend(package_entry(&reply.entry).split('/'));
    url.into()
}

/// Browser transport URL for the same session. Segments are percent-encoded by
/// the same path-segment mechanism as [`session_url`], so the desktop and web
/// adapters agree on escaping.
pub fn web_session_url(reply: &WebOpenReply) -> String {
    let mut url = url::Url::parse("http://plugin.local").expect("static plugin origin");
    url.path_segments_mut()
        .expect("static hierarchical origin")
        .extend([
            "plugin-ui",
            reply.plugin_id.as_str(),
            reply.generation.as_str(),
            reply.session_id.as_str(),
        ]);
    url.path_segments_mut()
        .expect("static hierarchical origin")
        .extend(package_entry(&reply.entry).split('/'));
    url.path().to_owned()
}

#[derive(Debug)]
struct Route {
    plugin_id: String,
    generation: String,
    session_id: String,
    path: String,
}

fn decode_segment(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = if bytes[i] == b'%' {
            let hex = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            i += 3;
            (hi << 4) | lo
        } else {
            let b = bytes[i];
            i += 1;
            b
        };
        // Reject alternate separators, double encoding, NUL and control characters.
        if matches!(byte, b'/' | b'\\' | b'%') || byte < 0x20 || byte == 0x7f {
            return None;
        }
        result.push(byte);
    }
    let result = String::from_utf8(result).ok()?;
    if result.is_empty() || result == "." || result == ".." {
        return None;
    }
    Some(result)
}

fn parse_segments(raw: &str) -> Option<Route> {
    let mut segments = raw.split('/');
    let plugin_id = decode_segment(segments.next()?)?;
    let generation = decode_segment(segments.next()?)?;
    let session_id = decode_segment(segments.next()?)?;
    let path = segments
        .map(decode_segment)
        .collect::<Option<Vec<_>>>()?
        .join("/");
    validate_web_close_request(&WebCloseRequest {
        session_id: session_id.clone(),
    })
    .ok()?;
    if path != SHIM_PATH && !is_package_path(&path) {
        return None;
    }
    // Match against the trusted Core open reply before forwarding anything to Core.
    Some(Route {
        plugin_id,
        generation,
        session_id,
        path,
    })
}

fn parse_route(request: &Request<Vec<u8>>) -> Option<Route> {
    let uri = request.uri();
    // Windows webviews expose custom protocols via their synthetic HTTP origin.
    match (uri.scheme_str(), uri.authority()?.as_str()) {
        (Some("se-plugin"), "localhost") | (Some("http"), "se-plugin.localhost") => {}
        _ => return None,
    }
    if uri.query().is_some() {
        return None;
    }
    parse_segments(uri.path().strip_prefix('/')?)
}

/// Browser transport route: `/plugin-ui/{plugin}/{generation}/{session}/{path}`.
/// Query strings are rejected; the exact session tuple must have been opened
/// through the web route's tracked session set.
fn parse_web_route(uri: &Uri) -> Option<Route> {
    if uri.query().is_some() {
        return None;
    }
    parse_segments(uri.path().strip_prefix(WEB_UI_PREFIX)?)
}

/// Transport origin decides the shim URL injected into the entry document and
/// which defense-in-depth headers the response carries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum UiOrigin {
    CustomScheme,
    Web,
}

fn extra_headers(origin: UiOrigin) -> &'static [(&'static str, &'static str)] {
    match origin {
        UiOrigin::CustomScheme => &[],
        // The browser-visible page is an opaque origin even on top-level
        // navigation; it can never read the app origin's storage or DOM.
        UiOrigin::Web => &[
            ("Content-Security-Policy", WEB_SANDBOX_CSP),
            ("Referrer-Policy", WEB_REFERRER_POLICY),
        ],
    }
}

fn response_with(
    status: StatusCode,
    mime: &str,
    body: Vec<u8>,
    head: bool,
    origin: UiOrigin,
) -> Response<Vec<u8>> {
    let length = body.len();
    let mut builder = Response::builder()
        .status(status)
        .header("Content-Type", mime)
        .header("Content-Security-Policy", WEB_UI_CSP);
    for &(name, value) in extra_headers(origin) {
        builder = builder.header(name, value);
    }
    builder = builder
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        // The iframe has an opaque origin; never enable credentials or reflect arbitrary origins.
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, HEAD")
        .header("Content-Length", length.to_string());
    if status == StatusCode::METHOD_NOT_ALLOWED {
        builder = builder.header("Allow", "GET, HEAD");
    }
    builder
        .body(if head { Vec::new() } else { body })
        .expect("static response headers")
}

fn shim_url(origin: UiOrigin, session: &Session) -> String {
    match origin {
        UiOrigin::CustomScheme => format!(
            "/{}/{}/{}/{}",
            session.plugin_id, session.generation, session.session_id, SHIM_PATH
        ),
        UiOrigin::Web => format!(
            "{}{}/{}/{}/{}",
            WEB_UI_PREFIX, session.plugin_id, session.generation, session.session_id, SHIM_PATH
        ),
    }
}

async fn serve<F, Fut>(
    method: &Method,
    route: Option<Route>,
    sessions: &PluginUiSessions,
    fetch: F,
    origin: UiOrigin,
) -> Response<Vec<u8>>
where
    F: FnOnce(WebAssetRequest) -> Fut,
    Fut: std::future::Future<Output = Result<WebAssetReply, PluginError>>,
{
    let head = *method == Method::HEAD;
    if *method != Method::GET && !head {
        return response_with(
            StatusCode::METHOD_NOT_ALLOWED,
            "text/plain",
            Vec::new(),
            head,
            origin,
        );
    }
    let Some(route) = route else {
        return response_with(
            StatusCode::NOT_FOUND,
            "text/plain",
            Vec::new(),
            head,
            origin,
        );
    };
    let Some(session) = sessions.find(&route) else {
        return response_with(
            StatusCode::NOT_FOUND,
            "text/plain",
            Vec::new(),
            head,
            origin,
        );
    };
    let shim = route.path == SHIM_PATH;
    let fetched = fetch(WebAssetRequest {
        session_id: route.session_id.clone(),
        path: if shim {
            session.entry.clone()
        } else {
            route.path.clone()
        },
    })
    .await;
    let Ok(asset) = fetched else {
        return response_with(
            StatusCode::NOT_FOUND,
            "text/plain",
            Vec::new(),
            head,
            origin,
        );
    };
    if shim {
        return response_with(
            StatusCode::OK,
            "text/javascript",
            SHIM.as_bytes().to_vec(),
            head,
            origin,
        );
    }
    let Ok(mut bytes) = base64::engine::general_purpose::STANDARD.decode(&asset.data_base64) else {
        return response_with(
            StatusCode::NOT_FOUND,
            "text/plain",
            Vec::new(),
            head,
            origin,
        );
    };
    // The package entry is hash-verified by Core first. Append the application-owned bridge;
    // the nonce is read by the standalone shim from W4's URL fragment, never from HTML.
    if route.path == session.entry && asset.mime == "text/html" {
        let script = format!(
            "\n<script src=\"{}\"></script>\n",
            shim_url(origin, &session)
        );
        // Preserve the doctype and put the shim before package application scripts.
        let position = bytes
            .windows(7)
            .position(|s| s.eq_ignore_ascii_case(b"<script"))
            .unwrap_or(bytes.len());
        bytes.splice(position..position, script.bytes());
    }
    response_with(StatusCode::OK, &asset.mime, bytes, head, origin)
}

/// Testable HTTP policy, independent of a real webview. Every asset, including the shim,
/// is authorized by Plugin Core on the *current authenticated connection*.
async fn respond_with<F, Fut>(
    request: Request<Vec<u8>>,
    sessions: &PluginUiSessions,
    fetch: F,
) -> Response<Vec<u8>>
where
    F: FnOnce(WebAssetRequest) -> Fut,
    Fut: std::future::Future<Output = Result<WebAssetReply, PluginError>>,
{
    let method = request.method().clone();
    let route = parse_route(&request);
    serve(&method, route, sessions, fetch, UiOrigin::CustomScheme).await
}

pub async fn respond(
    request: Request<Vec<u8>>,
    sessions: &PluginUiSessions,
    service: &PluginServiceHandle,
) -> Response<Vec<u8>> {
    respond_with(request, sessions, |payload| service.web_asset(payload)).await
}

/// Browser transport entry point. Only sessions inserted through the web
/// `/plugins/web/open` route are known to this web-owned session set, so a
/// desktop-only session tuple 404s here.
pub async fn respond_web(
    method: &Method,
    uri: &Uri,
    sessions: &PluginUiSessions,
    service: &PluginServiceHandle,
) -> Response<Vec<u8>> {
    let route = parse_web_route(uri);
    serve(
        method,
        route,
        sessions,
        |payload| service.web_asset(payload),
        UiOrigin::Web,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    const PLUGIN: &str = "org.example.plugin";
    const GENERATION: &str = "01234567-89ab-4def-8123-456789abcdef";
    const SESSION: &str = "abcdef01-2345-4678-89ab-cdef01234567";

    fn reply() -> WebOpenReply {
        // Decode a wire fixture to keep the test aligned with the frozen W1 DTO.
        serde_json::from_value(serde_json::json!({
            "pluginId": PLUGIN, "generation": GENERATION, "sessionId": SESSION,
            "viewId": "main", "entry": "ui/dist/index.html", "nonce": "a".repeat(64),
            "bootId": GENERATION, "packageDigest": "a".repeat(64), "allow": [], "shimVersion": 1
        }))
        .unwrap()
    }
    fn sessions() -> PluginUiSessions {
        let sessions = PluginUiSessions::default();
        sessions.insert(&reply()).unwrap();
        sessions
    }
    fn request(method: &str, path: &str) -> Request<Vec<u8>> {
        Request::builder()
            .method(method)
            .uri(format!(
                "se-plugin://localhost/{PLUGIN}/{GENERATION}/{SESSION}/{path}"
            ))
            .body(Vec::new())
            .unwrap()
    }
    async fn asset(_: WebAssetRequest) -> Result<WebAssetReply, PluginError> {
        Ok(WebAssetReply {
            mime: "text/html".into(),
            size: 13,
            data_base64: base64::engine::general_purpose::STANDARD.encode("<html></html>"),
            sha256: "a".repeat(64),
        })
    }

    #[tokio::test]
    async fn plugin_protocol_get_head_headers_and_shim() {
        let sessions = sessions();
        let get = respond_with(request("GET", "ui/dist/index.html"), &sessions, asset).await;
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.headers()["Content-Type"], "text/html");
        assert_eq!(get.headers()["Content-Security-Policy"], WEB_UI_CSP);
        assert_eq!(get.headers()["Cache-Control"], "no-store");
        assert_eq!(get.headers()["X-Content-Type-Options"], "nosniff");
        assert_eq!(get.headers()["Access-Control-Allow-Origin"], "*");
        assert!(!get
            .headers()
            .contains_key("Access-Control-Allow-Credentials"));
        assert!(String::from_utf8_lossy(get.body()).contains(SHIM_PATH));
        let head = respond_with(request("HEAD", "ui/dist/index.html"), &sessions, asset).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(head.body().is_empty());
        assert_eq!(
            head.headers()["Content-Length"],
            get.headers()["Content-Length"]
        );
        let shim = respond_with(request("GET", SHIM_PATH), &sessions, asset).await;
        assert_eq!(shim.headers()["Content-Type"], "text/javascript");
        assert_eq!(shim.body(), SHIM.as_bytes());
    }

    #[tokio::test]
    async fn plugin_protocol_rejects_methods_paths_and_unknown_sessions() {
        let sessions = sessions();
        let rejected = |_: WebAssetRequest| async { panic!("must not reach Core") };
        let post = respond_with(request("POST", "ui/dist/index.html"), &sessions, rejected).await;
        assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(post.headers()["Allow"], "GET, HEAD");
        for path in [
            "../secret",
            "%2e%2e/secret",
            "dist/%2fsecret",
            "dist/%5csecret",
            "dist/%252e%252e/secret",
            "dist/%00",
            "dist//x",
            "dist/%zz",
            "ui/dist/index.html?q=x",
        ] {
            assert_eq!(
                respond_with(request("GET", path), &sessions, rejected)
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        let mut wrong = request("GET", "ui/dist/index.html");
        *wrong.uri_mut() = wrong
            .uri()
            .to_string()
            .replace(GENERATION, "invalid")
            .parse()
            .unwrap();
        assert_eq!(
            respond_with(wrong, &sessions, rejected).await.status(),
            StatusCode::NOT_FOUND
        );
        sessions.remove(SESSION);
        assert_eq!(
            respond_with(request("GET", "ui/dist/index.html"), &sessions, rejected)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn plugin_protocol_core_revocation_also_revokes_shim() {
        for path in ["ui/dist/index.html", SHIM_PATH] {
            let response = respond_with(request("GET", path), &sessions(), |_| async {
                Err(unavailable())
            })
            .await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[test]
    fn plugin_protocol_accepts_only_scheme_origins_and_exact_session_tuple() {
        let original = request("GET", "ui/dist/index.html");
        let windows_uri = original
            .uri()
            .to_string()
            .replace("se-plugin://localhost", "http://se-plugin.localhost");
        let windows = Request::builder()
            .uri(windows_uri)
            .body(Vec::new())
            .unwrap();
        assert!(parse_route(&windows).is_some());
        for origin in [
            "https://se-plugin.localhost",
            "se-plugin://evil",
            "http://localhost",
            "se-plugin://localhost:80",
            "se-plugin://user@localhost",
        ] {
            let uri = original
                .uri()
                .to_string()
                .replace("se-plugin://localhost", origin);
            let request = Request::builder().uri(uri).body(Vec::new()).unwrap();
            assert!(parse_route(&request).is_none(), "{origin}");
        }
        let sessions = sessions();
        let mut route = parse_route(&original).unwrap();
        assert!(sessions.find(&route).is_some());
        route.plugin_id = "org.different.plugin".into();
        assert!(sessions.find(&route).is_none());
    }

    #[test]
    fn plugin_protocol_platform_url() {
        let url = session_url(&reply());
        let origin = if cfg!(target_os = "windows") {
            "http://se-plugin.localhost"
        } else {
            "se-plugin://localhost"
        };
        assert_eq!(
            url,
            format!("{origin}/{PLUGIN}/{GENERATION}/{SESSION}/ui/dist/index.html")
        );
    }

    /// Manifests declare `entry` relative to `ui/` (e.g. `index.html`). The session URL and
    /// the served entry must use the package path, and the page's `../__se__/` shim must resolve.
    #[tokio::test]
    async fn plugin_protocol_manifest_relative_entry() {
        let mut relative = reply();
        relative.entry = "index.html".into();
        let sessions = PluginUiSessions::default();
        sessions.insert(&relative).unwrap();
        let url = session_url(&relative);
        assert!(url.ends_with(&format!("/{SESSION}/ui/index.html")), "{url}");
        let shim_url = url::Url::parse(&url)
            .unwrap()
            .join("../__se__/se-plugin-ui.js")
            .unwrap();
        assert!(shim_url
            .as_str()
            .ends_with(&format!("/{SESSION}/{SHIM_PATH}")));
        let get = respond_with(request("GET", "ui/index.html"), &sessions, asset).await;
        assert_eq!(get.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(get.body()).contains(SHIM_PATH));
        let shim = respond_with(request("GET", SHIM_PATH), &sessions, |req| async move {
            assert_eq!(req.path, "ui/index.html");
            asset(req).await
        })
        .await;
        assert_eq!(shim.status(), StatusCode::OK);
        assert_eq!(shim.body(), SHIM.as_bytes());
    }

    #[test]
    fn plugin_protocol_web_session_url_uses_transport_prefix() {
        let url = web_session_url(&reply());
        assert_eq!(
            url,
            format!("{WEB_UI_PREFIX}{PLUGIN}/{GENERATION}/{SESSION}/ui/dist/index.html")
        );
        let mut relative = reply();
        relative.entry = "index.html".into();
        assert!(web_session_url(&relative).ends_with(&format!("/{SESSION}/ui/index.html")));
        // Percent-encoding matches the desktop scheme URL mechanism.
        let encoded = web_session_url(&reply());
        assert!(!encoded.contains(char::is_whitespace));
    }

    async fn web_response<F, Fut>(
        method: &str,
        uri: &str,
        sessions: &PluginUiSessions,
        fetch: F,
    ) -> Response<Vec<u8>>
    where
        F: FnOnce(WebAssetRequest) -> Fut,
        Fut: std::future::Future<Output = Result<WebAssetReply, PluginError>>,
    {
        let method: Method = method.parse().unwrap();
        let uri: Uri = uri.parse().unwrap();
        let route = parse_web_route(&uri);
        serve(&method, route, sessions, fetch, UiOrigin::Web).await
    }

    #[tokio::test]
    async fn plugin_protocol_web_serves_only_web_sessions_with_sandbox_and_referrer() {
        let sessions = sessions();
        let entry = format!("{WEB_UI_PREFIX}{PLUGIN}/{GENERATION}/{SESSION}/ui/dist/index.html");
        let get = web_response("GET", &entry, &sessions, asset).await;
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.headers()["Referrer-Policy"], WEB_REFERRER_POLICY);
        assert_eq!(get.headers()["Content-Security-Policy"], WEB_UI_CSP);
        assert!(
            get.headers()
                .get_all("Content-Security-Policy")
                .iter()
                .any(|value| value
                    .to_str()
                    .unwrap()
                    .contains("sandbox allow-scripts allow-forms")),
            "web responses must append the sandbox CSP"
        );
        let body = String::from_utf8_lossy(get.body());
        assert!(
            body.contains(&format!(
                "{WEB_UI_PREFIX}{PLUGIN}/{GENERATION}/{SESSION}/{SHIM_PATH}"
            )),
            "web entry must inject the /plugin-ui shim path: {body}"
        );
        let shim = web_response(
            "GET",
            &format!("{WEB_UI_PREFIX}{PLUGIN}/{GENERATION}/{SESSION}/{SHIM_PATH}"),
            &sessions,
            asset,
        )
        .await;
        assert_eq!(shim.headers()["Content-Type"], "text/javascript");
        assert_eq!(shim.body(), SHIM.as_bytes());
        let head = web_response("HEAD", &entry, &sessions, asset).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert!(head.body().is_empty());
        let rejected = |_: WebAssetRequest| async { panic!("must not reach Core") };
        assert_eq!(
            web_response("POST", &entry, &sessions, rejected)
                .await
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn plugin_protocol_web_rejects_query_unknown_and_traversal() {
        let sessions = sessions();
        let base = format!("{WEB_UI_PREFIX}{PLUGIN}/{GENERATION}/{SESSION}");
        let rejected = |_: WebAssetRequest| async { panic!("must not reach Core") };
        for path in [
            format!("{base}/ui/dist/index.html?q=x"),
            format!("{WEB_UI_PREFIX}{PLUGIN}/{GENERATION}/unknown/ui/dist/index.html"),
            format!("{base}/../secret"),
            format!("{base}/dist/%2fsecret"),
            format!("{base}/dist/%5csecret"),
            format!("{base}/dist/%252e%252e/secret"),
            format!("{base}/dist/%00"),
            format!("{base}/dist//x"),
            format!("{WEB_UI_PREFIX}../secret"),
        ] {
            assert_eq!(
                web_response("GET", &path, &sessions, rejected)
                    .await
                    .status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        // A session only present in the desktop registry (never inserted here) is unknown.
        let desktop_only = PluginUiSessions::default();
        assert_eq!(
            web_response(
                "GET",
                &format!("{base}/ui/dist/index.html"),
                &desktop_only,
                rejected
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}

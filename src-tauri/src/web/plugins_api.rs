//! Browser/phone facade over the desktop Plugin Core.
//!
//! Every operation still runs in the desktop Core: this module only adapts the
//! same JSON payloads the Tauri commands take to HTTP, and returns the standard
//! `IpcBody` envelope with HTTP 200 for domain errors. Host-context publish /
//! heartbeat are deliberately absent: they describe the desktop main window.
use super::{auth::RemoteRouteClass, fs_api::IpcBody, router::classified_routes, ws::AppState};
use crate::core::plugin::{METHOD_PROCESS_CONTROL, METHOD_PROCESS_LIST, METHOD_PROCESS_LOGS};
use crate::core::PluginServiceHandle;
use crate::plugin_ui_protocol::{respond_web, web_session_url, PluginUiSessions};
use axum::{
    body::Body,
    extract::{Extension, Json, Request},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use se_plugin_protocol::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// `None` is the standalone `se-server` and a desktop that has not started a
/// Plugin Core; routes degrade to `PLUGIN_CORE_UNAVAILABLE`.
type PluginService = Option<PluginServiceHandle>;

fn log_failure(operation: &'static str, code: &str) {
    // Stable codes only: never payloads, tokens, config values or session ids.
    log::warn!(
        target: "se_manager::web::plugins",
        "operation={} stable_code={}",
        operation,
        code
    );
}

fn unavailable<T>() -> IpcBody<T> {
    log_failure("plugin_call", PLUGIN_CORE_UNAVAILABLE);
    IpcBody::err("Plugin Core is unavailable", PLUGIN_CORE_UNAVAILABLE)
}

fn invalid<T>() -> IpcBody<T> {
    log_failure("plugin_request_invalid", PLUGIN_INVALID_REQUEST);
    IpcBody::err("Invalid plugin request", PLUGIN_INVALID_REQUEST)
}

async fn call<T, P>(service: &PluginService, method: &str, payload: P) -> IpcBody<T>
where
    T: DeserializeOwned,
    P: Serialize,
{
    let Some(service) = service else {
        return unavailable();
    };
    let Some(client) = service.client() else {
        return unavailable();
    };
    let payload = match serde_json::to_value(payload) {
        Ok(value) => value,
        Err(_) => return invalid(),
    };
    match client.request(method, payload).await {
        Ok(value) => match serde_json::from_value(value) {
            Ok(value) => IpcBody::ok(value),
            Err(_) => invalid(),
        },
        Err(error) => {
            log_failure("plugin_domain", &error.code);
            IpcBody::err(error.message, error.code)
        }
    }
}

async fn respond_typed<R, T>(service: &PluginService, method: &str, body: Value) -> Response
where
    R: DeserializeOwned + Serialize,
    T: DeserializeOwned + Serialize,
{
    // No Plugin Core is an availability condition, not a malformed request.
    if service.is_none() {
        return Json(unavailable::<T>()).into_response();
    }
    match serde_json::from_value::<R>(body) {
        Ok(payload) => Json(call::<T, _>(service, method, payload).await).into_response(),
        Err(_) => Json(invalid::<T>()).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Plugins class: view + plugins.use
// ---------------------------------------------------------------------------

async fn plugin_list(Extension(plugins): Extension<PluginService>) -> Response {
    Json(call::<PluginSnapshot, _>(&plugins, METHOD_LIST, ListRequest {}).await).into_response()
}

async fn get_view(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<GetViewRequest, ViewSnapshot>(&plugins, METHOD_GET_VIEW, body).await
}

async fn dispatch_action(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<ActionEvent, ViewSnapshot>(&plugins, METHOD_ACTION, body).await
}

async fn execute_command(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<ExecuteCommandRequest, CommandReply>(&plugins, METHOD_COMMAND, body).await
}

async fn host_ui(Extension(plugins): Extension<PluginService>) -> Response {
    let Some(service) = plugins.as_ref() else {
        return Json(unavailable::<HostUiSnapshot>()).into_response();
    };
    match service.host_ui_snapshot().await {
        Ok(value) => Json(IpcBody::ok(value)).into_response(),
        Err(error) => {
            log_failure("plugin_host_ui", &error.code);
            Json(IpcBody::<HostUiSnapshot>::err(error.message, error.code)).into_response()
        }
    }
}

async fn dismiss_host_ui(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<HostUiDismissRequest, HostUiDismissResult>(
        &plugins,
        METHOD_HOST_UI_DISMISS,
        body,
    )
    .await
}

async fn execute_contribution(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<ExecuteContributionRequest, CommandReply>(
        &plugins,
        METHOD_CONTRIBUTION_EXECUTE,
        body,
    )
    .await
}

/// Same flattened reply as the desktop adapter, plus the browser URL.
#[derive(Serialize)]
struct WebOpenBody {
    #[serde(flatten)]
    session: WebOpenReply,
    url: String,
}

async fn web_open(
    Extension(plugins): Extension<PluginService>,
    Extension(sessions): Extension<Arc<PluginUiSessions>>,
    Json(body): Json<Value>,
) -> Response {
    let Some(service) = plugins.as_ref() else {
        return Json(unavailable::<WebOpenBody>()).into_response();
    };
    let request = match serde_json::from_value::<WebOpenRequest>(body) {
        Ok(request) => request,
        Err(_) => return Json(invalid::<WebOpenBody>()).into_response(),
    };
    let Some(client) = service.client() else {
        return Json(unavailable::<WebOpenBody>()).into_response();
    };
    let payload = match serde_json::to_value(&request) {
        Ok(value) => value,
        Err(_) => return Json(invalid::<WebOpenBody>()).into_response(),
    };
    let reply = match client.request(WEB_OPEN_METHOD, payload).await {
        Ok(value) => match serde_json::from_value::<WebOpenReply>(value) {
            Ok(reply) => reply,
            Err(_) => return Json(invalid::<WebOpenBody>()).into_response(),
        },
        Err(error) => {
            log_failure("plugin_web_open", &error.code);
            return Json(IpcBody::<WebOpenBody>::err(error.message, error.code)).into_response();
        }
    };
    // Track the session in the web-owned set so `/plugin-ui` serves only
    // tuples opened through this route.
    if let Err(error) = sessions.insert(&reply) {
        log_failure("plugin_web_open_session", &error.code);
        return Json(IpcBody::<WebOpenBody>::err(error.message, error.code)).into_response();
    }
    Json(IpcBody::ok(WebOpenBody {
        url: web_session_url(&reply),
        session: reply,
    }))
    .into_response()
}

async fn web_request(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<WebRequest, Value>(&plugins, WEB_REQUEST_METHOD, body).await
}

async fn web_read(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<WebReadRequest, WebReadReply>(&plugins, WEB_READ_METHOD, body).await
}

async fn web_close(
    Extension(plugins): Extension<PluginService>,
    Extension(sessions): Extension<Arc<PluginUiSessions>>,
    Json(body): Json<Value>,
) -> Response {
    if plugins.is_none() {
        return Json(unavailable::<WebCloseReply>()).into_response();
    }
    let request = match serde_json::from_value::<WebCloseRequest>(body) {
        Ok(request) => request,
        Err(_) => return Json(invalid::<WebCloseReply>()).into_response(),
    };
    // Mirror the desktop adapter: forget the web session even if Core errors.
    sessions.remove(&request.session_id);
    Json(call::<WebCloseReply, _>(&plugins, WEB_CLOSE_METHOD, request).await).into_response()
}

// ---------------------------------------------------------------------------
// PluginsManage class: manage only (never grantable to a paired device)
// ---------------------------------------------------------------------------

async fn install(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<InstallRequest, PluginInfo>(&plugins, METHOD_INSTALL, body).await
}

async fn set_enabled(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<SetEnabledRequest, PluginInfo>(&plugins, METHOD_SET_ENABLED, body).await
}

async fn reload(Extension(plugins): Extension<PluginService>, Json(body): Json<Value>) -> Response {
    respond_typed::<ReloadRequest, PluginInfo>(&plugins, METHOD_RELOAD, body).await
}

async fn uninstall(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<UninstallRequest, UninstallResult>(&plugins, METHOD_UNINSTALL, body).await
}

async fn get_config(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<GetConfigRequest, ConfigSnapshot>(&plugins, METHOD_GET_CONFIG, body).await
}

async fn set_config(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    respond_typed::<SetConfigRequest, ConfigSnapshot>(&plugins, METHOD_SET_CONFIG, body).await
}

async fn process_list(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    use crate::core::plugin::parse_process_list_request;
    let Some(request) = parse_value(body, parse_process_list_request) else {
        return Json(invalid::<crate::core::plugin::ProcessListReply>()).into_response();
    };
    Json(
        call::<crate::core::plugin::ProcessListReply, _>(&plugins, METHOD_PROCESS_LIST, request)
            .await,
    )
    .into_response()
}

async fn process_logs(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    use crate::core::plugin::parse_process_logs_request;
    let Some(request) = parse_value(body, parse_process_logs_request) else {
        return Json(invalid::<HostProcessLogsReply>()).into_response();
    };
    Json(call::<HostProcessLogsReply, _>(&plugins, METHOD_PROCESS_LOGS, request).await)
        .into_response()
}

async fn process_control(
    Extension(plugins): Extension<PluginService>,
    Json(body): Json<Value>,
) -> Response {
    use crate::core::plugin::parse_process_control_request;
    let Some(request) = parse_value(body, parse_process_control_request) else {
        return Json(invalid::<HostProcessStatus>()).into_response();
    };
    Json(call::<HostProcessStatus, _>(&plugins, METHOD_PROCESS_CONTROL, request).await)
        .into_response()
}

fn parse_value<T>(body: Value, parser: fn(&[u8]) -> Result<T, PluginError>) -> Option<T> {
    let bytes = serde_json::to_vec(&body).ok()?;
    parser(&bytes).ok()
}

// ---------------------------------------------------------------------------
// PluginUi class: authorized by the exact session tuple, not by a bearer
// ---------------------------------------------------------------------------

async fn plugin_ui(
    Extension(plugins): Extension<PluginService>,
    Extension(sessions): Extension<Arc<PluginUiSessions>>,
    request: Request,
) -> Response {
    let Some(service) = plugins.as_ref() else {
        log_failure("plugin_ui_asset", PLUGIN_CORE_UNAVAILABLE);
        return StatusCode::NOT_FOUND.into_response();
    };
    respond_web(request.method(), request.uri(), &sessions, service)
        .await
        .map(Body::from)
}

// ---------------------------------------------------------------------------
// Route table
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    let plugins = classified_routes(
        Router::<AppState>::new()
            .route("/plugins", get(plugin_list))
            .route("/plugins/view", post(get_view))
            .route("/plugins/action", post(dispatch_action))
            .route("/plugins/command", post(execute_command))
            .route("/plugins/host-ui", get(host_ui))
            .route("/plugins/host-ui/dismiss", post(dismiss_host_ui))
            .route("/plugins/contribution", post(execute_contribution))
            .route("/plugins/web/open", post(web_open))
            .route("/plugins/web/request", post(web_request))
            .route("/plugins/web/read", post(web_read))
            .route("/plugins/web/close", post(web_close)),
        RemoteRouteClass::Plugins,
    );
    let manage = classified_routes(
        Router::<AppState>::new()
            .route("/plugins/install", post(install))
            .route("/plugins/enabled", post(set_enabled))
            .route("/plugins/reload", post(reload))
            .route("/plugins/uninstall", post(uninstall))
            .route("/plugins/config/get", post(get_config))
            .route("/plugins/config/set", post(set_config))
            .route("/plugins/processes", post(process_list))
            .route("/plugins/processes/logs", post(process_logs))
            .route("/plugins/processes/control", post(process_control)),
        RemoteRouteClass::PluginsManage,
    );
    let ui = classified_routes(
        Router::<AppState>::new().route("/plugin-ui/{*rest}", get(plugin_ui).head(plugin_ui)),
        RemoteRouteClass::PluginUi,
    );
    plugins.merge(manage).merge(ui)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::auth::RemoteAccessAuthority;
    use crate::web::devices::{PairRequest, GRANTABLE_SCOPES};
    use axum::{
        body::{to_bytes, Body as AxumBody},
        http::Request as HttpRequest,
        Router,
    };
    use serde_json::json;
    use tower::ServiceExt;

    fn state() -> AppState {
        let pty = crate::web::test_pty_manager();
        let terminal = crate::core::TerminalServiceHandle::in_process(Arc::clone(&pty));
        crate::web::test_state_with_terminal(pty, terminal)
    }

    fn build_app(plugins: PluginService) -> (Router, Arc<RemoteAccessAuthority>) {
        let authority = Arc::new(RemoteAccessAuthority::desktop_memory());
        authority.adopt_or_issue_desktop_credential(None).unwrap();
        let sessions = Arc::new(PluginUiSessions::default());
        let router = routes()
            .layer(Extension(Arc::clone(&authority)))
            .layer(Extension(plugins))
            .layer(Extension(sessions))
            .with_state(state());
        (router, authority)
    }

    fn paired(authority: &RemoteAccessAuthority, scopes: Vec<&str>) -> String {
        let code = authority.issue_pairing_code().unwrap().code;
        let paired = authority
            .pair_device(PairRequest {
                code,
                name: "Phone".into(),
                platform: "ios".into(),
            })
            .unwrap();
        authority
            .set_device_scopes(
                &paired.device_id,
                scopes.into_iter().map(str::to_owned).collect(),
            )
            .unwrap();
        paired.device_token
    }

    fn request(method: &str, uri: &str, bearer: Option<&str>) -> HttpRequest<AxumBody> {
        let mut builder = HttpRequest::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(token) = bearer {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder.body(AxumBody::from("{}")).unwrap()
    }

    async fn json_body(response: Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn view_scope_reads_but_posts_plugins_use_and_manage_refuses_devices() {
        let (app, authority) = build_app(None);
        // Only `view`: GET /plugins reaches the handler (200 + unavailable Core),
        // but a state-changing op is denied with the plugins.use capability.
        let token = paired(&authority, vec!["view"]);
        let response = app
            .clone()
            .oneshot(request("GET", "/plugins", Some(&token)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await["code"],
            json!(PLUGIN_CORE_UNAVAILABLE)
        );
        let denied = app
            .clone()
            .oneshot(request("POST", "/plugins/action", Some(&token)))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            json_body(denied).await,
            json!({"code": "FORBIDDEN", "capability": "plugins.use"})
        );

        // A device granted every grantable scope still cannot reach manage.
        let token = paired(&authority, GRANTABLE_SCOPES.to_vec());
        for (method, uri) in [
            ("POST", "/plugins/install"),
            ("POST", "/plugins/enabled"),
            ("POST", "/plugins/reload"),
            ("POST", "/plugins/uninstall"),
            ("POST", "/plugins/config/get"),
            ("POST", "/plugins/config/set"),
            ("POST", "/plugins/processes"),
            ("POST", "/plugins/processes/logs"),
            ("POST", "/plugins/processes/control"),
        ] {
            let response = app
                .clone()
                .oneshot(request(method, uri, Some(&token)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
            assert_eq!(
                json_body(response).await,
                json!({"code": "FORBIDDEN", "capability": "manage"}),
                "{uri}"
            );
        }
    }

    #[tokio::test]
    async fn anonymous_is_rejected_and_unavailable_core_is_stable() {
        let (app, _) = build_app(None);
        let anonymous = app.oneshot(request("GET", "/plugins", None)).await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        // A paired device with `plugins.use` reaches the handler for a domain
        // call, which answers PLUGIN_CORE_UNAVAILABLE with HTTP 200 rather than
        // an error status.
        let (app, authority) = build_app(None);
        let token = paired(&authority, vec!["view", "plugins.use"]);
        let response = app
            .oneshot(request("POST", "/plugins/web/open", Some(&token)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            json_body(response).await["code"],
            json!(PLUGIN_CORE_UNAVAILABLE)
        );
    }

    #[tokio::test]
    async fn plugin_ui_refuses_sessions_never_opened_through_the_web_route() {
        let (app, authority) = build_app(None);
        let token = paired(&authority, vec!["view"]);
        let response = app
            .oneshot(request(
                "GET",
                "/plugin-ui/org.example.plugin/01234567-89ab-4def-8123-456789abcdef/abcdef01-2345-4678-89ab-cdef01234567/ui/index.html",
                Some(&token),
            ))
            .await
            .unwrap();
        // PluginUi requires no bearer; with no Core/session it is a plain 404.
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

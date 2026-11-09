//! Local-operator device administration; the shared service is used by Tauri and HTTP.
use super::{
    auth::{self, RemoteAccessAuthority, RemoteAuthError, RemoteRouteClass},
    devices::{
        DeviceAuditEntry, DeviceList, PairRequest, PairResponse, PairingCode, GRANTABLE_SCOPES,
    },
    upgraded_connections::UpgradedConnectionRegistry,
    AppState,
};
use axum::{
    extract::Path,
    middleware,
    response::Response,
    routing::{get, patch, post},
    Extension, Json, Router,
};
use serde::Deserialize;
use std::sync::Arc;

struct DeviceApiError(RemoteAuthError);
impl axum::response::IntoResponse for DeviceApiError {
    fn into_response(self) -> Response {
        auth::auth_error_response(self.0)
    }
}
type ApiResult<T> = Result<Json<T>, DeviceApiError>;
fn respond<T>(result: Result<T, RemoteAuthError>) -> ApiResult<T> {
    result.map(Json).map_err(DeviceApiError)
}

pub fn routes() -> Router<AppState> {
    // The middleware classifier recognizes /pair as public. The 256-bit code
    // authorizes precisely one exchange, with no host credential in the URL.
    management_routes()
        .route("/pair", post(pair))
        .route("/info", get(host_info))
}

/// Management routes carry their own capability gate because production
/// merges them without `classified_routes`; nothing else authorizes them.
fn management_routes<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/remote/devices", get(list))
        .route("/remote/devices/audit", get(audit))
        .route("/remote/devices/disconnect-all", post(disconnect_all))
        .route("/remote/devices/{id}", patch(update))
        .route("/remote/devices/{id}/grant-all", post(grant_all))
        .route("/remote/devices/{id}/disconnect", post(disconnect))
        .route("/remote/devices/{id}/revoke", post(revoke))
        .route("/remote/pairing-code", post(pairing_code))
        .route_layer(middleware::from_fn(auth::capability_middleware))
        // Outer layer runs first: classify before the capability check.
        .route_layer(middleware::from_fn(
            move |mut request: axum::extract::Request, next: axum::middleware::Next| async move {
                request
                    .extensions_mut()
                    .insert(RemoteRouteClass::RemoteAccess);
                next.run(request).await
            },
        ))
}

pub fn list_service(authority: &RemoteAccessAuthority) -> Result<DeviceList, RemoteAuthError> {
    let mut list = authority.list_devices()?;
    let registry = UpgradedConnectionRegistry::global();
    for device in &mut list.devices {
        device.connection_count = registry.device_connection_count(&device.id) as usize;
    }
    Ok(list)
}
pub fn disconnect_service(
    authority: &RemoteAccessAuthority,
    id: &str,
) -> Result<(), RemoteAuthError> {
    authority.record_device_disconnect(id)?;
    UpgradedConnectionRegistry::global().cancel_device(id);
    Ok(())
}
pub fn revoke_service(authority: &RemoteAccessAuthority, id: &str) -> Result<(), RemoteAuthError> {
    // Do not cancel the registry ticket: WS observes the durable revoke and
    // sends policy close 1008, rather than an ordinary disconnect close.
    authority.revoke_device(id)
}
pub fn disconnect_all_service(authority: &RemoteAccessAuthority) -> Result<(), RemoteAuthError> {
    authority.revoke_all_devices()
}

async fn host_info(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
) -> ApiResult<serde_json::Value> {
    respond(
        authority
            .host_id()
            .map(|host_id| serde_json::json!({"hostId": host_id})),
    )
}

async fn pair(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    peer: Option<Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    Json(request): Json<PairRequest>,
) -> ApiResult<PairResponse> {
    let peer = peer
        .map(|Extension(axum::extract::ConnectInfo(address))| address.ip())
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED));
    respond(authority.pair_device_for_peer(request, peer))
}
async fn list(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
) -> ApiResult<DeviceList> {
    respond(list_service(&authority))
}
async fn audit(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
) -> Json<Vec<DeviceAuditEntry>> {
    Json(authority.device_audit())
}
async fn pairing_code(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
) -> ApiResult<PairingCode> {
    respond(authority.issue_pairing_code())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateDevice {
    name: Option<String>,
    scopes: Option<Vec<String>>,
}
async fn update(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Path(id): Path<String>,
    Json(update): Json<UpdateDevice>,
) -> ApiResult<()> {
    // Validate the entire patch before making either change.
    if update.scopes.as_ref().is_some_and(|scopes| {
        scopes
            .iter()
            .any(|scope| !GRANTABLE_SCOPES.contains(&scope.as_str()))
    }) {
        return respond(Err(RemoteAuthError::ScopeDenied("manage")));
    }
    if update
        .name
        .as_ref()
        .is_some_and(|name| name.trim().is_empty() || name.len() > 128)
    {
        return respond(Err(RemoteAuthError::InvalidDevice));
    }
    if let Some(name) = update.name {
        authority
            .rename_device(&id, &name)
            .map_err(DeviceApiError)?;
    }
    if let Some(scopes) = update.scopes {
        authority
            .set_device_scopes(&id, scopes)
            .map_err(DeviceApiError)?;
    }
    respond(Ok(()))
}
async fn grant_all(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Path(id): Path<String>,
) -> ApiResult<()> {
    respond(authority.set_device_scopes(
        &id,
        GRANTABLE_SCOPES.into_iter().map(str::to_owned).collect(),
    ))
}
async fn disconnect(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Path(id): Path<String>,
) -> ApiResult<()> {
    respond(disconnect_service(&authority, &id))
}
async fn revoke(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
    Path(id): Path<String>,
) -> ApiResult<()> {
    respond(revoke_service(&authority, &id))
}
async fn disconnect_all(
    Extension(authority): Extension<Arc<RemoteAccessAuthority>>,
) -> ApiResult<()> {
    respond(disconnect_all_service(&authority))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    fn router(authority: Arc<RemoteAccessAuthority>) -> Router {
        Router::new()
            .route("/pair", post(pair))
            .route("/info", get(host_info))
            .route("/remote/devices", get(list))
            .layer(middleware::from_fn(auth::capability_middleware))
            .layer(Extension(authority))
    }

    /// Regression: production `routes()` must gate management on its own.
    #[tokio::test]
    async fn production_management_routes_reject_anonymous_and_paired_callers() {
        let authority = Arc::new(RemoteAccessAuthority::desktop_memory());
        let (host, _) = authority.adopt_or_issue_desktop_credential(None).unwrap();
        let app = || {
            management_routes::<()>()
                .route("/pair", post(pair))
                .layer(Extension(authority.clone()))
        };
        let cases = [
            ("POST", "/remote/pairing-code"),
            ("GET", "/remote/devices"),
            ("GET", "/remote/devices/audit"),
            ("POST", "/remote/devices/disconnect-all"),
            ("PATCH", "/remote/devices/dev-1"),
            ("POST", "/remote/devices/dev-1/grant-all"),
            ("POST", "/remote/devices/dev-1/disconnect"),
            ("POST", "/remote/devices/dev-1/revoke"),
        ];
        let request = |method: &str, uri: &str, bearer: Option<&str>| {
            let mut builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json");
            if let Some(token) = bearer {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            builder.body(Body::from("{}")).unwrap()
        };
        for (method, uri) in cases {
            // A fresh authority per probe keeps the failed-auth limiter (429)
            // from masking the 401 this test asserts.
            let anonymous = Arc::new(RemoteAccessAuthority::desktop_memory());
            anonymous.adopt_or_issue_desktop_credential(None).unwrap();
            let response = management_routes::<()>()
                .layer(Extension(anonymous))
                .oneshot(request(method, uri, None))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} must reject anonymous callers"
            );
        }
        let code = authority.issue_pairing_code().unwrap().code;
        let paired = app()
            .oneshot(
                Request::post("/pair")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"code":code,"name":"iPhone","platform":"ios"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let paired: serde_json::Value =
            serde_json::from_slice(&to_bytes(paired.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        let device_token = paired["deviceToken"].as_str().unwrap().to_owned();
        for (method, uri) in cases {
            let response = app()
                .oneshot(request(method, uri, Some(&device_token)))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{method} {uri} must deny paired devices"
            );
        }
        let minted = app()
            .oneshot(request("POST", "/remote/pairing-code", Some(host.bearer())))
            .await
            .unwrap();
        assert_eq!(minted.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn http_pair_one_time_exchange_management_boundary_and_revoke() {
        let authority = Arc::new(RemoteAccessAuthority::desktop_memory());
        let (host, _) = authority.adopt_or_issue_desktop_credential(None).unwrap();
        let info = router(authority.clone())
            .oneshot(Request::get("/info").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(info.status(), StatusCode::OK);
        let identity: serde_json::Value =
            serde_json::from_slice(&to_bytes(info.into_body(), usize::MAX).await.unwrap()).unwrap();
        let code = authority.issue_pairing_code().unwrap().code;
        let request = || {
            Request::post("/pair")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"code":code,"name":"iPhone","platform":"ios"}).to_string(),
                ))
                .unwrap()
        };
        let response = router(authority.clone()).oneshot(request()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(value["hostId"], identity["hostId"]);
        let token = value["deviceToken"].as_str().unwrap();
        let id = value["deviceId"].as_str().unwrap();
        assert_eq!(value["scopes"].as_array().unwrap().len(), 8);
        let replay = router(authority.clone()).oneshot(request()).await.unwrap();
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);
        let replay: serde_json::Value =
            serde_json::from_slice(&to_bytes(replay.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(replay["code"], "INVALID_PAIRING_CODE");
        let read = |token: &str| {
            Request::get("/remote/devices")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        let denied = router(authority.clone())
            .oneshot(read(token))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let denied: serde_json::Value =
            serde_json::from_slice(&to_bytes(denied.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(
            denied,
            serde_json::json!({"code":"FORBIDDEN","capability":"manage"})
        );
        let host_response = router(authority.clone())
            .oneshot(read(host.bearer()))
            .await
            .unwrap();
        assert_eq!(host_response.status(), StatusCode::OK);
        let list: serde_json::Value = serde_json::from_slice(
            &to_bytes(host_response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(list["devices"].as_array().unwrap().len(), 2);
        assert!(list.to_string().find(token).is_none());
        authority.revoke_device(id).unwrap();
        let revoked = router(authority.clone())
            .oneshot(read(token))
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
        let revoked: serde_json::Value =
            serde_json::from_slice(&to_bytes(revoked.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(revoked["code"], "DEVICE_REVOKED");
    }
}

// after the manage scope middleware (which a paired device can never satisfy).
#[tauri::command]
pub fn remote_devices_list(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
) -> Result<DeviceList, String> {
    list_service(&authority).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_pairing_code(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
) -> Result<PairingCode, String> {
    authority
        .issue_pairing_code()
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_device_rename(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
    id: String,
    name: String,
) -> Result<(), String> {
    authority
        .rename_device(&id, &name)
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_device_set_permissions(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
    id: String,
    scopes: Vec<String>,
) -> Result<(), String> {
    authority
        .set_device_scopes(&id, scopes)
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_device_grant_all(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
    id: String,
) -> Result<(), String> {
    authority
        .set_device_scopes(
            &id,
            GRANTABLE_SCOPES.into_iter().map(str::to_owned).collect(),
        )
        .map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_device_disconnect(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
    id: String,
) -> Result<(), String> {
    disconnect_service(&authority, &id).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_device_revoke(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
    id: String,
) -> Result<(), String> {
    revoke_service(&authority, &id).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_devices_disconnect_all(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
) -> Result<(), String> {
    disconnect_all_service(&authority).map_err(|error| error.to_string())
}
#[tauri::command]
pub fn remote_devices_audit(
    authority: tauri::State<'_, Arc<RemoteAccessAuthority>>,
) -> Vec<DeviceAuditEntry> {
    authority.device_audit()
}

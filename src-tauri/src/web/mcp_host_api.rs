//! Authenticated local-operator API for the process-owned host capability service.
//! Provenance is attached by the ingress listener, never inferred from headers.
use std::sync::Arc;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::{
    auth::{IngressProvenance, RemotePrincipal},
    fs_api::IpcBody,
};
use crate::mcp_core::{
    host_capabilities::HostCapabilityBackend, host_policy::HostAccessPolicy,
    host_service::HostCapabilityService,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PutAccess {
    expected_revision: u64,
    policy: HostAccessPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCall {
    provider: String,
    tool: String,
    arguments: Map<String, Value>,
}

fn require_local(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<IngressProvenance>>,
) -> Result<(), Response> {
    if principal.is_none() {
        return Err(StatusCode::UNAUTHORIZED.into_response());
    }
    if !matches!(ingress, Some(Extension(IngressProvenance::LocalOperator))) {
        return Err(StatusCode::FORBIDDEN.into_response());
    }
    Ok(())
}

fn unavailable<T: serde::Serialize>() -> Response {
    Json(IpcBody::<T>::err(
        "Host capability service is unavailable",
        "UNAVAILABLE",
    ))
    .into_response()
}

pub async fn get_access(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<IngressProvenance>>,
    service: Option<Extension<Arc<HostCapabilityService>>>,
) -> Response {
    if let Err(response) = require_local(principal, ingress) {
        return response;
    }
    let Some(Extension(service)) = service else {
        return unavailable::<Value>();
    };
    match service.access_view() {
        Ok(value) => Json(IpcBody::ok(value)).into_response(),
        Err(error) => Json(IpcBody::<Value>::err(error.message, error.code)).into_response(),
    }
}

pub async fn put_access(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<IngressProvenance>>,
    service: Option<Extension<Arc<HostCapabilityService>>>,
    Json(body): Json<PutAccess>,
) -> Response {
    if let Err(response) = require_local(principal, ingress) {
        return response;
    }
    let Some(Extension(service)) = service else {
        return unavailable::<HostAccessPolicy>();
    };
    match service.put_access(body.expected_revision, body.policy) {
        Ok(policy) => Json(IpcBody::ok(policy)).into_response(),
        Err(error) => {
            Json(IpcBody::<HostAccessPolicy>::err(error.message, error.code)).into_response()
        }
    }
}

pub async fn call(
    principal: Option<Extension<RemotePrincipal>>,
    ingress: Option<Extension<IngressProvenance>>,
    service: Option<Extension<Arc<HostCapabilityService>>>,
    Json(body): Json<HostCall>,
) -> Response {
    if let Err(response) = require_local(principal, ingress) {
        return response;
    }
    let Some(Extension(service)) = service else {
        return unavailable::<Value>();
    };
    // Dropping an HTTP request also cancels any in-flight automation work.
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    match service
        .call(&body.provider, &body.tool, body.arguments, cancel)
        .await
    {
        Ok(value) => Json(IpcBody::ok(value)).into_response(),
        Err(error) => Json(IpcBody::<Value>::err(error.message, error.code)).into_response(),
    }
}

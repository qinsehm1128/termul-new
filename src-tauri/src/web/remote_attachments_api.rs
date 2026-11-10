//! `POST /remote/attachments/stage` — stage a phone screenshot for a terminal.
//!
//! The request body is a raw PNG or JPEG (`Content-Type: image/png` /
//! `image/jpeg`) because a paired phone sends the captured region directly. The
//! response is the standard [`IpcBody`] envelope carrying the absolute path,
//! byte size and dimensions; the client types that path into the terminal over
//! the existing terminal WS `write` frame.
//!
//! Chat (ACP) sessions do not use this route: they accept inline image blocks.

use axum::{
    body::Bytes,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use super::fs_api::IpcBody;
use crate::remote_attachments::{
    AttachmentError, AttachmentStager, ImageKind, MAX_ATTACHMENT_BYTES,
};

/// Success payload (`data` field of the [`IpcBody`] envelope).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedAttachmentDto {
    /// Absolute path to type into the terminal.
    pub path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
}

/// `POST /remote/attachments/stage` with a raw image body.
///
/// The route is registered with `DefaultBodyLimit::max(MAX_ATTACHMENT_BYTES)`;
/// the service re-checks the cap so direct callers cannot bypass it.
pub async fn stage(headers: HeaderMap, body: Bytes) -> Response {
    let declared = match declared_kind(&headers) {
        Ok(kind) => kind,
        Err(response) => return response,
    };

    let bytes = body.to_vec();
    let result = tokio::task::spawn_blocking(move || {
        AttachmentStager::default().stage(&bytes, Some(declared))
    })
    .await;

    match result {
        Ok(Ok(staged)) => {
            log::info!(
                target: "se_manager::remote_attachments",
                "operation=stage_attachment stable_code=OK bytes={} width={} height={}",
                staged.bytes,
                staged.width,
                staged.height
            );
            let dto = StagedAttachmentDto {
                path: staged.path.to_string_lossy().into_owned(),
                bytes: staged.bytes,
                width: staged.width,
                height: staged.height,
                mime: staged.mime,
            };
            (StatusCode::OK, Json(IpcBody::ok(dto))).into_response()
        }
        Ok(Err(error)) => error_response(error),
        Err(error) => {
            log::warn!(
                target: "se_manager::remote_attachments",
                "operation=stage_attachment stable_code=STAGE_FAILED error={error}"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(IpcBody::<()>::err(
                    "attachment staging task failed",
                    "STAGE_FAILED",
                )),
            )
                .into_response()
        }
    }
}

fn declared_kind(headers: &HeaderMap) -> Result<ImageKind, Response> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(unsupported_media_type("missing Content-Type"));
    };
    let Ok(value) = value.to_str() else {
        return Err(unsupported_media_type("invalid Content-Type"));
    };
    ImageKind::from_content_type(value).ok_or_else(|| {
        unsupported_media_type("Content-Type must be image/png or image/jpeg")
    })
}

fn unsupported_media_type(message: &str) -> Response {
    (
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Json(IpcBody::<()>::err(
            message.to_owned(),
            AttachmentError::UnsupportedMediaType.code(),
        )),
    )
        .into_response()
}

fn error_response(error: AttachmentError) -> Response {
    let status = match error {
        AttachmentError::Empty => StatusCode::BAD_REQUEST,
        AttachmentError::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        AttachmentError::UnsupportedMediaType | AttachmentError::ContentTypeMismatch => {
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        }
        AttachmentError::InvalidImage(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        AttachmentError::DimensionsTooLarge { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        AttachmentError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    log::warn!(
        target: "se_manager::remote_attachments",
        "operation=stage_attachment stable_code={} http_status={}",
        error.code(),
        status.as_u16()
    );
    (
        status,
        Json(IpcBody::<()>::err(error.to_string(), error.code())),
    )
        .into_response()
}

/// Shared route body limit. Exposed so `router.rs` and tests agree on the cap.
pub const BODY_LIMIT: usize = MAX_ATTACHMENT_BYTES;

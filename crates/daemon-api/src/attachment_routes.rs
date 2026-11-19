//! Chat attachments: `POST /v1/chats/{id}/attachments` and `GET .../attachments/{att}`.

use agent_core::attachments::{self, Attachment, AttachmentError, AttachmentKind};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use protocol::ChatId;
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::AppState;
use crate::dto::AttachmentDto;
use crate::error::{ApiError, ErrorBody};

/// Request body cap of the upload route (the file cap plus headroom).
pub(crate) const MAX_BODY: usize = attachments::MAX_UPLOAD_BYTES + 64 * 1024;

impl From<&Attachment> for AttachmentDto {
    fn from(a: &Attachment) -> Self {
        Self {
            id: a.id,
            name: a.name.clone(),
            mime_type: a.mime_type.clone(),
            kind: match a.kind {
                AttachmentKind::Image => "image",
                AttachmentKind::Document => "document",
                AttachmentKind::Text => "text",
            }
            .into(),
            size: a.size,
            truncated: a.truncated,
        }
    }
}

impl From<AttachmentError> for ApiError {
    fn from(e: AttachmentError) -> Self {
        match e {
            AttachmentError::Unsupported(m) => Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_attachment",
                m,
            ),
            AttachmentError::TooLarge(m) => {
                Self::new(StatusCode::PAYLOAD_TOO_LARGE, "attachment_too_large", m)
            }
            AttachmentError::NotFound => Self::not_found("attachment"),
            AttachmentError::Io(e) => Self::internal(e),
        }
    }
}

/// Query of the upload route.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadQuery {
    /// File name to show (a path is reduced to its last component).
    pub name: Option<String>,
}

/// Upload one file for a chat message. The body is the raw file; its type is sniffed, not
/// taken from the name. Use the returned id in `attachment_ids` of the message.
#[utoipa::path(post, path = "/v1/chats/{id}/attachments", tag = "chats", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Chat id"), UploadQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 201, body = AttachmentDto), (status = 404, body = ErrorBody),
              (status = 413, body = ErrorBody), (status = 415, body = ErrorBody)))]
pub(crate) async fn upload(
    State(s): State<AppState>,
    Path(cid): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> Result<(StatusCode, Json<AttachmentDto>), ApiError> {
    let chat = s.chat_of_user(ChatId(cid))?;
    let data_dir = s.storage().data_dir().to_path_buf();
    let name = q.name.unwrap_or_default();
    let agent = s.agent().clone();
    let att = tokio::task::spawn_blocking(move || {
        attachments::store(&data_dir, chat.id, &name, &body, &|t| {
            agent.redact_host_text(t)
        })
    })
    .await
    .map_err(|_| ApiError::internal("attachment worker failed"))??;
    Ok((StatusCode::CREATED, Json(AttachmentDto::from(&att))))
}

/// The stored original of an attachment (images inline, everything else as a download).
#[utoipa::path(get, path = "/v1/chats/{id}/attachments/{att}", tag = "chats", security(("bearer" = [])),
    params(("id" = Uuid, Path, description = "Chat id"), ("att" = Uuid, Path, description = "Attachment id")),
    responses((status = 200, content_type = "application/octet-stream", body = Vec<u8>),
              (status = 404, body = ErrorBody)))]
pub(crate) async fn download(
    State(s): State<AppState>,
    Path((cid, att)): Path<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let chat = s.chat_of_user(ChatId(cid))?;
    let dir = s.storage().data_dir().to_path_buf();
    let (meta, bytes) = tokio::task::spawn_blocking(move || {
        let meta = attachments::meta(&dir, chat.id, att)?;
        attachments::original(&dir, chat.id, att).map(|b| (meta, b))
    })
    .await
    .map_err(|_| ApiError::internal("attachment worker failed"))??;
    let inline = meta.kind == AttachmentKind::Image;
    // The original name is user data: never put it in a header.
    Response::builder()
        .status(StatusCode::OK)
        .header(
            "content-type",
            if inline {
                meta.mime_type.as_str()
            } else {
                "application/octet-stream"
            },
        )
        .header(
            "content-disposition",
            if inline { "inline" } else { "attachment" },
        )
        .header("x-content-type-options", "nosniff")
        .header("content-security-policy", "default-src 'none'; sandbox")
        .header("cache-control", "private, max-age=3600")
        .body(axum::body::Body::from(bytes))
        .map_err(ApiError::internal)
}

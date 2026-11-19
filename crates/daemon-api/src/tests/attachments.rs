//! Chat attachments: upload, send with `attachment_ids`, listing and refusals.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{TOKEN, call, make_chat, setup};

async fn upload(t: &super::T, chat: &str, name: &str, bytes: Vec<u8>) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(format!("/v1/chats/{chat}/attachments?name={name}"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("host", "127.0.0.1:47821")
        .header("content-type", "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap();
    let resp = t.app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn an_attached_file_is_listed_with_its_message_and_unknown_types_are_refused() {
    let t = setup();
    let (_, chat) = make_chat(&t).await;

    let (status, att) = upload(&t, &chat, "notes.txt", b"hello from a file".to_vec()).await;
    assert_eq!(status, StatusCode::CREATED, "{att}");
    assert_eq!(att["kind"], "text");

    let r = call(
        &t,
        Method::POST,
        &format!("/v1/chats/{chat}/messages"),
        Some(json!({"text": "", "attachment_ids": [att["id"]]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{:?}", r.json());

    let r = call(&t, Method::GET, &format!("/v1/chats/{chat}/messages"), None).await;
    let user = r.json().as_array().unwrap()[0].clone();
    assert_eq!(user["content"], "");
    assert_eq!(user["attachments"][0]["name"], "notes.txt");

    let (status, err) = upload(&t, &chat, "tool.exe", b"MZ\x90\x00\x03\x00".to_vec()).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{err}");

    // Another chat's attachment id is refused.
    let (_, other) = make_chat(&t).await;
    let r = call(
        &t,
        Method::POST,
        &format!("/v1/chats/{other}/messages"),
        Some(json!({"text": "x", "attachment_ids": [att["id"]]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

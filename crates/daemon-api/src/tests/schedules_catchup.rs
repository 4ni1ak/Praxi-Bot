#![allow(clippy::too_many_lines)]
//! Schedules: the catch-up window field and the time zone surviving an edit.

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{call, make_chat};

fn body(bot: &str, spec: Value) -> Value {
    json!({
        "bot_id": bot,
        "name": "morning",
        "spec": spec,
        "payload": {"kind": "agent_prompt", "text": "hi"},
    })
}

#[tokio::test]
async fn catchup_window_is_stored_kept_on_edit_cleared_with_zero_and_bounded() {
    let t = super::setup();
    let (bot, _) = make_chat(&t).await;
    let mut b = body(&bot, json!({"every": {"seconds": 3600}}));
    b["misfire_policy"] = json!("run_once");
    b["catchup_window_secs"] = json!(21_600);
    let r = call(&t, Method::POST, "/v1/schedules", Some(b.clone())).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let id = r.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(r.json()["catchup_window_secs"], 21_600);

    // Absent on edit keeps it.
    let mut keep = b.clone();
    keep.as_object_mut().unwrap().remove("catchup_window_secs");
    let r = call(&t, Method::PUT, &format!("/v1/schedules/{id}"), Some(keep)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["catchup_window_secs"], 21_600);

    // 0 removes it.
    let mut clear = b.clone();
    clear["catchup_window_secs"] = json!(0);
    let r = call(&t, Method::PUT, &format!("/v1/schedules/{id}"), Some(clear)).await;
    assert!(r.json()["catchup_window_secs"].is_null(), "{:?}", r.json());

    // More than 30 days is refused with the schedule validation error.
    let mut too_long = b;
    too_long["catchup_window_secs"] = json!(31 * 24 * 3600);
    let r = call(&t, Method::POST, "/v1/schedules", Some(too_long)).await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(r.json()["error"]["code"], "bad_schedule");
}

#[tokio::test]
async fn the_time_zone_survives_an_edit_through_put() {
    let t = super::setup();
    let (bot, _) = make_chat(&t).await;
    let spec = json!({"cron": "0 9 * * * Australia/Lord_Howe"});
    let r = call(
        &t,
        Method::POST,
        "/v1/schedules",
        Some(body(&bot, spec.clone())),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.json());
    let created = r.json();
    let id = created["id"].as_str().unwrap().to_owned();
    // The edit sends the stored spec back unchanged (what the UI's "keep" mode does).
    let mut edit = body(&bot, created["spec"].clone());
    edit["name"] = json!("renamed");
    let r = call(&t, Method::PUT, &format!("/v1/schedules/{id}"), Some(edit)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    let edited = r.json();
    assert_eq!(edited["spec"], created["spec"]);
    assert!(
        edited["spec"]["cron"]
            .as_str()
            .unwrap()
            .contains("Australia/Lord_Howe")
    );
    assert_eq!(edited["next_run_at_ms"], created["next_run_at_ms"]);
}

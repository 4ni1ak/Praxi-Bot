#![allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
//! `PATCH /v1/bots/{id}`: field-level edits with optimistic concurrency.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{T, call, setup};
use crate::{PeerIdentity, PeerRole, peer_router};

const FULL: &str = r#"
[bot]
id = "tester"
name = "Tester"
persona = "Careful."

[models]
executor = "fake/echo"
fallback = ["custom/odd:model"]

[runtime]
computer = "off"
"#;

async fn make_bot(t: &T) -> (String, Value) {
    let r = call(
        t,
        Method::POST,
        "/v1/bots",
        Some(json!({ "profile_toml": FULL })),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let v = r.json();
    (format!("/v1/bots/{}", v["id"].as_str().unwrap()), v)
}

fn set(path: &str, value: Value) -> Value {
    json!({"op": "set", "path": path, "value": value})
}

fn unset(path: &str) -> Value {
    json!({"op": "unset", "path": path})
}

#[tokio::test]
async fn patch_changes_only_the_named_fields_and_bumps_the_version() {
    let t = setup();
    let (path, bot) = make_bot(&t).await;
    assert_eq!(bot["version"], 1);

    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({
            "expected_version": 1,
            "name": "Renamed",
            "ops": [set("/bot/persona", json!("Bold.")), set("/models/planner", json!("fake/plan"))]
        })),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    assert_eq!(v["version"], 2);
    assert_eq!(v["name"], "Renamed");
    assert_eq!(v["config"]["bot"]["persona"], "Bold.");
    assert_eq!(v["config"]["models"]["planner"], "fake/plan");
    // Everything not named is exactly as stored: the other model keys, the runtime target.
    assert_eq!(v["config"]["models"]["executor"], "fake/echo");
    assert_eq!(
        v["config"]["models"]["fallback"],
        json!(["custom/odd:model"])
    );
    assert_eq!(v["config"]["runtime"]["computer"], "off");
    assert_eq!(v["runtime"]["computer"], "off");

    let got = call(&t, Method::GET, &path, None).await.json();
    assert_eq!(got["version"], 2);
    assert_eq!(got["config"], v["config"]);

    // unset removes one key and leaves its siblings.
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(
            json!({"expected_version": 2, "ops": [unset("/models/planner"), unset("/nope/deep")]}),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["version"], 3);
    assert!(v["config"]["models"].get("planner").is_none());
    assert_eq!(v["config"]["models"]["executor"], "fake/echo");
}

#[tokio::test]
async fn a_stale_version_is_refused_with_the_current_bot_and_writes_nothing() {
    let t = setup();
    let (path, _) = make_bot(&t).await;
    // Another client wins first.
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": 1, "ops": [set("/bot/persona", json!("Theirs."))]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": 1, "name": "Mine", "ops": [set("/bot/color", json!("#fff"))]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let body = r.json();
    assert_eq!(body["error"]["code"], "version_conflict");
    assert_eq!(body["current"]["version"], 2);
    assert_eq!(body["current"]["config"]["bot"]["persona"], "Theirs.");
    let got = call(&t, Method::GET, &path, None).await.json();
    assert_eq!(got["name"], "Tester");
    assert!(got["config"]["bot"].get("color").is_none());

    // A full PUT also bumps the version, so a patch built on the old one is stale.
    let r = call(
        &t,
        Method::PUT,
        &path,
        Some(json!({"config": {"bot": {"id": "tester", "name": "T"}, "models": {"executor": "fake/echo"}}})),
    )
    .await;
    assert_eq!(r.json()["version"], 3);
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": 2, "ops": []})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn each_permission_category_is_patched_on_its_own() {
    let t = setup();
    let (path, bot) = make_bot(&t).await;
    // Start: custom fs=ask, shell=ask.
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": bot["version"], "ops": [
            set("/permissions/categories/fs/level", json!({"level": "ask"})),
            set("/permissions/categories/shell/level", json!({"level": "ask"})),
        ]})),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v0 = r.json()["version"].as_i64().unwrap();

    // Client B sets shell=deny; client A (which loaded v0) edited only fs.
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": v0, "ops": [
            set("/permissions/categories/shell/level", json!({"level": "deny"})),
        ]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": v0, "ops": [
            set("/permissions/categories/fs/level", json!({"level": "deny"})),
        ]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // "Overwrite": A re-sends only fs against the version the conflict returned.
    let current = r.json()["current"]["version"].as_i64().unwrap();
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": current, "ops": [
            set("/permissions/categories/fs/level", json!({"level": "deny"})),
        ]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let cats = &r.json()["config"]["permissions"]["categories"];
    assert_eq!(cats["fs"]["level"]["level"], "deny");
    assert_eq!(cats["shell"]["level"]["level"], "deny");
}

#[tokio::test]
async fn bad_patches_are_refused_without_a_write() {
    let t = setup();
    let (path, bot) = make_bot(&t).await;
    let v = bot["version"].clone();
    for (ops, why) in [
        (vec![set("models/executor", json!("x"))], "no leading slash"),
        (vec![set("/models//executor", json!("x"))], "empty segment"),
        (
            vec![set("/models/executor/deep", json!(1))],
            "through a scalar",
        ),
        (
            vec![set("/models/fallback/0", json!("x"))],
            "through an array",
        ),
        (
            vec![set("/runtime/computer", json!("moon"))],
            "invalid profile",
        ),
    ] {
        let r = call(
            &t,
            Method::PATCH,
            &path,
            Some(json!({"expected_version": v, "ops": ops})),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{why}");
    }
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": v, "name": "  "})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let got = call(&t, Method::GET, &path, None).await.json();
    assert_eq!(got["version"], v);
    assert_eq!(
        call(
            &t,
            Method::PATCH,
            "/v1/bots/00000000-0000-7000-8000-000000000000",
            Some(json!({"expected_version": 1}))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn runtime_target_is_patched_separately_from_everything_else() {
    let t = setup();
    let (path, bot) = make_bot(&t).await;
    let r = call(
        &t,
        Method::PATCH,
        &path,
        Some(json!({"expected_version": bot["version"], "ops": [set("/runtime/computer", json!("host"))]})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["runtime"]["computer"], "host");
    assert_eq!(v["config"]["bot"]["persona"], "Careful.");
}

async fn peer(t: &T, role: PeerRole, method: Method, path: &str, body: Value) -> StatusCode {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(PeerIdentity {
        key: format!("peer-{role:?}"),
        role,
    });
    peer_router(t.state.clone())
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn peer_roles_treat_patch_like_put() {
    let t = setup();
    let (path, bot) = make_bot(&t).await;
    let body =
        json!({"expected_version": bot["version"], "ops": [set("/bot/persona", json!("P"))]});
    // Roles other than desktop and phone are refused every non-GET bot call, PUT included.
    for role in [PeerRole::Node, PeerRole::Host] {
        assert_eq!(
            peer(&t, role, Method::PATCH, &path, body.clone()).await,
            StatusCode::FORBIDDEN,
            "{role:?}"
        );
        assert_eq!(
            peer(&t, role, Method::PUT, &path, json!({"config": {}})).await,
            StatusCode::FORBIDDEN
        );
    }
    for role in [PeerRole::Desktop, PeerRole::Phone] {
        let b = call(&t, Method::GET, &path, None).await.json();
        let body =
            json!({"expected_version": b["version"], "ops": [set("/bot/persona", json!("P"))]});
        assert_eq!(
            peer(&t, role, Method::PATCH, &path, body).await,
            StatusCode::OK,
            "{role:?}"
        );
    }
}

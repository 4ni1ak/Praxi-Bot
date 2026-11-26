use super::*;

#[tokio::test]
async fn profile_routes_require_owner_key_and_check_bot_ownership() {
    let (t, _) = setup_desktop();
    let (bot, _) = make_chat(&t).await;
    let path = format!("/v1/bots/{bot}/browser-profile");
    for (method, suffix) in [
        (Method::GET, ""),
        (Method::HEAD, ""),
        (Method::DELETE, ""),
        (Method::POST, "/sign-in"),
    ] {
        let r = call_token_only(&t, method, &format!("{path}{suffix}"), None).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let r = call(&t, Method::GET, &path, None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["domains"], json!([]));
    assert_eq!(r.json()["size_bytes"], 0);
    assert_eq!(r.json()["signing_in"], false);
    assert!(r.json().get("path").is_none());
    let missing = format!("/v1/bots/{}/browser-profile", uuid::Uuid::now_v7());
    assert_eq!(
        call(&t, Method::GET, &missing, None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&t, Method::POST, &format!("{path}/finish-sign-in"), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    let bad_domain = call(
        &t,
        Method::POST,
        &format!("{path}/forget-site"),
        Some(json!({"domain": "../escape"})),
    )
    .await;
    assert_eq!(bad_domain.status, StatusCode::CONFLICT);
    assert_eq!(bad_domain.json()["error"]["code"], "browser_profile_error");
    assert_eq!(
        call(
            &t,
            Method::POST,
            &format!("{path}/forget-site"),
            Some(json!({"domain": "example.org", "extra": 1})),
        )
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(&t, Method::DELETE, &path, None).await.status,
        StatusCode::NO_CONTENT
    );

    let remote_toml = format!(
        "{PROFILE}\n[runtime]\ncomputer = \"server:{}\"\n",
        uuid::Uuid::now_v7()
    );
    let remote = call(
        &t,
        Method::POST,
        "/v1/bots",
        Some(json!({ "profile_toml": remote_toml })),
    )
    .await;
    assert_eq!(remote.status, StatusCode::CREATED, "{:?}", remote.json());
    let remote_body = remote.json();
    let remote_id = remote_body["id"].as_str().unwrap();
    let remote_resp = call(
        &t,
        Method::GET,
        &format!("/v1/bots/{remote_id}/browser-profile"),
        None,
    )
    .await;
    assert_eq!(remote_resp.status, StatusCode::CONFLICT);
    assert_eq!(remote_resp.json()["error"]["code"], "browser_profile_error");
}

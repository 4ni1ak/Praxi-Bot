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
    let missing = format!("/v1/bots/{}/browser-profile", uuid::Uuid::new_v4());
    assert_eq!(
        call(&t, Method::GET, &missing, None).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&t, Method::DELETE, &path, None).await.status,
        StatusCode::NO_CONTENT
    );
}

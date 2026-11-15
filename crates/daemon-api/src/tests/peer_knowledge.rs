//! Paired profile editors cannot bypass host-local knowledge authority routes.
use super::*;

async fn peer_request(
    t: &T,
    role: crate::PeerRole,
    method: Method,
    path: &str,
    body: Value,
) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(crate::PeerIdentity {
        key: "knowledge-review-peer".into(),
        role,
    });
    let response = crate::peer_router(t.state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

async fn rejects_knowledge_changes(role: crate::PeerRole) {
    let t = setup();
    let (id, _) = make_chat(&t).await;
    let bot_id = protocol::BotId(id.parse().unwrap());
    let mut bot = t.state.bot_of_user(bot_id).unwrap();
    let original = json!({"auto_retrieve":false,"embedding":{"profile":{"model":"approved-model"},"budget":{"requests_per_minute":1},"local_profile_id":"installed-profile"}});
    bot.config["knowledge"] = original.clone();
    t.state
        .storage()
        .update_bot(bot.id, &bot.name, &bot.config)
        .unwrap();
    let mut variants = Vec::new();
    let mut changed = original.clone();
    changed["auto_retrieve"] = true.into();
    variants.push(changed);
    let mut changed = original.clone();
    changed["embedding"]["budget"]["requests_per_minute"] = 100_000.into();
    variants.push(changed);
    let mut changed = original.clone();
    changed["embedding"]["local_profile_id"] = "different-profile".into();
    variants.push(changed);
    let mut changed = original.clone();
    changed["embedding"]["local_artifacts"] =
        json!({"runtime":{"path":"/tmp/untrusted-runtime","sha256":"a".repeat(64)}});
    variants.push(changed);
    let mut changed = original.clone();
    changed["embedding"]["query_consent"] =
        json!({"profile_fingerprint":"a".repeat(64),"granted_at_ms":1});
    variants.push(changed);
    let mut changed = original.clone();
    changed["sources"] = json!([{"remote_embedding":{"profile_fingerprint":"b".repeat(64)}}]);
    variants.push(changed);
    variants.push(json!({}));
    for (variant, knowledge) in variants.into_iter().enumerate() {
        let mut config = bot.config.clone();
        config["knowledge"] = knowledge;
        for body in [
            json!({"config":config}),
            json!({"profile_toml":toml::to_string(&config).unwrap()}),
        ] {
            for (method, path) in [
                (Method::POST, "/v1/bots".to_owned()),
                (Method::PUT, format!("/v1/bots/{id}")),
            ] {
                let response = peer_request(&t, role, method.clone(), &path, body.clone()).await;
                assert_eq!(
                    response.status,
                    StatusCode::FORBIDDEN,
                    "{role:?} {method} variant {variant}: {}",
                    response.json()
                );
                assert_eq!(
                    t.state.bot_of_user(bot_id).unwrap().config["knowledge"],
                    original
                );
            }
        }
    }
    if matches!(role, crate::PeerRole::Phone | crate::PeerRole::Desktop) {
        for include_knowledge in [true, false] {
            let mut config = bot.config.clone();
            config["bot"]["name"] = "Peer renamed bot".into();
            if !include_knowledge {
                config.as_object_mut().unwrap().remove("knowledge");
            }
            for body in [
                json!({"config":config}),
                json!({"profile_toml":toml::to_string(&config).unwrap()}),
            ] {
                let response =
                    peer_request(&t, role, Method::PUT, &format!("/v1/bots/{id}"), body).await;
                assert_eq!(
                    response.status,
                    StatusCode::OK,
                    "ordinary peer edit: {}",
                    response.json()
                );
                assert_eq!(response.json()["config"]["knowledge"], original);
                assert_eq!(response.json()["name"], "Peer renamed bot");
            }
        }
    }
    let owner = call(
        &t,
        Method::PUT,
        &format!("/v1/bots/{id}"),
        Some(json!({"name":"Owner rename","config":bot.config})),
    )
    .await;
    assert_eq!(owner.status, StatusCode::OK);
}

#[tokio::test]
async fn peer_knowledge_phone_cannot_change_profile_authority() {
    rejects_knowledge_changes(crate::PeerRole::Phone).await;
}
#[tokio::test]
async fn peer_knowledge_desktop_cannot_change_profile_authority() {
    rejects_knowledge_changes(crate::PeerRole::Desktop).await;
}
#[tokio::test]
async fn peer_knowledge_node_cannot_change_profile_authority() {
    rejects_knowledge_changes(crate::PeerRole::Node).await;
}
#[tokio::test]
async fn peer_knowledge_host_cannot_change_profile_authority() {
    rejects_knowledge_changes(crate::PeerRole::Host).await;
}

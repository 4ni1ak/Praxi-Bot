//! The per-site / per-app rule routes (`/v1/bots/{id}/site-rules*`): who may change a rule,
//! and what a rule must look like.
use super::*;
use crate::PeerRole;

async fn peer_request(t: &T, role: PeerRole, method: Method, path: &str, body: Value) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(crate::PeerIdentity {
        key: "site-rules-peer".into(),
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

fn block_site() -> Value {
    json!({ "kind": "site", "key": "bank.example", "capability": "any", "action": "block" })
}

async fn rules_of(t: &T, bot: &str) -> Vec<Value> {
    let r = call(t, Method::GET, &format!("/v1/bots/{bot}/site-rules"), None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    r.json().as_array().unwrap().clone()
}

#[tokio::test]
async fn the_general_token_cannot_add_or_remove_a_rule() {
    let t = setup();
    let (bot, _) = make_chat(&t).await;
    let path = format!("/v1/bots/{bot}/site-rules");
    let r = call_token_only(&t, Method::POST, &path, Some(block_site())).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["error"]["code"], "owner_key_required");
    assert_eq!(rules_of(&t, &bot).await.len(), 0);

    let made = call(&t, Method::POST, &path, Some(block_site())).await;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.json());
    let id = made.json()["id"].as_str().unwrap().to_owned();
    let r = call_token_only(&t, Method::DELETE, &format!("{path}/{id}"), None).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.json()["error"]["code"], "owner_key_required");
    assert_eq!(rules_of(&t, &bot).await.len(), 1);
}

#[tokio::test]
async fn a_paired_peer_cannot_add_or_remove_a_rule_but_may_read_them() {
    let t = setup();
    let (bot, _) = make_chat(&t).await;
    let path = format!("/v1/bots/{bot}/site-rules");
    let made = call(&t, Method::POST, &path, Some(block_site())).await;
    let id = made.json()["id"].as_str().unwrap().to_owned();
    for role in [PeerRole::Phone, PeerRole::Desktop] {
        let add = peer_request(&t, role, Method::POST, &path, block_site()).await;
        assert_eq!(add.status, StatusCode::FORBIDDEN, "{role:?} add");
        let del = peer_request(&t, role, Method::DELETE, &format!("{path}/{id}"), json!({})).await;
        assert_eq!(del.status, StatusCode::FORBIDDEN, "{role:?} delete");
    }
    assert_eq!(rules_of(&t, &bot).await.len(), 1);
}

#[tokio::test]
async fn an_invalid_rule_is_refused_and_nothing_is_stored() {
    let t = setup();
    let (bot, _) = make_chat(&t).await;
    let path = format!("/v1/bots/{bot}/site-rules");
    let bad = [
        json!({ "kind": "page", "key": "a.example", "capability": "any", "action": "block" }),
        json!({ "kind": "site", "key": "a.example", "capability": "write", "action": "block" }),
        json!({ "kind": "site", "key": "a.example", "capability": "any", "action": "permit" }),
        // not a plain host name
        json!({ "kind": "site", "key": "https://a.example/login", "capability": "any", "action": "block" }),
        // an allow rule names one class and never covers subdomains
        json!({ "kind": "site", "key": "a.example", "capability": "any", "action": "allow" }),
        json!({ "kind": "site", "key": "a.example", "capability": "read", "action": "allow", "include_subdomains": true }),
        // a program must be an absolute path
        json!({ "kind": "app", "key": "notepad.exe", "capability": "any", "action": "block" }),
    ];
    for body in bad {
        let r = call(&t, Method::POST, &path, Some(body.clone())).await;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(r.json()["error"]["code"], "bad_profile", "{body}");
    }
    assert_eq!(rules_of(&t, &bot).await.len(), 0);
    let r = call(&t, Method::DELETE, &format!("{path}/no-such-rule"), None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

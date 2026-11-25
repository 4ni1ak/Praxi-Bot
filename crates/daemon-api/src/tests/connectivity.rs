use super::*;
use provider_catalog::connectivity::{Connectivity, Target};

#[tokio::test]
async fn connectivity_is_owner_only_and_does_not_expose_endpoints() {
    let t = setup();
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("HEAD"))
        .respond_with(wiremock::ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let mut providers = t.agent.providers();
    providers.connectivity = Connectivity::new(vec![Target {
        id: "fake".into(),
        url: server.uri(),
    }]);
    t.agent.set_providers(providers);
    let denied = call_token_only(&t, Method::GET, "/v1/connectivity", None).await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let report = call(&t, Method::GET, "/v1/connectivity", None).await;
    assert_eq!(report.status, StatusCode::OK);
    assert_eq!(
        report.json(),
        json!({"services":[{"id":"fake","local":true,"reachable":true}]})
    );
    assert!(
        !String::from_utf8(report.body)
            .unwrap()
            .contains(&server.uri())
    );
    for (role, expected) in [
        (crate::PeerRole::Desktop, StatusCode::OK),
        (crate::PeerRole::Phone, StatusCode::FORBIDDEN),
    ] {
        let mut request = Request::builder()
            .uri("/v1/connectivity")
            .body(Body::empty())
            .unwrap();
        request.extensions_mut().insert(crate::PeerIdentity {
            key: "connectivity-review-peer".into(),
            role,
        });
        let response = crate::peer_router(t.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

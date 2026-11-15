use super::*;

#[tokio::test]
async fn registry_is_authenticated_and_preserves_confinement() {
    let t = setup();
    let no_auth = send(&t.app, Method::GET, "/v1/providers/cli", None, &[]).await;
    assert_eq!(no_auth.status, StatusCode::UNAUTHORIZED);
    let response = call(&t, Method::GET, "/v1/providers/cli", None).await;
    assert_eq!(response.status, StatusCode::OK);
    let body = response.json();
    let rows = body.as_array().unwrap();
    assert_eq!(rows.len(), 6);
    let claude = rows
        .iter()
        .find(|r| r["detected"]["id"] == "claude")
        .unwrap();
    assert_eq!(claude["minimum_version"], "2.1.257");
    assert_eq!(claude["version_args"], json!(["--version"]));
    assert_eq!(claude["auth_args"], json!(["auth", "status", "--json"]));
    let codex = rows
        .iter()
        .find(|r| r["detected"]["id"] == "codex")
        .unwrap();
    assert_eq!(codex["limitation"], "native_only");
    assert_eq!(codex["native"]["support"], "verified");
    let opencode = rows
        .iter()
        .find(|r| r["detected"]["id"] == "opencode")
        .unwrap();
    assert_eq!(opencode["detected"]["runnable"], false);
    assert_eq!(opencode["limitation"], "native_only");
    let levels = opencode["native"]["levels"].as_array().unwrap();
    assert_eq!(levels.len(), 4);
    assert_eq!(levels[2]["offered"], false, "OpenCode has no Auto reviewer");
    assert_eq!(levels[2]["applied"], "ask");
    let gemini = rows
        .iter()
        .find(|r| r["detected"]["id"] == "gemini")
        .unwrap();
    assert_eq!(gemini["native"]["support"], "experimental");
    assert_eq!(
        opencode["install"]["args"],
        json!(["install", "-g", "opencode-ai"])
    );
}

#[tokio::test]
async fn install_requires_single_use_approval_and_audits_before_execution() {
    let t = setup();
    let unknown = format!(
        "/v1/providers/cli/install/{}/decision",
        uuid::Uuid::now_v7()
    );
    assert_eq!(
        call(
            &t,
            Method::POST,
            &unknown,
            Some(json!({"decision":"approve"}))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
    for body in [
        json!({"id":"claude;whoami"}),
        json!({"id":"claude", "argv":["evil"]}),
        json!({"id":"gemini"}),
    ] {
        assert_eq!(
            call(&t, Method::POST, "/v1/providers/cli/install", Some(body))
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
    }
    let preview = call(
        &t,
        Method::POST,
        "/v1/providers/cli/install",
        Some(json!({"id":"opencode"})),
    )
    .await;
    assert_eq!(preview.status, StatusCode::OK);
    let preview = preview.json();
    assert_eq!(preview["command"], "npm install -g opencode-ai");
    let actions: Vec<_> = t
        .state
        .storage()
        .list_audit(t.state.user_id())
        .unwrap()
        .into_iter()
        .map(|a| a.action)
        .collect();
    assert_eq!(actions, ["provider.cli_install.requested"]);
    let path = format!(
        "/v1/providers/cli/install/{}/decision",
        preview["approval_id"].as_str().unwrap()
    );
    for extra in [
        json!({"decision":"approve", "command":"whoami"}),
        json!({"decision":"approve", "scope":"always"}),
    ] {
        assert_eq!(
            call(&t, Method::POST, &path, Some(extra)).await.status,
            StatusCode::BAD_REQUEST
        );
    }
    let decision = call(&t, Method::POST, &path, Some(json!({"decision":"approve"}))).await;
    assert_eq!(decision.status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&decision.body).contains("npm_missing"));
    let actions: Vec<_> = t
        .state
        .storage()
        .list_audit(t.state.user_id())
        .unwrap()
        .into_iter()
        .map(|a| a.action)
        .collect();
    assert_eq!(
        actions,
        [
            "provider.cli_install.requested",
            "provider.cli_install.decision",
            "provider.cli_install.started",
            "provider.cli_install.finished"
        ]
    );
    assert_eq!(
        call(&t, Method::POST, &path, Some(json!({"decision":"approve"})))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn denied_and_expired_install_approvals_never_start() {
    let t = setup();
    for expire in [false, true] {
        let preview = call(
            &t,
            Method::POST,
            "/v1/providers/cli/install",
            Some(json!({"id":"claude"})),
        )
        .await
        .json();
        let id = preview["approval_id"].as_str().unwrap();
        if expire {
            // Expiration is checked by the decision route, not trusted from UI.
            crate::cli_engines::expire_for_test(&t.state, id);
        }
        let decision = call(
            &t,
            Method::POST,
            &format!("/v1/providers/cli/install/{id}/decision"),
            Some(json!({"decision": if expire { "approve" } else { "deny" }})),
        )
        .await;
        assert_eq!(
            decision.status,
            if expire {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            }
        );
    }
    assert!(
        !t.state
            .storage()
            .list_audit(t.state.user_id())
            .unwrap()
            .iter()
            .any(|a| a.action == "provider.cli_install.started")
    );
}

#[tokio::test]
async fn every_peer_role_is_denied_cli_mutations_including_path_variants() {
    let t = setup();
    for role in [
        crate::PeerRole::Desktop,
        crate::PeerRole::Phone,
        crate::PeerRole::Node,
        crate::PeerRole::Host,
    ] {
        for path in [
            "/v1/providers/cli/install",
            "/v1/providers/cli/install/",
            "/v1/providers//cli/install",
            "/v1/providers/cli%2finstall",
            "/v1/providers/cli/install/00000000-0000-0000-0000-000000000000/decision",
            "/v1/providers/cli/install/00000000-0000-0000-0000-000000000000/decision/",
            "/v1/providers//cli/install/00000000-0000-0000-0000-000000000000/decision",
            "/v1/providers/cli%2Finstall/00000000-0000-0000-0000-000000000000%2Fdecision",
            "/v1/providers/cli/enable",
            "/v1/providers/cli%2Fenable/",
        ] {
            let mut req = Request::builder()
                .method(Method::POST)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"id":"claude"}"#))
                .unwrap();
            req.extensions_mut().insert(crate::PeerIdentity {
                key: "peer".into(),
                role,
            });
            assert_eq!(
                crate::peer_router(t.state.clone())
                    .oneshot(req)
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN,
                "{role:?} {path}"
            );
        }
    }
    assert_eq!(
        t.state
            .storage()
            .list_audit(t.state.user_id())
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn handler_owner_guard_cannot_be_granted_by_a_peer_role() {
    for role in [
        crate::PeerRole::Desktop,
        crate::PeerRole::Phone,
        crate::PeerRole::Node,
        crate::PeerRole::Host,
    ] {
        let peer = crate::PeerIdentity {
            key: "peer".into(),
            role,
        };
        assert!(crate::cli_engines::local_owner(&crate::live_view::Actor::peer(&peer)).is_err());
    }
}

#[tokio::test]
async fn audit_write_failure_refuses_install_before_scheduling_any_process() {
    let t = setup();
    let preview = call(
        &t,
        Method::POST,
        "/v1/providers/cli/install",
        Some(json!({"id":"opencode"})),
    )
    .await
    .json();
    let db = rusqlite::Connection::open(t.state.data_dir().join("bot.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_cli_start BEFORE INSERT ON audit_log
        WHEN NEW.action = 'provider.cli_install.started'
        BEGIN SELECT RAISE(ABORT, 'fixture audit unavailable'); END;",
    )
    .unwrap();
    let path = format!(
        "/v1/providers/cli/install/{}/decision",
        preview["approval_id"].as_str().unwrap()
    );
    assert_eq!(
        call(&t, Method::POST, &path, Some(json!({"decision":"approve"})))
            .await
            .status,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let actions: Vec<_> = t
        .state
        .storage()
        .list_audit(t.state.user_id())
        .unwrap()
        .into_iter()
        .map(|row| row.action)
        .collect();
    assert_eq!(
        actions,
        [
            "provider.cli_install.requested",
            "provider.cli_install.decision"
        ]
    );
}

#[tokio::test]
async fn unauthenticated_bot_or_tool_cannot_request_or_decide_install() {
    let t = setup();
    for path in [
        "/v1/providers/cli/install",
        "/v1/providers/cli/enable",
        "/v1/providers/cli/install/00000000-0000-0000-0000-000000000000/decision",
    ] {
        let r = send(
            &t.app,
            Method::POST,
            path,
            Some(json!({"id":"claude"})),
            &[("x-actor", "local-owner"), ("x-initiator", "user")],
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(
        t.state
            .storage()
            .list_audit(t.state.user_id())
            .unwrap()
            .len(),
        0
    );
}

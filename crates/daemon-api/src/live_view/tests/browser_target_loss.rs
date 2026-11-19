//! Exercise daemon recovery against the real browser adapter over hermetic CDP.
use super::*;
use browser_cdp::{BrowserViewConfig, BrowserViewTarget, CdpClient, Page};
use futures::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn browser_target_loss_reset_retry_and_local_recover_clear_the_latch() {
    // Page drop may leave the tab alive, or the session may actually be gone.
    // Both must terminate; the live session must receive the owed Shift up.
    for (session_gone, retry_first) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let commands = Arc::new(Mutex::new(Vec::<Value>::new()));
        let recorded = commands.clone();
        let gone = Arc::new(AtomicBool::new(false));
        let server_gone = gone.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                let command: Value = serde_json::from_str(&text).unwrap();
                recorded.lock().push(command.clone());
                let reply = if server_gone.load(Ordering::SeqCst)
                    && command["method"] == "Input.dispatchKeyEvent"
                {
                    json!({"id":command["id"],"error":{"code":-32001,"message":"Session with given id not found"}})
                } else {
                    json!({"id":command["id"],"result":{"sessionId":"original-session"}})
                };
                if ws.send(Message::text(reply.to_string())).await.is_err() {
                    break;
                }
            }
        });
        let client = CdpClient::connect(&format!("ws://{addr}")).await.unwrap();
        let page = Page::attach(client.clone(), "original-target")
            .await
            .unwrap();
        let t = Test::new();
        let (_id, owned, _body) = t.active(Some("alice")).await;
        // Replace only the synthetic backend, keeping the active daemon takeover
        // and its target identity. Recovery now runs the real adapter methods.
        let target = BrowserViewTarget::new_idle(BrowserViewConfig {
            target_id: owned.target_id,
            ..BrowserViewConfig::default()
        });
        target.attach_page(&page).unwrap();
        t.state.live_view().registry.register(target.clone());
        let sink = target.input_sink().unwrap();
        sink.set_takeover_active(true).await.unwrap();
        sink.inject(&InputEvent::Key {
            phase: KeyPhase::Down,
            code: "ShiftLeft".into(),
            key: "Shift".into(),
            location: 1,
            repeat: false,
            modifiers: vec![Modifier::Shift],
        })
        .await
        .unwrap();
        commands.lock().clear();
        gone.store(session_gone, Ordering::SeqCst);
        drop(page);
        assert!(!client.is_closed());
        assert!(!target.info().capabilities.input);
        assert!(t.state.hub().interrupt(owned.id, Reason::Unavailable));
        t.state.hub().mark_reset_pending();
        if retry_first {
            tokio::time::timeout(Duration::from_secs(2), retry_owed_reset(&t.state))
                .await
                .expect("heartbeat reset retry must terminate");
            assert!(!t.state.hub().override_pending().0, "reset debt cleared");
        }
        assert!(
            t.state.hub().latched().is_some(),
            "only explicit recovery clears the latch"
        );
        let (status, value) = tokio::time::timeout(
            Duration::from_secs(2),
            t.call(None, Method::POST, "/v1/control/recover", None),
        )
        .await
        .expect("local recovery must terminate");
        assert_eq!(status, StatusCode::OK, "{value}");
        assert!(t.state.hub().takeover().is_none());
        assert!(t.state.hub().latched().is_none());
        assert!(!target.status().takeover_active);
        let ups: Vec<_> = commands
            .lock()
            .iter()
            .filter(|c| c["method"] == "Input.dispatchKeyEvent")
            .cloned()
            .collect();
        assert_eq!(ups.len(), 1, "one owed up; no retained dead-session debt");
        assert_eq!(ups[0]["sessionId"], "original-session");
        assert_eq!(ups[0]["params"]["type"], "keyUp");
        assert_eq!(ups[0]["params"]["code"], "ShiftLeft");
        server.abort();
    }
}

#[tokio::test]
async fn browser_target_positional_input_continues_working_through_daemon() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let commands = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorded = commands.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let mut ack_id = 1u64;
        while let Some(Ok(Message::Text(text))) = ws.next().await {
            let command: Value = serde_json::from_str(&text).unwrap();
            recorded.lock().push(command.clone());
            let reply = if command["method"] == "Page.getLayoutMetrics" {
                json!({
                    "id": command["id"],
                    "result": {
                        "cssVisualViewport": {
                            "clientWidth": 800,
                            "clientHeight": 600,
                            "scale": 1,
                            "pageX": 0,
                            "pageY": 0
                        }
                    }
                })
            } else {
                json!({"id": command["id"], "result": {"sessionId": "browser-session"}})
            };
            if ws.send(Message::text(reply.to_string())).await.is_err() {
                break;
            }
            if command["method"] == "Page.startScreencast" {
                let frame_evt = json!({
                    "method": "Page.screencastFrame",
                    "sessionId": "browser-session",
                    "params": {
                        "data": "/9j/wAALCAGQAoABAREA/9k=",
                        "metadata": {
                            "offsetTop": 0,
                            "pageScaleFactor": 1,
                            "deviceWidth": 800,
                            "deviceHeight": 600,
                            "scrollOffsetX": 0,
                            "scrollOffsetY": 0
                        },
                        "sessionId": ack_id
                    }
                });
                ack_id += 1;
                if ws.send(Message::text(frame_evt.to_string())).await.is_err() {
                    break;
                }
            }
        }
    });

    let client = CdpClient::connect(&format!("ws://{addr}")).await.unwrap();
    let page = Page::attach(client.clone(), "browser-target")
        .await
        .unwrap();
    let target = BrowserViewTarget::new_idle(BrowserViewConfig::default());
    target.attach_page(&page).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(storage::Storage::open(dir.path()).unwrap());
    let agent = agent_core::AgentLoop::new(
        storage.clone(),
        tool_registry::ToolRegistry::new(),
        agent_core::Providers::single(Arc::new(crate::fake::FakeProvider)),
        agent_core::LoopConfig::default(),
    );
    let state = AppState::new(agent, storage, crate::DaemonConfig::new(TOKEN)).unwrap();
    state.set_owner_key("ownerkey0123456789abcdef0123456789abcdef0123456789abcdef".into());
    state.live_view().registry.register(target.clone());
    let t = Test {
        state,
        target: Arc::new(Target::new()),
        dir: Arc::new(dir),
        auto_grant: AtomicBool::new(true),
    };

    let target_id = target.info().target_id;
    let generation = target.info().generation;
    let (status, val) = t
        .call(
            None,
            Method::POST,
            "/v1/view/sessions",
            Some(json!({
                "target_id": target_id,
                "profile": "watch",
                "max_width": 960,
                "max_height": 600,
                "codecs": ["jpeg"],
                "client_id": Uuid::now_v7()
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{val}");
    let session_id = Uuid::parse_str(val["session_id"].as_str().unwrap()).unwrap();

    let response = t
        .response(
            None,
            Method::GET,
            &format!("/v1/view/sessions/{session_id}/frames"),
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    for _ in 0..8 {
        if t.state
            .live_view()
            .sessions
            .lock()
            .get(&session_id)
            .is_some_and(|s| s.geometry.is_some())
        {
            break;
        }
        let _ = tokio::time::timeout(Duration::from_millis(250), body.frame()).await;
    }
    let revision = t
        .state
        .live_view()
        .sessions
        .lock()
        .get(&session_id)
        .and_then(|s| s.geometry)
        .map(|(r, _, _)| r)
        .expect("browser frame geometry must be recorded");

    let (status, v) = t
        .call(
            None,
            Method::POST,
            "/v1/control/takeover",
            Some(json!({"session_id": session_id, "request_id": Uuid::now_v7()})),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{v}");
    for _ in 0..100 {
        if t.state
            .hub()
            .takeover()
            .is_some_and(|tk| tk.info.state == ViewState::Active)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    let takeover = t.state.hub().takeover().unwrap().info;
    assert_eq!(takeover.state, ViewState::Active);

    let batch = json!({
        "v": 1,
        "takeover_id": takeover.id,
        "control_epoch": takeover.control_epoch,
        "generation": generation,
        "geometry_revision": revision,
        "first_seq": 1,
        "events": [{
            "type": "pointer",
            "phase": "down",
            "x": 0.5,
            "y": 0.5,
            "button": "left",
            "buttons": 1,
            "modifiers": []
        }]
    });
    let (status, res) = t
        .call(
            None,
            Method::POST,
            &format!("/v1/view/sessions/{session_id}/input"),
            Some(batch),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    assert_eq!(res["applied_through"], 1);
    assert_eq!(res["state"], "active");
    assert!(
        commands
            .lock()
            .iter()
            .any(|c| c["method"] == "Input.dispatchMouseEvent"
                && c["params"]["type"] == "mousePressed"),
        "browser target must still receive mousePressed through daemon input path"
    );
    server.abort();
}

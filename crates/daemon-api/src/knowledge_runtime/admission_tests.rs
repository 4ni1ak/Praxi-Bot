use super::*;
use permission_engine::{Decision, Level, Policy};
use protocol::{ChatId, InitiatorKind, PermissionCategory, RunId};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn host_registry(host: &Host) -> std::path::PathBuf {
    let artifact =
        json!({"path":host._dir.path().join("operator-only-file"),"sha256":"c".repeat(64)});
    let file = host.state.data_dir().join("knowledge-host.json");
    std::fs::write(
        &file,
        serde_json::to_vec(
            &json!({"profiles":{"e5-installed":{"label":"Operator model","artifacts":{
                "runtime":artifact,"model":artifact,"tokenizer":artifact,"config":artifact,
                "special_tokens_map":artifact,"tokenizer_config":artifact
            }}}}),
        )
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    file
}

#[tokio::test]
async fn local_profile_identity_is_derived_only_from_operator_registry() {
    let host = Host::new();
    host_registry(&host);
    let profiles = list_local_profiles(&host.state).unwrap();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].id, "e5-installed");
    assert!(
        !serde_json::to_string(&profiles)
            .unwrap()
            .contains("operator-only-file")
    );
    let mut config = host.config("http://127.0.0.1:9");
    config.local_profile_id = Some("e5-installed".into());
    config.profile.model = "bot-asserted-model".into();
    host.save(host.bot, Some(&config));
    assert_eq!(
        configured_settings(&host.state, host.bot).unwrap().profile,
        profiles[0].profile
    );
    config.local_profile_id = Some("uninstalled".into());
    host.save(host.bot, Some(&config));
    assert!(configured_settings(&host.state, host.bot).is_err());
}

#[tokio::test]
#[cfg(unix)]
async fn host_registry_rejects_symlinks_and_untrusted_opened_file_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let host = Host::new();
    let file = host_registry(&host);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(list_local_profiles(&host.state).is_err());
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let target = file.with_file_name("replacement.json");
    std::fs::rename(&file, &target).unwrap();
    symlink(&target, &file).unwrap();
    assert!(list_local_profiles(&host.state).is_err());
}

async fn run_tool(host: &Host, initiator: InitiatorKind, message: &str) -> RunId {
    let chat = ChatId::new();
    host.state
        .storage()
        .insert_chat(&storage::Chat {
            id: chat,
            user_id: host.state.user_id(),
            bot_id: host.bot,
            title: "Embedding permission regression".into(),
            created_at_ms: 1,
        })
        .unwrap();
    let run = host
        .state
        .agent()
        .run_turn_as(chat, message, initiator)
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(5), host.state.agent().wait(run))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(format!("{outcome:?}"), "Succeeded");
    run
}

async fn direct_tool(host: &Host, run: String, query: &str) -> SearchResponse {
    let tool = host
        .state
        .agent()
        .tool_registry()
        .get("knowledge_search")
        .unwrap();
    let result = tool
        .call(
            &ToolContext {
                run_id: run,
                bot_id: Some(host.bot.to_string()),
                ..ToolContext::default()
            },
            json!({"query":query}),
        )
        .await
        .unwrap();
    let ContentPart::Text { text } = &result.content[0] else {
        panic!("expected knowledge JSON")
    };
    serde_json::from_str(text).unwrap()
}

#[tokio::test]
async fn actual_run_initiator_and_subagent_policy_block_query_http() {
    let _serial = test_guard().await;
    let server = server().await;
    let host = Host::new();
    ready(&host, &server).await;
    server.reset().await;
    mount_vectors(&server).await;
    let mut policy = Policy::full();
    let remote = policy
        .categories
        .get_mut(&PermissionCategory::Remote)
        .unwrap();
    remote
        .by_initiator
        .insert(InitiatorKind::Schedule, Level::Deny);
    remote
        .by_initiator
        .insert(InitiatorKind::Channel, Level::Deny);
    host.state.agent().set_global_policy(policy);
    for initiator in [InitiatorKind::Schedule, InitiatorKind::Channel] {
        let run = run_tool(
            &host,
            initiator,
            "call:knowledge_search {\"query\":\"disaster recovery\"}",
        )
        .await;
        let steps = host.state.storage().list_steps(run).unwrap();
        assert!(
            steps.iter().any(|step| step
                .output
                .as_ref()
                .is_some_and(|out| out.to_string().contains("lexical"))),
            "tool must really execute locally"
        );
    }
    let child_task = json!({"task":"call:knowledge_search {\"query\":\"inherited child recovery\"}","tools":["knowledge_search"]});
    let parent = run_tool(
        &host,
        InitiatorKind::Schedule,
        &format!("call:spawn_subagent {child_task}"),
    )
    .await;
    let children: Vec<_> = host
        .state
        .storage()
        .list_runs(host.state.user_id())
        .unwrap()
        .into_iter()
        .filter(|run| run.parent_run_id == Some(parent))
        .collect();
    assert_eq!(children.len(), 1);
    assert!(
        host.state
            .storage()
            .list_steps(children[0].id)
            .unwrap()
            .iter()
            .any(|step| step
                .output
                .as_ref()
                .is_some_and(|out| out.to_string().contains("lexical")))
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
    let human = run_tool(
        &host,
        InitiatorKind::Human,
        "call:knowledge_search {\"query\":\"human recovery\"}",
    )
    .await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "positive control must exercise remote query dispatch"
    );
    assert_eq!(
        host.state
            .agent()
            .embedding_egress_decision(host.bot, &human.to_string()),
        Decision::Deny
    );
    for run in [human.to_string(), RunId::new().to_string(), "forged".into()] {
        assert_eq!(
            direct_tool(&host, run, "replay must stay local").await.mode,
            SearchMode::Lexical
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn owner_challenges_are_policy_checked_single_use_and_revision_bound() {
    let _serial = test_guard().await;
    let server = server().await;
    let host = Host::new();
    let config = host.config(&server.uri());
    host.save(host.bot, Some(&config));
    let source = host.source(host.bot, "Saffron approval test.");
    let target = EmbeddingGrantTarget::Documents {
        source_id: source.id,
    };
    host.state.agent().set_global_policy(Policy::read_only());
    assert!(grant_request(&host.state, host.bot, target.clone()).is_err());
    host.state
        .agent()
        .set_global_policy(Policy::approve_destructive());
    let stale = grant_request(&host.state, host.bot, target.clone()).unwrap();
    assert_eq!(stale.policy_decision, "ask");
    host.store(host.bot)
        .update_source(
            source.id,
            source.revision,
            SourceUpdate {
                label: Some("Changed".into()),
                ..SourceUpdate::default()
            },
        )
        .unwrap();
    assert!(grant_confirm(&host.state, host.bot, stale.id).is_err());
    let challenge = grant_request(&host.state, host.bot, target.clone()).unwrap();
    host.state.agent().set_policy_locked(true);
    assert!(grant_confirm(&host.state, host.bot, challenge.id).is_err());
    host.state.agent().set_policy_locked(false);
    grant_confirm(&host.state, host.bot, challenge.id).unwrap();
    assert!(grant_confirm(&host.state, host.bot, challenge.id).is_err());
    assert!(
        authorized(
            &host.state,
            host.bot,
            &target,
            &config.profile.fingerprint().unwrap()
        )
        .is_some()
    );
    host.state.agent().set_global_policy(Policy::read_only());
    let source = host.store(host.bot).source(source.id).unwrap().unwrap();
    let progress = embedding_batch(&host.state, host.bot, source.id, source.revision)
        .await
        .unwrap();
    assert_eq!(progress.status, EmbeddingStatus::AwaitingConsent);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn imported_same_bot_source_and_profile_cannot_replay_host_grants() {
    let _serial = test_guard().await;
    let server = server().await;
    let host = Host::new();
    let (config, source) = ready(&host, &server).await;
    let store = host.store(host.bot);
    let old_epoch = store.authority_epoch().unwrap();
    let path = host
        .state
        .data_dir()
        .join("bots")
        .join(host.bot.to_string())
        .join("knowledge.db");
    drop(store);
    quiesce_bot(&host.state, host.bot).await.unwrap();
    KnowledgeStore::prepare_import(
        &path,
        Scope {
            user_id: host.state.user_id(),
            bot_id: host.bot,
        },
    )
    .unwrap();
    let store = host.store(host.bot);
    assert_ne!(store.authority_epoch().unwrap(), old_epoch);
    let imported = store.source(source.id).unwrap().unwrap();
    let rebound = store
        .update_source(
            imported.id,
            imported.revision,
            SourceUpdate {
                enabled: Some(true),
                remote_embedding: Some(Some(consent(&config))),
                ..SourceUpdate::default()
            },
        )
        .unwrap();
    assert!(
        authorized(
            &host.state,
            host.bot,
            &EmbeddingGrantTarget::Queries,
            &config.profile.fingerprint().unwrap()
        )
        .is_none()
    );
    assert!(
        authorized(
            &host.state,
            host.bot,
            &EmbeddingGrantTarget::Documents {
                source_id: source.id
            },
            &config.profile.fingerprint().unwrap()
        )
        .is_none()
    );
    server.reset().await;
    mount_vectors(&server).await;
    let progress = embedding_batch(&host.state, host.bot, rebound.id, rebound.revision)
        .await
        .unwrap();
    assert_eq!(progress.status, EmbeddingStatus::AwaitingConsent);
    assert_eq!(
        host.search(host.bot, "disaster recovery").await.mode,
        SearchMode::Lexical
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

#[tokio::test]
async fn slow_document_embedding_does_not_block_another_bot_and_revocation_drains_it() {
    let _serial = test_guard().await;
    let server = server().await;
    let host = Host::new();
    let config = host.config(&server.uri());
    let other = BotId::new();
    host.add_bot(other);
    host.save(other, Some(&config));
    let other_source = host.source(other, "Saffron normal reference.");
    let other_source = host.grant(other, &other_source, &config);
    host.approve_query(other);
    embedding_batch(&host.state, other, other_source.id, other_source.revision)
        .await
        .unwrap();
    host.save(host.bot, Some(&config));
    let slow = host.source(host.bot, "Slow-saffron pending private document.");
    let slow = host.grant(host.bot, &slow, &config);
    server.reset().await;
    let entered = Arc::new(AtomicBool::new(false));
    let observed = entered.clone();
    Mock::given(path("/embeddings"))
        .respond_with(move |request: &Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let slow = body.to_string().contains("Slow-saffron");
            if slow {
                observed.store(true, Ordering::SeqCst);
            }
            let response = ResponseTemplate::new(200).set_body_json(
                json!({"model":"vector-v1","data":[{"index":0,"embedding":[1.0,0.0]}]}),
            );
            if slow {
                response.set_delay(Duration::from_secs(3))
            } else {
                response
            }
        })
        .mount(&server)
        .await;
    let state = host.state.clone();
    let bot = host.bot;
    let pending =
        tokio::spawn(async move { embedding_batch(&state, bot, slow.id, slow.revision).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !entered.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = tokio::time::timeout(
        Duration::from_millis(500),
        host.search(other, "independent recovery"),
    )
    .await
    .expect("another bot waited for the first bot's network request");
    assert_eq!(response.mode, SearchMode::Hybrid);
    let gate = authority_lock(&host.state, host.bot).lock_owned().await;
    quiesce_bot(&host.state, host.bot).await.unwrap();
    revoke_grant(
        &host.state,
        host.bot,
        EmbeddingGrantTarget::Documents { source_id: slow.id },
    )
    .unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap().status,
        EmbeddingStatus::Cancelled
    );
    let db = rusqlite::Connection::open(
        host.state
            .data_dir()
            .join("bots")
            .join(host.bot.to_string())
            .join("knowledge.db"),
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM knowledge_embeddings", [], |row| row
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    drop(gate);
}

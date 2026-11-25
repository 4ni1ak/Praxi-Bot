use super::*;
use provider_catalog::connectivity::{Connectivity, Target};
use scheduler::{MisfirePolicy, Payload, Schedule, ScheduleExecutor, ScheduleSpec};

#[tokio::test]
async fn offline_schedule_skips_before_creating_a_run_or_sending_anything() {
    let t = setup();
    let (bot, _) = make_chat(&t).await;
    let bot = protocol::BotId(uuid::Uuid::parse_str(&bot).unwrap());
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let mut providers = t.agent.providers();
    providers.connectivity = Connectivity::new(vec![Target {
        id: "fake".into(),
        url: format!("http://{address}"),
    }]);
    t.agent.set_providers(providers);
    let schedule = Schedule::new(
        bot,
        None,
        "offline job",
        ScheduleSpec::Every { seconds: 60 },
        Payload::AgentPrompt {
            text: "must not send".into(),
        },
        MisfirePolicy::Skip,
        storage::now_ms(),
    )
    .unwrap();
    let executor = crate::schedule::DaemonExecutor {
        agent: t.agent.clone(),
        storage: t.state.storage_arc(),
        store: t.state.schedule_store().unwrap(),
        user_id: t.state.user_id(),
        leader: t.state.leader().clone(),
    };
    let error = executor.execute(&schedule).await.unwrap_err();
    assert!(error.to_string().contains("Nothing was started"));
    assert_eq!(
        t.state
            .storage()
            .list_runs(t.state.user_id())
            .unwrap()
            .len(),
        0
    );
}

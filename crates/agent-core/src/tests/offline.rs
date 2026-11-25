use super::*;
use provider_catalog::connectivity::{Connectivity, Target};

struct Named(&'static str, Arc<Fake>);

#[async_trait]
impl ChatProvider for Named {
    fn id(&self) -> &str {
        self.0
    }
    async fn stream(
        &self,
        req: ChatRequest,
    ) -> Result<provider_catalog::ChatStream, ProviderError> {
        self.1.stream(req).await
    }
}

#[tokio::test]
async fn offline_primary_uses_only_configured_fallback_and_records_local_provenance() {
    let env = setup();
    let fake = Fake::new(vec![text("local answer")]);
    let lp = make_loop(
        &env.storage,
        env.bot,
        &fake,
        &[],
        profile("fallback = [\"installed/model\"]\n"),
        cfg(Policy::full()),
    );
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("HEAD"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let mut providers = Providers::single(Arc::new(Named("fake", fake.clone())))
        .with(Arc::new(Named("installed", fake.clone())));
    providers.connectivity = Connectivity::new(vec![
        Target {
            id: "fake".into(),
            url: format!("http://{address}"),
        },
        Target {
            id: "installed".into(),
            url: server.uri(),
        },
    ]);
    lp.set_providers(providers);
    let run = lp.run_turn(env.chat, "hi").unwrap();
    assert_eq!(lp.wait(run).await.unwrap(), RunState::Succeeded);
    assert_eq!(
        fake.n_requests(),
        1,
        "unreachable primary never invokes inference"
    );
    assert_eq!(fake.request(0).model.0, "installed/model");
    let steps = env.storage.list_steps(run).unwrap();
    let model = steps.iter().find(|s| s.name == "model").unwrap();
    assert_eq!(model.output.as_ref().unwrap()["local_model"], true);

    // Installing a model never silently opts a bot into using it.
    lp.add_profile(env.bot, profile(""));
    assert!(!lp.model_connection_available(env.bot).await.unwrap());
    let second = lp.run_turn(env.chat, "again").unwrap();
    assert_eq!(lp.wait(second).await.unwrap(), RunState::Failed);
    assert_eq!(fake.n_requests(), 1);
}

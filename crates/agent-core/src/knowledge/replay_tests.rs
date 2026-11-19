use super::tests::{fixture, step};
use super::*;
use provider_catalog::{
    AnthropicProvider, Auth, ChatProvider, ChatRequest, CompatFlags, GeminiProvider,
    ModelCapabilities, ModelRef, OpenAiCompatConfig, ProviderEntry, ProviderKind,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn replay() -> Vec<Message> {
    let (storage, run, profile) = fixture();
    let steps = [
        step(
            &run,
            0,
            STEP_USER,
            json!({"text":"Find the saffron snapshot in my notes"}),
            None,
        ),
        step(
            &run,
            1,
            STEP_TOOLS,
            json!({"pre_retrieval":true,"calls":[{"id":"knowledge-host-id","name":"knowledge_search","arguments":"{\"query\":\"saffron snapshot\"}","repeat":1}]}),
            Some(json!({"results":[{
                "id":"knowledge-host-id","call_uuid":uuid::Uuid::now_v7(),"name":"knowledge_search","is_error":false,"text":"{\"content_trust\":\"untrusted_data\",\"hits\":[],\"mode\":\"lexical\",\"truncated\":false,\"estimated_tokens\":29}","spill_path":null,
                "fact":{"ran":true,"ingests_untrusted":true,"reads_private":true,"label":"knowledge"}
            }]})),
        ),
    ];
    build_messages(
        &storage,
        &run,
        &steps,
        &profile,
        None,
        None,
        None,
        &crate::compaction::Compaction::OFF,
    )
    .unwrap()
}

#[test]
fn pre_retrieval_replay_has_no_synthetic_assistant_call_or_tool_result() {
    let messages = replay();
    assert!(
        messages
            .iter()
            .all(|message| message.tool_calls.is_empty() && message.tool_call_id.is_none())
    );
    assert!(
        messages
            .iter()
            .all(|message| matches!(message.role, MessageRole::System | MessageRole::User))
    );
    assert!(
        messages
            .last()
            .unwrap()
            .content
            .as_str()
            .unwrap()
            .contains("UNTRUSTED")
    );
}

async fn capture_request(kind: ProviderKind, messages: Vec<Message>) -> Value {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let capture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let (body_start, length) = loop {
            let mut part = [0_u8; 4096];
            let count = stream.read(&mut part).await.unwrap();
            assert!(count > 0 && bytes.len() + count <= 65536);
            bytes.extend_from_slice(&part[..count]);
            if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let header = std::str::from_utf8(&bytes[..end]).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|value| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                break (end + 4, length);
            }
        };
        while bytes.len() < body_start + length {
            let mut part = [0_u8; 4096];
            let count = stream.read(&mut part).await.unwrap();
            assert!(count > 0 && bytes.len() + count <= 65536);
            bytes.extend_from_slice(&part[..count]);
        }
        // No vendor call or credential: exercise the real adapter and inspect
        // its outbound JSON; a local 400 terminates the single request.
        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await.unwrap();
        serde_json::from_slice::<Value>(&bytes[body_start..body_start + length]).unwrap()
    });
    let entry = ProviderEntry {
        id: "shape".into(),
        kind: kind.clone(),
        base_url: format!("http://{address}"),
        auth: Auth::None,
        capabilities: ModelCapabilities {
            tools: true,
            vision: true,
            streaming: true,
            json_mode: true,
        },
        compat_flags: CompatFlags::default(),
        models_endpoint: None,
        models: Vec::new(),
    };
    let config = OpenAiCompatConfig {
        max_retries: 0,
        ..Default::default()
    };
    let provider: Box<dyn ChatProvider> = match kind {
        ProviderKind::Gemini => {
            Box::new(GeminiProvider::with_api_key(entry, config, None).unwrap())
        }
        ProviderKind::Anthropic => {
            Box::new(AnthropicProvider::with_api_key(entry, config, None).unwrap())
        }
        _ => unreachable!(),
    };
    assert!(
        provider
            .stream(ChatRequest::new(ModelRef("shape/test".into()), messages))
            .await
            .is_err()
    );
    capture.await.unwrap()
}

#[tokio::test]
async fn actual_gemini_and_anthropic_wire_shapes_replay_retrieval_as_plain_data() {
    for kind in [ProviderKind::Gemini, ProviderKind::Anthropic] {
        let body = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            capture_request(kind, replay()),
        )
        .await
        .unwrap();
        let wire = body.to_string();
        for forbidden in [
            "functionCall",
            "functionResponse",
            "thoughtSignature",
            "tool_use",
            "tool_result",
            "knowledge-host-id",
        ] {
            assert!(
                !wire.contains(forbidden),
                "synthetic provider protocol in retrieval replay: {forbidden}: {body}"
            );
        }
        assert!(wire.contains("untrusted_data"));
    }
}

//! Deterministic flow/cost regression, NOT a live model-quality benchmark.
//! Each scripted completion has an assumed 10 input / 5 output tokens. The
//! artificial per-million price is 1.0 for standard, 0.2 light, 0.1 routing.

use super::*;

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep all fixture inputs and the two-side scoring visible together.
async fn twenty_task_fake_eval_reduces_assumed_cost_without_losing_fixture_success() {
    let mut costs = [0.0_f64; 2];
    let mut succeeded = [0_u32; 2];
    let mut call_counts = [0_usize; 2];
    for enabled in [false, true] {
        let side = usize::from(enabled);
        for task in 0..20 {
            let read = Arc::new(tool("t.read", Effect::Read, "read-result"));
            let write = Arc::new(tool("t.write", Effect::Write, "never"));
            let (user, classifier, mut replies, expected_reads) = match task {
                0..=7 => (
                    [
                        "Merhaba",
                        "selam",
                        "hello",
                        "hi",
                        " MERHABA ",
                        "HELLO",
                        "Hi",
                        "Selam",
                    ][task],
                    false,
                    vec![text("ok")],
                    0,
                ),
                8..=11 => (
                    "frobnicate the gizmo",
                    true,
                    vec![
                        text(r#"{"route":"tool","name":"t.read","confidence":0.9}"#),
                        calls(&[("r", "t.read", json!({}))]),
                        text("ok"),
                    ],
                    1,
                ),
                12..=15 => (
                    [
                        "Design a database migration",
                        "Plan a browser task",
                        "Review this code",
                        "Explain a multistep project",
                    ][task - 12],
                    false,
                    vec![text("ok")],
                    0,
                ),
                _ => (
                    "hello",
                    false,
                    if enabled {
                        vec![calls(&[("unsafe", "t.write", json!({}))]), text("ok")]
                    } else {
                        vec![text("ok")]
                    },
                    0,
                ),
            };
            // Every fake call advertises the same usage, including tool turns.
            for reply in &mut replies {
                if let Reply::Chunks(chunks) = reply
                    && !chunks.iter().any(|c| matches!(c, ChatChunk::Usage(_)))
                {
                    chunks.insert(
                        0,
                        ChatChunk::Usage(Usage {
                            prompt_tokens: 10,
                            completion_tokens: 5,
                            cached_tokens: None,
                            cache_write_tokens: None,
                        }),
                    );
                }
            }
            let env = setup();
            let fake = Fake::new(replies);
            let extra = format!(
                "{}{}",
                if enabled { LIGHT_CONFIG } else { "" },
                if classifier {
                    "classifier = \"fake/c\"\n"
                } else {
                    ""
                }
            );
            let lp = make_loop(
                &env.storage,
                env.bot,
                &fake,
                &[&read, &write],
                profile(&extra),
                cfg(Policy::full()),
            );
            let run = lp.run_turn(env.chat, user).unwrap();
            assert_eq!(
                lp.wait(run).await.unwrap(),
                RunState::Succeeded,
                "task {task}, tiering {enabled}"
            );
            assert_eq!(read.count.load(Ordering::SeqCst), expected_reads);
            assert_eq!(
                write.count.load(Ordering::SeqCst),
                0,
                "unsafe light output never executes"
            );
            let answer = env
                .storage
                .list_messages(env.chat)
                .unwrap()
                .into_iter()
                .rfind(|m| m.role == "assistant")
                .unwrap();
            assert_eq!(answer.content, "ok");
            succeeded[side] += 1;
            for model in models(&fake) {
                let price = match model.as_str() {
                    "fake/light" => 0.2,
                    "fake/c" => 0.1,
                    "fake/m" => 1.0,
                    _ => panic!("unexpected model"),
                };
                costs[side] += 15.0 * price / 1_000_000.0;
                call_counts[side] += 1;
            }
        }
    }
    let saving = 1.0 - costs[1] / costs[0];
    eprintln!(
        "fake tier eval: baseline={} calls, ${:.6}, {}/20; enabled={} calls, ${:.6}, {}/20; assumed cost reduction {:.2}%",
        call_counts[0],
        costs[0],
        succeeded[0],
        call_counts[1],
        costs[1],
        succeeded[1],
        saving * 100.0
    );
    assert_eq!(
        succeeded,
        [20, 20],
        "zero fixture success regressions allowed"
    );
    assert!(
        saving >= 0.20,
        "assumed cost reduction must be at least 20%"
    );
}

//! Mail identities: several bots per account, a default per bot, draft-only
//! sending and the sender line of the approval card.

use std::sync::Arc;

use secret_store::{MemoryStore, SecretStore};
use serde_json::{Value, json};
use tool_registry::{ContentPart, Tool, ToolContext};

use crate::testutil::{PASSWORD, Protocol, account, server};
use crate::{AccountConfig, EmailConnector, SendMode, SmtpSecurity, tools};

const BOT_A: &str = "11111111-1111-4111-8111-111111111111";
const BOT_B: &str = "22222222-2222-4222-8222-222222222222";
const BOT_C: &str = "33333333-3333-4333-8333-333333333333";

fn named(base: &AccountConfig, name: &str, bot: Option<&str>) -> AccountConfig {
    let mut a = base.clone();
    a.name = name.into();
    a.address = format!("{name}@example.com");
    a.bot = bot.map(str::to_owned);
    a
}

fn tool(conn: &Arc<EmailConnector>, name: &str) -> Arc<dyn Tool> {
    tools(conn)
        .into_iter()
        .find(|t| t.spec().name == name)
        .unwrap()
}

fn ctx(bot: &str) -> ToolContext {
    ToolContext {
        bot_id: Some(bot.to_owned()),
        ..ToolContext::default()
    }
}

async fn call(conn: &Arc<EmailConnector>, bot: &str, name: &str, input: Value) -> Value {
    let out = tool(conn, name).call(&ctx(bot), input).await.unwrap();
    let ContentPart::Json { data } = &out.content[0] else {
        panic!("expected JSON")
    };
    data.clone()
}

fn connector(list: Vec<AccountConfig>) -> Arc<EmailConnector> {
    let store = Arc::new(MemoryStore::new());
    store.set("email/test", PASSWORD).unwrap();
    Arc::new(EmailConnector::new(list, store).unwrap())
}

#[tokio::test]
async fn shared_account_is_usable_by_the_listed_bots_and_the_default_wins() {
    let imap = server(Protocol::Imap).await;
    let smtp = server(Protocol::Smtp).await;
    let base = account(&imap, &smtp, SmtpSecurity::Implicit);
    let mut work = named(&base, "work", Some(BOT_A));
    work.identity.shared_with = vec![BOT_B.into()];
    let mut own = named(&base, "own", Some(BOT_B));
    own.identity.default_for = vec![BOT_B.into()];
    let conn = connector(vec![work, own]);

    // Bot B owns "own" and shares "work": both are listed, its default is marked.
    let ids = call(&conn, BOT_B, "email_identities", json!({})).await;
    let list = ids["identities"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let default = list.iter().find(|i| i["default"] == true).unwrap();
    assert_eq!(default["account"], "own");
    assert_eq!(default["address"], "own@example.com");
    assert!(conn.scoped(Some(BOT_A)).resolve_name(None).is_ok());
    // A bot that was not listed neither sees nor reaches the account.
    assert!(conn.scoped(Some(BOT_C)).resolve_name(Some("work")).is_err());
    assert!(matches!(
        conn.scoped(Some(BOT_C)).identities().as_slice(),
        []
    ));
}

#[tokio::test]
async fn draft_only_identity_saves_a_draft_and_never_sends() {
    let imap = server(Protocol::Imap).await;
    let smtp = server(Protocol::Smtp).await;
    let base = account(&imap, &smtp, SmtpSecurity::Implicit);
    let mut a = named(&base, "test", Some(BOT_A));
    a.identity.send_mode = SendMode::DraftOnly;
    let conn = connector(vec![a]);

    let out = call(
        &conn,
        BOT_A,
        "email_send",
        json!({"to":["bob@example.com"],"subject":"Hi","body":"text"}),
    )
    .await;
    assert_eq!(out["sent"], false);
    assert_eq!(out["saved_as_draft"], true);
    assert_eq!(out["from"], "test@example.com");
    assert!(smtp.messages.lock().unwrap().is_empty());
    assert_eq!(imap.messages.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn draft_only_identity_refuses_every_send_path_in_the_connector() {
    let imap = server(Protocol::Imap).await;
    let smtp = server(Protocol::Smtp).await;
    let base = account(&imap, &smtp, SmtpSecurity::Implicit);
    let mut a = named(&base, "test", Some(BOT_A));
    a.identity.send_mode = SendMode::DraftOnly;
    let conn = connector(vec![a]);
    let mail = crate::smtp::OutgoingMail {
        to: vec!["bob@example.com".into()],
        subject: "Hi".into(),
        body: "text".into(),
        ..crate::smtp::OutgoingMail::default()
    };
    let scoped = conn.scoped(Some(BOT_A));
    for r in [
        scoped.send(None, &mail).await.map(|_| ()),
        scoped.send_receipt(Some("test"), &mail).await.map(|_| ()),
        scoped.reply(None, "1", &mail, false).await.map(|_| ()),
    ] {
        assert!(
            matches!(&r, Err(crate::EmailError::Input(m)) if m == "draft only"),
            "{r:?}"
        );
    }
    assert!(smtp.messages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn another_bot_cannot_name_an_identity_and_shared_with_on_an_unbound_account_is_inert() {
    let imap = server(Protocol::Imap).await;
    let smtp = server(Protocol::Smtp).await;
    let base = account(&imap, &smtp, SmtpSecurity::Implicit);
    let a_only = named(&base, "a-only", Some(BOT_A));
    let mut open = named(&base, "open", None);
    open.identity.shared_with = vec![BOT_B.into()];
    let conn = connector(vec![a_only, open]);
    // Bot B names bot A's account: same error as a missing one.
    let hidden = conn
        .scoped(Some(BOT_B))
        .resolve_name(Some("a-only"))
        .unwrap_err();
    let missing = conn
        .scoped(Some(BOT_B))
        .resolve_name(Some("nope"))
        .unwrap_err();
    assert_eq!(
        hidden.to_string().replace("a-only", "x"),
        missing.to_string().replace("nope", "x")
    );
    // An unbound account is open to everybody; listing a bot in shared_with
    // neither narrows it to that bot nor adds anything for it.
    for bot in [BOT_B, BOT_C] {
        let s = conn.scoped(Some(bot));
        assert_eq!(s.resolve_name(Some("open")).unwrap(), "open");
        assert_eq!(s.identities().len(), 1);
    }
}

#[tokio::test]
async fn approval_card_always_names_the_sender() {
    let imap = server(Protocol::Imap).await;
    let smtp = server(Protocol::Smtp).await;
    let base = account(&imap, &smtp, SmtpSecurity::Implicit);
    let mut a = named(&base, "test", Some(BOT_A));
    a.identity.send_mode = SendMode::DraftOnly;
    let conn = connector(vec![a]);
    let send = tool(&conn, "email_send");

    let line = send
        .egress_extra(&ctx(BOT_A), &json!({"to":["b@example.com"]}))
        .unwrap();
    assert!(line.contains("from: test@example.com (account: test) (default)"));
    assert!(line.contains("draft only"));
    // An explicit account shows too, without the "(default)" note.
    let line = send
        .egress_extra(&ctx(BOT_A), &json!({"account":"test"}))
        .unwrap();
    assert!(line.contains("from: test@example.com (account: test)"));
    assert!(!line.contains("(default)"));
}

#[test]
fn identity_settings_round_trip_and_stay_out_of_old_files() {
    let imap_less = r#"
name = "w"
address = "w@example.com"
password = "email/w"
[imap]
host = "imap.example.com"
[smtp]
host = "smtp.example.com"
[identity]
shared_with = ["22222222-2222-4222-8222-222222222222"]
send_mode = "draft_only"
"#;
    let a: AccountConfig = toml::from_str(imap_less).unwrap();
    assert_eq!(a.identity.send_mode, SendMode::DraftOnly);
    assert_eq!(a.identity.shared_with.len(), 1);
    a.validate().unwrap();
    let mut bad = a.clone();
    bad.identity.default_for = vec!["not a uuid!".into()];
    assert!(bad.validate().is_err());
    let mut plain = a;
    plain.identity = crate::IdentitySettings::default();
    assert!(!toml::to_string(&plain).unwrap().contains("identity"));
}

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tool_registry::{ContentPart, Tool, ToolContext};

use super::*;
use crate::browser::Wait;

/// Local "pages": the snapshot text a driver shows.
const RECAPTCHA_PAGE: &str = "# Sign up (https://shop.example/signup)\n  [e1] textbox \"Email\"\n  iframe \"reCAPTCHA\"\n    [e2] checkbox \"I'm not a robot\"\n  [e3] button \"Create account\"";
const HOLD_PAGE: &str = "# Just a moment (https://shop.example/x)\n  text \"Verify you are human\"\n  [e4] button \"Press & hold\"";
const TILES_PAGE: &str = "# Check (https://shop.example/x)\n  text \"hCaptcha\"\n  text \"Select all images with a bus\"";
const CODE_PAGE: &str = "# Verify (https://bank.example/2fa)\n  text \"We sent a code to your phone\"\n  [e7] textbox \"Verification code\"\n  [e8] button \"Verify\"";
const CODE_PAGE_TR: &str =
    "# Doğrulama (https://bank.example/2fa)\n  [e1] textbox \"Doğrulama kodu\"";
const CODE_PAGE_DE: &str =
    "# Prüfung (https://bank.example/2fa)\n  [e1] textbox \"Bestätigungscode\"";
const PROSE_ABOUT_CAPTCHA: &str = "# Blog (https://blog.example/p)\n  heading \"Why CAPTCHA tests are annoying\"\n  text \"A captcha is a test\"";
const COUPON_PAGE: &str = "# Cart (https://shop.example/cart)\n  [e1] textbox \"Discount code\"\n  [e2] textbox \"Postal code\"\n  [e3] button \"Apply\"";
const LOGIN_PAGE: &str = "# Sign in (https://bank.example/login)\n  [e1] textbox \"Email\"\n  [e2] textbox \"Password\"\n  [e3] button \"Sign in\"";

struct Page {
    snapshot: StdMutex<String>,
    url: StdMutex<Option<String>>,
    typed: StdMutex<Vec<(String, String)>>,
    fail_with_code: bool,
    stop_on_snapshot: StdMutex<Option<String>>,
}

impl Page {
    fn new(snapshot: &str, url: &str) -> Arc<Self> {
        Arc::new(Self {
            snapshot: StdMutex::new(snapshot.to_owned()),
            url: StdMutex::new(Some(url.to_owned())),
            typed: StdMutex::new(Vec::new()),
            fail_with_code: false,
            stop_on_snapshot: StdMutex::new(None),
        })
    }
    fn show(&self, snapshot: &str, url: &str) {
        *self.snapshot.lock().unwrap() = snapshot.to_owned();
        *self.url.lock().unwrap() = Some(url.to_owned());
    }
}

#[async_trait]
impl BrowserDriver for Page {
    async fn open(&self, _u: &str, _p: Option<&str>) -> Result<String, String> {
        Ok(self.snapshot.lock().unwrap().clone())
    }
    async fn snapshot(&self, _i: bool) -> Result<String, String> {
        if let Some(run) = self.stop_on_snapshot.lock().unwrap().take() {
            handover_hub().forget(&run);
        }
        Ok(self.snapshot.lock().unwrap().clone())
    }
    async fn click(&self, _r: &str) -> Result<String, String> {
        Ok(String::new())
    }
    async fn fill(&self, _r: &str, _t: &str) -> Result<String, String> {
        Ok(String::new())
    }
    async fn fill_secret(
        &self,
        r: &str,
        text: &str,
        accept: &(dyn Fn(Option<String>) -> bool + Send + Sync),
    ) -> Result<String, String> {
        let origin = self.url.lock().unwrap().clone();
        if !accept(origin) {
            return Err("refused".into());
        }
        if self.fail_with_code {
            return Err(format!("the page said no to {text}"));
        }
        self.typed
            .lock()
            .unwrap()
            .push((r.to_owned(), text.to_owned()));
        Ok(format!("filled {r}"))
    }
    async fn select(&self, _r: &str, _v: &str) -> Result<String, String> {
        Ok(String::new())
    }
    async fn press(&self, _k: &str) -> Result<String, String> {
        Ok(String::new())
    }
    async fn scroll(&self, _r: Option<&str>, _x: f64, _y: f64) -> Result<String, String> {
        Ok(String::new())
    }
    async fn wait(&self, _c: Wait, _t: Option<Duration>) -> Result<String, String> {
        Ok(String::new())
    }
    async fn back(&self) -> Result<String, String> {
        Ok(String::new())
    }
    async fn tabs(&self, _s: Option<usize>) -> Result<String, String> {
        Ok(String::new())
    }
    async fn screenshot(
        &self,
        _f: bool,
        _e: Option<&str>,
    ) -> Result<(Vec<u8>, &'static str), String> {
        Ok((vec![], "png"))
    }
    async fn eval(&self, _js: &str) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn current_url(&self) -> Option<String> {
        self.url.lock().unwrap().clone()
    }
}

fn tool(page: &Arc<Page>, name: &str) -> Arc<dyn Tool> {
    let dir = tempfile::tempdir().unwrap();
    crate::browser::tools(page.clone(), dir.path(), None)
        .into_iter()
        .find(|t| t.spec().name == name)
        .unwrap()
}

fn ctx(run: &str) -> ToolContext {
    ToolContext {
        run_id: run.into(),
        ..ToolContext::default()
    }
}

#[test]
fn detects_captchas_codes_and_ignores_look_alikes() {
    assert_eq!(
        detect(RECAPTCHA_PAGE),
        Some(Challenge::Captcha { hard: false })
    );
    assert_eq!(detect(HOLD_PAGE), Some(Challenge::Captcha { hard: true }));
    assert_eq!(detect(TILES_PAGE), Some(Challenge::Captcha { hard: true }));
    for page in [CODE_PAGE, CODE_PAGE_TR, CODE_PAGE_DE] {
        assert!(
            matches!(detect(page), Some(Challenge::Code { field: Some(_) })),
            "{page}"
        );
    }
    assert_eq!(
        detect(CODE_PAGE),
        Some(Challenge::Code {
            field: Some("e7".into())
        })
    );
    // An article about CAPTCHAs, a coupon box, a plain login: nothing to hand over.
    for page in [PROSE_ABOUT_CAPTCHA, COUPON_PAGE, LOGIN_PAGE] {
        assert_eq!(detect(page), None, "{page}");
    }
}

#[test]
fn own_captcha_attempts_are_capped_at_two_then_the_notice_says_hand_over() {
    let hub = HandoverHub::default();
    let c = Challenge::Captcha { hard: false };
    let seen: Vec<u32> = (0..3).map(|_| hub.note_captcha("run-cap")).collect();
    assert_eq!(seen, [1, 2, 3]);
    assert!(notice(&c, 1).contains("attempt 1 of 2"));
    assert!(notice(&c, 2).contains("attempt 2 of 2"));
    let third = notice(&c, 3);
    assert!(
        third.contains("must not try") && third.contains("browser_handover"),
        "{third}"
    );
    assert!(notice(&Challenge::Captcha { hard: true }, 1).contains("must not try"));
    hub.forget("run-cap");
    assert_eq!(hub.note_captcha("run-cap"), 1);
}

#[tokio::test]
async fn snapshot_ends_with_a_daemon_notice_only_when_a_challenge_is_there() {
    let page = Page::new(RECAPTCHA_PAGE, "https://shop.example/signup");
    let snap = tool(&page, "browser_snapshot");
    let out = snap.call(&ctx("run-notice"), json!({})).await.unwrap();
    let ContentPart::Text { text } = &out.content[0] else {
        panic!()
    };
    assert!(
        text.contains("[Praxi notice]") && text.contains("attempt 1 of 2"),
        "{text}"
    );
    page.show(LOGIN_PAGE, "https://bank.example/login");
    let out = snap.call(&ctx("run-notice"), json!({})).await.unwrap();
    let ContentPart::Text { text } = &out.content[0] else {
        panic!()
    };
    assert!(!text.contains("[Praxi notice]"), "{text}");
    handover_hub().forget("run-notice");
}

#[tokio::test]
async fn the_tool_checks_the_page_itself_and_names_the_site_the_browser_reports() {
    let page = Page::new(LOGIN_PAGE, "https://bank.example/login");
    let t = tool(&page, "browser_handover");
    // The model claims a code prompt where there is none: refused, nothing armed.
    let err = t
        .call(&ctx("run-h1"), json!({"kind":"code"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("does not show"), "{err}");
    assert_eq!(
        handover_hub().type_code("run-h1", "123456").await,
        Err(HandoverError::NotArmed)
    );
    page.show(CODE_PAGE, "https://bank.example/2fa");
    let out = t
        .call(&ctx("run-h1"), json!({"kind":"code"}))
        .await
        .unwrap();
    let ContentPart::Json { data } = &out.content[0] else {
        panic!("card part first")
    };
    let card = HandoverCard::from_part(data).unwrap();
    assert_eq!(card.kind, HandoverKind::Code);
    assert_eq!(card.site, "https://bank.example:443");
    handover_hub().forget("run-h1");
}

#[tokio::test]
async fn a_code_is_typed_into_the_page_it_was_asked_for_and_never_elsewhere() {
    let page = Page::new(CODE_PAGE, "https://bank.example/2fa");
    let t = tool(&page, "browser_handover");
    t.call(&ctx("run-h2"), json!({"kind":"code"}))
        .await
        .unwrap();
    let hub = handover_hub();
    // Not a code: refused before anything is typed.
    assert_eq!(
        hub.type_code("run-h2", "12").await,
        Err(HandoverError::BadCode)
    );
    assert_eq!(
        hub.type_code("run-h2", "12 34 <b>").await,
        Err(HandoverError::BadCode)
    );
    // The page moved to another site: nothing is typed there.
    page.show(CODE_PAGE, "https://evil.example/2fa");
    assert_eq!(
        hub.type_code("run-h2", "123456").await,
        Err(HandoverError::WrongPage)
    );
    // Same site but no code prompt any more.
    page.show(LOGIN_PAGE, "https://bank.example/home");
    assert_eq!(
        hub.type_code("run-h2", "123456").await,
        Err(HandoverError::NoPrompt)
    );
    assert!(page.typed.lock().unwrap().is_empty());
    // Back on the prompt: typed, with separators removed.
    page.show(CODE_PAGE, "https://bank.example/2fa");
    hub.type_code("run-h2", "123 456").await.unwrap();
    assert_eq!(
        page.typed.lock().unwrap().as_slice(),
        [("e7".to_owned(), "123456".to_owned())]
    );
    hub.forget("run-h2");
}

#[tokio::test]
async fn a_failing_page_never_echoes_the_code_back() {
    let page = Arc::new(Page {
        fail_with_code: true,
        ..Arc::try_unwrap(Page::new(CODE_PAGE, "https://bank.example/2fa"))
            .ok()
            .unwrap()
    });
    let t = tool(&page, "browser_handover");
    t.call(&ctx("run-h3"), json!({"kind":"code"}))
        .await
        .unwrap();
    let err = handover_hub()
        .type_code("run-h3", "987654")
        .await
        .unwrap_err();
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains("987654"), "{shown}");
    handover_hub().forget("run-h3");
}

#[tokio::test]
async fn a_captcha_handover_finishes_only_when_the_check_is_gone() {
    let page = Page::new(HOLD_PAGE, "https://shop.example/x");
    let t = tool(&page, "browser_handover");
    t.call(&ctx("run-h4"), json!({"kind":"captcha"}))
        .await
        .unwrap();
    let hub = handover_hub();
    assert_eq!(
        hub.verify_solved("run-h4").await,
        Err(HandoverError::StillThere)
    );
    page.show(LOGIN_PAGE, "https://shop.example/account");
    hub.verify_solved("run-h4").await.unwrap();
    // A captcha request cannot be used to type a code.
    assert_eq!(
        hub.type_code("run-h4", "123456").await,
        Err(HandoverError::NotArmed)
    );
    hub.forget("run-h4");
}

#[tokio::test]
async fn regression_third_captcha_action_refused_and_new_challenge_resets() {
    let page = Page::new(RECAPTCHA_PAGE, "https://shop.example/signup");
    let click = tool(&page, "browser_click");
    let context = ctx("regression-cap");
    for _ in 0..2 {
        click.call(&context, json!({"ref":"e2"})).await.unwrap();
    }
    // Refreshing/navigating while the challenge remains cannot restore tries.
    page.show(RECAPTCHA_PAGE, "https://shop.example/signup?retry=1");
    let err = click.call(&context, json!({"ref":"e2"})).await.unwrap_err();
    assert!(err.to_string().contains("browser_handover"));
    assert!(
        tool(&page, "browser_open")
            .call(&context, json!({"url":"https://shop.example/next"}))
            .await
            .is_err(),
        "navigation cannot bypass an exhausted challenge budget"
    );
    handover_hub().disarm(&context.run_id);
    assert!(
        click.call(&context, json!({"ref":"e2"})).await.is_err(),
        "cancelling a card cannot restore attempts"
    );
    page.show(LOGIN_PAGE, "https://shop.example/account");
    tool(&page, "browser_snapshot")
        .call(&context, json!({}))
        .await
        .unwrap();
    page.show(RECAPTCHA_PAGE, "https://shop.example/signup");
    click.call(&context, json!({"ref":"e2"})).await.unwrap();
    handover_hub().forget(&context.run_id);
}

#[tokio::test]
async fn regression_stop_during_code_preparation_types_nothing() {
    let page = Page::new(CODE_PAGE, "https://bank.example/2fa");
    tool(&page, "browser_handover")
        .call(&ctx("regression-stop"), json!({"kind":"code"}))
        .await
        .unwrap();
    *page.stop_on_snapshot.lock().unwrap() = Some("regression-stop".into());
    assert_eq!(
        handover_hub().type_code("regression-stop", "123456").await,
        Err(HandoverError::NotArmed)
    );
    assert!(page.typed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn regression_done_rejects_blank_foreign_and_unrendered_pages() {
    let page = Page::new(RECAPTCHA_PAGE, "https://shop.example/signup");
    tool(&page, "browser_handover")
        .call(&ctx("regression-done"), json!({"kind":"captcha"}))
        .await
        .unwrap();
    for (snapshot, url) in [
        ("", "https://shop.example/signup"),
        (LOGIN_PAGE, "https://evil.example/account"),
        (
            "# Loading\n  iframe \"verification\"",
            "https://shop.example/signup",
        ),
    ] {
        page.show(snapshot, url);
        assert!(
            handover_hub()
                .verify_solved("regression-done")
                .await
                .is_err(),
            "accepted {url}: {snapshot}"
        );
    }
    handover_hub().forget("regression-done");
}

#[tokio::test]
async fn regression_screenshot_refused_after_code_is_typed() {
    let page = Page::new(CODE_PAGE, "https://bank.example/2fa");
    let context = ctx("regression-capture");
    tool(&page, "browser_handover")
        .call(&context, json!({"kind":"code"}))
        .await
        .unwrap();
    handover_hub()
        .type_code(&context.run_id, "123456")
        .await
        .unwrap();
    assert!(
        tool(&page, "browser_screenshot")
            .call(&context, json!({}))
            .await
            .is_err()
    );
    handover_hub().forget(&context.run_id);
}

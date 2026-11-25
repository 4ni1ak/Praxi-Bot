//! `browser_open` of the page that is already open must not reload it, and the consent tool.

use super::*;
use parking_lot::Mutex as PMutex;
use tool_registry::ContentPart;

/// A page with one text field: a reload empties it.
#[derive(Default)]
struct TypedPage {
    url: PMutex<Option<String>>,
    field: PMutex<String>,
    opens: PMutex<Vec<String>>,
    consents: PMutex<usize>,
    /// `(asked, landed)`: opening `asked` ends on `landed`.
    redirect: PMutex<Option<(String, String)>>,
}

#[async_trait]
impl BrowserDriver for TypedPage {
    async fn open(&self, url: &str, _profile: Option<&str>) -> Result<String, String> {
        self.opens.lock().push(url.to_owned());
        let landed = match &*self.redirect.lock() {
            Some((from, to)) if from == url => to.clone(),
            _ => url.to_owned(),
        };
        *self.url.lock() = Some(landed.clone());
        self.field.lock().clear();
        Ok(format!("# {landed} field=[]"))
    }
    async fn snapshot(&self, _interactive_only: bool) -> Result<String, String> {
        Ok(format!(
            "# {} field=[{}]",
            self.url.lock().clone().unwrap_or_default(),
            self.field.lock()
        ))
    }
    async fn click(&self, _r: &str) -> Result<String, String> {
        Ok("clicked".into())
    }
    async fn fill(&self, _r: &str, text: &str) -> Result<String, String> {
        *self.field.lock() = text.to_owned();
        Ok("typed".into())
    }
    async fn select(&self, _r: &str, _value: &str) -> Result<String, String> {
        Ok(String::new())
    }
    async fn press(&self, _key: &str) -> Result<String, String> {
        Ok(String::new())
    }
    async fn scroll(&self, _r: Option<&str>, _dx: f64, _dy: f64) -> Result<String, String> {
        Ok(String::new())
    }
    async fn wait(&self, _cond: Wait, _timeout: Option<Duration>) -> Result<String, String> {
        Ok(String::new())
    }
    async fn back(&self) -> Result<String, String> {
        Ok(String::new())
    }
    async fn tabs(&self, _switch_to: Option<usize>) -> Result<String, String> {
        Ok(String::new())
    }
    async fn screenshot(
        &self,
        _full_page: bool,
        _element: Option<&str>,
    ) -> Result<(Vec<u8>, &'static str), String> {
        Ok((Vec::new(), "png"))
    }
    async fn eval(&self, _js: &str) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn current_url(&self) -> Option<String> {
        self.url.lock().clone()
    }
    async fn dismiss_consent(&self) -> Result<String, String> {
        *self.consents.lock() += 1;
        Ok("Consent dialog dismissed: clicked \"reject all\"".into())
    }
}

fn setup() -> (tempfile::TempDir, Arc<TypedPage>, Vec<Arc<dyn Tool>>) {
    let dir = tempfile::tempdir().unwrap();
    let page = Arc::new(TypedPage::default());
    let all = tools(page.clone(), dir.path(), None);
    (dir, page, all)
}

fn get(all: &[Arc<dyn Tool>], name: &str) -> Arc<dyn Tool> {
    all.iter().find(|t| t.spec().name == name).unwrap().clone()
}

async fn open(all: &[Arc<dyn Tool>], input: Value) -> String {
    let out = get(all, "browser_open")
        .call(&ToolContext::default(), input)
        .await
        .unwrap();
    match &out.content[0] {
        ContentPart::Text { text } => text.clone(),
        other => panic!("{other:?}"),
    }
}

const SIGNUP: &str = "https://www.example.com/signup";

#[tokio::test]
async fn reopening_the_open_page_keeps_what_was_typed() {
    let (_dir, page, all) = setup();
    open(&all, json!({"url": format!("{SIGNUP}?utm_source=ad")})).await;
    page.fill("e1", "me@agentmail.to").await.unwrap();
    // The same page spelled differently: host case, default port, trailing slash, fragment,
    // tracking parameter.
    for again in [
        SIGNUP.to_owned(),
        "https://WWW.Example.com:443/signup/".to_owned(),
        format!("{SIGNUP}#email"),
        format!("{SIGNUP}?gclid=1&utm_medium=x"),
    ] {
        let text = open(&all, json!({"url": again})).await;
        assert!(text.contains("already open (not reloaded)"), "{text}");
        assert!(text.contains("field=[me@agentmail.to]"), "{text}");
    }
    assert_eq!(page.opens.lock().len(), 1, "never navigated again");
}

#[tokio::test]
async fn reload_true_and_other_pages_do_navigate() {
    let (_dir, page, all) = setup();
    open(&all, json!({"url": SIGNUP})).await;
    page.fill("e1", "typed").await.unwrap();
    let text = open(&all, json!({"url": SIGNUP, "reload": true})).await;
    assert!(!text.contains("already open"), "{text}");
    assert_eq!(page.opens.lock().len(), 2);
    assert_eq!(*page.field.lock(), "", "a reload empties the field");
    // Another path, another query value, another host: all different pages.
    for other in [
        "https://www.example.com/login",
        "https://www.example.com/login?next=a",
        "https://www.example.com/login?next=b",
        "https://app.example.com/login?next=b",
    ] {
        open(&all, json!({"url": other})).await;
    }
    assert_eq!(page.opens.lock().len(), 6);
}

#[tokio::test]
async fn a_redirected_page_is_still_the_one_asked_for() {
    let (_dir, page, all) = setup();
    let step = "https://www.example.com/signup/step-1";
    *page.redirect.lock() = Some((SIGNUP.to_owned(), step.to_owned()));
    open(&all, json!({"url": SIGNUP})).await;
    page.fill("e1", "typed").await.unwrap();
    let text = open(&all, json!({"url": SIGNUP})).await;
    assert!(text.contains("already open (not reloaded)"), "{text}");
    assert_eq!(page.opens.lock().len(), 1);
    // Once the page went somewhere else, asking for it again is a real navigation.
    *page.url.lock() = Some("https://www.example.com/welcome".into());
    open(&all, json!({"url": SIGNUP})).await;
    assert_eq!(page.opens.lock().len(), 2);
}

#[test]
fn same_page_key_ignores_only_what_does_not_name_another_page() {
    let key = |u: &str| same_page_key(u).unwrap();
    assert_eq!(
        key("HTTPS://Example.COM:443/a/b/?utm_x=1#f"),
        key("https://example.com/a/b")
    );
    assert_eq!(key("http://example.com:80/"), key("http://example.com"));
    assert_ne!(
        key("https://example.com/a?id=1"),
        key("https://example.com/a?id=2")
    );
    assert_ne!(key("https://example.com/a"), key("http://example.com/a"));
    assert!(same_page_key("about:blank").is_none());
    assert!(same_page_key("javascript:alert(1)").is_none());
}

#[tokio::test]
async fn the_consent_tool_calls_the_driver_and_asks_like_a_click() {
    let (_dir, page, all) = setup();
    let tool = get(&all, "browser_dismiss_consent");
    assert_eq!(tool.spec().effect, Effect::Write);
    assert_eq!(tool.spec().category, PermissionCategory::Browser);
    let out = tool.call(&ToolContext::default(), json!({})).await.unwrap();
    let ContentPart::Text { text } = &out.content[0] else {
        panic!()
    };
    assert!(text.contains("reject all"), "{text}");
    assert_eq!(*page.consents.lock(), 1);
}

#[tokio::test]
async fn a_backend_without_the_helper_says_so_instead_of_clicking() {
    let dir = tempfile::tempdir().unwrap();
    let all = tools(Arc::new(fake::FakeBrowser::default()), dir.path(), None);
    let err = get(&all, "browser_dismiss_consent")
        .call(&ToolContext::default(), json!({}))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("cannot dismiss a consent dialog"),
        "{err}"
    );
}

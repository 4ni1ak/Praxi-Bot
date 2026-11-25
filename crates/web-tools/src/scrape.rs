//! `web_scrape`: the client of the optional "web scraping add-on" (see `packs/scrape/` and
//! `docs/guide/web-scraping.md`).
//!
//! The add-on is a small separate program on the same computer that reads pages for us (robots.txt
//! respected, polite pacing, its own address checks). This file only talks to it over
//! `127.0.0.1` and treats everything it says as untrusted:
//!
//! * the daemon may reach exactly ONE loopback `host:port` (the add-on's), through the same
//!   [`TrustedHostPolicy`] pattern the local `SearXNG` uses; nothing else on loopback opens up;
//! * every call carries the per-install token (`X-Praxi-Token`) read from a root-owned file that
//!   only the service user can read, so other local programs cannot use the add-on through us;
//! * the page text is wrapped as untrusted external content exactly like `web_extract`, and the
//!   tool has the same permission class and egress rules (`Browser`, `Effect::Write`,
//!   `ContentClass::untrusted_egress()`);
//! * the address the add-on reports as `final_url` is checked again with the daemon's own policy.
//!
//! No CAPTCHA solving, no proxy rotation, no stealth, no login: the add-on does none of it.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tool_registry::{
    ContentClass, Effect, PermissionCategory, Platform, Tool, ToolContext, ToolError, ToolOutput,
    ToolSpec,
};
use url::Url;

use crate::error::WebError;
use crate::http::{GetOptions, HttpConfig, HttpFetcher, JSON_TYPES};
use crate::policy::{TrustedHostPolicy, UrlPolicy};
use crate::tools::{MAX_URL_CHARS, check_len, finish_text, notice, parse};

/// Tool name. Not part of [`crate::TOOL_NAMES`]: it exists only when the add-on is installed.
pub const SCRAPE_TOOL: &str = "web_scrape";
/// Where the add-on listens (fixed: `127.0.0.1`, this port).
pub const DEFAULT_SCRAPE_URL: &str = "http://127.0.0.1:47811";
/// Where the install step puts the per-install token (root-owned, readable by the service user).
pub const DEFAULT_TOKEN_FILE: &str = "/etc/praxi/scrape.token";

/// Characters returned by default.
pub const DEFAULT_MAX_CHARS: usize = 20_000;
/// Hard ceiling on returned characters.
pub const HARD_MAX_CHARS: usize = 100_000;
/// Most pages one call may read.
pub const MAX_PAGES: usize = 10;
/// Calls per minute this client allows itself (the add-on paces per site on top of that).
pub const CALLS_PER_MINUTE: usize = 12;
const NOT_INSTALLED: &str = "The web scraping add-on is not installed on this computer. The owner can add it in Settings > Computers.";
/// Reply size cap: 100k characters of text plus links fit in 1 MiB.
const MAX_REPLY_BYTES: usize = 1024 * 1024;

/// Where the add-on is and how to prove who we are.
#[derive(Clone, Debug)]
pub struct ScrapeConfig {
    /// `http://127.0.0.1:<port>`.
    pub base_url: String,
    /// File holding the per-install token.
    pub token_file: PathBuf,
}

impl Default for ScrapeConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_SCRAPE_URL.into(),
            token_file: PathBuf::from(DEFAULT_TOKEN_FILE),
        }
    }
}

impl ScrapeConfig {
    /// The add-on is installed on this computer: its token file exists. (Health is checked on
    /// every call; a stopped add-on gives a plain "not running" answer.)
    #[must_use]
    pub fn installed(&self) -> bool {
        self.token_file.is_file()
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Reply {
    status: u16,
    final_url: String,
    title: String,
    markdown: String,
    links: Vec<ReplyLink>,
    truncated: bool,
    robots: String,
    fetched_at: String,
    pages: u32,
    error: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyLink {
    text: String,
    url: String,
}

#[derive(Deserialize)]
struct ScrapeIn {
    url: String,
    mode: Option<String>,
    max_chars: Option<usize>,
    max_pages: Option<usize>,
}

pub(crate) struct ScrapeTool {
    spec: ToolSpec,
    cfg: ScrapeConfig,
    fetcher: HttpFetcher,
    /// The daemon's public-web policy, used to re-check the address the add-on reports.
    public: Arc<dyn UrlPolicy>,
    spill_dir: Option<PathBuf>,
    recent: Mutex<VecDeque<Instant>>,
}

/// Build the tool.
///
/// # Errors
/// [`WebError::Config`] for a bad add-on URL (it must be `http://127.0.0.1:<port>`) or when the
/// HTTP client cannot be built.
pub(crate) fn tool(
    cfg: &ScrapeConfig,
    public: &Arc<dyn UrlPolicy>,
    http: &HttpConfig,
    spill_dir: Option<PathBuf>,
) -> Result<Arc<dyn Tool>, WebError> {
    let base =
        Url::parse(&cfg.base_url).map_err(|e| WebError::Config(format!("scrape url: {e}")))?;
    let local = matches!(
        base.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    let port = base.port_or_known_default();
    let (true, Some(port)) = (local && base.scheme() == "http", port) else {
        return Err(WebError::Config(
            "the web scraping add-on must be on http://127.0.0.1:<port>".into(),
        ));
    };
    let host = base.host_str().unwrap_or("127.0.0.1").to_string();
    let policy: Arc<dyn UrlPolicy> =
        Arc::new(TrustedHostPolicy::new(Arc::clone(public), &host, port));
    let mut limits = http.clone();
    // The add-on paces itself and may crawl a few pages: allow it a longer window than a plain GET.
    limits.timeout = Duration::from_secs(70);
    let fetcher = HttpFetcher::new(policy, limits)?;
    Ok(Arc::new(ScrapeTool {
        spec: ToolSpec {
            name: SCRAPE_TOOL.into(),
            description: "Read a web page through the optional scraping add-on. Use the built-in web_extract first; use web_scrape only when that is not enough: cleaner main-article text, a list of a page's links, or a small crawl of up to 10 pages of one site. It respects robots.txt and is rate limited, does not run JavaScript, and never logs in, solves CAPTCHAs or gets past paywalls, so it is not for logged-in pages. mode: article (main text, default), markdown (whole page) or links (only the link list).".into(),
            input_schema: json!({"type":"object","required":["url"],"properties":{
                "url":{"type":"string"},
                "mode":{"type":"string","enum":["article","markdown","links"],"description":"article = main text (default), markdown = whole page, links = only the links"},
                "max_chars":{"type":"integer","minimum":200,"maximum":HARD_MAX_CHARS,"description":"Cut the text at this many characters (default 20000, at most 100000)"},
                "max_pages":{"type":"integer","minimum":1,"maximum":MAX_PAGES,"description":"Read up to this many pages of the same site, following links from the first page (default 1)"}
            }}),
            effect: Effect::Write,
            category: PermissionCategory::Browser,
            platforms: vec![Platform::All],
            content: ContentClass::untrusted_egress(),
        },
        cfg: cfg.clone(),
        fetcher,
        public: Arc::clone(public),
        spill_dir,
        recent: Mutex::new(VecDeque::new()),
    }))
}

fn plain(code: &str) -> String {
    match code {
        "robots_blocked" => "This site's robots.txt asks automated readers not to read this page, so it was not read. Do not look for another way around it; tell the user.",
        "blocked_address" => "That address is not on the public internet, so it was not read.",
        "rate_limited" => "Too many requests to this site. Wait a minute before trying again.",
        "busy" => "The web scraping add-on is busy. Try again in a moment.",
        "timeout" => "The page took too long to load.",
        "too_large" => "The page is too large to read.",
        "bad_content_type" => "That address is a file or media, not a web page.",
        "fetch_failed" => "The page could not be loaded.",
        "bad_request" => "The add-on did not accept that request.",
        _ => "The add-on could not read this page.",
    }
    .into()
}

impl ScrapeTool {
    fn token(&self) -> Result<String, ToolError> {
        match std::fs::read_to_string(&self.cfg.token_file) {
            Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            _ => Err(ToolError::Execution(NOT_INSTALLED.into())),
        }
    }

    /// Client-side pacing: a runaway loop cannot hammer the add-on.
    fn allow_call(&self) -> Result<(), ToolError> {
        let now = Instant::now();
        let mut q = self.recent.lock().unwrap_or_else(|e| e.into_inner());
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(60))
        {
            q.pop_front();
        }
        if q.len() >= CALLS_PER_MINUTE {
            return Err(ToolError::Execution(
                "Too many web scraping calls in the last minute. Wait a little before trying again."
                    .into(),
            ));
        }
        q.push_back(now);
        Ok(())
    }

    async fn run(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let i: ScrapeIn = parse(input)?;
        check_len("url", &i.url, MAX_URL_CHARS)?;
        // The daemon's own public-web check first: the add-on is not the only guard.
        let wanted = Url::parse(i.url.trim())
            .map_err(|e| ToolError::InvalidInput(format!("invalid URL: {e}")))?;
        self.public
            .check_url(&wanted)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let mode = i.mode.as_deref().unwrap_or("article");
        if !matches!(mode, "article" | "markdown" | "links") {
            return Err(ToolError::InvalidInput(
                "mode must be article, markdown or links".into(),
            ));
        }
        let max_chars = i
            .max_chars
            .unwrap_or(DEFAULT_MAX_CHARS)
            .clamp(200, HARD_MAX_CHARS);
        let pages = i.max_pages.unwrap_or(1).clamp(1, MAX_PAGES);
        let token = self.token()?;
        self.allow_call()?;

        let timeout_ms: u64 = if pages > 1 { 60_000 } else { 25_000 };
        let body = json!({
            "url": wanted.as_str(), "mode": mode, "max_chars": max_chars,
            "timeout_ms": timeout_ms, "max_pages": pages,
        });
        let mut opts = GetOptions::accepting(JSON_TYPES);
        opts.headers.push(("X-Praxi-Token".into(), token));
        opts.max_bytes = Some(MAX_REPLY_BYTES);
        opts.timeout = Some(Duration::from_millis(timeout_ms + 5_000));
        let endpoint = format!("{}/v1/extract", self.cfg.base_url.trim_end_matches('/'));
        let fetched = self
            .fetcher
            .post_json(
                &endpoint,
                &serde_json::to_vec(&body).unwrap_or_default(),
                &opts,
            )
            .await
            .map_err(|e| match e {
                WebError::Status(429) => ToolError::Execution(plain("rate_limited")),
                WebError::Status(401 | 403) => ToolError::Execution(
                    "The web scraping add-on did not accept this computer's key. The owner can reinstall it in Settings > Computers.".into(),
                ),
                WebError::Timeout => ToolError::Execution(plain("timeout")),
                _ => ToolError::Execution(
                    "The web scraping add-on is not running. The owner can check it in Settings > Computers.".into(),
                ),
            })?;
        if fetched.truncated {
            return Err(ToolError::Execution(plain("too_large")));
        }
        let reply: Reply = serde_json::from_slice(&fetched.body).map_err(|_| {
            ToolError::Execution(
                "The web scraping add-on sent an answer that could not be understood.".into(),
            )
        })?;
        if let Some(code) = reply.error.as_deref().filter(|c| !c.is_empty()) {
            return Err(ToolError::Execution(plain(code)));
        }
        Ok(self.render(ctx, &reply, mode, max_chars))
    }

    fn render(&self, ctx: &ToolContext, r: &Reply, mode: &str, max_chars: usize) -> ToolOutput {
        // Never show text from an address the daemon itself would have refused.
        let shown = match Url::parse(&r.final_url) {
            Ok(u) if self.public.check_url(&u).is_ok() => u.to_string(),
            _ => {
                return ToolOutput::text(format!(
                    "{}\nThe add-on reported a page address that is not allowed; its text was dropped.",
                    notice("the web scraping add-on")
                ));
            }
        };
        let mut body = r.markdown.clone();
        let mut cut = r.truncated;
        if let Some((idx, _)) = body.char_indices().nth(max_chars) {
            body.truncate(idx);
            cut = true;
        }
        let mut text = format!("{}\nURL: {shown}\n", notice(&shown));
        if !r.title.is_empty() {
            let _ = writeln!(
                text,
                "Title: {}",
                r.title.chars().take(300).collect::<String>()
            );
        }
        let _ = writeln!(text, "Status: {}", r.status);
        if !r.robots.is_empty() {
            let robots: String = r
                .robots
                .chars()
                .filter(char::is_ascii_lowercase)
                .take(12)
                .collect();
            let _ = writeln!(text, "robots.txt: {robots}");
        }
        if !r.fetched_at.is_empty() {
            let at: String = r.fetched_at.chars().take(32).collect();
            let _ = writeln!(text, "Fetched: {at}");
        }
        if r.pages > 1 {
            let _ = writeln!(text, "Pages read: {}", r.pages);
        }
        if cut {
            text.push_str("Note: content truncated by size limits.\n");
        }
        text.push('\n');
        text.push_str(&body);
        if mode == "links" && !r.links.is_empty() {
            text.push_str("## Links\n");
            for l in r.links.iter().take(200) {
                let label = if l.text.is_empty() { &l.url } else { &l.text };
                let _ = writeln!(text, "- [{label}]({})", l.url);
            }
        }
        finish_text(ctx, self.spill_dir.as_deref(), text)
    }
}

#[async_trait]
impl Tool for ScrapeTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn call(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        self.run(ctx, input).await
    }
}

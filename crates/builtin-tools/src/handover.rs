//! `browser_handover`: the bot asks the OWNER for a step it must not do itself.
//!
//! Two kinds, both for the managed browser:
//!
//! * **CAPTCHA** (`kind: "captcha"`): the page shows a "prove you are human"
//!   check. The bot may try a simple one itself, at most [`CAPTCHA_OWN_ATTEMPTS`]
//!   actions per challenge (see [`HandoverHub::captcha_action`]); after that, or for a
//!   behavioural one, it hands over: the owner takes control in the live view,
//!   solves it and presses "Done". The daemon then checks that the check is gone
//!   before the run resumes. No solver service, no evasion plugin.
//! * **One-time code** (`kind: "code"`): the page wants a 2FA code. The owner types
//!   it into a card the app draws; the daemon types it into the page like a secret.
//!   The code never reaches the model, the journal, the audit log or any log, and
//!   is never stored: it lives in one local variable for the time of the typing.
//!
//! The tool itself only validates (the page really shows such a prompt, read by
//! the daemon, not claimed by the model) and returns a card description the agent
//! loop turns into a parked request, exactly like `secret_request`. Detection
//! works on the rendered snapshot text ([`detect`]); a page can fake a prompt, so
//! the card names the site the BROWSER reports and warns when the run has read
//! untrusted content.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::Value;

use crate::browser::BrowserDriver;

/// Name of the model-facing tool.
pub const BROWSER_HANDOVER: &str = "browser_handover";

/// `kind` of the JSON part the tool returns; the loop parks the run on it.
pub const HANDOVER_CARD_KIND: &str = "handover_card";

/// How many actions the bot may attempt on the same CAPTCHA.
pub const CAPTCHA_OWN_ATTEMPTS: u32 = 2;

/// What a handover asks the owner to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoverKind {
    /// Solve a "prove you are human" check in the live view.
    Captcha,
    /// Type a one-time code.
    Code,
}

impl HandoverKind {
    /// The wire name (`captcha` | `code`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Captcha => "captcha",
            Self::Code => "code",
        }
    }
}

/// A validated request, as the loop reads it back from the card part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoverCard {
    /// What the owner is asked to do.
    pub kind: HandoverKind,
    /// Origin the browser reports for the page (never page text).
    pub site: String,
}

impl HandoverCard {
    /// Read the card part [`browser_handover_card`] produced. `None` for any other JSON.
    #[must_use]
    pub fn from_part(data: &Value) -> Option<Self> {
        Some(Self {
            kind: match data.get("handover")?.as_str()? {
                "captcha" => HandoverKind::Captcha,
                "code" => HandoverKind::Code,
                _ => return None,
            },
            site: data.get("site")?.as_str()?.to_owned(),
        })
    }
}

/// The card part for `kind` on `site`.
#[must_use]
pub fn browser_handover_card(kind: HandoverKind, site: &str) -> Value {
    serde_json::json!({
        "kind": HANDOVER_CARD_KIND,
        "handover": kind.as_str(),
        "site": site,
    })
}

// ── Detection ────────────────────────────────────────────────────────────────

/// What a page asks for that the bot must not (or cannot) do alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Challenge {
    /// A CAPTCHA. `hard`: a behavioural or picture-tile one the bot should not try.
    Captcha {
        /// Press-and-hold, tile or slider puzzles: hand over without trying.
        hard: bool,
    },
    /// A one-time code prompt; `field` is the ref of the field that takes it.
    Code {
        /// Ref (`e12`) of the code field.
        field: Option<String>,
    },
}

/// Markers that name a CAPTCHA wherever they appear.
const CAPTCHA_STRONG: &[&str] = &[
    "recaptcha",
    "hcaptcha",
    "turnstile",
    "not a robot",
    "verify you are human",
    "verify you're human",
    "verify that you are human",
    "prove you are human",
    "robot değilim",
    "insan olduğunuzu",
    "ich bin kein roboter",
    "sind sie ein mensch",
    "bestätigen sie, dass sie ein mensch",
];

/// Markers of a puzzle that needs pointer behaviour or picture tiles.
const CAPTCHA_HARD: &[&str] = &[
    "press & hold",
    "press and hold",
    "select all images",
    "select all squares",
    "click each image",
    "click on all",
    "slide to",
    "drag the slider",
    "rotate the",
    "complete the puzzle",
    "basılı tutun",
    "tüm görselleri seçin",
    "gedrückt halten",
    "alle bilder auswählen",
];

/// Words that name a one-time-code field.
const CODE_FIELD_WORDS: &[&str] = &[
    "verification code",
    "security code",
    "authentication code",
    "authenticator code",
    "confirmation code",
    "one-time",
    "one time",
    "otp",
    "2fa",
    "two-factor",
    "6-digit",
    "6 digit",
    "sms code",
    "passcode",
    "doğrulama kodu",
    "güvenlik kodu",
    "onay kodu",
    "tek kullanımlık",
    "bestätigungscode",
    "sicherheitscode",
    "verifizierungscode",
    "einmalcode",
    "einmalpasswort",
];

/// Fields that merely contain the word "code" for another reason.
const CODE_FIELD_NOT: &[&str] = &[
    "coupon",
    "promo",
    "discount",
    "voucher",
    "gift",
    "zip",
    "postal",
    "post code",
    "postcode",
    "country",
    "area code",
    "kupon",
    "indirim",
    "posta kodu",
    "gutschein",
    "rabatt",
    "postleitzahl",
    "password",
    "parola",
    "şifre",
    "passwort",
];

/// Prompt sentences that say a code was sent / is needed.
const CODE_PROMPTS: &[&str] = &[
    "enter the code",
    "enter the 6",
    "enter your code",
    "code we sent",
    "code we've sent",
    "we sent a code",
    "we've sent a code",
    "sent you a code",
    "sent a verification code",
    "verification code",
    "two-step verification",
    "2-step verification",
    "two-factor",
    "authenticator app",
    "kodu girin",
    "gönderdiğimiz kod",
    "doğrulama kodu",
    "iki adımlı doğrulama",
    "iki aşamalı",
    "code eingeben",
    "geben sie den code",
    "bestätigungscode",
    "zwei-faktor",
    "zwei-schritt",
];

/// Roles that take typed text.
fn takes_text(role: &str) -> bool {
    matches!(role, "textbox" | "spinbutton" | "searchbox")
}

/// Role and quoted name of a snapshot line like `  [e5] textbox "Code" value="x"`.
fn parts(line: &str) -> (Option<String>, &str, String) {
    let mut rest = line.trim_start();
    let mut r = None;
    if let Some(stripped) = rest.strip_prefix('[')
        && let Some((id, tail)) = stripped.split_once(']')
    {
        r = Some(id.to_owned());
        rest = tail.trim_start();
    }
    let role_end = rest.find([' ', '"']).unwrap_or(rest.len());
    let role = &rest[..role_end];
    let name = rest[role_end..]
        .trim_start()
        .strip_prefix('"')
        .and_then(|n| n.split('"').next())
        .unwrap_or("")
        .to_owned();
    (r, role, name)
}

/// Look for a CAPTCHA or a one-time-code prompt in the snapshot text of a page.
///
/// A CAPTCHA wins over a code prompt (the check comes first). The result only
/// ever feeds a daemon-written notice and the `browser_handover` check: page text
/// cannot make the bot do anything by itself.
#[must_use]
pub fn detect(snapshot: &str) -> Option<Challenge> {
    let mut captcha = false;
    let mut hard = false;
    let mut prompt = false;
    let mut field: Option<String> = None;
    let mut first_text: Option<String> = None;
    for line in snapshot.lines() {
        let low = line.to_lowercase();
        let (r, role, name) = parts(line);
        let name_low = name.to_lowercase();
        if CAPTCHA_STRONG.iter().any(|m| low.contains(m))
            || (low.contains("captcha")
                && matches!(
                    role,
                    "iframe" | "checkbox" | "img" | "image" | "textbox" | "button" | "group"
                ))
        {
            captcha = true;
        }
        if CAPTCHA_HARD.iter().any(|m| low.contains(m)) {
            hard = true;
        }
        if CODE_PROMPTS.iter().any(|m| low.contains(m)) {
            prompt = true;
        }
        if let (Some(r), true) = (&r, takes_text(role)) {
            let blocked = CODE_FIELD_NOT.iter().any(|m| name_low.contains(m));
            if !blocked && first_text.is_none() {
                first_text = Some(r.clone());
            }
            if !blocked && field.is_none() && CODE_FIELD_WORDS.iter().any(|m| name_low.contains(m))
            {
                field = Some(r.clone());
            }
        }
    }
    if captcha {
        return Some(Challenge::Captcha {
            hard: hard || snapshot.to_lowercase().contains("hcaptcha"),
        });
    }
    if field.is_some() {
        return Some(Challenge::Code { field });
    }
    if prompt && first_text.is_some() {
        return Some(Challenge::Code { field: first_text });
    }
    None
}

/// The notice appended to a snapshot that shows `challenge`; `seen` counts the
/// looks at a CAPTCHA so far (this one included). Written by the daemon.
#[must_use]
pub fn notice(challenge: &Challenge, seen: u32) -> String {
    match challenge {
        Challenge::Captcha { hard } if !*hard && seen <= CAPTCHA_OWN_ATTEMPTS => format!(
            "[Praxi notice] This page shows a CAPTCHA (a check that wants proof of a human). If you can see images and it is a simple one (an \"I'm not a robot\" checkbox or an easy picture or text puzzle) you may try it yourself, as an ordinary user would: attempt {seen} of {CAPTCHA_OWN_ATTEMPTS}. Never use a solving service or tricks. If it does not work, or you cannot see images, call browser_handover with kind \"captcha\"."
        ),
        Challenge::Captcha { .. } => "[Praxi notice] This page shows a CAPTCHA you must not try to solve yourself (it is a behavioural or picture puzzle, or your own attempts are used up). Call browser_handover with kind \"captcha\" now: the owner takes control, solves it, and the run continues.".to_owned(),
        Challenge::Code { field } => format!(
            "[Praxi notice] This page asks for a one-time code (2FA){}. Do NOT ask the owner to type the code in chat and do not guess one: call browser_handover with kind \"code\". The owner enters it in a private card and Praxi types it into the page; you never see it.",
            field
                .as_deref()
                .map_or_else(String::new, |f| format!(", the code field is [{f}]"))
        ),
    }
}

// ── Hub ──────────────────────────────────────────────────────────────────────

/// Empty documents and iframe placeholders are not evidence of a completed page.
pub(crate) fn rendered_page(snapshot: &str) -> bool {
    !snapshot.lines().any(|line| parts(line).1 == "iframe")
        && snapshot.lines().skip(1).any(|line| {
            matches!(
                parts(line).1,
                "heading" | "text" | "textbox" | "button" | "link" | "paragraph"
            )
        })
}

/// Why the daemon could not finish an owner's answer. The owner's card shows
/// these in plain words and stays open so they can try again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoverError {
    /// Nothing is armed for this run (it ended, or the tool was never called).
    NotArmed,
    /// The code is not 4 to 12 letters or digits.
    BadCode,
    /// The page is not on the site the card named.
    WrongPage,
    /// The page no longer shows a code prompt.
    NoPrompt,
    /// The check is still on the page.
    StillThere,
    /// The browser could not take the code (the message never holds it).
    Typing(String),
}

impl std::fmt::Display for HandoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotArmed => "this request is not waiting any more",
            Self::BadCode => "a code has 4 to 12 letters or digits",
            Self::WrongPage => "the browser is not on the site the card named any more",
            Self::NoPrompt => "the page does not ask for a code any more",
            Self::StillThere => "the page still shows the check",
            Self::Typing(m) => m,
        })
    }
}

#[derive(Clone)]
struct Armed {
    driver: Arc<dyn BrowserDriver>,
    kind: HandoverKind,
    origin: String,
    fingerprint: u64,
    cancelled: Arc<AtomicBool>,
}

/// What the daemon remembers about CAPTCHAs one run met.
///
/// The attempt budget belongs to a SITE (origin), never to a page or a moment: leaving
/// the page, opening another site, coming back or refreshing does not give tries back.
/// Only the owner's verified "Done" ([`HandoverHub::verify_solved`]) or the end of the run
/// clears it.
#[derive(Default)]
struct CaptchaBudget {
    /// Own actions spent, per origin (`""` when the browser did not report one).
    attempts: HashMap<String, u32>,
    /// Origins whose challenge is still unresolved (seen on the page, not seen gone).
    active: HashSet<String>,
}

fn fingerprint(snapshot: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    snapshot.hash(&mut hash);
    hash.finish()
}

/// The armed handovers of the process, keyed by run id.
#[derive(Default)]
pub struct HandoverHub {
    armed: Mutex<HashMap<String, Armed>>,
    /// Own action budgets per run and challenge page.
    seen: Mutex<HashMap<String, CaptchaBudget>>,
    captures: Mutex<Vec<(Weak<dyn BrowserDriver>, String)>>,
}

/// The process-wide hub the browser tool arms and the daemon's owner route answers.
#[must_use]
pub fn handover_hub() -> &'static HandoverHub {
    static HUB: OnceLock<HandoverHub> = OnceLock::new();
    HUB.get_or_init(HandoverHub::default)
}

/// Strip separators and check a typed code (`123 456`, `123-456`).
fn clean_code(raw: &str) -> Result<String, HandoverError> {
    let c: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if (4..=12).contains(&c.chars().count()) && c.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(c)
    } else {
        Err(HandoverError::BadCode)
    }
}

impl HandoverHub {
    fn with_budget<T>(&self, run_id: &str, f: impl FnOnce(&mut CaptchaBudget) -> T) -> T {
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(seen.entry(run_id.to_owned()).or_default())
    }

    /// Record what an observation of the page at `origin` showed. A challenge marks the
    /// site as unresolved; a rendered page WITHOUT it clears only that mark, never the
    /// attempts already spent there, and nothing at all for any other origin.
    pub fn observed(&self, run_id: &str, origin: Option<&str>, snapshot: &str) {
        let key = origin.unwrap_or_default().to_owned();
        let challenge = matches!(detect(snapshot), Some(Challenge::Captcha { .. }));
        if challenge || rendered_page(snapshot) {
            self.with_budget(run_id, |b| {
                if challenge {
                    b.active.insert(key);
                } else {
                    b.active.remove(&key);
                }
            });
        }
    }

    /// Clear the budget of one site after the owner's verified "Done".
    fn clear_captcha(&self, run_id: &str, origin: &str) {
        self.with_budget(run_id, |b| {
            b.attempts.remove(origin);
            b.active.remove(origin);
        });
    }

    /// Admit one own action on the page at `origin`, never an unbounded batch or a third attempt.
    ///
    /// # Errors
    /// Refuses hard challenges and actions after the second own attempt at this site.
    pub fn captcha_action(
        &self,
        run_id: &str,
        origin: Option<&str>,
        snapshot: &str,
    ) -> Result<(), String> {
        let key = origin.unwrap_or_default();
        if let Some(Challenge::Captcha { hard }) = detect(snapshot) {
            return self.with_budget(run_id, |b| {
                b.active.insert(key.to_owned());
                let n = b.attempts.entry(key.to_owned()).or_default();
                if hard || *n >= CAPTCHA_OWN_ATTEMPTS {
                    return Err("CAPTCHA own attempts exhausted: hand over with browser_handover(kind: captcha) before any further browser action".to_owned());
                }
                *n += 1;
                Ok(())
            });
        }
        self.observed(run_id, origin, snapshot);
        Ok(())
    }

    /// Number of the next permitted own attempt at `origin` (observations do not consume it).
    pub fn next_captcha_attempt(&self, run_id: &str, origin: Option<&str>) -> u32 {
        self.with_budget(run_id, |b| {
            b.attempts
                .get(origin.unwrap_or_default())
                .copied()
                .unwrap_or(0)
                + 1
        })
    }

    /// Whether this run met a CAPTCHA it has not seen gone, or has a CAPTCHA handover
    /// waiting. The desktop input tools refuse then: a visible browser window could be
    /// clicked through them without the attempt cap.
    pub fn captcha_unresolved(&self, run_id: &str) -> bool {
        let armed = self
            .armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .is_some_and(|a| a.kind == HandoverKind::Captcha);
        armed
            || self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(run_id)
                .is_some_and(|b| !b.active.is_empty())
    }

    /// Whether any browser holds a freshly typed code on its page right now.
    pub async fn typed_secret_visible(&self) -> bool {
        let drivers: Vec<Arc<dyn BrowserDriver>> = {
            let mut captures = self
                .captures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            captures.retain(|(d, _)| d.strong_count() > 0);
            captures.iter().filter_map(|(d, _)| d.upgrade()).collect()
        };
        for d in &drivers {
            if self.capture_latched(d).await {
                return true;
            }
        }
        false
    }

    /// Suppress visual observations of a driver holding a freshly typed code.
    /// The latch survives the approval/run ending; only navigation clears it.
    pub async fn capture_latched(&self, driver: &Arc<dyn BrowserDriver>) -> bool {
        let url = driver.capture_epoch().await.unwrap_or_default();
        let mut captures = self
            .captures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        captures.retain(|(d, at)| {
            d.upgrade()
                .is_some_and(|d| !Arc::ptr_eq(&d, driver) || *at == url)
        });
        captures
            .iter()
            .any(|(d, _)| d.upgrade().is_some_and(|d| Arc::ptr_eq(&d, driver)))
    }

    /// Remember what a handover of `run_id` needs to finish: the driver, the kind
    /// and the origin the owner was told about.
    pub fn arm(
        &self,
        run_id: &str,
        driver: Arc<dyn BrowserDriver>,
        kind: HandoverKind,
        origin: &str,
        fingerprint: &str,
    ) {
        self.armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                run_id.to_owned(),
                Armed {
                    driver,
                    kind,
                    origin: origin.to_owned(),
                    fingerprint: self::fingerprint(fingerprint),
                    cancelled: Arc::new(AtomicBool::new(false)),
                },
            );
    }

    /// Invalidate the per-call token without restoring CAPTCHA attempts.
    pub fn disarm(&self, run_id: &str) {
        if let Some(a) = self
            .armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id)
        {
            a.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// Drop run state after Stop or run completion.
    pub fn forget(&self, run_id: &str) {
        self.disarm(run_id);
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
    }

    fn get(&self, run_id: &str, kind: HandoverKind) -> Result<Armed, HandoverError> {
        let armed = self
            .armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let a = armed.get(run_id).ok_or(HandoverError::NotArmed)?;
        if a.kind != kind {
            return Err(HandoverError::NotArmed);
        }
        Ok(a.clone())
    }

    /// Type the owner's one-time `code` into the page of `run_id`.
    ///
    /// The code is checked, typed through the driver's secret path (which refuses
    /// unless the receiving frame is on the origin the card named, re-read from
    /// the browser right before the text is inserted) and dropped. Nothing
    /// returned or logged contains it.
    ///
    /// # Errors
    /// See [`HandoverError`]; the message never contains the code.
    pub async fn type_code(&self, run_id: &str, code: &str) -> Result<(), HandoverError> {
        let code = clean_code(code)?;
        let Armed {
            driver,
            origin,
            cancelled,
            ..
        } = self.get(run_id, HandoverKind::Code)?;
        let here = driver
            .current_url()
            .await
            .and_then(|u| crate::browser::origin_of(&u));
        if here.as_deref() != Some(origin.as_str()) {
            return Err(HandoverError::WrongPage);
        }
        // A fresh read: the field is found again, so a page that moved on or a
        // stale ref never receives the code.
        let snap = driver
            .snapshot(false)
            .await
            .map_err(|e| HandoverError::Typing(scrub(&e, &code)))?;
        let Some(Challenge::Code { field: Some(field) }) = detect(&snap) else {
            return Err(HandoverError::NoPrompt);
        };
        let want = origin.clone();
        let accept = |o: Option<String>| {
            let armed = self
                .armed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            armed
                .get(run_id)
                .is_some_and(|a| Arc::ptr_eq(&a.cancelled, &cancelled))
                && !cancelled.load(Ordering::SeqCst)
                && o.as_deref().and_then(crate::browser::origin_of).as_deref()
                    == Some(want.as_str())
        };
        if !accept(driver.current_url().await) {
            return Err(HandoverError::NotArmed);
        }
        let url = driver.capture_epoch().await.unwrap_or_default();
        if !accept(driver.current_url().await) {
            return Err(HandoverError::NotArmed);
        }
        self.captures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((Arc::downgrade(&driver), url));
        match driver.fill_secret(&field, &code, &accept).await {
            Ok(_) => Ok(()),
            Err(e) => Err(HandoverError::Typing(scrub(&e, &code))),
        }
    }

    /// Check that the CAPTCHA of `run_id` is gone (the owner pressed "Done").
    ///
    /// # Errors
    /// [`HandoverError::StillThere`] while the page still shows it.
    pub async fn verify_solved(&self, run_id: &str) -> Result<(), HandoverError> {
        let Armed {
            driver,
            origin,
            fingerprint: at_arm,
            cancelled,
            ..
        } = self.get(run_id, HandoverKind::Captcha)?;
        if driver
            .current_url()
            .await
            .and_then(|u| crate::browser::origin_of(&u))
            .as_deref()
            != Some(origin.as_str())
        {
            return Err(HandoverError::WrongPage);
        }
        let snap = driver
            .snapshot(false)
            .await
            .map_err(HandoverError::Typing)?;
        // Empty documents and iframe placeholders are not evidence of completion.
        if cancelled.load(Ordering::SeqCst) {
            return Err(HandoverError::NotArmed);
        }
        if driver
            .current_url()
            .await
            .and_then(|u| crate::browser::origin_of(&u))
            .as_deref()
            != Some(origin.as_str())
        {
            return Err(HandoverError::WrongPage);
        }
        if !rendered_page(&snap)
            || fingerprint(&snap) == at_arm
            || matches!(detect(&snap), Some(Challenge::Captcha { .. }))
        {
            Err(HandoverError::StillThere)
        } else {
            self.clear_captcha(run_id, &origin);
            Ok(())
        }
    }
}

/// Remove every copy of `code` from `text`.
fn scrub(text: &str, code: &str) -> String {
    if code.is_empty() {
        text.to_owned()
    } else {
        text.replace(code, "[code]")
    }
}

#[cfg(test)]
mod tests;

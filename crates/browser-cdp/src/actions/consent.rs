//! `browser_dismiss_consent`: find the cookie / consent dialog of the page that is open and click
//! its privacy-preserving button.
//!
//! The finder runs in the main frame only (it cannot see into other frames, so a dialog that
//! lives in a cross-origin iframe is reported, not touched). It looks for the well-known
//! consent platforms by their own ids, then for banner-like elements that talk about cookies,
//! and prefers "reject / only necessary" over "accept"; "accept" is clicked only when the
//! dialog offers nothing narrower. Links that would leave the page are never clicked.

use serde_json::Value;

use super::{ActionResult, DialogOptions, Feel, Jitter, grant_origin_matches, human, sleep};
use crate::error::{Error, Result};
use crate::page::Page;

/// The in-page finder: evaluates to `{found, kind, cmp, label, x, y, origin}` or
/// `{found: false, banner, frame_hint, origin}`.
const FINDER_JS: &str = include_str!("consent.js");

/// Which button the finder chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentChoice {
    /// "Reject all" / "Only necessary": the privacy-preserving button.
    Reject,
    /// "Accept all": clicked only when the dialog has no narrower choice.
    Accept,
}

impl ConsentChoice {
    /// Short words for the result line.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject / necessary only",
            Self::Accept => "accept, because no narrower choice was offered",
        }
    }
}

/// What a [`Page::dismiss_consent`] call did.
#[derive(Debug, Clone)]
pub struct ConsentOutcome {
    /// The choice made, the platform that showed the dialog (`generic` when unknown) and the
    /// label of the clicked button; `None` when there was nothing to click.
    pub clicked: Option<(ConsentChoice, String, String)>,
    /// What to tell the model when nothing was clicked.
    pub note: Option<String>,
    /// The page after the click.
    pub action: Option<ActionResult>,
}

/// What the finder script reported.
#[derive(Debug, PartialEq)]
enum Found {
    Button {
        choice: ConsentChoice,
        cmp: String,
        label: String,
        x: f64,
        y: f64,
        origin: String,
    },
    Nothing {
        banner: bool,
        frame_hint: bool,
        origin: String,
    },
}

fn parse_found(v: &Value) -> Result<Found> {
    let origin = v
        .get("origin")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if v.get("found").and_then(Value::as_bool) != Some(true) {
        return Ok(Found::Nothing {
            banner: v.get("banner").and_then(Value::as_bool) == Some(true),
            frame_hint: v.get("frame_hint").and_then(Value::as_bool) == Some(true),
            origin,
        });
    }
    let num = |k: &str| {
        v.get(k)
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::Js(format!("consent finder returned no {k}")))
    };
    let choice = match v.get("kind").and_then(Value::as_str) {
        Some("reject") => ConsentChoice::Reject,
        Some("accept") => ConsentChoice::Accept,
        other => return Err(Error::Js(format!("consent finder returned kind {other:?}"))),
    };
    Ok(Found::Button {
        choice,
        cmp: v
            .get("cmp")
            .and_then(Value::as_str)
            .unwrap_or("generic")
            .to_owned(),
        label: v
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        x: num("x")?,
        y: num("y")?,
        origin,
    })
}

impl Page {
    /// Click the privacy-preserving button of the cookie / consent dialog of the main frame.
    /// `expected` is the origin the approval named: a page on another origin is refused.
    pub async fn dismiss_consent(
        &self,
        expected: Option<&str>,
        feel: Feel,
    ) -> Result<ConsentOutcome> {
        let found = parse_found(&self.evaluate_readonly(FINDER_JS).await?)?;
        let origin = match &found {
            Found::Button { origin, .. } | Found::Nothing { origin, .. } => origin.as_str(),
        };
        if let Some(expected) = expected
            && !grant_origin_matches(Some(origin), expected)
        {
            return Err(Error::Element(
                "receiving frame origin changed; ask again before input".into(),
            ));
        }
        let (choice, cmp, label, x, y) = match found {
            Found::Nothing {
                banner, frame_hint, ..
            } => {
                let note = if banner {
                    "A consent dialog may be showing but none of its buttons is a clear accept or reject choice (it may only offer settings). Look with browser_snapshot and click the least permissive button yourself; do not reload the page."
                } else if frame_hint {
                    "The consent dialog seems to live in an embedded frame, which this tool does not click into. Look with browser_snapshot: if its button has a ref, click it with browser_click; do not reload the page."
                } else {
                    "No cookie or consent dialog was found on this page, so nothing was clicked."
                };
                return Ok(ConsentOutcome {
                    clicked: None,
                    note: Some(note.to_owned()),
                    action: None,
                });
            }
            Found::Button {
                choice,
                cmp,
                label,
                x,
                y,
                ..
            } => (choice, cmp, label, x, y),
        };
        let before = self.baseline_with(&DialogOptions::default()).await?;
        let res = self.click_at(x, y, feel).await;
        let action = self.finish(before, res).await?;
        Ok(ConsentOutcome {
            clicked: Some((choice, cmp, label)),
            note: None,
            action: Some(action),
        })
    }

    /// Glide to a point and press / release the left button there.
    async fn click_at(&self, x: f64, y: f64, feel: Feel) -> Result<()> {
        let mut rng = Jitter::from_clock();
        self.glide((x, y), feel, &mut rng).await?;
        sleep(human::settle_pause(feel, &mut rng)).await;
        let btn = serde_json::json!({"button": "left", "clickCount": 1});
        self.mouse("mousePressed", x, y, btn.clone()).await?;
        sleep(human::hold_pause(feel, &mut rng)).await;
        self.mouse("mouseReleased", x, y, btn).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_button_and_a_miss() {
        let hit = parse_found(&json!({
            "found": true, "kind": "reject", "cmp": "OneTrust", "label": "reject all",
            "x": 10.5, "y": 20.0, "origin": "https://a.test"
        }))
        .unwrap();
        assert_eq!(
            hit,
            Found::Button {
                choice: ConsentChoice::Reject,
                cmp: "OneTrust".into(),
                label: "reject all".into(),
                x: 10.5,
                y: 20.0,
                origin: "https://a.test".into(),
            }
        );
        let miss =
            parse_found(&json!({"found": false, "banner": false, "origin": "https://a.test"}))
                .unwrap();
        assert!(matches!(miss, Found::Nothing { banner: false, .. }));
        assert!(parse_found(&json!({"found": true, "kind": "x", "x": 1, "y": 1})).is_err());
    }
}

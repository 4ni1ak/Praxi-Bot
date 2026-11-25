---
name: cookie-consent-banners
description: Dismiss cookie and consent banners choosing the most privacy-preserving option.
version: 0.2.0
when:
  keywords: [cookie, consent, gdpr, privacy banner, accept all, reject all]
---
# Cookie and consent banners

Goal: get the banner out of the way while sharing as little as possible.

Fastest way: call `browser_dismiss_consent` (no arguments). It finds the dialog (OneTrust, Cookiebot, Didomi, Quantcast, TrustArc, Usercentrics, Klaro or a generic cookie banner) and clicks the privacy-preserving button itself: reject all or necessary only first, accept only when nothing narrower is offered. Never reload or reopen the page to get rid of a banner: that throws away what you typed. If the tool says nothing was found or the dialog only offers settings, do it by hand as below.

Preference order:
1. "Reject all", "Decline", "Only necessary", "Essential only".
2. "Manage" or "Customize preferences": switch every non-essential category off, then "Save".
3. Close icon (X) if it clearly means no choice made.
4. Never click "Accept all" unless the user explicitly says so or the site is unusable otherwise; if so, tell the user.

Tips:
- The banner may live in an iframe or shadow root; see the `shadow-dom-and-iframes` skill.
- If the page is blocked until a choice is made and no reject option exists, use `ask_user`.
- "Legitimate interest" toggles are non-essential too; turn them off.
- Do not sign in, subscribe or accept terms in the same step; those are separate approvals.

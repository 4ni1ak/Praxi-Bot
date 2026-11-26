//! The ONE rule that decides whether a mail sender is allowed to start a run.
//!
//! Both the mail channel (allowed senders) and the mail trigger registry call
//! [`sender_matches`], so the two paths can never disagree. Mail is untrusted
//! input: the rule looks at the parsed ADDRESS only (never a display name,
//! never free text) and fails closed on anything odd.
//!
//! A rule value is one of:
//! - an address (contains `@`, e.g. `ann@corp.com`): equal to the sender's
//!   address, ASCII case-insensitive. `ann@corp.com` does not match
//!   `ann+news@corp.com`: plus-addressing is a different address.
//! - a domain (`corp.com` or `@corp.com`): the sender's domain is that domain
//!   or a subdomain of it on a LABEL boundary. `corp.com` matches `x@corp.com`
//!   and `x@mail.corp.com`, never `x@corp.com.evil.test` or `x@evilcorp.com`.
//!
//! There is no substring mode. An empty or blank value matches nothing, and so
//! does a value that is not a well-formed address or domain. Comparison is on
//! the ASCII form only: a non-ASCII (Unicode) address or rule never matches,
//! so a homograph such as a Cyrillic `о` in `corp.com` can never pass; an
//! internationalised domain is written in its punycode form (`xn--...`).

/// Longest rule value or address accepted (an address is at most 254 characters).
const MAX_LEN: usize = 254;
/// Longest local part of an address.
const MAX_LOCAL_LEN: usize = 64;

/// A parsed rule value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderRule {
    /// Exactly this address (lower-case).
    Address(String),
    /// This domain or any subdomain of it (lower-case, no leading `@`).
    Domain(String),
}

impl SenderRule {
    /// Parse one rule value: an address, `@domain` or a bare domain with at
    /// least one dot. `None` (match nothing) for empty, oversized, non-ASCII
    /// or malformed input.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.is_empty() || v.len() > MAX_LEN || !v.is_ascii() {
            return None;
        }
        let v = v.to_ascii_lowercase();
        if let Some(domain) = v.strip_prefix('@') {
            return (valid_domain(domain) && domain.contains('.'))
                .then(|| Self::Domain(domain.to_owned()));
        }
        if v.contains('@') {
            return valid_address(&v).then_some(Self::Address(v));
        }
        (valid_domain(&v) && v.contains('.')).then_some(Self::Domain(v))
    }

    /// The canonical stored form: the address, or `@domain`.
    #[must_use]
    pub fn canonical(&self) -> String {
        match self {
            Self::Address(a) => a.clone(),
            Self::Domain(d) => format!("@{d}"),
        }
    }

    /// Whether `address` (already the bare sender address) satisfies the rule.
    #[must_use]
    pub fn matches(&self, address: &str) -> bool {
        let a = address;
        if a.len() > MAX_LEN || !a.is_ascii() {
            return false;
        }
        let a = a.to_ascii_lowercase();
        if !valid_address(&a) {
            return false;
        }
        match self {
            Self::Address(want) => a == *want,
            Self::Domain(want) => {
                let Some((_, d)) = a.rsplit_once('@') else {
                    return false;
                };
                d == want
                    || (d.len() > want.len()
                        && d.ends_with(want.as_str())
                        && d.as_bytes()[d.len() - want.len() - 1] == b'.')
            }
        }
    }
}

/// Whether the sender address `address` passes the rule `value`. Empty or
/// malformed on either side is `false` (fail closed).
#[must_use]
pub fn sender_matches(address: &str, value: &str) -> bool {
    SenderRule::parse(value).is_some_and(|r| r.matches(address))
}

/// Whether `value` is a usable rule value (an address, `@domain` or domain).
#[must_use]
pub fn sender_rule_ok(value: &str) -> bool {
    SenderRule::parse(value).is_some()
}

/// Turn a legacy "contains" sender filter into a rule value: `Some(canonical)`
/// when it already looks like an address or a domain, `None` when it is free
/// text (the caller must then DISABLE what it guards and tell the owner,
/// never leave it open).
#[must_use]
pub fn migrate_legacy_contains(value: &str) -> Option<String> {
    SenderRule::parse(value).map(|r| r.canonical())
}

fn valid_domain(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 253
        && d.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn valid_address(a: &str) -> bool {
    let Some((local, domain)) = a.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && local.len() <= MAX_LOCAL_LEN
        && !domain.contains('@')
        && valid_domain(domain)
        && local.bytes().all(|b| {
            b.is_ascii_graphic()
                && !matches!(
                    b,
                    b'<' | b'>' | b',' | b';' | b'"' | b'(' | b')' | b'[' | b']' | b'\\' | b':'
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_rule_is_exact_and_case_insensitive() {
        assert!(sender_matches("ann@corp.com", "ann@corp.com"));
        assert!(sender_matches("ANN@Corp.COM", " ann@corp.com "));
        assert!(!sender_matches("xann@corp.com", "ann@corp.com"));
        assert!(!sender_matches("ann@corp.com.evil.test", "ann@corp.com"));
        assert!(!sender_matches("ann@mail.corp.com", "ann@corp.com"));
    }

    #[test]
    fn plus_addressing_is_a_different_address() {
        assert!(!sender_matches("ann+news@corp.com", "ann@corp.com"));
        assert!(sender_matches("ann+news@corp.com", "corp.com"));
        assert!(sender_matches("ann+news@corp.com", "ann+news@corp.com"));
    }

    #[test]
    fn domain_rule_stops_at_label_boundaries() {
        for rule in ["corp.com", "@corp.com", "CORP.com"] {
            assert!(sender_matches("x@corp.com", rule), "{rule}");
            assert!(sender_matches("x@mail.corp.com", rule), "{rule}");
            assert!(sender_matches("x@a.b.corp.com", rule), "{rule}");
            assert!(!sender_matches("x@corp.com.evil.test", rule), "{rule}");
            assert!(!sender_matches("x@evilcorp.com", rule), "{rule}");
            assert!(!sender_matches("x@corp.co", rule), "{rule}");
            assert!(!sender_matches("x@xcorp.com", rule), "{rule}");
            assert!(!sender_matches("x@.corp.com", rule), "{rule}");
        }
    }

    #[test]
    fn display_name_text_and_odd_addresses_never_match() {
        assert!(!sender_matches("Ann <ann@corp.com>", "ann@corp.com"));
        assert!(!sender_matches("ann@corp.com <evil@evil.io>", "corp.com"));
        assert!(!sender_matches("ann@corp.com, evil@evil.io", "corp.com"));
        assert!(!sender_matches("ann@corp.com\n", "evil@evil.io"));
        assert!(!sender_matches("a@b@corp.com", "corp.com"));
        assert!(!sender_matches("", "corp.com"));
        assert!(!sender_matches("corp.com", "corp.com"));
        assert!(!sender_matches("@corp.com", "corp.com"));
    }

    #[test]
    fn empty_blank_and_free_text_rules_match_nothing() {
        for rule in [
            "",
            "   ",
            "@",
            ".",
            "com",
            "@com",
            "ann",
            "ann@",
            "@@corp.com",
            "a b@corp.com",
            "corp..com",
            "-corp.com",
            "*.corp.com",
            "corp.com/x",
        ] {
            assert!(!sender_rule_ok(rule), "{rule:?}");
            assert!(!sender_matches("ann@corp.com", rule), "{rule:?}");
            assert!(!sender_matches("ann.evil@x.io", rule), "{rule:?}");
        }
    }

    #[test]
    fn unicode_and_idn_compare_on_the_ascii_form_only() {
        // Cyrillic о in the sender or the rule: never equal to the Latin one.
        assert!(!sender_matches("ann@c\u{43e}rp.com", "corp.com"));
        assert!(!sender_matches("ann@corp.com", "c\u{43e}rp.com"));
        assert!(!sender_rule_ok("c\u{43e}rp.com"));
        assert!(!sender_matches("ann@c\u{43e}rp.com", "c\u{43e}rp.com"));
        // The punycode form is just another ASCII name: exact only.
        assert!(sender_matches("ann@xn--bcher-kva.de", "xn--bcher-kva.de"));
        assert!(!sender_matches("ann@xn--bcher-kva.de", "bucher.de"));
        assert!(!sender_matches("ann@b\u{fc}cher.de", "xn--bcher-kva.de"));
    }

    #[test]
    fn legacy_contains_values_migrate_only_when_they_look_like_a_sender() {
        assert_eq!(
            migrate_legacy_contains("Corp.com").as_deref(),
            Some("@corp.com")
        );
        assert_eq!(
            migrate_legacy_contains("@corp.com").as_deref(),
            Some("@corp.com")
        );
        assert_eq!(
            migrate_legacy_contains("Ann@Corp.com").as_deref(),
            Some("ann@corp.com")
        );
        for free_text in ["ann", "invoice", "@", ".", "", "  ", "billing team"] {
            assert_eq!(migrate_legacy_contains(free_text), None, "{free_text:?}");
        }
    }

    #[test]
    fn rule_never_matches_more_than_it_says_for_generated_input() {
        // Property style: for many generated hosts, a domain rule matches a
        // sender exactly when the sender host equals it or ends in ".<rule>".
        let labels = ["corp", "com", "mail", "evil", "test", "x", "evilcorp"];
        let mut hosts = Vec::new();
        for a in labels {
            for b in labels {
                hosts.push(format!("{a}.{b}"));
                for c in labels {
                    hosts.push(format!("{a}.{b}.{c}"));
                }
            }
        }
        for rule in ["corp.com", "mail.corp.com", "evil.test"] {
            for h in &hosts {
                let want = h == rule || h.ends_with(&format!(".{rule}"));
                assert_eq!(
                    sender_matches(&format!("u@{h}"), rule),
                    want,
                    "{h} vs {rule}"
                );
            }
        }
    }
}

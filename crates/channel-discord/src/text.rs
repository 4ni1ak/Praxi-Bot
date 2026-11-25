//! Text rules for Discord: mention escaping, length splitting and untrusted
//! attachment labels. All pure functions.

/// Discord refuses messages longer than 2000 characters; stay below it.
pub const MAX_MESSAGE_CHARS: usize = 1900;

const ZWSP: char = '\u{200b}';

/// Break every mention the model (or any other text) could write so Discord
/// cannot turn it into a ping: `@everyone`, `@here`, `<@id>`, `<@&role>`.
///
/// This is defence in depth: every request also sends `allowed_mentions` with
/// an empty `parse` list, so Discord never pings from our messages anyway.
#[must_use]
pub fn neutralize_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut rest = text;
    while let Some(at) = rest.find(['@', '<']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let ch = tail.chars().next().unwrap_or('@');
        // Already broken (text that went through here before): leave it alone,
        // so the function can be applied twice without piling up invisible marks.
        if tail[1..].starts_with(ZWSP) {
            out.push(ch);
        } else if ch == '@' {
            out.push('@');
            out.push(ZWSP);
        } else if tail[1..].starts_with('@') {
            // `<@123>`, `<@!123>`, `<@&123>`
            out.push('<');
            out.push(ZWSP);
        } else {
            out.push('<');
        }
        rest = &tail[ch.len_utf8()..];
    }
    out.push_str(rest);
    out
}

/// Split `text` into pieces of at most `max` characters, preferring line then
/// word boundaries. Nothing is dropped; an empty text gives one empty piece.
#[must_use]
pub fn split_message(text: &str, max: usize) -> Vec<String> {
    let max = max.max(2);
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.encode_utf16().count() > max {
        let mut units = 0;
        let cut_byte = rest
            .char_indices()
            .find_map(|(i, c)| {
                units += c.len_utf16();
                (units > max).then_some(i)
            })
            .unwrap_or(rest.len());
        let window = &rest[..cut_byte];
        let split = window
            .rfind('\n')
            .or_else(|| window.rfind(' '))
            .filter(|&i| i > 0)
            .map_or(cut_byte, |i| i + 1);
        pieces.push(rest[..split].to_owned());
        rest = &rest[split..];
    }
    pieces.push(rest.to_owned());
    pieces
}

/// A file name that is safe to show to a model or a person: letters, digits and
/// `. - _ space` only, at most 60 characters.
#[must_use]
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .take(60)
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim().is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

/// One line describing an attachment. The file is never downloaded or opened;
/// the label says so, so the model treats name and type as data, not as
/// instructions.
#[must_use]
pub fn attachment_label(name: &str, content_type: Option<&str>, size: Option<u64>) -> String {
    let kind: String = content_type
        .unwrap_or("unknown type")
        .chars()
        .take(40)
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '+'))
        .collect();
    let size = size.map_or_else(String::new, |s| format!(", {} KB", s.div_ceil(1024)));
    format!(
        "[Attachment from Discord, untrusted and not opened: \"{}\" ({}{size})]",
        sanitize_filename(name),
        if kind.is_empty() {
            "unknown type"
        } else {
            &kind
        },
    )
}

/// Keep `text` within `max` characters for an in-place edit (edits cannot be
/// split into several messages).
#[must_use]
pub fn clamp_chars(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_owned();
    }
    let mut units = 0;
    let mut cut: String = text
        .chars()
        .take_while(|c| {
            units += c.len_utf16();
            units < max
        })
        .collect();
    cut.push('…');
    cut
}

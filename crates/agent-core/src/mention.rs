//! `@bot` mentions in a user's own message.
//!
//! Only text the user typed is parsed (human turns); model output, tool output
//! and earlier assistant messages never reach this module.

use protocol::BotId;

/// Most bots one message can call.
pub(crate) const MAX_MENTIONS: usize = 3;

fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Bots named by `@handle` tokens in `text`, in order of first appearance,
/// without repeats, at most [`MAX_MENTIONS`]. A handle is a bot name without
/// whitespace, compared case-insensitively. Unknown handles are ignored (they
/// stay plain text). A message that only mentions the chat's own bot is not a
/// mention at all (`Vec::new()`).
pub(crate) fn resolve(bots: &[(BotId, String)], chat_bot: BotId, text: &str) -> Vec<BotId> {
    let mut found: Vec<BotId> = Vec::new();
    let mut prev: Option<char> = None;
    for (i, c) in text.char_indices() {
        let boundary = prev.is_none_or(|p| !(p.is_alphanumeric() || p == '_'));
        prev = Some(c);
        if c != '@' || !boundary {
            continue;
        }
        let rest = &text[i + 1..];
        let end = rest
            .char_indices()
            .find(|(_, ch)| !(ch.is_alphanumeric() || *ch == '_' || *ch == '-'))
            .map_or(rest.len(), |(j, _)| j);
        let handle = fold(&rest[..end]);
        if handle.is_empty() {
            continue;
        }
        if let Some((id, _)) = bots.iter().find(|(_, name)| fold(name) == handle)
            && !found.contains(id)
        {
            found.push(*id);
            if found.len() == MAX_MENTIONS {
                break;
            }
        }
    }
    if found.iter().all(|b| *b == chat_bot) {
        return Vec::new();
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bots() -> (Vec<(BotId, String)>, BotId, BotId, BotId) {
        let (x, y, z) = (BotId::new(), BotId::new(), BotId::new());
        (
            vec![
                (x, "Xena".into()),
                (y, "Atlas Prime".into()),
                (z, "Nova".into()),
            ],
            x,
            y,
            z,
        )
    }

    #[test]
    fn mentions_resolve_in_order_capped_and_unknown_stay_text() {
        let (b, x, y, z) = bots();
        assert_eq!(resolve(&b, x, "hi @AtlasPrime, and @nova!"), [y, z]);
        assert_eq!(resolve(&b, x, "@ghost hello"), Vec::<BotId>::new());
        assert_eq!(
            resolve(&b, x, "mail me at a@atlasprime.io"),
            Vec::<BotId>::new()
        );
        assert_eq!(resolve(&b, x, "@Xena only me"), Vec::<BotId>::new());
        assert_eq!(resolve(&b, x, "@Nova @Nova @Xena @AtlasPrime"), [z, x, y]);
    }
}

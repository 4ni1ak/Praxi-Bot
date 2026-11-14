//! Local, explicit user review; never registered as an agent tool.
use clap::Subcommand;
use std::{io::Write, path::Path};

#[derive(Subcommand)]
pub enum SkillsCmd {
    /// Print every file in full (JSON-quoted whitespace) and its review fingerprint.
    Review {
        name: String,
        #[arg(long)]
        bot_id: Option<String>,
    },
    /// Trust exactly the content printed by `skills review`; changes require re-review.
    Trust {
        name: String,
        #[arg(long)]
        bot_id: Option<String>,
        #[arg(long)]
        reviewed: String,
    },
}

pub fn run(data: &Path, action: &SkillsCmd, out: &mut dyn Write) -> crate::Result<()> {
    let (name, bot) = match action {
        SkillsCmd::Review { name, bot_id } | SkillsCmd::Trust { name, bot_id, .. } => {
            (name, bot_id)
        }
    };
    let root = skills::SkillIndex::load_roots(skills::SkillRoots::from_data_dir(data));
    let index = root.for_bot(bot.as_deref())?;
    match action {
        SkillsCmd::Review { .. } => {
            let review = index.review_for_trust(name)?;
            for (file, text) in review.files {
                let text = serde_json::to_string(&text)?;
                writeln!(
                    out,
                    "FILE {}: {}",
                    serde_json::to_string(&file)?,
                    protocol::approval_text::reveal_hidden(&text, true)
                )?;
            }
            writeln!(out, "Reviewed fingerprint: {}", review.fingerprint)?;
            writeln!(
                out,
                "To trust this exact content, run skills trust with --reviewed and this fingerprint."
            )?;
        }
        SkillsCmd::Trust { reviewed, .. } => {
            index.trust_reviewed(name, reviewed)?;
            writeln!(out, "Trusted skill {name} after explicit review.")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cli_review_then_trust_rejects_stale_content_and_restores_trust() {
        let tmp = tempfile::tempdir().unwrap();
        let root = skills::SkillIndex::load_roots(skills::SkillRoots::from_data_dir(tmp.path()));
        let bot = root.for_bot(Some("a")).unwrap();
        bot.write_versioned(
            "x",
            "---\nname: x\ndescription: d\n---\nbody\tend",
            None,
            true,
        )
        .unwrap();
        let review = bot.review_for_trust("x").unwrap();
        let mut out = Vec::new();
        run(
            tmp.path(),
            &SkillsCmd::Review {
                name: "x".into(),
                bot_id: Some("a".into()),
            },
            &mut out,
        )
        .unwrap();
        let shown = String::from_utf8(out).unwrap();
        assert!(shown.contains("body\\t"));
        assert!(shown.contains(&review.fingerprint));
        // An actual content change after review must invalidate the previously
        // printed fingerprint, even if the skill still has the same name.
        bot.write_versioned(
            "x",
            "---\nname: x\ndescription: d\n---\nchanged body",
            Some("0.1.0"),
            true,
        )
        .unwrap();
        assert!(
            run(
                tmp.path(),
                &SkillsCmd::Trust {
                    name: "x".into(),
                    bot_id: Some("a".into()),
                    reviewed: review.fingerprint
                },
                &mut Vec::new()
            )
            .is_err()
        );
        assert!(bot.get("x").unwrap().untrusted);
        let current = bot.review_for_trust("x").unwrap();
        run(
            tmp.path(),
            &SkillsCmd::Trust {
                name: "x".into(),
                bot_id: Some("a".into()),
                reviewed: current.fingerprint,
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(!root.for_bot(Some("a")).unwrap().get("x").unwrap().untrusted);
    }
}

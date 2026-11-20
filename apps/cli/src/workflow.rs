//! Explicit user actions over the bearer-protected workflow API.
use crate::{Result, client::Client};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf};

#[derive(Subcommand)]
pub(crate) enum WorkflowCmd {
    /// List saved workflows for a bot.
    List {
        /// Bot ID (see `bot bots list`).
        #[arg(long)]
        bot: String,
    },
    /// Show every step and parameter of a saved workflow.
    Show {
        /// Saved workflow name.
        name: String,
        /// Bot ID (see `bot bots list`).
        #[arg(long)]
        bot: String,
    },
    /// Print an unsaved workflow proposal for review (redirect JSON to a file to edit).
    Propose {
        /// Completed run ID to build a workflow proposal from.
        run: String,
    },
    /// Save reviewed workflow metadata and defaults (source steps are rebuilt and compared).
    Save {
        /// Path to the reviewed proposal JSON file.
        file: PathBuf,
        /// Override the workflow name from the proposal file.
        #[arg(long)]
        name: Option<String>,
        /// Override the workflow description from the proposal file.
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a saved workflow.
    Delete {
        /// Saved workflow name.
        name: String,
        /// Bot ID (see `bot bots list`).
        #[arg(long)]
        bot: String,
    },
    /// Replay a saved workflow in an existing chat through the normal permission engine.
    Run {
        /// Saved workflow name.
        name: String,
        /// Chat ID to replay the workflow in.
        #[arg(long)]
        chat: String,
        /// Read the JSON parameter object from stdin (never from argv).
        #[arg(long)]
        params_stdin: bool,
    },
}

fn scoped(path: &str, bot: &str) -> Result<String> {
    let bot: uuid::Uuid = bot
        .parse()
        .map_err(|_| format!("invalid bot ID `{bot}`: expected a UUID (see `bot bots list`)"))?;
    Ok(format!("{path}?bot_id={bot}"))
}

fn read_params(
    mut input: impl std::io::Read,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut text = String::new();
    input.read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|_| {
        "invalid workflow parameter JSON on stdin; expected a JSON object of string values".into()
    })
}

fn workflow_path(name: &str) -> Result<String> {
    if !workflow_replay::format::valid_name(name) {
        return Err(
            format!("invalid workflow name `{name}`; use letters, digits, `_`, or `-`").into(),
        );
    }
    Ok(format!("/v1/workflows/{name}"))
}

fn save_body(proposal: &Value, name: Option<&str>, description: Option<&str>) -> Result<Value> {
    let run_id = proposal["run_id"]
        .as_str()
        .ok_or("workflow proposal is missing `run_id`")?;
    let _: uuid::Uuid = run_id
        .parse()
        .map_err(|_| format!("invalid `run_id` `{run_id}` in proposal: expected a UUID"))?;
    let wf = proposal.get("workflow").unwrap_or(proposal);
    let name = name
        .or_else(|| wf["name"].as_str())
        .ok_or("workflow proposal is missing `name`; pass `--name <NAME>` or set it in the file")?;
    workflow_path(name)?;
    let params = wf["params"]
        .as_array()
        .ok_or("workflow proposal is missing `params`")?;
    let steps = wf["steps"]
        .as_array()
        .ok_or("workflow proposal is missing reviewed `steps`")?;
    Ok(
        json!({"run_id":run_id, "name":name, "description":description.or_else(|| wf["description"].as_str()).unwrap_or_default(), "params":params, "steps":steps, "confirmed_steps":proposal.get("confirmed_steps").cloned().unwrap_or_else(|| json!([]))}),
    )
}

pub(crate) async fn run(
    client: &Client,
    command: &WorkflowCmd,
    out: &mut impl Write,
) -> Result<()> {
    let (method, path, body) = match command {
        WorkflowCmd::List { bot } => (Method::GET, scoped("/v1/workflows", bot)?, None),
        WorkflowCmd::Show { name, bot } => (Method::GET, scoped(&workflow_path(name)?, bot)?, None),
        WorkflowCmd::Propose { run } => {
            let run: uuid::Uuid = run
                .parse()
                .map_err(|_| format!("invalid run ID `{run}`: expected a UUID"))?;
            (
                Method::POST,
                format!("/v1/runs/{run}/workflow-proposal"),
                None,
            )
        }
        WorkflowCmd::Save {
            file,
            name,
            description,
        } => {
            let raw = std::fs::read_to_string(file).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    format!(
                        "workflow proposal file {} not found; check the path and try again",
                        file.display()
                    )
                } else {
                    format!("cannot read workflow proposal file {}: {e}", file.display())
                }
            })?;
            let proposal: Value = serde_json::from_str(&raw).map_err(|e| {
                format!("invalid JSON in workflow proposal {}: {e}", file.display())
            })?;
            (
                Method::POST,
                "/v1/workflows".into(),
                Some(save_body(
                    &proposal,
                    name.as_deref(),
                    description.as_deref(),
                )?),
            )
        }
        WorkflowCmd::Delete { name, bot } => {
            (Method::DELETE, scoped(&workflow_path(name)?, bot)?, None)
        }
        WorkflowCmd::Run {
            name,
            chat,
            params_stdin,
        } => {
            let chat: uuid::Uuid = chat
                .parse()
                .map_err(|_| format!("invalid chat ID `{chat}`: expected a UUID"))?;
            let params = if *params_stdin {
                read_params(std::io::stdin().lock())?
            } else {
                std::collections::BTreeMap::default()
            };
            (
                Method::POST,
                format!("{}/run", workflow_path(name)?),
                Some(json!({"chat_id":chat,"params":params})),
            )
        }
    };
    let result = client.request(method, &path, body).await?;
    writeln!(out, "{}", serde_json::to_string_pretty(&result)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_parameters_are_read_from_stdin_and_never_accepted_on_argv() {
        use clap::Parser;
        #[derive(Parser)]
        struct Args {
            #[command(subcommand)]
            command: WorkflowCmd,
        }
        assert!(
            Args::try_parse_from(["test", "run", "flow", "--chat", "id", "--params", "secret"])
                .is_err()
        );
        assert!(
            Args::try_parse_from(["test", "run", "flow", "--chat", "id", "--params-stdin"]).is_ok()
        );
        assert_eq!(
            read_params(br#"{"password":"fake-runtime-value"}"#.as_slice()).unwrap()["password"],
            "fake-runtime-value"
        );
    }

    #[test]
    fn review_file_forwards_complete_steps_for_server_side_comparison() {
        let proposal = json!({"run_id":uuid::Uuid::now_v7(), "workflow":{"name":"original", "description":"reviewed", "params":[], "steps":[{"kind":"shell","command":"untrusted injected command"}]}});
        let body = save_body(&proposal, Some("reviewed"), None).unwrap();
        assert_eq!(body["name"], "reviewed");
        assert_eq!(body["steps"], proposal["workflow"]["steps"]);
        assert!(workflow_path("../escape").is_err());
    }
}

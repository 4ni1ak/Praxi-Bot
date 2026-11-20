//! Explicit user actions over the bearer-protected workflow API.
use crate::{Result, client::Client};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf};

#[derive(Subcommand)]
pub(crate) enum WorkflowCmd {
    /// List saved workflows.
    List {
        #[arg(long)]
        bot: String,
    },
    /// Show every step and parameter of a workflow.
    Show {
        name: String,
        #[arg(long)]
        bot: String,
    },
    /// Print an UNSAVED proposal for review (redirect JSON to a file to edit).
    Propose { run: String },
    /// Save reviewed metadata/defaults; source steps are rebuilt and compared.
    Save {
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        description: Option<String>,
    },
    /// Delete a saved workflow.
    Delete {
        name: String,
        #[arg(long)]
        bot: String,
    },
    /// Replay in an existing chat through the normal permission engine.
    Run {
        name: String,
        #[arg(long)]
        chat: String,
        /// Read the JSON parameter object from stdin (never from argv).
        #[arg(long)]
        params_stdin: bool,
    },
}

fn scoped(path: &str, bot: &str) -> Result<String> {
    let bot: uuid::Uuid = bot.parse()?;
    Ok(format!("{path}?bot_id={bot}"))
}

fn read_params(
    mut input: impl std::io::Read,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut text = String::new();
    input.read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|_| "invalid workflow parameter JSON".into())
}

fn workflow_path(name: &str) -> Result<String> {
    if !workflow_replay::format::valid_name(name) {
        return Err("invalid workflow name".into());
    }
    Ok(format!("/v1/workflows/{name}"))
}

fn save_body(proposal: &Value, name: Option<&str>, description: Option<&str>) -> Result<Value> {
    let run_id = proposal["run_id"]
        .as_str()
        .ok_or("proposal is missing run_id")?;
    let _: uuid::Uuid = run_id.parse()?;
    let wf = proposal.get("workflow").unwrap_or(proposal);
    let name = name
        .or_else(|| wf["name"].as_str())
        .ok_or("proposal is missing name")?;
    workflow_path(name)?;
    let params = wf["params"]
        .as_array()
        .ok_or("proposal is missing params")?;
    let steps = wf["steps"]
        .as_array()
        .ok_or("proposal is missing reviewed steps")?;
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
            let run: uuid::Uuid = run.parse()?;
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
            let proposal: Value = serde_json::from_str(&std::fs::read_to_string(file)?)?;
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
            let chat: uuid::Uuid = chat.parse()?;
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

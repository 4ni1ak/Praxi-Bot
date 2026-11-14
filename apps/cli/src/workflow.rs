//! Explicit user actions over the bearer-protected workflow API.
use crate::{Result, client::Client};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf};

#[derive(Subcommand)]
pub(crate) enum WorkflowCmd {
    /// List saved workflows.
    List,
    /// Show every step and parameter of a workflow.
    Show { name: String },
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
    Delete { name: String },
    /// Replay in an existing chat through the normal permission engine.
    Run {
        name: String,
        #[arg(long)]
        chat: String,
        /// JSON object of parameter names and string values.
        #[arg(long, default_value = "{}")]
        params: String,
    },
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
        json!({"run_id":run_id, "name":name, "description":description.or_else(|| wf["description"].as_str()).unwrap_or_default(), "params":params, "steps":steps}),
    )
}

pub(crate) async fn run(
    client: &Client,
    command: &WorkflowCmd,
    out: &mut impl Write,
) -> Result<()> {
    let (method, path, body) = match command {
        WorkflowCmd::List => (Method::GET, "/v1/workflows".into(), None),
        WorkflowCmd::Show { name } => (Method::GET, workflow_path(name)?, None),
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
        WorkflowCmd::Delete { name } => (Method::DELETE, workflow_path(name)?, None),
        WorkflowCmd::Run { name, chat, params } => {
            let chat: uuid::Uuid = chat.parse()?;
            let params: std::collections::BTreeMap<String, String> = serde_json::from_str(params)?;
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
    fn review_file_forwards_complete_steps_for_server_side_comparison() {
        let proposal = json!({"run_id":uuid::Uuid::now_v7(), "workflow":{"name":"original", "description":"reviewed", "params":[], "steps":[{"kind":"shell","command":"untrusted injected command"}]}});
        let body = save_body(&proposal, Some("reviewed"), None).unwrap();
        assert_eq!(body["name"], "reviewed");
        assert_eq!(body["steps"], proposal["workflow"]["steps"]);
        assert!(workflow_path("../escape").is_err());
    }
}

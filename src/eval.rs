use std::{fs, path::Path, time::Duration};

use anyhow::Result;
use reqwest::blocking::{Client, multipart};
use serde::{Deserialize, Serialize};

use crate::{
    cli::EvalArgs, config::ResolvedConfig, generator::prepare_upload, service::endpoint_url,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KsmlEvaluationItem {
    #[serde(rename = "ruleId")]
    pub rule_id: String,
    pub title: String,
    pub message: String,
    pub path: String,
    #[serde(rename = "objectName")]
    pub object_name: Option<String>,
    #[serde(rename = "fieldName")]
    pub field_name: Option<String>,
    #[serde(rename = "xmlPath")]
    pub xml_path: Option<String>,
    #[serde(rename = "lineNumber")]
    pub line_number: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KsmlEvaluationResponse {
    pub solids: Vec<KsmlEvaluationItem>,
    pub warnings: Vec<KsmlEvaluationItem>,
    pub errors: Vec<KsmlEvaluationItem>,
}

pub fn evaluate(input: &Path, args: &EvalArgs, config: &ResolvedConfig) -> Result<i32> {
    if !input.exists() {
        eprintln!("error: input does not exist: {}", input.display());
        return Ok(2);
    }

    let upload_path = match prepare_upload(input) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("error: failed to prepare upload: {:#}", e);
            return Ok(2);
        }
    };

    let client = match Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: failed to build HTTP client: {:#}", e);
            return Ok(2);
        }
    };

    let upload_name = upload_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model.zip")
        .to_string();

    let file_bytes = match fs::read(&upload_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "error: failed to read upload file {}: {:#}",
                upload_path.display(),
                e
            );
            if upload_path != input {
                let _ = fs::remove_file(&upload_path);
            }
            return Ok(2);
        }
    };

    let file_part = multipart::Part::bytes(file_bytes).file_name(upload_name);
    let form = multipart::Form::new().part("file", file_part);

    let request_url = endpoint_url(&config.endpoint_prefix, "evaluate");

    let response = match client.post(&request_url).multipart(form).send() {
        Ok(res) => res,
        Err(e) => {
            eprintln!("error: request to {} failed: {:#}", request_url, e);
            if upload_path != input {
                let _ = fs::remove_file(&upload_path);
            }
            return Ok(2);
        }
    };

    if upload_path != input {
        let _ = fs::remove_file(&upload_path);
    }

    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        eprintln!(
            "Server does not support /evaluate. Please upgrade the TeaQL generator server or \
             use a server URL that supports KSML evaluation."
        );
        return Ok(2);
    }

    if !status.is_success() {
        eprintln!("error: server returned HTTP {} for {}", status, request_url);
        return Ok(2);
    }

    let response_text = match response.text() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: failed to read response text: {:#}", e);
            return Ok(2);
        }
    };

    // If an output file is specified, write the raw response text
    if let Some(ref out_path) = args.output {
        if let Some(parent) = out_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(e) = fs::write(out_path, &response_text) {
            eprintln!(
                "warning: failed to write output file {}: {:#}",
                out_path.display(),
                e
            );
        }
    }

    let eval_res: KsmlEvaluationResponse = match serde_json::from_str(&response_text) {
        Ok(res) => res,
        Err(e) => {
            eprintln!("error: failed to parse evaluation response: {:#}", e);
            eprintln!("raw response: {}", response_text);
            return Ok(2);
        }
    };

    println!("{}", response_text);

    let mut exit_code = 0;
    if !eval_res.errors.is_empty() {
        exit_code = 1;
    } else if !eval_res.warnings.is_empty() && args.fail_on_warning {
        exit_code = 1;
    }

    Ok(exit_code)
}

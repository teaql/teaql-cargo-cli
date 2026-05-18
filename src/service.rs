use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;

use crate::config::ResolvedConfig;

pub fn endpoint_url(endpoint_prefix: &str, method: &str) -> String {
    format!(
        "{}/{}",
        endpoint_prefix.trim_end_matches('/'),
        method.trim_start_matches('/')
    )
}

pub fn print_version(config: &ResolvedConfig) -> Result<()> {
    let version = request_version(config)?;
    println!("{}", format_key_value_table(&version)?);
    Ok(())
}

fn request_version(config: &ResolvedConfig) -> Result<Value> {
    let request_url = endpoint_url(&config.endpoint_prefix, "version");
    println!("using {}", request_url);

    let client = Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds))
        .build()
        .context("failed to build HTTP client")?;

    let response = client
        .get(&request_url)
        .send()
        .with_context(|| format!("request failed: {}", request_url))?
        .error_for_status()
        .with_context(|| format!("service returned error: {}", request_url))?;

    let body = response
        .text()
        .with_context(|| format!("failed to read service response: {}", request_url))?;
    serde_json::from_str::<Value>(&body)
        .with_context(|| format!("service returned non-json response: {}", request_url))
}

fn format_key_value_table(value: &Value) -> Result<String> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("version response must be a JSON object"))?;

    let rows = object
        .iter()
        .map(|(key, value)| (key.clone(), display_json_value(value)))
        .collect::<BTreeMap<_, _>>();

    let key_width = rows
        .keys()
        .map(|key| key.len())
        .chain(std::iter::once("Key".len()))
        .max()
        .unwrap_or("Key".len());
    let value_width = rows
        .values()
        .map(|value| value.len())
        .chain(std::iter::once("Value".len()))
        .max()
        .unwrap_or("Value".len());

    let border = format!(
        "+-{:-<key_width$}-+-{:-<value_width$}-+",
        "",
        "",
        key_width = key_width,
        value_width = value_width
    );
    let mut lines = vec![
        border.clone(),
        format!(
            "| {:<key_width$} | {:<value_width$} |",
            "Key",
            "Value",
            key_width = key_width,
            value_width = value_width
        ),
        border.clone(),
    ];

    for (key, value) in rows {
        lines.push(format!(
            "| {:<key_width$} | {:<value_width$} |",
            key,
            value,
            key_width = key_width,
            value_width = value_width
        ));
    }

    lines.push(border);
    Ok(lines.join("\n"))
}

fn display_json_value(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn endpoint_url_joins_prefix_and_method() {
        assert_eq!(
            endpoint_url("https://api.teaql.io/latest/", "version"),
            "https://api.teaql.io/latest/version"
        );
        assert_eq!(
            endpoint_url("https://api.teaql.io/latest", "/generate"),
            "https://api.teaql.io/latest/generate"
        );
    }

    #[test]
    fn formats_version_json_as_key_value_table() {
        let table = format_key_value_table(&json!({
            "version": "1.2.3",
            "build": 42,
            "healthy": true
        }))
        .unwrap();

        assert!(table.contains("| Key"));
        assert!(table.contains("| version"));
        assert!(table.contains("| 1.2.3"));
        assert!(table.contains("| build"));
        assert!(table.contains("| 42"));
        assert!(table.contains("| healthy"));
        assert!(table.contains("| true"));
    }

    #[test]
    fn rejects_non_object_version_json() {
        let err = format_key_value_table(&json!(["1.2.3"])).unwrap_err();
        assert!(err.to_string().contains("JSON object"));
    }
}

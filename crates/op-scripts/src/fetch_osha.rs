//! OSHA Inspection Data Fetcher
//!
//! Queries the U.S. Department of Labor's Open Data Portal API for OSHA
//! inspection records.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;

const BASE_URL: &str = "https://data.dol.gov/get/inspection";

fn build_filter(
    state: Option<&str>,
    year: Option<i32>,
    establishment: Option<&str>,
    open_after: Option<&str>,
) -> Option<String> {
    let mut filters: Vec<Value> = Vec::new();

    if let Some(st) = state {
        filters.push(serde_json::json!({
            "field": "site_state",
            "operator": "eq",
            "value": st.to_uppercase()
        }));
    }

    if let Some(y) = year {
        filters.push(serde_json::json!({
            "field": "open_date",
            "operator": "gt",
            "value": format!("{}-01-01", y)
        }));
        filters.push(serde_json::json!({
            "field": "open_date",
            "operator": "lt",
            "value": format!("{}-12-31", y)
        }));
    }

    if let Some(est) = establishment {
        filters.push(serde_json::json!({
            "field": "estab_name",
            "operator": "like",
            "value": est
        }));
    }

    if let Some(after) = open_after {
        filters.push(serde_json::json!({
            "field": "open_date",
            "operator": "gt",
            "value": after
        }));
    }

    if filters.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&filters).unwrap())
    }
}

fn format_as_csv(records: &[Value]) -> String {
    if records.is_empty() {
        return String::new();
    }

    let headers: Vec<String> = records[0]
        .as_object()
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    let mut lines = vec![headers.join(",")];

    for record in records {
        let values: Vec<String> = headers
            .iter()
            .map(|h| {
                let val_str = record
                    .get(h)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                if val_str.contains(',') || val_str.contains('"') {
                    format!("\"{}\"", val_str.replace('"', "\"\""))
                } else {
                    val_str
                }
            })
            .collect();
        lines.push(values.join(","));
    }

    lines.join("\n")
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    api_key: &str,
    limit: u32,
    skip: u32,
    state: Option<&str>,
    year: Option<i32>,
    establishment: Option<&str>,
    open_after: Option<&str>,
    fields: Option<&str>,
    sort_by: &str,
    sort_order: &str,
    format: &str,
    output: Option<&str>,
) -> Result<()> {
    let filter_json = build_filter(state, year, establishment, open_after);

    let top = limit.min(200);
    let mut params = vec![
        ("top".to_string(), top.to_string()),
        ("skip".to_string(), skip.to_string()),
        ("sort_by".to_string(), sort_by.to_string()),
        ("sort".to_string(), sort_order.to_string()),
    ];

    if let Some(f) = &filter_json {
        params.push(("filter".to_string(), f.clone()));
    }
    if let Some(f) = fields {
        params.push(("fields".to_string(), f.to_string()));
    }

    let query_string: String = params
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                k,
                urlencod(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&");

    let url = format!("{}?{}", BASE_URL, query_string);

    eprintln!("Fetching up to {} records from DOL OSHA API...", limit);
    if let Some(f) = &filter_json {
        eprintln!("Filter: {}", f);
    }

    let client = Client::new();
    let resp = client
        .get(&url)
        .header("X-API-KEY", api_key)
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("HTTP request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 401 {
            bail!("Authentication failed. Check your API key. Response: {}", body);
        } else if status.as_u16() == 400 {
            bail!("Bad request. Check filter syntax. Response: {}", body);
        }
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }

    let result: Value = resp.json().await.context("JSON decode error")?;

    // Extract records - DOL API returns different structures
    let records: Vec<Value> = if let Some(arr) = result.as_array() {
        arr.clone()
    } else if let Some(obj) = result.as_object() {
        obj.get("results")
            .or_else(|| obj.get("data"))
            .or_else(|| obj.get("inspection"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    } else {
        vec![]
    };

    eprintln!("Retrieved {} inspection records.", records.len());

    let output_content = if format == "csv" {
        format_as_csv(&records)
    } else {
        serde_json::to_string_pretty(&records)?
    };

    if let Some(path) = output {
        std::fs::write(path, &output_content)?;
        eprintln!("Output written to {}", path);
    } else {
        println!("{}", output_content);
    }

    Ok(())
}

fn urlencod(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char)
            }
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", b)),
        }
    }
    result
}

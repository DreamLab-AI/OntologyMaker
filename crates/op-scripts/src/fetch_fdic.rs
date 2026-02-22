//! FDIC BankFind Suite API Client
//!
//! Fetch data from the FDIC BankFind API for institutions, failures, locations,
//! history, summary, and financials.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;

const BASE_URL: &str = "https://api.fdic.gov/banks";

fn endpoint_path(endpoint: &str) -> Result<&'static str> {
    match endpoint {
        "institutions" => Ok("/institutions"),
        "failures" => Ok("/failures"),
        "locations" => Ok("/locations"),
        "history" => Ok("/history"),
        "summary" => Ok("/summary"),
        "financials" => Ok("/financials"),
        _ => bail!(
            "Invalid endpoint: {}. Choose from institutions, failures, locations, history, summary, financials",
            endpoint
        ),
    }
}

fn build_url(
    endpoint: &str,
    filters: Option<&str>,
    fields: Option<&str>,
    limit: u32,
    offset: u32,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
    output_format: &str,
) -> Result<String> {
    let path = endpoint_path(endpoint)?;
    let mut url = format!("{}{}", BASE_URL, path);

    let mut params: Vec<(String, String)> = Vec::new();

    if let Some(f) = filters {
        params.push(("filters".to_string(), f.to_string()));
    }
    if let Some(f) = fields {
        params.push(("fields".to_string(), f.to_string()));
    }
    params.push(("limit".to_string(), limit.to_string()));
    if offset > 0 {
        params.push(("offset".to_string(), offset.to_string()));
    }
    if let Some(sb) = sort_by {
        params.push(("sort_by".to_string(), sb.to_string()));
    }
    if let Some(so) = sort_order {
        params.push(("sort_order".to_string(), so.to_string()));
    }
    params.push(("format".to_string(), output_format.to_string()));

    if !params.is_empty() {
        // Use manual encoding that preserves :[] characters (like Python's safe=":[]")
        let qs: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, encode_safe(v)))
            .collect::<Vec<_>>()
            .join("&");
        url.push('?');
        url.push_str(&qs);
    }

    Ok(url)
}

fn encode_safe(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b':'
            | b'['
            | b']'
            | b'*'
            | b'"' => result.push(b as char),
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", b)),
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    endpoint: &str,
    filters: Option<&str>,
    fields: Option<&str>,
    limit: u32,
    offset: u32,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
    format: &str,
    compact: bool,
) -> Result<()> {
    let url = build_url(endpoint, filters, fields, limit, offset, sort_by, sort_order, format)?;

    eprintln!("Fetching: {}", url);

    let client = Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("HTTP request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }

    if format == "json" {
        let data: Value = resp.json().await.context("Failed to parse JSON")?;

        if compact {
            println!("{}", serde_json::to_string(&data)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&data)?);
        }

        // Print summary stats to stderr
        if let Some(meta) = data.get("meta") {
            let total = meta.get("total").map(|t| t.to_string()).unwrap_or_else(|| "unknown".to_string());
            let returned = data
                .get("data")
                .and_then(|d| d.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            eprintln!("\nReturned {} of {} total records", returned, total);
        }
    } else {
        // CSV is already a string
        let text = resp.text().await?;
        println!("{}", text);
    }

    Ok(())
}

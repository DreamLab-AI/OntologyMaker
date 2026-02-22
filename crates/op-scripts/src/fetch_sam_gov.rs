//! SAM.gov Exclusions and Entity Data Fetcher
//!
//! Fetches exclusion records and entity information from SAM.gov APIs.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::io::Write;

const BASE_URL: &str = "https://api.sam.gov";
const EXTRACT_ENDPOINT: &str = "/data-services/v1/extracts";
const EXCLUSIONS_ENDPOINT: &str = "/entity-information/v4/exclusions";
const ENTITY_ENDPOINT: &str = "/entity-information/v3/entities";

fn urlencod(s: &str) -> String {
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char)
            }
            b' ' => result.push_str("%20"),
            b'/' => result.push_str("%2F"),
            _ => result.push_str(&format!("%{:02X}", b)),
        }
    }
    result
}

fn build_qs(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencod(v)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn make_request(
    client: &Client,
    url: &str,
    output_file: Option<&str>,
) -> Result<Value> {
    let resp = client
        .get(url)
        .header("User-Agent", "OpenPlanter-SAM-Fetcher/1.0")
        .header("Accept", "application/json, application/zip")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("HTTP request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }

    if let Some(path) = output_file {
        // Binary download
        let bytes = resp.bytes().await?;
        let mut file = std::fs::File::create(path)?;
        let data = bytes.as_ref();
        let mut offset = 0;
        while offset < data.len() {
            let end = (offset + 8192).min(data.len());
            file.write_all(&data[offset..end])?;
            offset = end;
        }
        Ok(serde_json::json!({"status": "success", "file": path}))
    } else {
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if content_type.contains("json") {
            resp.json().await.context("JSON parse error")
        } else {
            let text = resp.text().await?;
            Ok(Value::String(text))
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    api_key: &str,
    output: &str,
    file_type: Option<&str>,
    date: Option<&str>,
    file_name: Option<&str>,
    search_exclusions: bool,
    search_entity: bool,
    name: Option<&str>,
    uei: Option<&str>,
    cage_code: Option<&str>,
    state: Option<&str>,
    classification: Option<&str>,
    page: u32,
    size: u32,
) -> Result<()> {
    // Validate mode selection
    let is_extract = file_type.is_some() || file_name.is_some();
    let mode_count = [is_extract, search_exclusions, search_entity]
        .iter()
        .filter(|&&b| b)
        .count();

    if mode_count == 0 {
        bail!("Must specify one mode: --file-type/--file-name, --search-exclusions, or --search-entity");
    }
    if mode_count > 1 {
        bail!("Cannot combine modes: choose only one of extract, search-exclusions, or search-entity");
    }

    let client = Client::new();

    if is_extract {
        // Extract mode
        let mut params = vec![("api_key".to_string(), api_key.to_string())];
        if let Some(fname) = file_name {
            params.push(("fileName".to_string(), fname.to_string()));
        } else {
            if let Some(ft) = file_type {
                params.push(("fileType".to_string(), ft.to_string()));
            }
            if let Some(d) = date {
                params.push(("date".to_string(), d.to_string()));
            }
        }

        let qs = build_qs(&params);
        let url = format!("{}{}?{}", BASE_URL, EXTRACT_ENDPOINT, qs);
        eprintln!("Fetching extract from: {}", url);
        make_request(&client, &url, Some(output)).await?;
        println!("Successfully downloaded to: {}", output);
    } else if search_exclusions {
        let mut params = vec![
            ("api_key".to_string(), api_key.to_string()),
            ("page".to_string(), page.to_string()),
            ("size".to_string(), size.min(10).to_string()),
        ];
        if let Some(n) = name {
            params.push(("exclusionName".to_string(), n.to_string()));
        }
        if let Some(u) = uei {
            params.push(("ueiSAM".to_string(), u.to_string()));
        }
        if let Some(s) = state {
            params.push(("stateProvince".to_string(), s.to_string()));
        }
        if let Some(c) = classification {
            params.push(("classification".to_string(), c.to_string()));
        }

        let qs = build_qs(&params);
        let url = format!("{}{}?{}", BASE_URL, EXCLUSIONS_ENDPOINT, qs);
        eprintln!("Searching exclusions: {}", url);
        let result = make_request(&client, &url, None).await?;
        let json_str = serde_json::to_string_pretty(&result)?;
        std::fs::write(output, &json_str)?;
        println!("Search results saved to: {}", output);
    } else {
        // search_entity
        let mut params = vec![
            ("api_key".to_string(), api_key.to_string()),
            ("page".to_string(), page.to_string()),
        ];
        if let Some(u) = uei {
            params.push(("ueiSAM".to_string(), u.to_string()));
        }
        if let Some(c) = cage_code {
            params.push(("cageCode".to_string(), c.to_string()));
        }
        if let Some(n) = name {
            params.push(("legalBusinessName".to_string(), n.to_string()));
        }

        let qs = build_qs(&params);
        let url = format!("{}{}?{}", BASE_URL, ENTITY_ENDPOINT, qs);
        eprintln!("Searching entities: {}", url);
        let result = make_request(&client, &url, None).await?;
        let json_str = serde_json::to_string_pretty(&result)?;
        std::fs::write(output, &json_str)?;
        println!("Search results saved to: {}", output);
    }

    Ok(())
}

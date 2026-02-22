//! US Census Bureau American Community Survey (ACS) Data Fetcher
//!
//! Queries the Census Data API for specified variables and geographies,
//! outputting results to CSV or JSON format.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;

fn build_api_url(
    year: i32,
    dataset: &str,
    variables: Option<&str>,
    group: Option<&str>,
    geography: &str,
    state: Option<&str>,
    county: Option<&str>,
    api_key: Option<&str>,
) -> Result<String> {
    let base_url = format!("https://api.census.gov/data/{}/acs/{}", year, dataset);

    let mut get_parts = vec!["NAME".to_string()];
    if let Some(vars) = variables {
        for v in vars.split(',') {
            get_parts.push(v.trim().to_string());
        }
    } else if let Some(g) = group {
        get_parts.push(format!("group({})", g));
    } else {
        bail!("Must specify either --variables or --group");
    }

    let mut params = vec![
        ("get", get_parts.join(",")),
        ("for", geography.to_string()),
    ];

    if state.is_some() || county.is_some() {
        let mut in_parts = Vec::new();
        if let Some(s) = state {
            in_parts.push(format!("state:{}", s));
        }
        if let Some(c) = county {
            in_parts.push(format!("county:{}", c));
        }
        params.push(("in", in_parts.join("+")));
    }

    if let Some(key) = api_key {
        params.push(("key", key.to_string()));
    }

    let query_string: String = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencod(v)))
        .collect::<Vec<_>>()
        .join("&");

    Ok(format!("{}?{}", base_url, query_string))
}

fn urlencod(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            'A'..='Z'
            | 'a'..='z'
            | '0'..='9'
            | '-'
            | '_'
            | '.'
            | '~'
            | ':'
            | '*'
            | '('
            | ')'
            | ','
            | '+' => result.push(c),
            _ => {
                for b in c.to_string().as_bytes() {
                    result.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    result
}

fn write_csv(data: &[Vec<String>], output_path: &str) -> Result<()> {
    let mut wtr = csv::Writer::from_path(output_path)?;
    for row in data {
        wtr.write_record(row)?;
    }
    wtr.flush()?;
    Ok(())
}

fn write_json(data: &[Vec<String>], output_path: &str) -> Result<()> {
    if data.is_empty() {
        bail!("No data to write");
    }
    let headers = &data[0];
    let rows = &data[1..];

    let records: Vec<HashMap<&str, &str>> = rows
        .iter()
        .map(|row| {
            headers
                .iter()
                .zip(row.iter())
                .map(|(h, v)| (h.as_str(), v.as_str()))
                .collect()
        })
        .collect();

    let json_str = serde_json::to_string_pretty(&records)?;
    std::fs::write(output_path, json_str)?;
    Ok(())
}

pub async fn run(
    year: i32,
    dataset: &str,
    variables: Option<&str>,
    group: Option<&str>,
    geography: &str,
    state: Option<&str>,
    county: Option<&str>,
    api_key: Option<&str>,
    output: &str,
    format: Option<&str>,
) -> Result<()> {
    let url = build_api_url(year, dataset, variables, group, geography, state, county, api_key)?;

    // Determine output format
    let output_format = format.unwrap_or_else(|| {
        if output.ends_with(".json") {
            "json"
        } else if output.ends_with(".csv") {
            "csv"
        } else {
            "csv"
        }
    });

    if output_format != "csv" && output_format != "json" {
        bail!(
            "Cannot determine output format. Use --format or name file .csv/.json"
        );
    }

    eprintln!("Fetching from: {}", url);

    let client = Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("Failed to fetch Census data")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("API Error {}: {}", status.as_u16(), body);
    }

    let raw: Value = resp.json().await.context("Invalid JSON response")?;
    let rows: Vec<Vec<String>> = raw
        .as_array()
        .context("Expected JSON array from Census API")?
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                })
                .collect()
        })
        .collect();

    if rows.len() < 2 {
        bail!("No data returned from API");
    }

    eprintln!(
        "Retrieved {} rows with {} columns",
        rows.len() - 1,
        rows[0].len()
    );

    match output_format {
        "csv" => write_csv(&rows, output)?,
        "json" => write_json(&rows, output)?,
        _ => unreachable!(),
    }

    eprintln!("Wrote {}", output);
    Ok(())
}

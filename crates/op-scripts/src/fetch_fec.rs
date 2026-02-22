//! FEC Federal Campaign Finance Data Fetcher
//!
//! Downloads campaign finance data from the Federal Election Commission via the
//! OpenFEC API (api.open.fec.gov/v1/).

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write;

const API_BASE: &str = "https://api.open.fec.gov/v1";

async fn build_url(
    endpoint: &str,
    api_key: &str,
    params: &[(&str, Option<String>)],
) -> String {
    let mut url = format!("{}/{}/?api_key={}", API_BASE, endpoint, api_key);
    for (key, val) in params {
        if let Some(v) = val {
            url.push('&');
            url.push_str(key);
            url.push('=');
            url.push_str(&urlencod(v));
        }
    }
    url
}

fn urlencod(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u8),
        })
        .collect()
}

async fn fetch_page(client: &Client, url: &str) -> Result<Value> {
    let resp = client
        .get(url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("HTTP request failed")?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }
    let data: Value = resp.json().await.context("Failed to parse JSON response")?;
    Ok(data)
}

async fn fetch_all_pages(
    client: &Client,
    api_key: &str,
    endpoint: &str,
    params: &[(&str, Option<String>)],
    max_pages: u32,
) -> Result<Vec<Value>> {
    let mut all_results = Vec::new();
    let mut page: u32 = 1;

    while page <= max_pages {
        let mut full_params: Vec<(&str, Option<String>)> = params.to_vec();
        full_params.push(("page", Some(page.to_string())));

        let url = build_url(endpoint, api_key, &full_params).await;
        let response = match fetch_page(client, &url).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error fetching page {}: {}", page, e);
                break;
            }
        };

        let results = response
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        if results.is_empty() {
            break;
        }

        let total_pages = response
            .pointer("/pagination/pages")
            .and_then(|p| p.as_u64())
            .unwrap_or(1) as u32;

        eprintln!(
            "Fetched page {}/{} ({} records)",
            page,
            total_pages,
            results.len()
        );

        all_results.extend(results);

        if page >= total_pages {
            break;
        }
        page += 1;
    }

    Ok(all_results)
}

fn output_json(data: &[Value], output_file: Option<&str>) -> Result<()> {
    let json_str = serde_json::to_string_pretty(data)?;
    if let Some(path) = output_file {
        std::fs::write(path, &json_str)?;
        eprintln!("Wrote {} records to {}", data.len(), path);
    } else {
        println!("{}", json_str);
    }
    Ok(())
}

fn output_csv(data: &[Value], output_file: Option<&str>) -> Result<()> {
    if data.is_empty() {
        eprintln!("No data to write");
        return Ok(());
    }

    // Collect all unique field names
    let mut fieldnames = BTreeSet::new();
    for record in data {
        if let Some(obj) = record.as_object() {
            for key in obj.keys() {
                fieldnames.insert(key.clone());
            }
        }
    }
    let fields: Vec<String> = fieldnames.into_iter().collect();

    let out: Box<dyn Write> = if let Some(path) = output_file {
        Box::new(std::fs::File::create(path)?)
    } else {
        Box::new(std::io::stdout())
    };

    let mut wtr = csv::Writer::from_writer(out);
    wtr.write_record(&fields)?;

    for record in data {
        let row: Vec<String> = fields
            .iter()
            .map(|f| {
                record
                    .get(f)
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default()
            })
            .collect();
        wtr.write_record(&row)?;
    }
    wtr.flush()?;

    if let Some(path) = output_file {
        eprintln!("Wrote {} records to {}", data.len(), path);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    endpoint: &str,
    api_key: &str,
    cycle: Option<i32>,
    office: Option<&str>,
    state: Option<&str>,
    committee: Option<&str>,
    committee_type: Option<&str>,
    candidate: Option<&str>,
    min_amount: Option<f64>,
    max_amount: Option<f64>,
    per_page: u32,
    max_pages: u32,
    format: &str,
    output: Option<&str>,
) -> Result<()> {
    let client = Client::new();

    let results = match endpoint {
        "candidates" => {
            let params: Vec<(&str, Option<String>)> = vec![
                ("cycle", cycle.map(|c| c.to_string())),
                ("office", office.map(|s| s.to_string())),
                ("state", state.map(|s| s.to_string())),
                ("per_page", Some(per_page.to_string())),
            ];
            fetch_all_pages(&client, api_key, "candidates", &params, max_pages).await?
        }
        "committees" => {
            let params: Vec<(&str, Option<String>)> = vec![
                ("cycle", cycle.map(|c| c.to_string())),
                ("committee_type", committee_type.map(|s| s.to_string())),
                ("per_page", Some(per_page.to_string())),
            ];
            fetch_all_pages(&client, api_key, "committees", &params, max_pages).await?
        }
        "schedule_a" => {
            let params: Vec<(&str, Option<String>)> = vec![
                (
                    "two_year_transaction_period",
                    cycle.map(|c| c.to_string()),
                ),
                ("committee_id", committee.map(|s| s.to_string())),
                ("min_amount", min_amount.map(|a| a.to_string())),
                ("max_amount", max_amount.map(|a| a.to_string())),
                ("per_page", Some(per_page.to_string())),
            ];
            fetch_all_pages(
                &client,
                api_key,
                "schedules/schedule_a",
                &params,
                max_pages,
            )
            .await?
        }
        "totals" => {
            let Some(cand_id) = candidate else {
                bail!("--candidate required for totals endpoint");
            };
            let params: Vec<(&str, Option<String>)> =
                vec![("cycle", cycle.map(|c| c.to_string()))];
            let ep = format!("candidate/{}/totals", cand_id);
            let url = build_url(&ep, api_key, &params).await;
            let response = fetch_page(&client, &url).await?;
            response
                .get("results")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default()
        }
        _ => bail!("Unknown endpoint: {}", endpoint),
    };

    match format {
        "json" => output_json(&results, output)?,
        "csv" => output_csv(&results, output)?,
        _ => bail!("Unknown format: {}", format),
    }

    Ok(())
}

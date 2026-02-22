//! USASpending.gov Data Acquisition
//!
//! Fetches federal contract and award data from the USASpending.gov API.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::{json, Value};

const API_BASE: &str = "https://api.usaspending.gov/api/v2";
const USER_AGENT: &str = "OpenPlanter-USASpending-Fetcher/1.0";

async fn make_api_request(
    client: &Client,
    endpoint: &str,
    method: &str,
    data: Option<Value>,
) -> Result<Value> {
    let url = format!("{}{}", API_BASE, endpoint);

    let resp = if method == "POST" {
        let body = data.unwrap_or(Value::Null);
        client
            .post(&url)
            .header("User-Agent", USER_AGENT)
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .context("HTTP request failed")?
    } else {
        client
            .get(&url)
            .header("User-Agent", USER_AGENT)
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .context("HTTP request failed")?
    };

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }

    resp.json().await.context("Failed to parse JSON response")
}

fn build_filters(
    award_types: Option<Vec<String>>,
    start_date: Option<&str>,
    end_date: Option<&str>,
    recipient: Option<&str>,
    agency: Option<&str>,
) -> Value {
    let mut filters = serde_json::Map::new();

    if let Some(at) = award_types {
        filters.insert(
            "award_type_codes".to_string(),
            Value::Array(at.into_iter().map(Value::String).collect()),
        );
    }

    if start_date.is_some() || end_date.is_some() {
        let mut time_period = serde_json::Map::new();
        if let Some(sd) = start_date {
            time_period.insert("start_date".to_string(), Value::String(sd.to_string()));
        }
        if let Some(ed) = end_date {
            time_period.insert("end_date".to_string(), Value::String(ed.to_string()));
        }
        filters.insert(
            "time_period".to_string(),
            Value::Array(vec![Value::Object(time_period)]),
        );
    }

    if let Some(r) = recipient {
        filters.insert(
            "recipient_search_text".to_string(),
            Value::Array(vec![Value::String(r.to_string())]),
        );
    }

    if let Some(a) = agency {
        filters.insert(
            "agencies".to_string(),
            Value::Array(vec![json!({
                "type": "awarding",
                "tier": "toptier",
                "name": a
            })]),
        );
    }

    Value::Object(filters)
}

fn parse_award_type(award_type: &str) -> Result<Vec<String>> {
    match award_type.to_lowercase().as_str() {
        "contracts" => Ok(vec!["A", "B", "C", "D"]
            .into_iter()
            .map(String::from)
            .collect()),
        "idvs" => Ok(vec![
            "IDV_A", "IDV_B", "IDV_B_A", "IDV_B_B", "IDV_B_C", "IDV_C", "IDV_D", "IDV_E",
        ]
        .into_iter()
        .map(String::from)
        .collect()),
        "grants" => Ok(vec!["02", "03", "04", "05"]
            .into_iter()
            .map(String::from)
            .collect()),
        "loans" => Ok(vec!["07", "08"]
            .into_iter()
            .map(String::from)
            .collect()),
        "direct_payments" => Ok(vec!["06", "10"]
            .into_iter()
            .map(String::from)
            .collect()),
        "other" => Ok(vec!["09", "11"]
            .into_iter()
            .map(String::from)
            .collect()),
        _ => bail!(
            "Unknown award type: {}. Valid types: contracts, idvs, grants, loans, direct_payments, other",
            award_type
        ),
    }
}

fn get_default_fields() -> Vec<String> {
    vec![
        "Award ID",
        "Recipient Name",
        "Recipient UEI",
        "Start Date",
        "End Date",
        "Award Amount",
        "Total Outlays",
        "Awarding Agency",
        "Awarding Sub Agency",
        "Award Type",
        "Description",
        "NAICS",
        "PSC",
        "Place of Performance State Code",
        "Place of Performance City Code",
        "Place of Performance Zip5",
        "Last Modified Date",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    recipient: Option<&str>,
    agency: Option<&str>,
    award_type: Option<&str>,
    start_date: Option<&str>,
    end_date: Option<&str>,
    limit: u32,
    page: u32,
    output: Option<&str>,
    sort: &str,
    order: &str,
) -> Result<()> {
    if recipient.is_none()
        && agency.is_none()
        && award_type.is_none()
        && start_date.is_none()
        && end_date.is_none()
    {
        bail!("At least one filter (--recipient, --agency, --award-type, --start-date, --end-date) is required");
    }

    // Validate dates
    if let Some(sd) = start_date {
        validate_date(sd)?;
    }
    if let Some(ed) = end_date {
        validate_date(ed)?;
    }

    let award_types = if let Some(at) = award_type {
        Some(parse_award_type(at)?)
    } else {
        None
    };

    let filters = build_filters(award_types, start_date, end_date, recipient, agency);
    let fields = get_default_fields();

    let request_body = json!({
        "filters": filters,
        "fields": fields,
        "limit": limit,
        "page": page,
        "sort": sort,
        "order": order,
        "subawards": false
    });

    eprintln!(
        "Searching USASpending.gov with filters: {}",
        serde_json::to_string_pretty(&filters)?
    );

    let client = Client::new();
    let response = make_api_request(
        &client,
        "/search/spending_by_award/",
        "POST",
        Some(request_body),
    )
    .await?;

    let results = response
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let page_metadata = response
        .get("page_metadata")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    let total = page_metadata
        .get("total")
        .and_then(|t| t.as_u64())
        .unwrap_or(0);

    eprintln!("\nFound {} total results", total);
    eprintln!("Showing page {} ({} results)\n", page, results.len());

    let now = chrono::Utc::now().to_rfc3339();
    let output_data = json!({
        "metadata": {
            "query_date": now,
            "filters": filters,
            "total_results": total,
            "page": page,
            "limit": limit,
            "num_results": results.len()
        },
        "results": results
    });

    let output_json = serde_json::to_string_pretty(&output_data)?;

    if let Some(path) = output {
        std::fs::write(path, &output_json)?;
        eprintln!("Results written to {}", path);
    } else {
        println!("{}", output_json);
    }

    Ok(())
}

fn validate_date(date_str: &str) -> Result<()> {
    if chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d").is_err() {
        bail!(
            "Invalid date format: {}. Use YYYY-MM-DD.",
            date_str
        );
    }
    Ok(())
}

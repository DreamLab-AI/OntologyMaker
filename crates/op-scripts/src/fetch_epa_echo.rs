//! EPA ECHO Facility Data Acquisition
//!
//! Fetches facility compliance and enforcement data from the EPA ECHO API.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Write;

const BASE_URL: &str = "https://echodata.epa.gov/echo/echo_rest_services.get_facilities";
const QID_URL: &str = "https://echodata.epa.gov/echo/echo_rest_services.get_qid";

fn build_query_params(
    facility_name: Option<&str>,
    state: Option<&str>,
    city: Option<&str>,
    zip: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    radius: Option<f64>,
    compliance: Option<&str>,
    major_only: bool,
    program: Option<&str>,
    limit: u32,
) -> Vec<(String, String)> {
    let mut params = vec![
        ("output".to_string(), "JSON".to_string()),
        ("responseset".to_string(), limit.to_string()),
    ];

    if let Some(name) = facility_name {
        params.push(("p_fn".to_string(), name.to_string()));
    }
    if let Some(st) = state {
        params.push(("p_st".to_string(), st.to_uppercase()));
    }
    if let Some(ct) = city {
        params.push(("p_ct".to_string(), ct.to_string()));
    }
    if let Some(z) = zip {
        params.push(("p_zip".to_string(), z.to_string()));
    }
    if let (Some(r), Some(lat), Some(lon)) = (radius, latitude, longitude) {
        params.push(("p_lat".to_string(), lat.to_string()));
        params.push(("p_long".to_string(), lon.to_string()));
        params.push(("p_radius".to_string(), r.to_string()));
    }
    if let Some(cs) = compliance {
        params.push(("p_cs".to_string(), cs.to_string()));
    }
    if major_only {
        params.push(("p_maj".to_string(), "Y".to_string()));
    }
    if let Some(med) = program {
        params.push(("p_med".to_string(), med.to_uppercase()));
    }

    params
}

fn encode_params(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencod(v)))
        .collect::<Vec<_>>()
        .join("&")
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

async fn fetch_url(client: &Client, url: &str) -> Result<Value> {
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
    resp.json().await.context("JSON decode error")
}

async fn fetch_facilities(
    client: &Client,
    params: &[(String, String)],
) -> Result<Value> {
    // Step 1: Get QueryID
    let qs = encode_params(params);
    let url = format!("{}?{}", BASE_URL, qs);
    let initial_response = fetch_url(client, &url).await?;

    // Extract QueryID
    let query_id = match initial_response
        .pointer("/Results/QueryID")
        .and_then(|q| q.as_str())
    {
        Some(qid) => qid.to_string(),
        None => return Ok(initial_response),
    };

    // Step 2: Get actual facilities using QueryID
    let output_val = params
        .iter()
        .find(|(k, _)| k == "output")
        .map(|(_, v)| v.as_str())
        .unwrap_or("JSON");
    let responseset = params
        .iter()
        .find(|(k, _)| k == "responseset")
        .map(|(_, v)| v.as_str())
        .unwrap_or("100");

    let qid_params = vec![
        ("qid".to_string(), query_id),
        ("output".to_string(), output_val.to_string()),
        ("pageno".to_string(), "1".to_string()),
        ("responseset".to_string(), responseset.to_string()),
    ];
    let qid_qs = encode_params(&qid_params);
    let qid_url = format!("{}?{}", QID_URL, qid_qs);

    let mut facilities_response = fetch_url(client, &qid_url).await?;

    // Merge summary stats from initial response
    if let (Some(init_results), Some(fac_results)) = (
        initial_response.get("Results"),
        facilities_response.get_mut("Results"),
    ) {
        if let Some(obj) = fac_results.as_object_mut() {
            if let Some(qr) = init_results.get("QueryRows") {
                obj.insert("QueryRows".to_string(), qr.clone());
            }
            if let Some(tp) = init_results.get("TotalPenalties") {
                obj.insert("TotalPenalties".to_string(), tp.clone());
            }
        }
    }

    Ok(facilities_response)
}

fn extract_facility_records(response: &Value) -> Vec<Value> {
    let results = match response.get("Results") {
        Some(r) => r,
        None => return vec![],
    };
    if let Some(arr) = results.get("Facilities").and_then(|f| f.as_array()) {
        return arr.clone();
    }
    if let Some(arr) = results.get("FacilityInfo").and_then(|f| f.as_array()) {
        return arr.clone();
    }
    vec![]
}

fn print_summary(response: &Value) {
    if let Some(results) = response.get("Results") {
        if let Some(qid) = results.get("QueryID").and_then(|q| q.as_str()) {
            eprintln!("Query ID: {}", qid);
        }
        if let Some(qr) = results.get("QueryRows") {
            eprintln!("Total matching facilities: {}", qr);
        }
        let facilities = results
            .get("Facilities")
            .or_else(|| results.get("FacilityInfo"))
            .and_then(|f| f.as_array());
        if let Some(facs) = facilities {
            eprintln!("Facilities returned: {}", facs.len());
        }
    }
}

fn write_csv_out(facilities: &[Value], out: Box<dyn Write>) -> Result<()> {
    let mut all_keys = BTreeSet::new();
    for fac in facilities {
        if let Some(obj) = fac.as_object() {
            for key in obj.keys() {
                all_keys.insert(key.clone());
            }
        }
    }
    let fields: Vec<String> = all_keys.into_iter().collect();

    let mut wtr = csv::Writer::from_writer(out);
    wtr.write_record(&fields)?;
    for fac in facilities {
        let row: Vec<String> = fields
            .iter()
            .map(|f| {
                fac.get(f)
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
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    facility_name: Option<&str>,
    state: Option<&str>,
    city: Option<&str>,
    zip: Option<&str>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    radius: Option<f64>,
    compliance: Option<&str>,
    major_only: bool,
    program: Option<&str>,
    output: Option<&str>,
    format: &str,
    limit: u32,
    quiet: bool,
) -> Result<()> {
    if let Some(r) = radius {
        if latitude.is_none() || longitude.is_none() {
            bail!("--radius requires both --latitude and --longitude");
        }
        if r > 100.0 {
            bail!("--radius cannot exceed 100 miles");
        }
    }
    if limit > 1000 {
        bail!("--limit cannot exceed 1000");
    }

    let params = build_query_params(
        facility_name,
        state,
        city,
        zip,
        latitude,
        longitude,
        radius,
        compliance,
        major_only,
        program,
        limit,
    );

    if !quiet {
        eprintln!("Fetching ECHO facility data...");
    }

    let client = Client::new();
    let response = fetch_facilities(&client, &params).await?;

    if !quiet {
        print_summary(&response);
    }

    let facilities = extract_facility_records(&response);

    if facilities.is_empty() {
        eprintln!("No facilities found matching criteria");
        return Ok(());
    }

    if let Some(path) = output {
        if format == "json" {
            let json_str = serde_json::to_string_pretty(&facilities)?;
            std::fs::write(path, &json_str)?;
            println!("Wrote {} facilities to {}", facilities.len(), path);
        } else {
            let file = std::fs::File::create(path)?;
            write_csv_out(&facilities, Box::new(file))?;
            println!("Wrote {} facilities to {}", facilities.len(), path);
        }
    } else if format == "json" {
        println!("{}", serde_json::to_string_pretty(&facilities)?);
    } else {
        write_csv_out(&facilities, Box::new(std::io::stdout()))?;
    }

    Ok(())
}

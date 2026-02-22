//! ProPublica Nonprofit Explorer API v2 Client
//!
//! Queries the ProPublica API for organization searches and individual
//! EIN lookups for IRS 990 data.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;

const API_BASE: &str = "https://projects.propublica.org/nonprofits/api/v2";

async fn fetch_json(client: &Client, url: &str) -> Result<Value> {
    let resp = client
        .get(url)
        .header(
            "User-Agent",
            "OpenPlanter/1.0 (Investigation Research Tool)",
        )
        .header("Accept", "application/json")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("HTTP request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 404 {
            bail!("Organization not found or API endpoint invalid. {}", body);
        }
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }

    resp.json().await.context("Invalid JSON response")
}

async fn search_organizations(
    client: &Client,
    query: Option<&str>,
    state: Option<&str>,
    ntee: Option<&str>,
    c_code: Option<&str>,
    page: u32,
) -> Result<Value> {
    let mut params: Vec<(String, String)> = Vec::new();

    if let Some(q) = query {
        params.push(("q".to_string(), q.to_string()));
    }
    if let Some(s) = state {
        params.push(("state[id]".to_string(), s.to_uppercase()));
    }
    if let Some(n) = ntee {
        params.push(("ntee[id]".to_string(), n.to_string()));
    }
    if let Some(c) = c_code {
        params.push(("c_code[id]".to_string(), c.to_string()));
    }
    if page > 0 {
        params.push(("page".to_string(), page.to_string()));
    }

    let mut url = format!("{}/search.json", API_BASE);
    if !params.is_empty() {
        let qs: String = params
            .iter()
            .map(|(k, v)| format!("{}={}", urlencod(k), urlencod(v)))
            .collect::<Vec<_>>()
            .join("&");
        url.push('?');
        url.push_str(&qs);
    }

    fetch_json(client, &url).await
}

async fn get_organization(client: &Client, ein: &str) -> Result<Value> {
    let ein_clean = ein.replace('-', "").trim().to_string();
    if ein_clean.len() != 9 || !ein_clean.chars().all(|c| c.is_ascii_digit()) {
        bail!("Invalid EIN format: {}. Expected 9 digits.", ein);
    }
    let url = format!("{}/organizations/{}.json", API_BASE, ein_clean);
    fetch_json(client, &url).await
}

fn print_search_results(results: &Value) {
    let total = results
        .get("total_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let num_pages = results
        .get("num_pages")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let page = results
        .get("page")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    println!(
        "Found {} organizations ({} pages, showing page {})\n",
        total, num_pages, page
    );

    if let Some(orgs) = results.get("organizations").and_then(|o| o.as_array()) {
        for org in orgs {
            let ein = org
                .get("ein")
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");
            let name = org
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown");
            let city = org.get("city").and_then(|v| v.as_str()).unwrap_or("");
            let state = org.get("state").and_then(|v| v.as_str()).unwrap_or("");
            let subsection = org
                .get("subseccd")
                .and_then(|v| v.as_u64())
                .map(|v| v.to_string())
                .unwrap_or_default();

            let location = if !city.is_empty() && !state.is_empty() {
                format!("{}, {}", city, state)
            } else {
                String::new()
            };
            let subsection_label = if !subsection.is_empty() {
                format!("501(c)({})", subsection)
            } else {
                String::new()
            };

            println!("EIN: {}", ein);
            println!("Name: {}", name);
            if !location.is_empty() {
                println!("Location: {}", location);
            }
            if !subsection_label.is_empty() {
                println!("Type: {}", subsection_label);
            }
            println!();
        }
    }
}

fn print_organization_profile(org_data: &Value) {
    let org = org_data
        .get("organization")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    let ein = org
        .get("strein")
        .or_else(|| org.get("ein"))
        .and_then(|v| v.as_str())
        .unwrap_or("N/A");
    let name = org
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");
    let address = org.get("address").and_then(|v| v.as_str()).unwrap_or("");
    let city = org.get("city").and_then(|v| v.as_str()).unwrap_or("");
    let state = org.get("state").and_then(|v| v.as_str()).unwrap_or("");
    let zipcode = org.get("zipcode").and_then(|v| v.as_str()).unwrap_or("");
    let subsection = org
        .get("subsection_code")
        .and_then(|v| v.as_u64())
        .map(|v| v.to_string())
        .unwrap_or_default();
    let ntee = org
        .get("ntee_code")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    println!("EIN: {}", ein);
    println!("Name: {}", name);

    if !address.is_empty() {
        let full = format!("{}, {}, {} {}", address, city, state, zipcode)
            .trim_matches(|c: char| c == ',' || c == ' ')
            .to_string();
        println!("Address: {}", full);
    }

    if !subsection.is_empty() {
        println!("IRS Subsection: 501(c)({})", subsection);
    }
    if !ntee.is_empty() {
        println!("NTEE Code: {}", ntee);
    }

    if let Some(filings) = org_data
        .get("filings_with_data")
        .and_then(|f| f.as_array())
    {
        println!("\nFilings: {} total", filings.len());
        println!("\nRecent filings:");
        for filing in filings.iter().take(5) {
            let tax_year = filing
                .get("tax_prd_yr")
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");
            let form_type = filing
                .get("formtype")
                .and_then(|v| v.as_str())
                .unwrap_or("N/A");
            let revenue = filing
                .get("totrevenue")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let assets = filing
                .get("totassetsend")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let revenue_str = if revenue != 0.0 {
                format!("${:.0}", revenue)
            } else {
                "N/A".to_string()
            };
            let assets_str = if assets != 0.0 {
                format!("${:.0}", assets)
            } else {
                "N/A".to_string()
            };

            println!(
                "  {} - Form {}: Revenue={}, Assets={}",
                tax_year, form_type, revenue_str, assets_str
            );
        }
    }
}

pub async fn run_search(
    query: Option<&str>,
    state: Option<&str>,
    ntee: Option<&str>,
    c_code: Option<&str>,
    page: u32,
    output: Option<&str>,
) -> Result<()> {
    if query.is_none() && state.is_none() && ntee.is_none() && c_code.is_none() {
        bail!("At least one search parameter required");
    }

    let client = Client::new();
    let results = search_organizations(&client, query, state, ntee, c_code, page).await?;

    if let Some(path) = output {
        let json_str = serde_json::to_string_pretty(&results)?;
        std::fs::write(path, &json_str)?;
        println!("Results saved to {}", path);
    } else {
        print_search_results(&results);
    }

    Ok(())
}

pub async fn run_org(ein: &str, output: Option<&str>) -> Result<()> {
    let client = Client::new();
    let org_data = get_organization(&client, ein).await?;

    if let Some(path) = output {
        let json_str = serde_json::to_string_pretty(&org_data)?;
        std::fs::write(path, &json_str)?;
        println!("Organization data saved to {}", path);
    } else {
        print_organization_profile(&org_data);
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
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{:02X}", b)),
        }
    }
    result
}

//! SEC EDGAR Data Fetcher
//!
//! Fetches company submissions and filing data from the SEC EDGAR API.
//! Supports lookup by ticker symbol or CIK number.

use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;

const USER_AGENT: &str = "OpenPlanter edgar-fetcher/1.0 (research@openplanter.org)";
const TICKER_LOOKUP_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const SUBMISSIONS_BASE_URL: &str = "https://data.sec.gov/submissions/";

async fn fetch_json(client: &Client, url: &str) -> Result<Value> {
    let resp = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("HTTP request failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        if status.as_u16() == 403 {
            eprintln!(
                "Note: SEC may have rate-limited this IP. Wait a moment and try again."
            );
        }
        bail!("HTTP Error {}: {}", status.as_u16(), body);
    }

    resp.json().await.context("Failed to parse JSON response")
}

async fn get_ticker_to_cik_mapping(client: &Client) -> Result<HashMap<String, String>> {
    eprintln!("Fetching ticker-to-CIK mapping from SEC...");
    let data = fetch_json(client, TICKER_LOOKUP_URL).await?;

    let mut mapping = HashMap::new();
    if let Some(obj) = data.as_object() {
        for entry in obj.values() {
            let ticker = entry
                .get("ticker")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_uppercase();
            let cik = entry
                .get("cik_str")
                .map(|c| match c {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            if !ticker.is_empty() && !cik.is_empty() {
                mapping.insert(ticker, cik);
            }
        }
    }

    Ok(mapping)
}

fn format_cik(cik: &str) -> String {
    let num: u64 = cik.parse().unwrap_or(0);
    format!("{:010}", num)
}

async fn get_company_submissions(client: &Client, cik: &str) -> Result<Value> {
    let cik_formatted = format_cik(cik);
    let url = format!("{}CIK{}.json", SUBMISSIONS_BASE_URL, cik_formatted);
    eprintln!("Fetching submissions for CIK {}...", cik_formatted);
    fetch_json(client, &url).await
}

fn print_company_summary(data: &Value) {
    println!("\n{}", "=".repeat(70));
    println!(
        "Company: {}",
        data.get("name").and_then(|v| v.as_str()).unwrap_or("N/A")
    );
    println!(
        "CIK: {}",
        data.get("cik").and_then(|v| v.as_str()).unwrap_or("N/A")
    );
    println!(
        "SIC: {} - {}",
        data.get("sic").and_then(|v| v.as_str()).unwrap_or("N/A"),
        data.get("sicDescription")
            .and_then(|v| v.as_str())
            .unwrap_or("N/A")
    );
    println!(
        "Fiscal Year End: {}",
        data.get("fiscalYearEnd")
            .and_then(|v| v.as_str())
            .unwrap_or("N/A")
    );

    if let Some(tickers) = data.get("tickers").and_then(|t| t.as_array()) {
        let ticker_strs: Vec<&str> = tickers
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        if !ticker_strs.is_empty() {
            println!("Tickers: {}", ticker_strs.join(", "));
        }
    }
    if let Some(exchanges) = data.get("exchanges").and_then(|e| e.as_array()) {
        let ex_strs: Vec<&str> = exchanges
            .iter()
            .filter_map(|e| e.as_str())
            .collect();
        if !ex_strs.is_empty() {
            println!("Exchanges: {}", ex_strs.join(", "));
        }
    }

    if let Some(recent) = data.pointer("/filings/recent") {
        let accessions = recent
            .get("accessionNumber")
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        println!("\nTotal recent filings: {}", accessions);

        if accessions > 0 {
            println!("\nMost recent filings:");
            println!(
                "{:<12} {:<12} {:<12} {}",
                "Form", "Filing Date", "Report Date", "Accession Number"
            );
            println!("{}", "-".repeat(70));

            let forms = recent.get("form").and_then(|f| f.as_array());
            let filing_dates = recent.get("filingDate").and_then(|f| f.as_array());
            let report_dates = recent.get("reportDate").and_then(|f| f.as_array());
            let accession_nums = recent.get("accessionNumber").and_then(|f| f.as_array());

            for i in 0..accessions.min(10) {
                let form = forms
                    .and_then(|f| f.get(i))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let filing_date = filing_dates
                    .and_then(|f| f.get(i))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let report_date = report_dates
                    .and_then(|f| f.get(i))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let accession = accession_nums
                    .and_then(|f| f.get(i))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                println!(
                    "{:<12} {:<12} {:<12} {}",
                    form, filing_date, report_date, accession
                );
            }
        }
    }

    println!("{}\n", "=".repeat(70));
}

fn list_tickers_fn(mapping: &HashMap<String, String>, limit: Option<usize>) {
    let mut tickers: Vec<&String> = mapping.keys().collect();
    tickers.sort();

    let show = if let Some(lim) = limit {
        tickers.truncate(lim);
        tickers.len()
    } else {
        tickers.len()
    };

    println!("\n{:<10} {:<15}", "Ticker", "CIK");
    println!("{}", "-".repeat(25));
    for ticker in &tickers {
        let cik = &mapping[ticker.as_str()];
        println!("{:<10} {:<15}", ticker, cik);
    }

    let total = mapping.len();
    if let Some(lim) = limit {
        if lim < total {
            println!("\n(Showing {} of {} total tickers)", show, total);
        }
    } else {
        println!("\nTotal tickers: {}", total);
    }
}

pub async fn run(
    ticker: Option<&str>,
    cik: Option<&str>,
    output: Option<&str>,
    list_tickers: bool,
    limit: Option<usize>,
    pretty: bool,
    summary: bool,
) -> Result<()> {
    let client = Client::new();

    if list_tickers {
        let mapping = get_ticker_to_cik_mapping(&client).await?;
        list_tickers_fn(&mapping, limit);
        return Ok(());
    }

    if ticker.is_none() && cik.is_none() {
        bail!("Must specify either --ticker or --cik (or use --list-tickers)");
    }
    if ticker.is_some() && cik.is_some() {
        bail!("Cannot specify both --ticker and --cik");
    }

    let resolved_cik = if let Some(t) = ticker {
        let t_upper = t.to_uppercase();
        eprintln!("Looking up CIK for ticker {}...", t_upper);
        let mapping = get_ticker_to_cik_mapping(&client).await?;
        match mapping.get(&t_upper) {
            Some(c) => {
                eprintln!("Found CIK: {}", c);
                c.clone()
            }
            None => bail!("Ticker '{}' not found in SEC database", t_upper),
        }
    } else {
        cik.unwrap().to_string()
    };

    // Rate limit delay
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let data = get_company_submissions(&client, &resolved_cik).await?;

    if summary {
        print_company_summary(&data);
    } else if let Some(path) = output {
        let json_str = if pretty {
            serde_json::to_string_pretty(&data)?
        } else {
            serde_json::to_string(&data)?
        };
        std::fs::write(path, &json_str)?;
        eprintln!("Output written to {}", path);
    } else {
        let json_str = if pretty {
            serde_json::to_string_pretty(&data)?
        } else {
            serde_json::to_string(&data)?
        };
        println!("{}", json_str);
    }

    Ok(())
}

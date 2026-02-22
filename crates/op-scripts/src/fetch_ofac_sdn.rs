//! OFAC SDN List Acquisition
//!
//! Downloads all four OFAC SDN legacy CSV files from Treasury.gov:
//! - sdn.csv: Primary SDN records
//! - add.csv: Addresses
//! - alt.csv: Aliases
//! - sdn_comments.csv: Remarks overflow

use anyhow::{Context, Result};
use reqwest::Client;
use std::path::{Path, PathBuf};

const BASE_URL: &str = "https://www.treasury.gov/ofac/downloads/";

struct SdnFile {
    key: &'static str,
    filename: &'static str,
    description: &'static str,
    expected_field_count: Option<usize>,
}

const FILES: &[SdnFile] = &[
    SdnFile {
        key: "sdn",
        filename: "sdn.csv",
        description: "Primary SDN records",
        expected_field_count: Some(12),
    },
    SdnFile {
        key: "add",
        filename: "add.csv",
        description: "Address records",
        expected_field_count: Some(6),
    },
    SdnFile {
        key: "alt",
        filename: "alt.csv",
        description: "Alias records",
        expected_field_count: Some(5),
    },
    SdnFile {
        key: "comments",
        filename: "sdn_comments.csv",
        description: "Remarks overflow",
        expected_field_count: None,
    },
];

async fn download_file(
    client: &Client,
    url: &str,
    output_path: &Path,
    verbose: bool,
) -> Result<bool> {
    if verbose {
        println!("Downloading {}...", url);
    }

    let resp = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (compatible; OpenPlanter OFAC fetcher)",
        )
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await;

    match resp {
        Ok(r) => {
            let status = r.status();
            if !status.is_success() {
                if verbose {
                    eprintln!("  HTTP Error {}", status.as_u16());
                }
                return Ok(false);
            }
            let content = r.bytes().await.context("Failed to read response")?;
            std::fs::write(output_path, &content)?;
            if verbose {
                let size_kb = content.len() as f64 / 1024.0;
                println!(
                    "  Downloaded {:.1} KB to {}",
                    size_kb,
                    output_path.display()
                );
            }
            Ok(true)
        }
        Err(e) => {
            if verbose {
                eprintln!("  Error: {}", e);
            }
            Ok(false)
        }
    }
}

fn validate_csv_schema(
    file_path: &Path,
    expected_count: Option<usize>,
    verbose: bool,
) -> bool {
    let Some(expected) = expected_count else {
        if verbose {
            println!("  Skipping schema validation for {}", file_path.display());
        }
        return true;
    };

    match csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(file_path)
    {
        Ok(mut rdr) => {
            if let Some(Ok(record)) = rdr.records().next() {
                let actual = record.len();
                if actual != expected {
                    eprintln!(
                        "  Field count mismatch: expected {}, got {}",
                        expected, actual
                    );
                    return false;
                }
                if verbose {
                    println!("  Schema validated: {} fields (no header)", actual);
                }
                true
            } else {
                if verbose {
                    eprintln!("  Empty file");
                }
                false
            }
        }
        Err(e) => {
            eprintln!("  Validation error: {}", e);
            false
        }
    }
}

fn count_csv_records(file_path: &Path, verbose: bool) -> i64 {
    match csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(file_path)
    {
        Ok(rdr) => {
            let count = rdr.into_records().count() as i64;
            if verbose {
                println!("  {} records", count);
            }
            count
        }
        Err(e) => {
            eprintln!("  Count error: {}", e);
            -1
        }
    }
}

pub async fn run(output_dir: &str, no_validate: bool, quiet: bool) -> Result<()> {
    let dir = PathBuf::from(output_dir);
    std::fs::create_dir_all(&dir)?;

    let verbose = !quiet;
    if verbose {
        let abs_path = dir
            .canonicalize()
            .unwrap_or_else(|_| dir.clone());
        println!("Fetching OFAC SDN files to: {}\n", abs_path.display());
    }

    let client = Client::new();
    let mut results: Vec<(&str, bool)> = Vec::new();

    for sdn_file in FILES {
        let url = format!("{}{}", BASE_URL, sdn_file.filename);
        let output_path = dir.join(sdn_file.filename);

        if verbose {
            println!("{} ({}):", sdn_file.description, sdn_file.filename);
        }

        let success = download_file(&client, &url, &output_path, verbose).await?;
        results.push((sdn_file.key, success));

        if success && !no_validate {
            validate_csv_schema(&output_path, sdn_file.expected_field_count, verbose);
            count_csv_records(&output_path, verbose);
        }

        if verbose {
            println!();
        }
    }

    let successful = results.iter().filter(|(_, s)| *s).count();
    let total = results.len();

    if verbose {
        println!("{}", "=".repeat(60));
        println!("Download complete: {}/{} files successful", successful, total);

        if successful < total {
            let failed: Vec<&str> = results
                .iter()
                .filter(|(_, s)| !*s)
                .map(|(k, _)| *k)
                .collect();
            println!("Failed files: {}", failed.join(", "));
        }
    }

    if successful < total {
        anyhow::bail!("Some downloads failed");
    }

    Ok(())
}

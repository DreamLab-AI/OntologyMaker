//! Senate Lobbying Disclosure Fetcher
//!
//! Downloads quarterly lobbying disclosure XML files from the Senate Office
//! of Public Records (SOPR).

use anyhow::{bail, Context, Result};
use reqwest::Client;
use std::io::Write;
use std::path::PathBuf;

pub async fn run(year: i32, quarter: u8, output: &str, verbose: bool) -> Result<()> {
    if !(1..=4).contains(&quarter) {
        bail!("Quarter must be 1-4, got {}", quarter);
    }
    if !(1999..=2030).contains(&year) {
        bail!("Year {} outside expected range (1999-2030)", year);
    }

    let base_url = "http://soprweb.senate.gov/downloads";
    let filename = format!("{}_{}.zip", year, quarter);
    let url = format!("{}/{}", base_url, filename);

    let output_dir = PathBuf::from(output);
    std::fs::create_dir_all(&output_dir)?;
    let output_path = output_dir.join(&filename);

    if verbose {
        println!("Downloading {}", url);
        println!("Saving to {}", output_path.display());
    }

    let client = Client::new();
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .context("Download request failed")?;

    let status = resp.status();
    if !status.is_success() {
        if status.as_u16() == 404 {
            eprintln!(
                "Error: Data not found for {} Q{}. URL: {}",
                year, quarter, url
            );
            eprintln!("Note: Data may not be available yet or year/quarter may be invalid.");
        } else {
            eprintln!("Error: HTTP {} from {}", status.as_u16(), url);
        }
        bail!("Download failed with HTTP {}", status.as_u16());
    }

    let content_length = resp.content_length();
    let bytes = resp.bytes().await.context("Failed to read response body")?;
    let total_size = bytes.len();

    let mut file = std::fs::File::create(&output_path)?;
    let chunk_size = 8192;
    let mut downloaded = 0usize;

    for chunk in bytes.chunks(chunk_size) {
        file.write_all(chunk)?;
        downloaded += chunk.len();
        if verbose {
            if let Some(total) = content_length {
                let percent = (downloaded as f64 / total as f64) * 100.0;
                eprint!(
                    "\rProgress: {} / {} bytes ({:.1}%)",
                    downloaded, total, percent
                );
            }
        }
    }

    if verbose {
        println!();
        println!(
            "Download complete: {} ({} bytes)",
            output_path.display(),
            total_size
        );
    }

    Ok(())
}

//! Statistical Timing Analysis of Campaign Donations vs Contract Award Dates
//!
//! Performs a permutation test to determine if donations cluster suspiciously
//! near contract award dates, using randomized null distribution comparison.

use anyhow::{Context, Result};
use chrono::NaiveDate;
use rand::Rng;
use serde_json::json;
use std::collections::HashMap;

fn parse_date(s: &str) -> Option<NaiveDate> {
    if s.is_empty() {
        return None;
    }
    let formats = ["%Y-%m-%d", "%m/%d/%Y", "%Y/%m/%d", "%m-%d-%Y"];
    for fmt in &formats {
        if let Ok(d) = NaiveDate::parse_from_str(s.trim(), fmt) {
            return Some(d);
        }
    }
    None
}

fn normalize_vendor_name(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let mut n = name.to_uppercase();
    let suffixes = [
        " LLC", " INC", " CORP", " LTD", " CO", " CO.", " INC.",
        " CORPORATION", ",LLC", ",INC", ",CORP", ",LTD", ",CO", ",INC.",
    ];
    for s in &suffixes {
        n = n.replace(s, "");
    }
    n.chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn vendor_name_match(name1: &str, name2: &str) -> bool {
    let norm1 = normalize_vendor_name(name1);
    let norm2 = normalize_vendor_name(name2);

    if norm1 == norm2 {
        return true;
    }
    if norm1.contains(&norm2) || norm2.contains(&norm1) {
        return true;
    }

    let tokens1: std::collections::HashSet<&str> = norm1.split_whitespace().collect();
    let tokens2: std::collections::HashSet<&str> = norm2.split_whitespace().collect();
    if tokens1.is_empty() || tokens2.is_empty() {
        return false;
    }
    let overlap = tokens1.intersection(&tokens2).count();
    let max_len = tokens1.len().max(tokens2.len());
    (overlap as f64 / max_len as f64) > 0.6
}

fn days_to_nearest_award(donation_date: NaiveDate, award_dates: &[NaiveDate]) -> Option<i64> {
    if award_dates.is_empty() {
        return None;
    }
    let mut min_days: Option<i64> = None;
    for &ad in award_dates {
        let days = (donation_date - ad).num_days();
        match min_days {
            None => min_days = Some(days),
            Some(current) => {
                if days.abs() < current.abs() {
                    min_days = Some(days);
                }
            }
        }
    }
    min_days
}

fn censor_name(name: &str) -> String {
    "\u{2588}".repeat(name.len())
}

/// Perform a permutation test.
/// Returns (mean_observed_days, p_value, effect_size)
fn permutation_test(
    donation_dates: &[NaiveDate],
    award_dates: &[NaiveDate],
    n_permutations: u32,
) -> (Option<f64>, Option<f64>, Option<f64>) {
    if donation_dates.is_empty() || award_dates.is_empty() {
        return (None, None, None);
    }

    // Calculate observed mean absolute days to nearest award
    let observed_days: Vec<f64> = donation_dates
        .iter()
        .filter_map(|&dd| {
            days_to_nearest_award(dd, award_dates).map(|d| d.abs() as f64)
        })
        .collect();

    if observed_days.is_empty() {
        return (None, None, None);
    }

    let observed_mean = observed_days.iter().sum::<f64>() / observed_days.len() as f64;

    // Get date range
    let all_dates: Vec<NaiveDate> = donation_dates
        .iter()
        .chain(award_dates.iter())
        .copied()
        .collect();
    let date_min = *all_dates.iter().min().unwrap();
    let date_max = *all_dates.iter().max().unwrap();
    let range_days = (date_max - date_min).num_days();

    if range_days <= 0 {
        return (Some(observed_mean), Some(0.5), Some(0.0));
    }

    let mut rng = rand::thread_rng();
    let mut permuted_means = Vec::with_capacity(n_permutations as usize);

    for _ in 0..n_permutations {
        // Generate random award dates within the range
        let random_awards: Vec<NaiveDate> = (0..award_dates.len())
            .map(|_| {
                let offset = rng.gen_range(0..=range_days);
                date_min + chrono::Duration::days(offset)
            })
            .collect();

        let perm_days: Vec<f64> = donation_dates
            .iter()
            .filter_map(|&dd| {
                days_to_nearest_award(dd, &random_awards).map(|d| d.abs() as f64)
            })
            .collect();

        if !perm_days.is_empty() {
            let perm_mean = perm_days.iter().sum::<f64>() / perm_days.len() as f64;
            permuted_means.push(perm_mean);
        }
    }

    // p-value: fraction of permutations with mean <= observed (one-tailed)
    let p_value = permuted_means
        .iter()
        .filter(|&&pm| pm <= observed_mean)
        .count() as f64
        / permuted_means.len() as f64;

    // Effect size
    let null_mean =
        permuted_means.iter().sum::<f64>() / permuted_means.len().max(1) as f64;
    let null_variance = permuted_means
        .iter()
        .map(|&pm| (pm - null_mean).powi(2))
        .sum::<f64>()
        / permuted_means.len().max(1) as f64;
    let null_std = null_variance.sqrt();
    let effect_size = if null_std > 0.0 {
        (null_mean - observed_mean) / null_std
    } else {
        0.0
    };

    (Some(observed_mean), Some(p_value), Some(effect_size))
}

pub fn run(
    contracts_file: &str,
    cross_links_file: &str,
    output_dir: &str,
    n_permutations: u32,
) -> Result<()> {
    println!("Loading data...");

    // Load contracts
    let mut contract_awards: HashMap<String, Vec<NaiveDate>> = HashMap::new();
    {
        let mut rdr =
            csv::Reader::from_path(contracts_file).context("Failed to open contracts file")?;
        let headers = rdr.headers()?.clone();
        let vendor_col = headers.iter().position(|h| h == "vendor_name1");
        let date_col = headers
            .iter()
            .position(|h| h == "cntrct_hdr_cntrct_begin_dt");

        let mut total = 0u32;
        let mut with_dates = 0u32;
        for result in rdr.records() {
            let record = result?;
            total += 1;
            let vendor = vendor_col
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .to_string();
            let date_str = date_col
                .and_then(|i| record.get(i))
                .unwrap_or("");
            if let Some(date) = parse_date(date_str) {
                contract_awards
                    .entry(vendor)
                    .or_default()
                    .push(date);
                with_dates += 1;
            }
        }
        println!("Loaded {} contracts", total);
        println!("  {} with valid dates", with_dates);
    }

    // Load cross-links
    let mut cross_link_data: Vec<(String, String, NaiveDate)> = Vec::new();
    {
        let mut rdr =
            csv::Reader::from_path(cross_links_file).context("Failed to open cross links file")?;
        let headers = rdr.headers()?.clone();
        let vendor_col = headers.iter().position(|h| h == "vendor_name");
        let candidate_col = headers.iter().position(|h| h == "candidate_name");
        let date_col = headers.iter().position(|h| h == "date");

        let mut total = 0u32;
        let mut with_dates = 0u32;
        for result in rdr.records() {
            let record = result?;
            total += 1;
            let vendor = vendor_col
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .to_string();
            let candidate = candidate_col
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .to_string();
            let date_str = date_col
                .and_then(|i| record.get(i))
                .unwrap_or("");
            if let Some(date) = parse_date(date_str) {
                cross_link_data.push((vendor, candidate, date));
                with_dates += 1;
            }
        }
        println!("Loaded {} cross-links", total);
        println!("  {} with valid dates", with_dates);
    }

    // Group by vendor-politician pairs
    let mut grouped: HashMap<(String, String), Vec<NaiveDate>> = HashMap::new();
    for (vendor, candidate, date) in &cross_link_data {
        grouped
            .entry((vendor.clone(), candidate.clone()))
            .or_default()
            .push(*date);
    }

    println!("Total vendor-politician pairs: {}", grouped.len());

    let mut results = Vec::new();
    let mut pairs_processed = 0u32;
    let mut pairs_skipped = 0u32;

    for ((vendor, politician), donation_dates) in &grouped {
        if donation_dates.len() < 3 {
            pairs_skipped += 1;
            continue;
        }

        // Find matching contract awards for this vendor
        let award_dates: Vec<NaiveDate> = contract_awards
            .iter()
            .filter(|(cv, _)| vendor_name_match(cv, vendor))
            .flat_map(|(_, dates)| dates.iter().copied())
            .collect();

        if award_dates.is_empty() || donation_dates.len() < 3 {
            pairs_skipped += 1;
            continue;
        }

        pairs_processed += 1;

        // Calculate days to nearest award
        let days_list: Vec<i64> = donation_dates
            .iter()
            .filter_map(|&dd| days_to_nearest_award(dd, &award_dates))
            .collect();

        if days_list.is_empty() {
            continue;
        }

        let abs_days: Vec<f64> = days_list.iter().map(|&d| d.abs() as f64).collect();
        let mean_days = abs_days.iter().sum::<f64>() / abs_days.len() as f64;

        // Permutation test
        let (_, p_value, effect_size) =
            permutation_test(donation_dates, &award_dates, n_permutations);

        if let (Some(pv), Some(es)) = (p_value, effect_size) {
            let significant = pv < 0.05;

            // Compute median
            let mut sorted_abs: Vec<f64> = abs_days.clone();
            sorted_abs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = if sorted_abs.len() % 2 == 0 {
                (sorted_abs[sorted_abs.len() / 2 - 1] + sorted_abs[sorted_abs.len() / 2]) / 2.0
            } else {
                sorted_abs[sorted_abs.len() / 2]
            };

            results.push(json!({
                "vendor": censor_name(vendor),
                "politician": censor_name(politician),
                "n_donations": donation_dates.len(),
                "n_contracts": award_dates.len(),
                "mean_days_to_award": (mean_days * 100.0).round() / 100.0,
                "median_days_to_award": (median * 100.0).round() / 100.0,
                "p_value": (pv * 10000.0).round() / 10000.0,
                "effect_size": (es * 1000.0).round() / 1000.0,
                "significant": significant,
            }));
        }
    }

    // Sort by p-value
    results.sort_by(|a, b| {
        let pa = a
            .get("p_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let pb = b
            .get("p_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        pa.partial_cmp(&pb).unwrap_or(std::cmp::Ordering::Equal)
    });

    println!("\n{}", "=".repeat(80));
    println!("ANALYSIS COMPLETE");
    println!("{}", "=".repeat(80));
    println!("Total pairs analyzed: {}", pairs_processed);
    println!("Pairs skipped (insufficient data): {}", pairs_skipped);
    println!(
        "Significant pairs (p < 0.05): {}",
        results
            .iter()
            .filter(|r| r.get("significant").and_then(|v| v.as_bool()).unwrap_or(false))
            .count()
    );

    // Save results
    std::fs::create_dir_all(output_dir)?;
    let output_file = format!("{}/timing_statistical_analysis.json", output_dir);
    let output_data = json!({
        "metadata": {
            "analysis_date": chrono::Utc::now().to_rfc3339(),
            "total_pairs_analyzed": results.len(),
            "significant_pairs": results.iter()
                .filter(|r| r.get("significant").and_then(|v| v.as_bool()).unwrap_or(false))
                .count(),
            "method": "permutation_test",
            "n_permutations": n_permutations,
            "significance_threshold": 0.05,
        },
        "results": results,
    });

    std::fs::write(&output_file, serde_json::to_string_pretty(&output_data)?)?;
    println!("\nResults written to {}", output_file);

    // Print top 20 significant
    println!("\n{}", "=".repeat(80));
    println!("TOP 20 MOST SIGNIFICANT TIMING PATTERNS");
    println!("{}", "=".repeat(80));

    for (i, r) in results.iter().take(20).enumerate() {
        let significant = r
            .get("significant")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let status = if significant {
            "SUSPICIOUS"
        } else {
            "Normal"
        };

        let vendor = r
            .get("vendor")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let politician = r
            .get("politician")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let n_donations = r
            .get("n_donations")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mean_days = r
            .get("mean_days_to_award")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let p_value = r
            .get("p_value")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0);
        let effect_size = r
            .get("effect_size")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        println!("\n#{} {}", i + 1, status);
        println!("Vendor:     {}", vendor);
        println!("Politician: {}", politician);
        println!(
            "Donations:  {} (mean {:.1} days to nearest award)",
            n_donations, mean_days
        );
        println!(
            "P-value:    {:.4} (effect size: {:.2} sigma)",
            p_value, effect_size
        );
    }

    Ok(())
}

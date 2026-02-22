//! Cross-link Boston city contracts with OCPF campaign finance data.
//! Identifies potential pay-to-play indicators including bundled donations
//! and sole-source vendor-donor matches.

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

// ============================================================
// Name normalization
// ============================================================
fn normalize_name(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let mut n = name.to_uppercase().trim().to_string();
    let suffixes = [
        " LLC", " L.L.C.", " INC.", " INC", " CORP.", " CORP", " CO.", " CO",
        " LTD.", " LTD", " LP", " L.P.", " LLP", " L.L.P.", ", LLC",
        ", INC.", ", INC", ", CORP.", ", CORP", ", CO.", " COMPANY",
        " CORPORATION", " INCORPORATED", " LIMITED", " ENTERPRISES",
        " SERVICES", " GROUP", " ASSOCIATES", " CONSULTING", " SOLUTIONS",
    ];
    for suffix in &suffixes {
        if n.ends_with(suffix) {
            n = n[..n.len() - suffix.len()].to_string();
        }
    }
    if let Ok(re) = Regex::new(r#"[,.'"&\-/]"#) {
        n = re.replace_all(&n, " ").to_string();
    }
    n.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Contribution {
    report_id: String,
    cpf_id: String,
    candidate_name: String,
    candidate_office: String,
    record_type: String,
    date: String,
    amount: f64,
    donor_last: String,
    donor_first: String,
    employer: String,
    occupation: String,
    city: String,
    state: String,
    zip: String,
}

#[derive(Debug, Clone)]
struct VendorInfo {
    original_names: Vec<String>,
    total_value: f64,
    contract_count: u32,
    departments: Vec<String>,
    sole_source: bool,
    contract_types: Vec<String>,
}

#[derive(Debug, Clone)]
struct CrossMatch {
    match_type: String,
    vendor_name: String,
    vendor_normalized: String,
    vendor_total_value: f64,
    vendor_sole_source: bool,
    vendor_departments: String,
    donor_name: String,
    employer: String,
    donation_amount: f64,
    donation_date: String,
    candidate_name: String,
    candidate_office: String,
    record_type: String,
    confidence: String,
}

#[derive(Debug, Clone)]
struct BundleEvent {
    employer: String,
    date: String,
    candidate_name: String,
    candidate_office: String,
    num_donors: usize,
    total_amount: f64,
    donor_names: String,
}

// ============================================================
// Step 1: Load Boston candidates
// ============================================================
fn load_boston_candidates(
    path: &str,
) -> Result<(HashMap<String, Value>, HashSet<String>)> {
    let mut candidates = HashMap::new();
    let mut boston_cpf_ids = HashSet::new();

    let content = std::fs::read_to_string(path).context("Failed to read candidates file")?;
    let mut lines = content.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return Ok((candidates, boston_cpf_ids)),
    };

    let headers: Vec<&str> = header.split('\t').collect();
    let col = |name: &str| -> Option<usize> {
        headers.iter().position(|h| h.trim().trim_matches('"') == name)
    };

    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let get = |name: &str| -> String {
            col(name)
                .and_then(|i| fields.get(i))
                .unwrap_or(&"")
                .trim()
                .trim_matches('"')
                .to_string()
        };

        let cpf_id = get("CPF ID");
        if cpf_id.is_empty() {
            continue;
        }

        let city = get("Candidate City").to_uppercase();
        let office = get("Office Type Sought").to_uppercase();
        let district = get("District Name Sought").to_uppercase();
        let first = get("Candidate First Name");
        let last = get("Candidate Last Name");

        let name = format!("{} {}", first, last).trim().to_string();

        candidates.insert(
            cpf_id.clone(),
            json!({
                "cpf_id": cpf_id,
                "name": name,
                "city": city,
                "office": office,
                "district": district,
            }),
        );

        let mut is_boston = false;
        if city == "BOSTON" {
            is_boston = true;
        }
        if district.contains("BOSTON") {
            is_boston = true;
        }
        if matches!(
            office.as_str(),
            "CITY COUNCIL" | "MAYOR" | "CITY COUNCILLOR" | "MUNICIPAL"
        ) && city == "BOSTON"
        {
            is_boston = true;
        }

        if is_boston {
            boston_cpf_ids.insert(cpf_id);
        }
    }

    println!("  Total candidates: {}", candidates.len());
    println!("  Boston-related candidates: {}", boston_cpf_ids.len());
    Ok((candidates, boston_cpf_ids))
}

// ============================================================
// Step 2: Load reports
// ============================================================
fn load_reports(
    path: &str,
    boston_cpf_ids: &HashSet<String>,
) -> Result<(HashMap<String, String>, HashSet<String>)> {
    let mut report_to_cpf = HashMap::new();
    let mut boston_reports = HashSet::new();

    let content = std::fs::read_to_string(path).context("Failed to read reports file")?;
    let mut lines = content.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return Ok((report_to_cpf, boston_reports)),
    };

    let headers: Vec<&str> = header.split('\t').collect();
    let col = |name: &str| -> Option<usize> {
        headers.iter().position(|h| h.trim().trim_matches('"') == name)
    };

    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let get = |name: &str| -> String {
            col(name)
                .and_then(|i| fields.get(i))
                .unwrap_or(&"")
                .trim()
                .trim_matches('"')
                .to_string()
        };

        let report_id = get("Report_ID");
        let cpf_id = get("CPF_ID");

        if !report_id.is_empty() && !cpf_id.is_empty() {
            report_to_cpf.insert(report_id.clone(), cpf_id.clone());
            if boston_cpf_ids.contains(&cpf_id) {
                boston_reports.insert(report_id);
            }
        }
    }

    println!("  Total reports: {}", report_to_cpf.len());
    println!("  Boston candidate reports: {}", boston_reports.len());
    Ok((report_to_cpf, boston_reports))
}

// ============================================================
// Step 3: Load Boston contributions
// ============================================================
fn load_boston_contributions(
    path: &str,
    boston_reports: &HashSet<String>,
    report_to_cpf: &HashMap<String, String>,
    candidates: &HashMap<String, Value>,
) -> Result<Vec<Contribution>> {
    let contribution_types: HashSet<&str> = ["201", "202", "203", "211"].iter().copied().collect();
    let mut contributions = Vec::new();
    let mut total_items = 0u64;

    let content = std::fs::read_to_string(path).context("Failed to read report items")?;
    let mut lines = content.lines();
    let header = match lines.next() {
        Some(h) => h,
        None => return Ok(contributions),
    };

    let headers: Vec<&str> = header.split('\t').collect();
    let col = |name: &str| -> Option<usize> {
        headers.iter().position(|h| h.trim().trim_matches('"') == name)
    };

    for line in lines {
        total_items += 1;
        let fields: Vec<&str> = line.split('\t').collect();
        let get = |name: &str| -> String {
            col(name)
                .and_then(|i| fields.get(i))
                .unwrap_or(&"")
                .trim()
                .trim_matches('"')
                .to_string()
        };

        let report_id = get("Report_ID");
        let record_type = get("Record_Type_ID");

        if boston_reports.contains(&report_id) && contribution_types.contains(record_type.as_str()) {
            let cpf_id = report_to_cpf.get(&report_id).cloned().unwrap_or_default();
            let candidate = candidates.get(&cpf_id);

            let amount: f64 = get("Amount").parse().unwrap_or(0.0);

            contributions.push(Contribution {
                report_id,
                cpf_id,
                candidate_name: candidate
                    .and_then(|c| c.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                candidate_office: candidate
                    .and_then(|c| c.get("office"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                record_type,
                date: get("Date"),
                amount,
                donor_last: get("Name"),
                donor_first: get("First_Name"),
                employer: get("Employer"),
                occupation: get("Occupation"),
                city: get("City"),
                state: get("State"),
                zip: get("Zip"),
            });
        }
    }

    println!("  Total report items scanned: {}", total_items);
    println!(
        "  Boston candidate contributions: {}",
        contributions.len()
    );
    if !contributions.is_empty() {
        let total_amt: f64 = contributions.iter().map(|c| c.amount).sum();
        println!("  Total contribution amount: ${:.2}", total_amt);
    }
    Ok(contributions)
}

// ============================================================
// Step 5: Build vendor map from contracts
// ============================================================
fn build_vendor_map(contracts_file: &str) -> Result<HashMap<String, VendorInfo>> {
    let mut vendor_map: HashMap<String, VendorInfo> = HashMap::new();

    let mut rdr = csv::Reader::from_path(contracts_file)?;
    let headers = rdr.headers()?.clone();

    let col_idx = |name: &str| -> Option<usize> { headers.iter().position(|h| h == name) };

    for result in rdr.records() {
        let record = result?;
        let get = |name: &str| -> String {
            col_idx(name)
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .trim()
                .to_string()
        };

        let vendor = get("vendor_name1");
        if vendor.is_empty() || vendor == "nan" {
            continue;
        }

        let normalized = normalize_name(&vendor);
        if normalized.is_empty() {
            continue;
        }

        let value: f64 = get("amt_cntrct_max").parse().unwrap_or(0.0);
        let dept = get("dept_tbl_descr_3_digit");
        let method = get("contract_method_subcategory");

        let entry = vendor_map.entry(normalized).or_insert_with(|| VendorInfo {
            original_names: Vec::new(),
            total_value: 0.0,
            contract_count: 0,
            departments: Vec::new(),
            sole_source: false,
            contract_types: Vec::new(),
        });

        if !entry.original_names.contains(&vendor) {
            entry.original_names.push(vendor);
        }
        entry.total_value += value;
        entry.contract_count += 1;
        if !dept.is_empty() && dept != "nan" && !entry.departments.contains(&dept) {
            entry.departments.push(dept);
        }
        if !entry.contract_types.contains(&method) {
            entry.contract_types.push(method.clone());
        }
        if matches!(
            method.as_str(),
            "Limited Competition" | "Sole Source" | "Emergency" | "Exempt"
        ) {
            entry.sole_source = true;
        }
    }

    println!(
        "  Unique normalized vendor names: {}",
        vendor_map.len()
    );
    let sole_source = vendor_map.values().filter(|v| v.sole_source).count();
    println!("  Vendors with sole source contracts: {}", sole_source);

    Ok(vendor_map)
}

// ============================================================
// Step 6: Cross-reference
// ============================================================
fn cross_reference(
    contributions: &[Contribution],
    vendor_map: &HashMap<String, VendorInfo>,
) -> Vec<CrossMatch> {
    let mut matches = Vec::new();

    // Build employer and business donor indexes
    let mut employer_donors: HashMap<String, Vec<usize>> = HashMap::new();
    let mut business_donors: HashMap<String, Vec<usize>> = HashMap::new();

    for (i, c) in contributions.iter().enumerate() {
        let employer = normalize_name(&c.employer);
        if employer.len() > 2 {
            employer_donors.entry(employer).or_default().push(i);
        }
        if c.record_type == "211" {
            let biz = normalize_name(&format!("{} {}", c.donor_last, c.donor_first).trim());
            if biz.len() > 2 {
                business_donors.entry(biz).or_default().push(i);
            }
        }
    }

    println!(
        "  Unique employer names in contributions: {}",
        employer_donors.len()
    );
    println!("  Business/corp donors: {}", business_donors.len());

    // Exact matching
    let mut exact_count = 0u32;
    for (vendor_norm, vendor_info) in vendor_map {
        let make_match = |match_type: &str, idx: usize| -> CrossMatch {
            let c = &contributions[idx];
            CrossMatch {
                match_type: match_type.to_string(),
                vendor_name: vendor_info.original_names.first().cloned().unwrap_or_default(),
                vendor_normalized: vendor_norm.clone(),
                vendor_total_value: vendor_info.total_value,
                vendor_sole_source: vendor_info.sole_source,
                vendor_departments: vendor_info.departments.join("; "),
                donor_name: format!("{} {}", c.donor_first, c.donor_last),
                employer: c.employer.clone(),
                donation_amount: c.amount,
                donation_date: c.date.clone(),
                candidate_name: c.candidate_name.clone(),
                candidate_office: c.candidate_office.clone(),
                record_type: c.record_type.clone(),
                confidence: "high".to_string(),
            }
        };

        if let Some(indices) = employer_donors.get(vendor_norm) {
            for &idx in indices {
                matches.push(make_match("exact_employer", idx));
                exact_count += 1;
            }
        }
        if let Some(indices) = business_donors.get(vendor_norm) {
            for &idx in indices {
                matches.push(make_match("exact_business_donor", idx));
                exact_count += 1;
            }
        }
    }

    println!("  Exact matches: {}", exact_count);
    println!("  Total cross-references: {}", matches.len());
    matches
}

// ============================================================
// Step 7: Find bundled donations
// ============================================================
fn find_bundled_donations(contributions: &[Contribution]) -> Vec<BundleEvent> {
    let mut bundles: HashMap<(String, String, String), Vec<usize>> = HashMap::new();

    for (i, c) in contributions.iter().enumerate() {
        let employer = normalize_name(&c.employer);
        if employer.len() > 3 {
            let key = (employer, c.date.clone(), c.cpf_id.clone());
            bundles.entry(key).or_default().push(i);
        }
    }

    let mut bundled: Vec<BundleEvent> = Vec::new();
    for ((employer, date, _cpf_id), indices) in &bundles {
        if indices.len() >= 3 {
            let total: f64 = indices.iter().map(|&i| contributions[i].amount).sum();
            let donor_names: Vec<String> = indices
                .iter()
                .map(|&i| {
                    let c = &contributions[i];
                    format!("{} {}", c.donor_first, c.donor_last)
                })
                .collect();

            let first_c = &contributions[indices[0]];
            bundled.push(BundleEvent {
                employer: employer.clone(),
                date: date.clone(),
                candidate_name: first_c.candidate_name.clone(),
                candidate_office: first_c.candidate_office.clone(),
                num_donors: indices.len(),
                total_amount: total,
                donor_names: donor_names.join("; "),
            });
        }
    }

    bundled.sort_by(|a, b| b.total_amount.partial_cmp(&a.total_amount).unwrap());
    println!(
        "  Bundled donation events (3+ donors, same employer/day): {}",
        bundled.len()
    );
    bundled
}

// ============================================================
// Main
// ============================================================
pub fn run(base_dir: &str, contracts_file: &str, output_dir: &str) -> Result<()> {
    std::fs::create_dir_all(output_dir)?;

    println!("{}", "=".repeat(60));
    println!("BOSTON CORRUPTION INVESTIGATION: CROSS-DATASET ANALYSIS");
    println!("{}", "=".repeat(60));

    // Step 1
    println!("\n[1/7] Loading Boston candidates...");
    let candidates_path = format!("{}/candidates.txt", base_dir);
    let (candidates, boston_cpf_ids) = load_boston_candidates(&candidates_path)?;

    // Step 2
    println!("\n[2/7] Loading campaign finance reports...");
    let reports_path = format!("{}/reports.txt", base_dir);
    let (report_to_cpf, boston_reports) = load_reports(&reports_path, &boston_cpf_ids)?;

    // Step 3
    println!("\n[3/7] Loading contributions to Boston candidates...");
    let items_path = format!("{}/report-items.txt", base_dir);
    let contributions =
        load_boston_contributions(&items_path, &boston_reports, &report_to_cpf, &candidates)?;

    // Step 4
    println!("\n[4/7] Loading city contracts...");
    let total_contracts = {
        let rdr = csv::Reader::from_path(contracts_file)?;
        rdr.into_records().count()
    };
    println!("  Total contracts: {}", total_contracts);

    // Step 5
    println!("\n[5/7] Building vendor entity map...");
    let vendor_map = build_vendor_map(contracts_file)?;

    // Step 6
    println!("\n[6/7] Cross-referencing donors with contractors...");
    let matches = cross_reference(&contributions, &vendor_map);

    // Step 7
    println!("\n[7/7] Detecting bundled donations...");
    let bundled = find_bundled_donations(&contributions);

    // ============================================================
    // Output
    // ============================================================
    println!("\n{}", "=".repeat(60));
    println!("WRITING OUTPUT FILES");
    println!("{}", "=".repeat(60));

    // Matches CSV
    if !matches.is_empty() {
        let path = format!("{}/donor_contractor_matches.csv", output_dir);
        let mut wtr = csv::Writer::from_path(&path)?;
        wtr.write_record([
            "match_type",
            "vendor_name",
            "vendor_normalized",
            "vendor_total_value",
            "vendor_sole_source",
            "vendor_departments",
            "donor_name",
            "employer",
            "donation_amount",
            "donation_date",
            "candidate_name",
            "candidate_office",
            "record_type",
            "confidence",
        ])?;
        for m in &matches {
            wtr.write_record([
                &m.match_type,
                &m.vendor_name,
                &m.vendor_normalized,
                &m.vendor_total_value.to_string(),
                &m.vendor_sole_source.to_string(),
                &m.vendor_departments,
                &m.donor_name,
                &m.employer,
                &m.donation_amount.to_string(),
                &m.donation_date,
                &m.candidate_name,
                &m.candidate_office,
                &m.record_type,
                &m.confidence,
            ])?;
        }
        wtr.flush()?;
        println!("\n  output/donor_contractor_matches.csv ({} records)", matches.len());
    } else {
        println!("\n  No cross-reference matches found");
    }

    // Bundled donations CSV
    if !bundled.is_empty() {
        let path = format!("{}/bundled_donations.csv", output_dir);
        let mut wtr = csv::Writer::from_path(&path)?;
        wtr.write_record([
            "employer",
            "date",
            "candidate_name",
            "candidate_office",
            "num_donors",
            "total_amount",
            "donor_names",
        ])?;
        for b in &bundled {
            wtr.write_record([
                &b.employer,
                &b.date,
                &b.candidate_name,
                &b.candidate_office,
                &b.num_donors.to_string(),
                &b.total_amount.to_string(),
                &b.donor_names,
            ])?;
        }
        wtr.flush()?;
        println!("  output/bundled_donations.csv ({} events)", bundled.len());
    }

    // Red flags CSV
    let red_flags: Vec<&CrossMatch> = matches.iter().filter(|m| m.vendor_sole_source).collect();
    if !red_flags.is_empty() {
        let path = format!("{}/red_flags.csv", output_dir);
        let mut wtr = csv::Writer::from_path(&path)?;
        wtr.write_record([
            "match_type",
            "vendor_name",
            "vendor_normalized",
            "vendor_total_value",
            "vendor_sole_source",
            "vendor_departments",
            "donor_name",
            "employer",
            "donation_amount",
            "donation_date",
            "candidate_name",
            "candidate_office",
            "record_type",
            "confidence",
        ])?;
        for m in &red_flags {
            wtr.write_record([
                &m.match_type,
                &m.vendor_name,
                &m.vendor_normalized,
                &m.vendor_total_value.to_string(),
                &m.vendor_sole_source.to_string(),
                &m.vendor_departments,
                &m.donor_name,
                &m.employer,
                &m.donation_amount.to_string(),
                &m.donation_date,
                &m.candidate_name,
                &m.candidate_office,
                &m.record_type,
                &m.confidence,
            ])?;
        }
        wtr.flush()?;
        println!("  output/red_flags.csv ({} records)", red_flags.len());
    }

    // Summary JSON
    let summary = json!({
        "analysis_timestamp": chrono::Utc::now().to_rfc3339(),
        "data_sources": {
            "contracts": {
                "file": contracts_file,
                "record_count": total_contracts,
                "source": "data.boston.gov"
            },
            "campaign_finance": {
                "file": format!("{}/report-items.txt", base_dir),
                "source": "Massachusetts OCPF",
                "boston_candidates": boston_cpf_ids.len(),
                "boston_contributions": contributions.len(),
            }
        },
        "cross_reference_results": {
            "total_matches": matches.len(),
            "exact_matches": matches.iter().filter(|m| m.confidence == "high").count(),
            "unique_vendors_matched": matches.iter().map(|m| &m.vendor_name).collect::<HashSet<_>>().len(),
            "sole_source_vendor_matches": red_flags.len(),
        },
        "bundled_donations": {
            "total_events": bundled.len(),
            "total_donations_bundled": bundled.iter().map(|b| b.total_amount).sum::<f64>(),
        },
    });

    let summary_path = format!("{}/cross_link_analysis.json", output_dir);
    std::fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)?;
    println!("  output/cross_link_analysis.json");

    // Key findings
    println!("\n{}", "=".repeat(60));
    println!("KEY FINDINGS");
    println!("{}", "=".repeat(60));
    println!("\n  Boston candidates identified: {}", boston_cpf_ids.len());
    println!("  Contributions to Boston candidates: {}", contributions.len());
    println!("  Contractor-donor cross-references: {}", matches.len());
    println!("  Sole-source vendor red flags: {}", red_flags.len());
    println!("  Bundled donation events: {}", bundled.len());

    if !matches.is_empty() {
        println!("\n  Top 5 matched vendors (by donation total):");
        let mut sorted_matches = matches.clone();
        sorted_matches.sort_by(|a, b| {
            b.donation_amount
                .partial_cmp(&a.donation_amount)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut seen = HashSet::new();
        let mut count = 0;
        for m in &sorted_matches {
            if !seen.contains(&m.vendor_name) && count < 5 {
                seen.insert(&m.vendor_name);
                count += 1;
                let ss = if m.vendor_sole_source {
                    " SOLE SOURCE"
                } else {
                    ""
                };
                println!(
                    "    {}: ${:.2} -> {}{}",
                    m.vendor_name, m.donation_amount, m.candidate_name, ss
                );
            }
        }
    }

    Ok(())
}

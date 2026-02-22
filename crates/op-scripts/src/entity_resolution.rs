//! Entity Resolution & Cross-Linking Pipeline
//!
//! Links Boston contract vendors to OCPF campaign finance donors/employers.
//! Implements fuzzy name matching via normalized token overlap (Levenshtein-free
//! approach matching the Python original's strategy).

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ============================================================
// Contribution record types
// ============================================================
fn contribution_type_desc(code: &str) -> &'static str {
    match code {
        "201" => "Individual Contribution",
        "202" => "Committee Contribution",
        "203" => "Union/Association Contribution",
        "204" => "Non-contribution receipt",
        "211" => "Business/Corporation Contribution",
        _ => "Unknown",
    }
}

fn is_contribution_type(code: &str) -> bool {
    matches!(code, "201" | "202" | "203" | "204" | "211")
}

// ============================================================
// Name normalization
// ============================================================
fn normalize_name(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    let mut n = name.to_uppercase().trim().to_string();
    n = n.replace('"', "").replace('\'', "");

    let suffixes = [
        r"\bINC\.?\b",
        r"\bLLC\.?\b",
        r"\bCORP\.?\b",
        r"\bLTD\.?\b",
        r"\bCO\.?\b",
        r"\bCOMPANY\b",
        r"\bCORPORATION\b",
        r"\bINCORPORATED\b",
        r"\bL\.?L\.?C\.?\b",
        r"\bLIMITED\b",
        r"\bGROUP\b",
        r"\bSERVICES\b",
        r"\bENTERPRISE[S]?\b",
        r"\bHOLDINGS?\b",
        r"\bINTERNATIONAL\b",
        r"\bAMERICA[S]?\b",
        r"\bASSOCIATES?\b",
        r"\bPARTNERS?\b",
        r"\bSOLUTIONS?\b",
        r"\bTECHNOLOG(Y|IES)\b",
        r"\bCONSULTING\b",
        r"\bMANAGEMENT\b",
    ];
    for suffix in &suffixes {
        if let Ok(re) = Regex::new(suffix) {
            n = re.replace_all(&n, "").to_string();
        }
    }
    // Remove punctuation
    if let Ok(re) = Regex::new(r"[.,;:!@#$%^&*()\-_+=\[\]{}|\\/<>~`]") {
        n = re.replace_all(&n, " ").to_string();
    }
    // Collapse whitespace
    if let Ok(re) = Regex::new(r"\s+") {
        n = re.replace_all(&n, " ").to_string();
    }
    n.trim().to_string()
}

fn normalize_name_aggressive(name: &str) -> String {
    let n = normalize_name(name);
    let mut tokens: Vec<&str> = n.split_whitespace().collect();
    tokens.sort();
    tokens.dedup();
    tokens.join(" ")
}

// ============================================================
// Data structures
// ============================================================
#[derive(Debug, Clone, Serialize)]
struct Candidate {
    first_name: String,
    last_name: String,
    office: String,
    district: String,
    city: String,
    full_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct Contribution {
    item_id: String,
    report_id: String,
    record_type: String,
    record_type_desc: String,
    date: String,
    amount: f64,
    donor_last_name: String,
    donor_first_name: String,
    donor_address: String,
    donor_city: String,
    donor_state: String,
    donor_zip: String,
    description: String,
    occupation: String,
    employer: String,
    candidate_cpf_id: String,
    candidate_name: String,
    candidate_office: String,
    data_year: i32,
}

#[derive(Debug, Clone, Default)]
struct VendorInfo {
    original_names: HashSet<String>,
    total_value: f64,
    contract_count: u32,
    departments: HashSet<String>,
    fiscal_years: HashSet<String>,
    methods: HashSet<String>,
    sole_source_value: f64,
    sole_source_count: u32,
    contracts: Vec<ContractEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct ContractEntry {
    id: String,
    value: f64,
    department: String,
    method: String,
    fy: String,
    begin_date: String,
}

#[derive(Debug, Clone, Serialize)]
struct MatchRecord {
    match_type: String,
    confidence: String,
    vendor_normalized: String,
    vendor_original: String,
    donor_name: String,
    employer_raw: String,
    contribution_idx: usize,
}

#[derive(Debug, Clone, Serialize)]
struct RedFlag {
    flag_type: String,
    severity: String,
    vendor_name: String,
    vendor_normalized: String,
    description: String,
    sole_source_value: f64,
    total_contract_value: f64,
    total_donated: f64,
    unique_donors: usize,
    departments: Vec<String>,
}

// ============================================================
// Step 1: Load Boston candidates
// ============================================================
fn load_boston_candidates(candidates_file: &str) -> Result<HashMap<String, Candidate>> {
    let mut candidates = HashMap::new();
    let content = std::fs::read_to_string(candidates_file)
        .context("Failed to read candidates file")?;
    let mut lines = content.lines();

    let header_line = match lines.next() {
        Some(h) => h,
        None => return Ok(candidates),
    };

    let headers: Vec<&str> = header_line.split('\t').collect();
    let col_idx = |name: &str| -> Option<usize> {
        headers.iter().position(|h| h.trim().trim_matches('"') == name)
    };

    let cpf_idx = 0;
    let first_idx = col_idx("Candidate First Name").unwrap_or(4);
    let last_idx = col_idx("Candidate Last Name").unwrap_or(5);
    let city_idx = col_idx("Candidate City").unwrap_or(7);
    let office_idx = col_idx("Office Type Sought").unwrap_or(20);
    let district_idx = col_idx("District Name Sought").unwrap_or(21);

    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        let max_idx = [cpf_idx, first_idx, last_idx, city_idx, office_idx, district_idx]
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        if fields.len() <= max_idx {
            continue;
        }

        let clean = |idx: usize| -> String {
            fields[idx].trim().trim_matches('"').to_string()
        };

        let cpf_id = clean(cpf_idx);
        let first = clean(first_idx);
        let last = clean(last_idx);
        let city = clean(city_idx);
        let office = clean(office_idx);
        let district = clean(district_idx);

        let mut is_boston = false;
        if office == "City Councilor" || office == "Mayoral" {
            if district.contains("Boston") || city.contains("Boston") {
                is_boston = true;
            }
            if office == "Mayoral"
                && (district == "Boston" || district == "Local Filer" || district.is_empty())
            {
                is_boston = true;
            }
        }

        if is_boston {
            let full_name = format!("{} {}", first, last).trim().to_string();
            candidates.insert(
                cpf_id,
                Candidate {
                    first_name: first,
                    last_name: last,
                    office,
                    district,
                    city,
                    full_name,
                },
            );
        }
    }

    Ok(candidates)
}

// ============================================================
// Step 2: Link candidates to reports
// ============================================================
fn load_reports_for_candidates(
    years: &[i32],
    base_dir: &str,
    cpf_ids: &HashSet<String>,
) -> Result<(HashMap<String, String>, HashMap<String, i32>)> {
    let mut report_to_cpf: HashMap<String, String> = HashMap::new();
    let mut report_year: HashMap<String, i32> = HashMap::new();

    for &year in years {
        let reports_file = format!("{}/yearly/{}/reports.txt", base_dir, year);
        if !Path::new(&reports_file).exists() {
            eprintln!("  Warning: {} not found", reports_file);
            continue;
        }

        let content = std::fs::read_to_string(&reports_file)?;
        let mut lines = content.lines();
        let header_line = match lines.next() {
            Some(h) => h,
            None => continue,
        };

        let headers: Vec<&str> = header_line.split('\t').collect();
        let col = |name: &str| headers.iter().position(|h| h.trim().trim_matches('"') == name);

        let report_id_idx = 0;
        let cpf_id_idx = col("CPF_ID").unwrap_or(2);
        let filer_cpf_idx = col("Filer_CPF_ID").unwrap_or(3);

        let mut count = 0u32;
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            let max_idx = [report_id_idx, cpf_id_idx, filer_cpf_idx]
                .iter()
                .copied()
                .max()
                .unwrap_or(0);
            if fields.len() <= max_idx {
                continue;
            }

            let rid = fields[report_id_idx].trim().trim_matches('"');
            let cpf = fields[cpf_id_idx].trim().trim_matches('"');
            let filer_cpf = fields[filer_cpf_idx].trim().trim_matches('"');

            if cpf_ids.contains(cpf) || cpf_ids.contains(filer_cpf) {
                let matched = if cpf_ids.contains(cpf) { cpf } else { filer_cpf };
                report_to_cpf.insert(rid.to_string(), matched.to_string());
                report_year.insert(rid.to_string(), year);
                count += 1;
            }
        }

        eprintln!(
            "  Year {}: {} reports matched to Boston candidates",
            year, count
        );
    }

    Ok((report_to_cpf, report_year))
}

// ============================================================
// Step 3: Extract contributions
// ============================================================
fn extract_contributions(
    years: &[i32],
    base_dir: &str,
    report_to_cpf: &HashMap<String, String>,
    candidates: &HashMap<String, Candidate>,
) -> Result<Vec<Contribution>> {
    let mut contributions = Vec::new();

    for &year in years {
        let items_file = format!("{}/yearly/{}/report-items.txt", base_dir, year);
        if !Path::new(&items_file).exists() {
            eprintln!("  Warning: {} not found", items_file);
            continue;
        }

        let content = std::fs::read_to_string(&items_file)?;
        let mut lines = content.lines();
        let header_line = match lines.next() {
            Some(h) => h,
            None => continue,
        };

        let headers: Vec<&str> = header_line.split('\t').collect();
        let col =
            |name: &str| headers.iter().position(|h| h.trim().trim_matches('"') == name);

        let item_id_idx = 0;
        let report_id_idx = col("Report_ID").unwrap_or(1);
        let type_idx = col("Record_Type_ID").unwrap_or(2);
        let date_idx = col("Date").unwrap_or(3);
        let amount_idx = col("Amount").unwrap_or(4);
        let name_idx = col("Name").unwrap_or(5);
        let first_idx = col("First_Name").unwrap_or(6);
        let addr_idx = col("Street_Address").unwrap_or(7);
        let city_idx = col("City").unwrap_or(8);
        let state_idx = col("State").unwrap_or(9);
        let zip_idx = col("Zip").unwrap_or(10);
        let desc_idx = col("Description").unwrap_or(11);
        let occ_idx = col("Occupation").unwrap_or(13);
        let emp_idx = col("Employer").unwrap_or(14);

        let mut year_count = 0u32;
        for line in lines {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() <= report_id_idx.max(type_idx) {
                continue;
            }

            let rid = fields[report_id_idx].trim().trim_matches('"');
            let rtype = fields[type_idx].trim().trim_matches('"');

            if !is_contribution_type(rtype) {
                continue;
            }
            if !report_to_cpf.contains_key(rid) {
                continue;
            }

            let cpf_id = &report_to_cpf[rid];
            let candidate = candidates.get(cpf_id.as_str());

            let safe_get = |idx: usize| -> String {
                if idx < fields.len() {
                    fields[idx].trim().trim_matches('"').to_string()
                } else {
                    String::new()
                }
            };

            let amount_str = safe_get(amount_idx).replace(',', "");
            let amount: f64 = amount_str.parse().unwrap_or(0.0);

            contributions.push(Contribution {
                item_id: safe_get(item_id_idx),
                report_id: rid.to_string(),
                record_type: rtype.to_string(),
                record_type_desc: contribution_type_desc(rtype).to_string(),
                date: safe_get(date_idx),
                amount,
                donor_last_name: safe_get(name_idx),
                donor_first_name: safe_get(first_idx),
                donor_address: safe_get(addr_idx),
                donor_city: safe_get(city_idx),
                donor_state: safe_get(state_idx),
                donor_zip: safe_get(zip_idx),
                description: safe_get(desc_idx),
                occupation: safe_get(occ_idx),
                employer: safe_get(emp_idx),
                candidate_cpf_id: cpf_id.clone(),
                candidate_name: candidate.map(|c| c.full_name.clone()).unwrap_or_default(),
                candidate_office: candidate.map(|c| c.office.clone()).unwrap_or_default(),
                data_year: year,
            });
            year_count += 1;
        }

        eprintln!(
            "  Year {}: {} contributions to Boston candidates",
            year, year_count
        );
    }

    Ok(contributions)
}

// ============================================================
// Step 4: Build vendor index
// ============================================================
fn build_vendor_index(contracts_file: &str) -> Result<HashMap<String, VendorInfo>> {
    let mut vendors: HashMap<String, VendorInfo> = HashMap::new();

    let mut rdr = csv::Reader::from_path(contracts_file)
        .context("Failed to open contracts file")?;

    let headers = rdr.headers()?.clone();

    for result in rdr.records() {
        let record = result?;
        let col = |name: &str| -> String {
            headers
                .iter()
                .position(|h| h == name)
                .and_then(|i| record.get(i))
                .unwrap_or("")
                .trim()
                .to_string()
        };

        let vendor = col("vendor_name1");
        if vendor.is_empty() {
            continue;
        }

        let method = col("contract_method_subcategory");
        let norm = normalize_name(&vendor);
        if norm.is_empty() {
            continue;
        }

        let value: f64 = col("amt_cntrct_max")
            .replace(',', "")
            .parse()
            .unwrap_or(0.0);
        let dept = col("dept_tbl_descr_3_digit");
        let fy = col("fy_cntrct_begin_dt");
        let contract_id = col("cntrct_hdr_cntrct_id");
        let begin_date = col("cntrct_hdr_cntrct_begin_dt");

        let entry = vendors.entry(norm.clone()).or_default();
        entry.original_names.insert(vendor);
        entry.total_value += value;
        entry.contract_count += 1;
        entry.departments.insert(dept.clone());
        entry.fiscal_years.insert(fy.clone());
        entry.methods.insert(method.clone());

        if matches!(
            method.as_str(),
            "Sole Source" | "Limited Competition" | "Emergency"
        ) {
            entry.sole_source_value += value;
            entry.sole_source_count += 1;
        }

        entry.contracts.push(ContractEntry {
            id: contract_id,
            value,
            department: dept,
            method,
            fy,
            begin_date,
        });
    }

    Ok(vendors)
}

// ============================================================
// Step 5: Entity resolution (matching)
// ============================================================
fn match_entities(
    vendors: &HashMap<String, VendorInfo>,
    contributions: &[Contribution],
) -> (Vec<MatchRecord>, HashMap<String, u32>) {
    // Build indexes
    let mut vendor_token_index: HashMap<String, HashSet<String>> = HashMap::new();
    for norm_name in vendors.keys() {
        for token in norm_name.split_whitespace() {
            if token.len() >= 4 {
                vendor_token_index
                    .entry(token.to_string())
                    .or_default()
                    .insert(norm_name.clone());
            }
        }
    }

    let mut vendor_aggressive_index: HashMap<String, String> = HashMap::new();
    for (norm_name, info) in vendors {
        if let Some(orig) = info.original_names.iter().next() {
            let agg = normalize_name_aggressive(orig);
            if agg.len() >= 4 {
                vendor_aggressive_index.insert(agg, norm_name.clone());
            }
        }
    }

    let mut matches = Vec::new();
    let mut match_stats: HashMap<String, u32> = HashMap::new();

    for (idx, c) in contributions.iter().enumerate() {
        let donor_name = format!("{} {}", c.donor_last_name, c.donor_first_name)
            .trim()
            .to_string();

        // Strategy 1: Match employer to vendor (individual contributions)
        if !c.employer.is_empty() && c.record_type == "201" {
            let emp_norm = normalize_name(&c.employer);
            if emp_norm.len() >= 3 {
                // Exact match
                if vendors.contains_key(&emp_norm) {
                    let orig = vendors[&emp_norm]
                        .original_names
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    matches.push(MatchRecord {
                        match_type: "employer_exact".to_string(),
                        confidence: "high".to_string(),
                        vendor_normalized: emp_norm.clone(),
                        vendor_original: orig,
                        donor_name: donor_name.clone(),
                        employer_raw: c.employer.clone(),
                        contribution_idx: idx,
                    });
                    *match_stats.entry("employer_exact".to_string()).or_default() += 1;
                    continue;
                }

                // Aggressive match
                let emp_agg = normalize_name_aggressive(&c.employer);
                if let Some(vnorm) = vendor_aggressive_index.get(&emp_agg) {
                    let orig = vendors[vnorm]
                        .original_names
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    matches.push(MatchRecord {
                        match_type: "employer_fuzzy".to_string(),
                        confidence: "medium".to_string(),
                        vendor_normalized: vnorm.clone(),
                        vendor_original: orig,
                        donor_name: donor_name.clone(),
                        employer_raw: c.employer.clone(),
                        contribution_idx: idx,
                    });
                    *match_stats.entry("employer_fuzzy".to_string()).or_default() += 1;
                    continue;
                }

                // Token overlap match
                let emp_tokens: HashSet<&str> = emp_norm.split_whitespace().collect();
                let mut best_overlap = 0usize;
                let mut best_vendor: Option<String> = None;

                for token in &emp_tokens {
                    if token.len() >= 4 {
                        if let Some(vkeys) = vendor_token_index.get(*token) {
                            for vkey in vkeys {
                                let vtokens: HashSet<&str> =
                                    vkey.split_whitespace().collect();
                                let overlap = emp_tokens
                                    .intersection(&vtokens)
                                    .count();
                                let min_len = emp_tokens.len().min(vtokens.len());
                                if min_len > 0
                                    && (overlap as f64 / min_len as f64) > 0.6
                                    && overlap > best_overlap
                                {
                                    best_overlap = overlap;
                                    best_vendor = Some(vkey.clone());
                                }
                            }
                        }
                    }
                }

                if let Some(bv) = best_vendor {
                    if best_overlap >= 2 {
                        let orig = vendors[&bv]
                            .original_names
                            .iter()
                            .next()
                            .cloned()
                            .unwrap_or_default();
                        matches.push(MatchRecord {
                            match_type: "employer_token_overlap".to_string(),
                            confidence: "low".to_string(),
                            vendor_normalized: bv,
                            vendor_original: orig,
                            donor_name: donor_name.clone(),
                            employer_raw: c.employer.clone(),
                            contribution_idx: idx,
                        });
                        *match_stats
                            .entry("employer_token_overlap".to_string())
                            .or_default() += 1;
                    }
                }
            }
        }

        // Strategy 2: Direct donor match (committee/business contributions)
        if matches!(c.record_type.as_str(), "202" | "203" | "211") {
            let donor_norm = normalize_name(&donor_name);
            if donor_norm.len() >= 3 {
                if vendors.contains_key(&donor_norm) {
                    let orig = vendors[&donor_norm]
                        .original_names
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    matches.push(MatchRecord {
                        match_type: "donor_exact".to_string(),
                        confidence: "high".to_string(),
                        vendor_normalized: donor_norm.clone(),
                        vendor_original: orig,
                        donor_name: donor_name.clone(),
                        employer_raw: String::new(),
                        contribution_idx: idx,
                    });
                    *match_stats.entry("donor_exact".to_string()).or_default() += 1;
                    continue;
                }

                let donor_agg = normalize_name_aggressive(&donor_name);
                if let Some(vnorm) = vendor_aggressive_index.get(&donor_agg) {
                    let orig = vendors[vnorm]
                        .original_names
                        .iter()
                        .next()
                        .cloned()
                        .unwrap_or_default();
                    matches.push(MatchRecord {
                        match_type: "donor_fuzzy".to_string(),
                        confidence: "medium".to_string(),
                        vendor_normalized: vnorm.clone(),
                        vendor_original: orig,
                        donor_name: donor_name.clone(),
                        employer_raw: String::new(),
                        contribution_idx: idx,
                    });
                    *match_stats.entry("donor_fuzzy".to_string()).or_default() += 1;
                }
            }
        }
    }

    (matches, match_stats)
}

// ============================================================
// Step 6: Red flag analysis
// ============================================================
fn analyze_red_flags(
    matches: &[MatchRecord],
    contributions: &[Contribution],
    vendors: &HashMap<String, VendorInfo>,
) -> Vec<RedFlag> {
    let mut red_flags = Vec::new();

    // Group matches by vendor
    let mut vendor_matches: HashMap<String, Vec<&MatchRecord>> = HashMap::new();
    for m in matches {
        vendor_matches
            .entry(m.vendor_normalized.clone())
            .or_default()
            .push(m);
    }

    for (vnorm, vm_list) in &vendor_matches {
        let vendor_info = match vendors.get(vnorm) {
            Some(v) => v,
            None => continue,
        };

        let original_name = vendor_info
            .original_names
            .iter()
            .next()
            .cloned()
            .unwrap_or_else(|| vnorm.clone());

        let mut donors = HashSet::new();
        let mut total_donated = 0.0f64;
        let mut candidates_receiving = HashSet::new();

        for m in vm_list {
            let c = &contributions[m.contribution_idx];
            let donor_key = format!("{}_{}", c.donor_last_name, c.donor_first_name);
            donors.insert(donor_key);
            total_donated += c.amount;
            candidates_receiving.insert(c.candidate_name.clone());
        }

        // Flag 1: Sole-source vendor whose employees donate
        if vendor_info.sole_source_count > 0 {
            let severity = if vendor_info.sole_source_value > 1_000_000.0 {
                "HIGH"
            } else {
                "MEDIUM"
            };
            red_flags.push(RedFlag {
                flag_type: "sole_source_vendor_donor".to_string(),
                severity: severity.to_string(),
                vendor_name: original_name.clone(),
                vendor_normalized: vnorm.clone(),
                sole_source_value: vendor_info.sole_source_value,
                total_contract_value: vendor_info.total_value,
                unique_donors: donors.len(),
                total_donated,
                departments: vendor_info.departments.iter().cloned().collect(),
                description: format!(
                    "Sole-source vendor {} (${:.0} in {} contracts) has {} donor(s) contributing ${:.0} to {} Boston candidate(s)",
                    original_name,
                    vendor_info.sole_source_value,
                    vendor_info.sole_source_count,
                    donors.len(),
                    total_donated,
                    candidates_receiving.len()
                ),
            });
        }

        // Flag 2: Bundled donations
        let mut candidate_donor_groups: HashMap<String, HashSet<String>> = HashMap::new();
        for m in vm_list {
            let c = &contributions[m.contribution_idx];
            let donor_key = format!("{}_{}", c.donor_last_name, c.donor_first_name);
            candidate_donor_groups
                .entry(c.candidate_name.clone())
                .or_default()
                .insert(donor_key);
        }
        for (cand, donor_set) in &candidate_donor_groups {
            if donor_set.len() >= 3 {
                red_flags.push(RedFlag {
                    flag_type: "bundled_donations".to_string(),
                    severity: "HIGH".to_string(),
                    vendor_name: original_name.clone(),
                    vendor_normalized: vnorm.clone(),
                    sole_source_value: vendor_info.sole_source_value,
                    total_contract_value: vendor_info.total_value,
                    unique_donors: donor_set.len(),
                    total_donated,
                    departments: vendor_info.departments.iter().cloned().collect(),
                    description: format!(
                        "{} employees of {} donated to {}",
                        donor_set.len(),
                        original_name,
                        cand
                    ),
                });
            }
        }

        // Flag 3: Significant donation amounts
        if total_donated > 1000.0 && vendor_info.total_value > 0.0 {
            let severity = if total_donated > 5000.0 {
                "MEDIUM"
            } else {
                "LOW"
            };
            red_flags.push(RedFlag {
                flag_type: "significant_donor_amount".to_string(),
                severity: severity.to_string(),
                vendor_name: original_name.clone(),
                vendor_normalized: vnorm.clone(),
                sole_source_value: vendor_info.sole_source_value,
                total_contract_value: vendor_info.total_value,
                unique_donors: donors.len(),
                total_donated,
                departments: vendor_info.departments.iter().cloned().collect(),
                description: format!(
                    "Vendor {} connections donated ${:.0} total; vendor has ${:.0} in contracts (${:.0} sole-source)",
                    original_name, total_donated, vendor_info.total_value, vendor_info.sole_source_value
                ),
            });
        }
    }

    // Sort by severity then by donated amount
    red_flags.sort_by(|a, b| {
        let sev_ord = |s: &str| match s {
            "HIGH" => 0,
            "MEDIUM" => 1,
            "LOW" => 2,
            _ => 3,
        };
        let cmp = sev_ord(&a.severity).cmp(&sev_ord(&b.severity));
        if cmp == std::cmp::Ordering::Equal {
            b.total_donated
                .partial_cmp(&a.total_donated)
                .unwrap_or(std::cmp::Ordering::Equal)
        } else {
            cmp
        }
    });

    red_flags
}

// ============================================================
// Main entry point
// ============================================================
pub fn run(
    base_dir: &str,
    contracts_file: &str,
    output_dir: &str,
    years: &[i32],
) -> Result<()> {
    println!("{}", "=".repeat(60));
    println!("ENTITY RESOLUTION & CROSS-LINKING PIPELINE");
    println!("{}", "=".repeat(60));

    // Step 1
    println!("\n[1] Loading Boston candidates...");
    let candidates_path = format!("{}/candidates.txt", base_dir);
    let candidates = load_boston_candidates(&candidates_path)?;
    let cpf_ids: HashSet<String> = candidates.keys().cloned().collect();
    println!("  Found {} Boston candidates", candidates.len());

    let mut sorted_candidates: Vec<_> = candidates.iter().collect();
    sorted_candidates.sort_by_key(|(_, c)| &c.last_name);
    for (cpf, info) in sorted_candidates.iter().take(10) {
        println!("    CPF {}: {} ({})", cpf, info.full_name, info.office);
    }

    // Step 2
    println!("\n[2] Linking candidates to OCPF reports...");
    let (report_to_cpf, _report_year) =
        load_reports_for_candidates(years, base_dir, &cpf_ids)?;
    println!("  Total reports linked: {}", report_to_cpf.len());

    // Step 3
    println!("\n[3] Extracting contributions to Boston candidates...");
    let contributions =
        extract_contributions(years, base_dir, &report_to_cpf, &candidates)?;
    println!("  Total contributions extracted: {}", contributions.len());

    let mut type_counts: HashMap<String, u32> = HashMap::new();
    let mut total_amount = 0.0f64;
    for c in &contributions {
        *type_counts.entry(c.record_type_desc.clone()).or_default() += 1;
        total_amount += c.amount;
    }
    println!("  Total amount: ${:.2}", total_amount);
    let mut sorted_types: Vec<_> = type_counts.iter().collect();
    sorted_types.sort_by(|a, b| b.1.cmp(a.1));
    for (t, count) in &sorted_types {
        println!("    {}: {}", t, count);
    }

    // Step 4
    println!("\n[4] Building vendor index from contracts...");
    let vendors = build_vendor_index(contracts_file)?;
    println!(
        "  Unique vendor entities (normalized): {}",
        vendors.len()
    );

    let sole_source_count = vendors
        .values()
        .filter(|v| v.sole_source_count > 0)
        .count();
    println!("  Sole-source vendors: {}", sole_source_count);

    // Step 5
    println!("\n[5] Running entity resolution (matching employers/donors to vendors)...");
    let (matches, match_stats) = match_entities(&vendors, &contributions);
    println!("  Total matches found: {}", matches.len());
    let mut sorted_stats: Vec<_> = match_stats.iter().collect();
    sorted_stats.sort_by(|a, b| b.1.cmp(a.1));
    for (mtype, count) in &sorted_stats {
        println!("    {}: {}", mtype, count);
    }

    // Step 6
    println!("\n[6] Analyzing red flags...");
    let red_flags = analyze_red_flags(&matches, &contributions, &vendors);
    println!("  Total red flags: {}", red_flags.len());
    let mut flag_counts: HashMap<String, u32> = HashMap::new();
    for rf in &red_flags {
        *flag_counts.entry(rf.flag_type.clone()).or_default() += 1;
    }
    for (ft, count) in &flag_counts {
        println!("    {}: {}", ft, count);
    }

    // Step 7: Write output files
    println!("\n[7] Writing output files...");
    std::fs::create_dir_all(output_dir)?;

    // 7a: Entity map
    let mut entity_map = serde_json::Map::new();
    for (vnorm, vinfo) in &vendors {
        if vinfo.sole_source_count > 0 {
            let orig_names: Vec<String> = vinfo.original_names.iter().cloned().collect();
            entity_map.insert(
                vnorm.clone(),
                serde_json::json!({
                    "canonical_name": orig_names.first().unwrap_or(&vnorm.clone()),
                    "name_variants": orig_names,
                    "normalized": vnorm,
                    "total_contract_value": vinfo.total_value,
                    "sole_source_value": vinfo.sole_source_value,
                    "sole_source_count": vinfo.sole_source_count,
                    "departments": vinfo.departments.iter().cloned().collect::<Vec<_>>(),
                }),
            );
        }
    }
    let entity_map_path = format!("{}/entity_map.json", output_dir);
    std::fs::write(
        &entity_map_path,
        serde_json::to_string_pretty(&Value::Object(entity_map.clone()))?,
    )?;
    println!("  entity_map.json: {} sole-source vendor entities", entity_map.len());

    // 7b: Cross-links CSV
    let cross_links_path = format!("{}/cross_links.csv", output_dir);
    {
        let mut wtr = csv::Writer::from_path(&cross_links_path)?;
        wtr.write_record([
            "match_type",
            "confidence",
            "vendor_name",
            "vendor_normalized",
            "donor_name",
            "employer",
            "amount",
            "date",
            "candidate_name",
            "candidate_office",
            "record_type",
            "sole_source_value",
            "total_contract_value",
            "item_id",
        ])?;
        for m in &matches {
            let c = &contributions[m.contribution_idx];
            let vinfo = vendors.get(&m.vendor_normalized);
            wtr.write_record([
                &m.match_type,
                &m.confidence,
                &m.vendor_original,
                &m.vendor_normalized,
                &m.donor_name,
                &m.employer_raw,
                &c.amount.to_string(),
                &c.date,
                &c.candidate_name,
                &c.candidate_office,
                &c.record_type_desc,
                &vinfo
                    .map(|v| v.sole_source_value.to_string())
                    .unwrap_or_default(),
                &vinfo
                    .map(|v| v.total_value.to_string())
                    .unwrap_or_default(),
                &c.item_id,
            ])?;
        }
        wtr.flush()?;
    }
    println!("  cross_links.csv: {} matches", matches.len());

    // 7c: Red flags CSV
    let red_flags_path = format!("{}/red_flags.csv", output_dir);
    {
        let mut wtr = csv::Writer::from_path(&red_flags_path)?;
        wtr.write_record([
            "flag_type",
            "severity",
            "vendor_name",
            "description",
            "sole_source_value",
            "total_contract_value",
            "total_donated",
            "unique_donors",
            "departments",
        ])?;
        for rf in &red_flags {
            wtr.write_record([
                &rf.flag_type,
                &rf.severity,
                &rf.vendor_name,
                &rf.description,
                &rf.sole_source_value.to_string(),
                &rf.total_contract_value.to_string(),
                &rf.total_donated.to_string(),
                &rf.unique_donors.to_string(),
                &rf.departments.join("; "),
            ])?;
        }
        wtr.flush()?;
    }
    println!("  red_flags.csv: {} flags", red_flags.len());

    // 7d: Boston contributions CSV
    let contributions_path = format!("{}/boston_contributions.csv", output_dir);
    {
        let mut wtr = csv::Writer::from_path(&contributions_path)?;
        if !contributions.is_empty() {
            wtr.write_record([
                "item_id",
                "report_id",
                "record_type",
                "record_type_desc",
                "date",
                "amount",
                "donor_last_name",
                "donor_first_name",
                "donor_address",
                "donor_city",
                "donor_state",
                "donor_zip",
                "description",
                "occupation",
                "employer",
                "candidate_cpf_id",
                "candidate_name",
                "candidate_office",
                "data_year",
            ])?;
            for c in &contributions {
                wtr.write_record([
                    &c.item_id,
                    &c.report_id,
                    &c.record_type,
                    &c.record_type_desc,
                    &c.date,
                    &c.amount.to_string(),
                    &c.donor_last_name,
                    &c.donor_first_name,
                    &c.donor_address,
                    &c.donor_city,
                    &c.donor_state,
                    &c.donor_zip,
                    &c.description,
                    &c.occupation,
                    &c.employer,
                    &c.candidate_cpf_id,
                    &c.candidate_name,
                    &c.candidate_office,
                    &c.data_year.to_string(),
                ])?;
            }
            wtr.flush()?;
        }
    }
    println!(
        "  boston_contributions.csv: {} contributions",
        contributions.len()
    );

    // 7e: Summary JSON
    let summary = serde_json::json!({
        "pipeline_run": chrono::Utc::now().to_rfc3339(),
        "boston_candidates": candidates.len(),
        "reports_linked": report_to_cpf.len(),
        "total_contributions": contributions.len(),
        "total_contributed_amount": total_amount,
        "contribution_types": type_counts,
        "vendor_entities": vendors.len(),
        "sole_source_vendors": sole_source_count,
        "entity_matches": matches.len(),
        "match_breakdown": match_stats,
        "red_flags_total": red_flags.len(),
        "red_flag_breakdown": flag_counts,
    });
    let summary_path = format!("{}/cross_link_summary.json", output_dir);
    std::fs::write(&summary_path, serde_json::to_string_pretty(&summary)?)?;
    println!("  cross_link_summary.json written");

    println!("\n{}", "=".repeat(60));
    println!("PIPELINE COMPLETE");
    println!("{}", "=".repeat(60));

    Ok(())
}

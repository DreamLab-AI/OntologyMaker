mod build_findings_json;
mod cross_link_analysis;
mod entity_resolution;
mod fetch_census_acs;
mod fetch_epa_echo;
mod fetch_fdic;
mod fetch_fec;
mod fetch_icij_leaks;
mod fetch_ofac_sdn;
mod fetch_osha;
mod fetch_propublica_990;
mod fetch_sam_gov;
mod fetch_sec_edgar;
mod fetch_senate_lobbying;
mod fetch_usaspending;
mod timing_analysis;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "openplanter-scripts")]
#[command(about = "OpenPlanter data acquisition and analysis scripts")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch FEC federal campaign finance data
    Fec {
        #[arg(long, value_parser = ["candidates", "committees", "schedule_a", "totals"])]
        endpoint: String,
        #[arg(long, default_value = "DEMO_KEY")]
        api_key: String,
        #[arg(long)]
        cycle: Option<i32>,
        #[arg(long, value_parser = ["H", "S", "P"])]
        office: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        committee: Option<String>,
        #[arg(long)]
        committee_type: Option<String>,
        #[arg(long)]
        candidate: Option<String>,
        #[arg(long)]
        min_amount: Option<f64>,
        #[arg(long)]
        max_amount: Option<f64>,
        #[arg(long, default_value_t = 20)]
        per_page: u32,
        #[arg(long, default_value_t = 10)]
        max_pages: u32,
        #[arg(long, default_value = "json", value_parser = ["json", "csv"])]
        format: String,
        #[arg(long)]
        output: Option<String>,
    },
    /// Fetch US Census Bureau ACS data
    CensusAcs {
        #[arg(long)]
        year: i32,
        #[arg(long, value_parser = ["acs1", "acs5", "acs1/profile", "acs5/profile", "acs5/subject"])]
        dataset: String,
        #[arg(long)]
        variables: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        geography: String,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        county: Option<String>,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        output: String,
        #[arg(long, value_parser = ["csv", "json"])]
        format: Option<String>,
    },
    /// Fetch EPA ECHO facility compliance data
    EpaEcho {
        #[arg(long)]
        facility_name: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        city: Option<String>,
        #[arg(long)]
        zip: Option<String>,
        #[arg(long)]
        latitude: Option<f64>,
        #[arg(long)]
        longitude: Option<f64>,
        #[arg(long)]
        radius: Option<f64>,
        #[arg(long)]
        compliance: Option<String>,
        #[arg(long)]
        major_only: bool,
        #[arg(long)]
        program: Option<String>,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long, default_value = "csv", value_parser = ["csv", "json"])]
        format: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(short, long)]
        quiet: bool,
    },
    /// Fetch FDIC BankFind data
    Fdic {
        #[arg(value_parser = ["institutions", "failures", "locations", "history", "summary", "financials"])]
        endpoint: String,
        #[arg(short = 'f', long = "filter")]
        filters: Option<String>,
        #[arg(long)]
        fields: Option<String>,
        #[arg(short = 'l', long, default_value_t = 10)]
        limit: u32,
        #[arg(short = 'o', long, default_value_t = 0)]
        offset: u32,
        #[arg(long)]
        sort_by: Option<String>,
        #[arg(long, value_parser = ["ASC", "DESC"])]
        sort_order: Option<String>,
        #[arg(long, default_value = "json", value_parser = ["json", "csv"])]
        format: String,
        #[arg(long)]
        compact: bool,
    },
    /// Download ICIJ Offshore Leaks bulk data
    IcijLeaks {
        #[arg(short, long, default_value = "data/icij_leaks")]
        output: String,
        #[arg(long, default_value = "https://offshoreleaks-data.icij.org/offshoreleaks/csv/full-oldb.LATEST.zip")]
        url: String,
        #[arg(long)]
        no_extract: bool,
        #[arg(long)]
        keep_zip: bool,
        #[arg(short, long)]
        quiet: bool,
    },
    /// Fetch OSHA inspection data
    Osha {
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        skip: u32,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        year: Option<i32>,
        #[arg(long)]
        establishment: Option<String>,
        #[arg(long)]
        open_after: Option<String>,
        #[arg(long)]
        fields: Option<String>,
        #[arg(long, default_value = "open_date")]
        sort_by: String,
        #[arg(long, default_value = "desc", value_parser = ["asc", "desc"])]
        sort_order: String,
        #[arg(long, default_value = "json", value_parser = ["json", "csv"])]
        format: String,
        #[arg(long)]
        output: Option<String>,
    },
    /// Download OFAC SDN list CSV files
    OfacSdn {
        #[arg(long, default_value = "./data/ofac")]
        output_dir: String,
        #[arg(long)]
        no_validate: bool,
        #[arg(long)]
        quiet: bool,
    },
    /// Fetch ProPublica nonprofit 990 data
    Propublica990 {
        #[command(subcommand)]
        action: Propublica990Action,
    },
    /// Fetch SAM.gov exclusion and entity data
    SamGov {
        #[arg(long)]
        api_key: String,
        #[arg(long)]
        output: String,
        #[arg(long, value_parser = ["EXCLUSION", "ENTITY", "SCR", "BIO"])]
        file_type: Option<String>,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        file_name: Option<String>,
        #[arg(long)]
        search_exclusions: bool,
        #[arg(long)]
        search_entity: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        uei: Option<String>,
        #[arg(long)]
        cage_code: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        classification: Option<String>,
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long, default_value_t = 10)]
        size: u32,
    },
    /// Fetch SEC EDGAR company filings
    SecEdgar {
        #[arg(long)]
        ticker: Option<String>,
        #[arg(long)]
        cik: Option<String>,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
        list_tickers: bool,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        pretty: bool,
        #[arg(long)]
        summary: bool,
    },
    /// Download Senate lobbying disclosure data
    SenateLobby {
        #[arg(long)]
        year: i32,
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
        quarter: u8,
        #[arg(long, default_value = "data/lobbying")]
        output: String,
        #[arg(short, long)]
        verbose: bool,
    },
    /// Fetch USASpending.gov federal spending data
    Usaspending {
        #[arg(long)]
        recipient: Option<String>,
        #[arg(long)]
        agency: Option<String>,
        #[arg(long)]
        award_type: Option<String>,
        #[arg(long)]
        start_date: Option<String>,
        #[arg(long)]
        end_date: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: u32,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long)]
        output: Option<String>,
        #[arg(long, default_value = "Award Amount")]
        sort: String,
        #[arg(long, default_value = "desc", value_parser = ["asc", "desc"])]
        order: String,
    },
    /// Run entity resolution pipeline
    EntityResolution {
        #[arg(long, default_value = "data/ocpf_contributions")]
        base_dir: String,
        #[arg(long, default_value = "data/contracts.csv")]
        contracts_file: String,
        #[arg(long, default_value = "output")]
        output_dir: String,
        #[arg(long, value_delimiter = ',', default_values_t = vec![2019,2020,2021,2022,2023,2024,2025])]
        years: Vec<i32>,
    },
    /// Run cross-link pay-to-play analysis
    CrossLink {
        #[arg(long, default_value = "data/ocpf_contributions")]
        base_dir: String,
        #[arg(long, default_value = "data/contracts.csv")]
        contracts_file: String,
        #[arg(long, default_value = "output")]
        output_dir: String,
    },
    /// Build structured investigation findings JSON
    BuildFindings {
        #[arg(long, default_value = "output")]
        input_dir: String,
        #[arg(long, default_value = "corruption_investigation_data.json")]
        output: String,
    },
    /// Run timing analysis of donations vs contract awards
    TimingAnalysis {
        #[arg(long, default_value = "data/contracts.csv")]
        contracts_file: String,
        #[arg(long, default_value = "output/cross_links.csv")]
        cross_links_file: String,
        #[arg(long, default_value = "output")]
        output_dir: String,
        #[arg(long, default_value_t = 1000)]
        permutations: u32,
    },
}

#[derive(Subcommand)]
enum Propublica990Action {
    /// Search for nonprofit organizations
    Search {
        query: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long)]
        ntee: Option<String>,
        #[arg(long)]
        c_code: Option<String>,
        #[arg(long, default_value_t = 0)]
        page: u32,
        #[arg(long)]
        output: Option<String>,
    },
    /// Get organization details by EIN
    Org {
        ein: String,
        #[arg(long)]
        output: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env(),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Fec {
            endpoint,
            api_key,
            cycle,
            office,
            state,
            committee,
            committee_type,
            candidate,
            min_amount,
            max_amount,
            per_page,
            max_pages,
            format,
            output,
        } => {
            fetch_fec::run(
                &endpoint,
                &api_key,
                cycle,
                office.as_deref(),
                state.as_deref(),
                committee.as_deref(),
                committee_type.as_deref(),
                candidate.as_deref(),
                min_amount,
                max_amount,
                per_page,
                max_pages,
                &format,
                output.as_deref(),
            )
            .await?;
        }
        Commands::CensusAcs {
            year,
            dataset,
            variables,
            group,
            geography,
            state,
            county,
            key,
            output,
            format,
        } => {
            fetch_census_acs::run(
                year,
                &dataset,
                variables.as_deref(),
                group.as_deref(),
                &geography,
                state.as_deref(),
                county.as_deref(),
                key.as_deref(),
                &output,
                format.as_deref(),
            )
            .await?;
        }
        Commands::EpaEcho {
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
            output,
            format,
            limit,
            quiet,
        } => {
            fetch_epa_echo::run(
                facility_name.as_deref(),
                state.as_deref(),
                city.as_deref(),
                zip.as_deref(),
                latitude,
                longitude,
                radius,
                compliance.as_deref(),
                major_only,
                program.as_deref(),
                output.as_deref(),
                &format,
                limit,
                quiet,
            )
            .await?;
        }
        Commands::Fdic {
            endpoint,
            filters,
            fields,
            limit,
            offset,
            sort_by,
            sort_order,
            format,
            compact,
        } => {
            fetch_fdic::run(
                &endpoint,
                filters.as_deref(),
                fields.as_deref(),
                limit,
                offset,
                sort_by.as_deref(),
                sort_order.as_deref(),
                &format,
                compact,
            )
            .await?;
        }
        Commands::IcijLeaks {
            output,
            url,
            no_extract,
            keep_zip,
            quiet,
        } => {
            fetch_icij_leaks::run(&output, &url, no_extract, keep_zip, quiet).await?;
        }
        Commands::Osha {
            api_key,
            limit,
            skip,
            state,
            year,
            establishment,
            open_after,
            fields,
            sort_by,
            sort_order,
            format,
            output,
        } => {
            let resolved_key = api_key.or_else(|| std::env::var("DOL_API_KEY").ok());
            let Some(key) = resolved_key else {
                anyhow::bail!(
                    "API key required. Use --api-key or set DOL_API_KEY environment variable."
                );
            };
            fetch_osha::run(
                &key,
                limit,
                skip,
                state.as_deref(),
                year,
                establishment.as_deref(),
                open_after.as_deref(),
                fields.as_deref(),
                &sort_by,
                &sort_order,
                &format,
                output.as_deref(),
            )
            .await?;
        }
        Commands::OfacSdn {
            output_dir,
            no_validate,
            quiet,
        } => {
            fetch_ofac_sdn::run(&output_dir, no_validate, quiet).await?;
        }
        Commands::Propublica990 { action } => match action {
            Propublica990Action::Search {
                query,
                state,
                ntee,
                c_code,
                page,
                output,
            } => {
                fetch_propublica_990::run_search(
                    query.as_deref(),
                    state.as_deref(),
                    ntee.as_deref(),
                    c_code.as_deref(),
                    page,
                    output.as_deref(),
                )
                .await?;
            }
            Propublica990Action::Org { ein, output } => {
                fetch_propublica_990::run_org(&ein, output.as_deref()).await?;
            }
        },
        Commands::SamGov {
            api_key,
            output,
            file_type,
            date,
            file_name,
            search_exclusions,
            search_entity,
            name,
            uei,
            cage_code,
            state,
            classification,
            page,
            size,
        } => {
            fetch_sam_gov::run(
                &api_key,
                &output,
                file_type.as_deref(),
                date.as_deref(),
                file_name.as_deref(),
                search_exclusions,
                search_entity,
                name.as_deref(),
                uei.as_deref(),
                cage_code.as_deref(),
                state.as_deref(),
                classification.as_deref(),
                page,
                size,
            )
            .await?;
        }
        Commands::SecEdgar {
            ticker,
            cik,
            output,
            list_tickers,
            limit,
            pretty,
            summary,
        } => {
            fetch_sec_edgar::run(
                ticker.as_deref(),
                cik.as_deref(),
                output.as_deref(),
                list_tickers,
                limit,
                pretty,
                summary,
            )
            .await?;
        }
        Commands::SenateLobby {
            year,
            quarter,
            output,
            verbose,
        } => {
            fetch_senate_lobbying::run(year, quarter, &output, verbose).await?;
        }
        Commands::Usaspending {
            recipient,
            agency,
            award_type,
            start_date,
            end_date,
            limit,
            page,
            output,
            sort,
            order,
        } => {
            fetch_usaspending::run(
                recipient.as_deref(),
                agency.as_deref(),
                award_type.as_deref(),
                start_date.as_deref(),
                end_date.as_deref(),
                limit,
                page,
                output.as_deref(),
                &sort,
                &order,
            )
            .await?;
        }
        Commands::EntityResolution {
            base_dir,
            contracts_file,
            output_dir,
            years,
        } => {
            entity_resolution::run(&base_dir, &contracts_file, &output_dir, &years)?;
        }
        Commands::CrossLink {
            base_dir,
            contracts_file,
            output_dir,
        } => {
            cross_link_analysis::run(&base_dir, &contracts_file, &output_dir)?;
        }
        Commands::BuildFindings { input_dir, output } => {
            build_findings_json::run(&input_dir, &output)?;
        }
        Commands::TimingAnalysis {
            contracts_file,
            cross_links_file,
            output_dir,
            permutations,
        } => {
            timing_analysis::run(&contracts_file, &cross_links_file, &output_dir, permutations)?;
        }
    }

    Ok(())
}

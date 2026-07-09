use std::process::ExitCode;

use serde_json::{json, Value};

use crate::api::{auth, client};
use crate::targeting;

pub enum Format {
    Table,
    Json,
    Tsv,
}

#[derive(serde::Serialize)]
struct Idea {
    keyword: String,
    avg_monthly_searches: Option<u64>,
    competition: Option<String>,
    competition_index: Option<u64>,
    low_top_of_page_bid_micros: Option<u64>,
    high_top_of_page_bid_micros: Option<u64>,
}

pub fn run(
    seeds: &[String],
    url: Option<&str>,
    locations: &[String],
    language: &str,
    limit: usize,
    format: Format,
    verbose: bool,
) -> ExitCode {
    let seeds: Vec<String> = seeds
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let url = url.map(str::trim).filter(|u| !u.is_empty());
    if seeds.is_empty() && url.is_none() {
        eprintln!("keyword-ideas: provide at least one seed keyword or --url");
        return ExitCode::from(2);
    }

    let language_constant = match targeting::resolve_language(language) {
        Some(c) => c,
        None => {
            eprintln!(
                "keyword-ideas: unknown language '{language}' (use an ISO code like `en`, or a raw languageConstants/NNNN)",
            );
            return ExitCode::from(2);
        }
    };

    let mut geo_constants: Vec<String> = Vec::new();
    for loc in locations {
        match targeting::resolve_location(loc) {
            Some(c) => geo_constants.push(c),
            None => {
                eprintln!(
                    "keyword-ideas: unknown location '{loc}' (use an ISO country code like `US`, or a raw geoTargetConstants/NNNN)",
                );
                return ExitCode::from(2);
            }
        }
    }

    let mut body = json!({
        "language": language_constant,
        "keywordPlanNetwork": "GOOGLE_SEARCH",
        "includeAdultKeywords": false,
    });
    if !geo_constants.is_empty() {
        body["geoTargetConstants"] = json!(geo_constants);
    }
    match (seeds.is_empty(), url) {
        (false, Some(u)) => body["keywordAndUrlSeed"] = json!({ "url": u, "keywords": seeds }),
        (false, None) => body["keywordSeed"] = json!({ "keywords": seeds }),
        (true, Some(u)) => body["urlSeed"] = json!({ "url": u }),
        (true, None) => unreachable!(),
    }

    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("keyword-ideas: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("keyword-ideas: {e}");
            return ExitCode::from(1);
        }
    };

    if verbose {
        eprintln!(
            "keyword-ideas: POST /{}/customers/{}:generateKeywordIdeas",
            client::api_version(),
            client.customer_id,
        );
        eprintln!("keyword-ideas: request: {body}");
    }

    let response = match client.generate_keyword_ideas(&token.token, &body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("keyword-ideas: {e}");
            return ExitCode::from(1);
        }
    };
    if response.status < 200 || response.status >= 300 {
        eprintln!(
            "keyword-ideas: HTTP {} from generateKeywordIdeas",
            response.status,
        );
        eprintln!("{}", response.body_raw);
        return ExitCode::from(1);
    }

    let mut ideas = collect_ideas(&response.body);
    ideas.sort_by(|a, b| b.avg_monthly_searches.cmp(&a.avg_monthly_searches));
    let total = ideas.len();
    if limit > 0 && ideas.len() > limit {
        ideas.truncate(limit);
    }

    match format {
        Format::Json => print_json(&ideas),
        Format::Tsv => print_tsv(&ideas),
        Format::Table => print_table(&ideas, total),
    }
    ExitCode::SUCCESS
}

fn collect_ideas(body: &Value) -> Vec<Idea> {
    let Some(results) = body.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    results
        .iter()
        .map(|r| {
            let metrics = r.get("keywordIdeaMetrics");
            Idea {
                keyword: r.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
                avg_monthly_searches: metrics.and_then(|m| int_field(m, "avgMonthlySearches")),
                competition: metrics
                    .and_then(|m| m.get("competition"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                competition_index: metrics.and_then(|m| int_field(m, "competitionIndex")),
                low_top_of_page_bid_micros: metrics
                    .and_then(|m| int_field(m, "lowTopOfPageBidMicros")),
                high_top_of_page_bid_micros: metrics
                    .and_then(|m| int_field(m, "highTopOfPageBidMicros")),
            }
        })
        .collect()
}

/// Google's REST transport encodes int64 fields (search volume, bid micros) as
/// JSON strings; accept a bare number too, defensively.
fn int_field(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

fn opt_num(v: Option<u64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_default()
}

fn opt_micros(v: Option<u64>) -> String {
    v.map(|m| format!("{:.2}", m as f64 / 1_000_000.0))
        .unwrap_or_default()
}

const HEADERS: [&str; 6] = [
    "keyword",
    "avg_monthly_searches",
    "competition",
    "competition_index",
    "top_page_bid_low",
    "top_page_bid_high",
];
const NUMERIC: [bool; 6] = [false, true, false, true, true, true];

fn cells(i: &Idea) -> [String; 6] {
    [
        i.keyword.clone(),
        opt_num(i.avg_monthly_searches),
        i.competition.clone().unwrap_or_default(),
        opt_num(i.competition_index),
        opt_micros(i.low_top_of_page_bid_micros),
        opt_micros(i.high_top_of_page_bid_micros),
    ]
}

fn print_table(ideas: &[Idea], total: usize) {
    if ideas.is_empty() {
        println!("(no keyword ideas)");
        return;
    }
    let rows: Vec<[String; 6]> = ideas.iter().map(cells).collect();
    let mut widths: Vec<usize> = HEADERS.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let pad = |s: &str, w: usize, right: bool| {
        let n = s.chars().count();
        if n >= w {
            s.to_string()
        } else if right {
            format!("{}{s}", " ".repeat(w - n))
        } else {
            format!("{s}{}", " ".repeat(w - n))
        }
    };
    let render = |row: &[String]| {
        row.iter()
            .enumerate()
            .map(|(i, c)| pad(c, widths[i], NUMERIC[i]))
            .collect::<Vec<_>>()
            .join("  ")
    };
    let header: Vec<String> = HEADERS.iter().map(|h| h.to_string()).collect();
    println!("{}", render(&header));
    println!(
        "{}",
        widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  "),
    );
    for row in &rows {
        println!("{}", render(row));
    }
    let shown = rows.len();
    if shown < total {
        println!(
            "\n{shown} of {total} idea(s) — top {shown} by search volume (--limit 0 for all).",
        );
    } else {
        println!("\n{shown} idea(s).");
    }
    println!("Bids are top-of-page estimates in the account currency.");
}

fn print_tsv(ideas: &[Idea]) {
    println!("{}", HEADERS.join("\t"));
    for idea in ideas {
        let row = cells(idea)
            .iter()
            .map(|c| c.replace(['\t', '\n'], " "))
            .collect::<Vec<_>>()
            .join("\t");
        println!("{row}");
    }
}

fn print_json(ideas: &[Idea]) {
    println!(
        "{}",
        serde_json::to_string_pretty(ideas).unwrap_or_else(|_| "[]".to_string()),
    );
}

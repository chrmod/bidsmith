use std::collections::BTreeSet;
use std::process::ExitCode;

use serde_json::Value;

use crate::api::{auth, client};

pub enum Format {
    Table,
    Json,
    Tsv,
}

pub fn run(query: &str, format: Format, verbose: bool) -> ExitCode {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        eprintln!("query: GAQL string is empty");
        return ExitCode::from(2);
    }

    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("query: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::exchange_refresh_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("query: {e}");
            return ExitCode::from(1);
        }
    };

    if verbose {
        eprintln!("query: POST /{}/googleAds:searchStream", client::api_version());
        eprintln!("query: GAQL: {trimmed}");
    }

    let response = match client.search_stream(&token.token, trimmed) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("query: {e}");
            return ExitCode::from(1);
        }
    };
    if response.status < 200 || response.status >= 300 {
        eprintln!("query: HTTP {} from googleAds:searchStream", response.status);
        eprintln!("{}", response.body_raw);
        return ExitCode::from(1);
    }

    let rows = collect_rows(&response.body);
    match format {
        Format::Json => print_json(&rows),
        Format::Tsv => print_tsv(&rows),
        Format::Table => print_table(&rows),
    }
    ExitCode::SUCCESS
}

fn collect_rows(body: &Value) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    if let Some(arr) = body.as_array() {
        for batch in arr {
            if let Some(results) = batch.get("results").and_then(Value::as_array) {
                rows.extend(results.iter().cloned());
            }
        }
    } else if let Some(results) = body.get("results").and_then(Value::as_array) {
        rows.extend(results.iter().cloned());
    }
    rows
}

fn flat_keys(rows: &[Value]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        collect_paths(row, String::new(), &mut seen);
    }
    seen.into_iter().collect()
}

fn collect_paths(v: &Value, prefix: String, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                collect_paths(child, path, out);
            }
        }
        _ => {
            out.insert(prefix);
        }
    }
}

fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for segment in path.split('.') {
        cur = cur.get(segment)?;
    }
    Some(cur)
}

fn render_cell(v: Option<&Value>) -> String {
    match v {
        None => String::new(),
        Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

fn print_json(rows: &[Value]) {
    println!(
        "{}",
        serde_json::to_string_pretty(rows).unwrap_or_else(|_| "[]".to_string())
    );
}

fn print_tsv(rows: &[Value]) {
    let keys = flat_keys(rows);
    if keys.is_empty() {
        return;
    }
    println!("{}", keys.join("\t"));
    for row in rows {
        let cells: Vec<String> = keys
            .iter()
            .map(|k| sanitize_tsv(&render_cell(get_path(row, k))))
            .collect();
        println!("{}", cells.join("\t"));
    }
}

fn sanitize_tsv(s: &str) -> String {
    s.replace('\t', " ").replace('\n', " ")
}

fn print_table(rows: &[Value]) {
    let keys = flat_keys(rows);
    if keys.is_empty() {
        println!("(no rows)");
        return;
    }
    let mut widths: Vec<usize> = keys.iter().map(|k| k.chars().count()).collect();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            keys.iter()
                .enumerate()
                .map(|(i, k)| {
                    let cell = render_cell(get_path(row, k));
                    let w = cell.chars().count();
                    if w > widths[i] {
                        widths[i] = w;
                    }
                    cell
                })
                .collect()
        })
        .collect();
    let pad = |s: &str, w: usize| {
        let n = s.chars().count();
        if n >= w {
            s.to_string()
        } else {
            format!("{s}{}", " ".repeat(w - n))
        }
    };
    let header = keys
        .iter()
        .enumerate()
        .map(|(i, k)| pad(k, widths[i]))
        .collect::<Vec<_>>()
        .join("  ");
    println!("{header}");
    let sep = widths
        .iter()
        .map(|w| "-".repeat(*w))
        .collect::<Vec<_>>()
        .join("  ");
    println!("{sep}");
    for row in &cells {
        let line = row
            .iter()
            .enumerate()
            .map(|(i, c)| pad(c, widths[i]))
            .collect::<Vec<_>>()
            .join("  ");
        println!("{line}");
    }
    println!("\n{} row(s)", rows.len());
}

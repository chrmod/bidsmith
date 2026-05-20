use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::api::{auth, client, diff, import, live_state, mutate};
use crate::commands::export::ExportInput;
use crate::diagnostics::Diag;
use crate::parser::{ParsedFile, parse_file};
use crate::schema::validate_files;

pub fn run(path: Option<&str>, whoami: bool, read_live: bool, verbose: bool) -> ExitCode {
    if whoami {
        return run_whoami();
    }
    if read_live {
        return run_read_live(verbose);
    }

    let Some(path) = path else {
        eprintln!("plan: provide a .bid file or directory, or pass --whoami / --read-live");
        return ExitCode::from(2);
    };

    let prepared = match prepare(path, "plan") {
        Ok(Some(p)) => p,
        Ok(None) => return ExitCode::SUCCESS,
        Err(code) => return code,
    };

    execute(&prepared, /* validate_only */ true, verbose, DisplayMode::PerResource)
}

/// State produced by the parse/import/diff stages, ready to be sent through
/// `googleAds:mutate` one or more times (e.g. validateOnly then real).
pub struct Prepared {
    pub label: &'static str,
    pub client: client::Client,
    pub token: auth::AccessToken,
    pub imported: import::ImportResult,
    pub report: diff::DiffReport,
    pub width: usize,
}

pub enum DisplayMode {
    /// Print each resource's diff row with its API outcome, then a summary.
    PerResource,
    /// Print only errors and a final one-line summary (used for the real-mutate
    /// pass of `apply`, where the user has already seen the diff).
    Summary,
}

fn run_read_live(verbose: bool) -> ExitCode {
    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::exchange_refresh_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("plan: {e}");
            return ExitCode::from(1);
        }
    };

    let _ = verbose;
    eprintln!(
        "plan: fetching live state from customers/{} (via /{}/googleAds:searchStream)...",
        client.customer_id,
        client::api_version(),
    );

    let state = match live_state::fetch(&client, &token.token) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("plan: live-state fetch failed: {e}");
            return ExitCode::from(1);
        }
    };

    println!("live state (customer {}):", state.customer_id);
    println!("  campaign_budgets    : {}", state.campaign_budgets.len());
    println!("  campaigns           : {}", state.campaigns.len());
    println!("  ad_groups           : {}", state.ad_groups.len());
    println!("  ad_group_ads        : {}", state.ad_group_ads.len());
    println!("  ad_group_criteria   : {}", state.ad_group_criteria.len());
    println!(
        "  campaign_criteria   : {} (keyword / location / language / proximity only)",
        state.campaign_criteria.len(),
    );

    ExitCode::SUCCESS
}

/// Parse, validate, import, fetch live state, and compute the diff.
///
/// Returns `Ok(Some(Prepared))` when there is at least one declared resource
/// to act on. Returns `Ok(None)` when the .bid declares nothing recognised —
/// the caller's job is just to exit successfully; the user-facing message has
/// already been printed. Returns `Err(code)` for any fatal stage.
pub fn prepare(path: &str, label: &'static str) -> Result<Option<Prepared>, ExitCode> {
    let parsed = load_and_validate(path)?;

    let mut imported = match import::import_files(&parsed) {
        Ok(v) => v,
        Err(diags) => {
            for d in diags {
                let report = miette::Report::new(d);
                eprintln!("{report:?}");
            }
            return Err(ExitCode::from(1));
        }
    };

    let total = imported.input.campaign_budgets.len()
        + imported.input.campaigns.len()
        + imported.input.ad_groups.len()
        + imported.input.ad_group_ads.len()
        + imported.input.ad_group_criteria.len()
        + imported.input.campaign_criteria.len();
    if total == 0 {
        eprintln!("{label}: nothing to do (no recognised resources in the .bid).");
        if !imported.skipped.is_empty() {
            eprintln!(
                "{label}: skipped {} resource(s) of unsupported types.",
                imported.skipped.len()
            );
        }
        return Ok(None);
    }

    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{label}: {e}");
            return Err(ExitCode::from(1));
        }
    };

    imported.input.customer_id = client.customer_id.clone();
    if let Some(login) = &client.login_customer_id {
        imported.input.login_customer_id = Some(login.clone());
    }

    let token = match auth::exchange_refresh_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{label}: {e}");
            return Err(ExitCode::from(1));
        }
    };

    eprintln!(
        "{label}: fetching live state from customers/{}...",
        client.customer_id,
    );
    let live = match live_state::fetch(&client, &token.token) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{label}: live-state fetch failed: {e}");
            return Err(ExitCode::from(1));
        }
    };

    let report = diff::diff(&imported.input, &live);
    let width = report
        .diffs
        .iter()
        .map(|d| d.address.len())
        .max()
        .unwrap_or(0)
        .max(40);

    Ok(Some(Prepared {
        label,
        client,
        token,
        imported,
        report,
        width,
    }))
}

/// Build the mutate body for `prepared.report` and POST it. Display behaviour
/// is controlled by `display` — see [`DisplayMode`].
pub fn execute(
    prepared: &Prepared,
    validate_only: bool,
    verbose: bool,
    display: DisplayMode,
) -> ExitCode {
    let report = &prepared.report;
    let width = prepared.width;
    let label = prepared.label;

    if report.create_count == 0 && report.update_count == 0 {
        if matches!(display, DisplayMode::PerResource) {
            for d in &report.diffs {
                println!("{addr:<width$}  no-op", addr = d.address, width = width);
            }
            println!();
        }
        let title = summary_title(validate_only);
        println!(
            "{title}: 0 to create, 0 to update, {} unchanged. (no API call needed)",
            report.noop_count,
        );
        return ExitCode::SUCCESS;
    }

    let plan_body =
        match mutate::build_mutate_with_diff(&prepared.imported.input, report, validate_only) {
            Ok(b) => b,
            Err(errs) => {
                for e in errs {
                    eprintln!("{label}: {} — {}", e.address, e.message);
                }
                return ExitCode::from(1);
            }
        };

    if verbose {
        let mode = if validate_only { "validateOnly" } else { "real apply" };
        eprintln!(
            "{label}: POST /{}/customers/{}/googleAds:mutate ({} op(s), {mode})",
            client::api_version(),
            prepared.client.customer_id,
            plan_body.operations.len(),
        );
        eprintln!("--- request body ---");
        eprintln!("{}", serde_json::to_string_pretty(&plan_body.body).unwrap_or_default());
    }

    let response = match prepared.client.googleads_mutate(&prepared.token.token, &plan_body.body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{label}: {e}");
            return ExitCode::from(1);
        }
    };

    if verbose {
        eprintln!("--- response (HTTP {}) ---", response.status);
        eprintln!("{}", response.body_raw);
    }

    let mut errors_by_address: HashMap<String, Vec<&str>> = HashMap::new();
    let parsed_errors = extract_google_ads_errors(&response.body);
    let success = response.status >= 200 && response.status < 300;
    if !success {
        if response.status == 404 && response.body_raw.contains("<!DOCTYPE html>") {
            eprintln!(
                "{label}: HTTP 404 + HTML body — likely a retired Google Ads API version. \
                 Try BIDSMITH_API_VERSION=v22 (or current).",
            );
            return ExitCode::from(1);
        }
        for err in &parsed_errors {
            if let Some(addr) = err
                .op_index
                .and_then(|i| plan_body.operations.get(i))
                .map(|op| op.address.clone())
            {
                errors_by_address.entry(addr).or_default().push(&err.message);
            }
        }
    }

    let mut accepted = 0usize;
    let mut rejected = 0usize;
    for d in &report.diffs {
        let (verb, detail): (&str, String) = match &d.action {
            diff::Action::NoOp { .. } => ("no-op", String::new()),
            diff::Action::Create => ("+ create", String::new()),
            diff::Action::Update { changed_fields, .. } => {
                ("~ update", format!(" ({})", changed_fields.join(", ")))
            }
        };
        let outcome = match &d.action {
            diff::Action::NoOp { .. } => "".to_string(),
            _ => match errors_by_address.get(&d.address) {
                Some(msgs) => format!("  err: {}", msgs.first().copied().unwrap_or("(unknown)")),
                None if success => "  ok".to_string(),
                None => "  (no result — batch rejected)".to_string(),
            },
        };
        if matches!(d.action, diff::Action::Create | diff::Action::Update { .. }) {
            if errors_by_address.contains_key(&d.address) {
                rejected += 1;
            } else if success {
                accepted += 1;
            } else {
                rejected += 1;
            }
        }
        let printable = matches!(display, DisplayMode::PerResource)
            || (matches!(display, DisplayMode::Summary)
                && errors_by_address.contains_key(&d.address));
        if printable {
            println!(
                "{addr:<width$}  {verb}{detail}{outcome}",
                addr = d.address,
                width = width,
            );
        }
    }

    if matches!(display, DisplayMode::PerResource) {
        println!();
    }
    let title = summary_title(validate_only);
    if validate_only {
        println!(
            "{title}: {} to create, {} to update, {} unchanged. ({} accepted, {} rejected)",
            report.create_count, report.update_count, report.noop_count, accepted, rejected,
        );
    } else {
        println!(
            "{title}: {} created, {} updated, {} unchanged. ({} succeeded, {} failed)",
            report.create_count, report.update_count, report.noop_count, accepted, rejected,
        );
    }

    let unattributed: Vec<_> = parsed_errors
        .iter()
        .filter(|e| {
            e.op_index
                .and_then(|i| plan_body.operations.get(i))
                .map(|op| !errors_by_address.contains_key(&op.address))
                .unwrap_or(true)
        })
        .collect();
    if !success && !unattributed.is_empty() {
        eprintln!();
        eprintln!("Other errors:");
        for err in &unattributed {
            eprintln!("  - {}", err.message);
            if !err.path.is_empty() {
                eprintln!("    at {}", err.path);
            }
            for topic in &err.policy_topics {
                eprintln!("    policy: {topic}");
            }
        }
    }

    if rejected > 0 || !success {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn summary_title(validate_only: bool) -> &'static str {
    if validate_only { "Plan" } else { "Apply" }
}

/// Convenience: returns `true` iff the prepared diff would touch anything.
pub fn has_pending_changes(prepared: &Prepared) -> bool {
    prepared.report.create_count > 0 || prepared.report.update_count > 0
}

/// Reference to the underlying ExportInput, exposed so apply can print
/// pre-prompt context (e.g. customer id) without re-routing through `client`.
#[allow(dead_code)]
pub fn declared_input(prepared: &Prepared) -> &ExportInput {
    &prepared.imported.input
}

struct GoogleAdsErrorEntry {
    message: String,
    path: String,
    op_index: Option<usize>,
    policy_topics: Vec<String>,
}

fn extract_google_ads_errors(body: &Value) -> Vec<GoogleAdsErrorEntry> {
    let mut out = Vec::new();
    let Some(details) = body
        .get("error")
        .and_then(|e| e.get("details"))
        .and_then(Value::as_array)
    else {
        return out;
    };
    for detail in details {
        let Some(errors) = detail.get("errors").and_then(Value::as_array) else {
            continue;
        };
        for err in errors {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("(no message)")
                .to_string();
            let mut path_parts: Vec<String> = Vec::new();
            let mut op_index: Option<usize> = None;
            if let Some(elems) = err
                .get("location")
                .and_then(|l| l.get("fieldPathElements"))
                .and_then(Value::as_array)
            {
                for elem in elems {
                    let name = elem.get("fieldName").and_then(Value::as_str).unwrap_or("?");
                    let index = elem.get("index").and_then(Value::as_u64);
                    match index {
                        Some(i) => {
                            path_parts.push(format!("{name}[{i}]"));
                            if name == "mutate_operations" || name == "mutateOperations" {
                                op_index = Some(i as usize);
                            }
                        }
                        None => path_parts.push(name.to_string()),
                    }
                }
            }
            let policy_topics = extract_policy_topics(err);
            out.push(GoogleAdsErrorEntry {
                message,
                path: path_parts.join("."),
                op_index,
                policy_topics,
            });
        }
    }
    out
}

fn extract_policy_topics(err: &Value) -> Vec<String> {
    let mut topics: Vec<String> = Vec::new();
    let candidates = [
        err.get("details").and_then(|d| d.get("policyFindingDetails")),
        err.get("policyFindingDetails"),
    ];
    for detail in candidates.into_iter().flatten() {
        if let Some(entries) = detail
            .get("policyTopicEntries")
            .and_then(Value::as_array)
        {
            for entry in entries {
                let topic = entry
                    .get("topic")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN");
                let typ = entry.get("type").and_then(Value::as_str);
                topics.push(match typ {
                    Some(t) => format!("{topic} ({t})"),
                    None => topic.to_string(),
                });
            }
        }
    }
    topics
}

fn load_and_validate(path: &str) -> Result<Vec<ParsedFile>, ExitCode> {
    let target = Path::new(path);
    if !target.exists() {
        eprintln!("no such file or directory: {}", target.display());
        return Err(ExitCode::from(1));
    }

    let files: Vec<PathBuf> = if target.is_file() {
        vec![target.to_path_buf()]
    } else {
        match collect_bid_files(target) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("plan: {e}");
                return Err(ExitCode::from(1));
            }
        }
    };

    if files.is_empty() {
        eprintln!("plan: no .bid files under {}", target.display());
        return Err(ExitCode::from(1));
    }

    let mut parsed: Vec<ParsedFile> = Vec::new();
    let mut diags: Vec<Diag> = Vec::new();
    for f in &files {
        match parse_file(f) {
            Ok(pf) => parsed.push(pf),
            Err(d) => diags.push(d),
        }
    }
    diags.extend(validate_files(&parsed));

    let blocking_errors: Vec<Diag> = diags.into_iter().filter(|d| d.is_error()).collect();
    if !blocking_errors.is_empty() {
        for d in blocking_errors {
            let report = miette::Report::new(d);
            eprintln!("{report:?}");
        }
        eprintln!("plan: refusing to plan an invalid .bid (fix `validate` errors first).");
        return Err(ExitCode::from(1));
    }

    Ok(parsed)
}

fn collect_bid_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    walk(dir, &mut out).map_err(|e| format!("failed to walk {}: {e}", dir.display()))?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&path, out)?;
        } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("bid") {
            out.push(path);
        }
    }
    Ok(())
}

fn run_whoami() -> ExitCode {
    let customer_id = env_or_blank("GOOGLE_ADS_CUSTOMER_ID");
    let login_customer_id = env_or_blank("GOOGLE_ADS_LOGIN_CUSTOMER_ID");
    let developer_token_present = !env_or_blank("GOOGLE_ADS_DEVELOPER_TOKEN").is_empty();

    match auth::exchange_refresh_token() {
        Ok(token) => {
            println!("plan: refresh-token exchange succeeded.");
            println!(
                "  access token       : {}…{} ({} chars)",
                head(&token.token, 8),
                tail(&token.token, 4),
                token.token.len(),
            );
            println!("  expires_in         : {}s", token.expires_in);
            println!(
                "  customer_id        : {}",
                show_or("(missing GOOGLE_ADS_CUSTOMER_ID)", &customer_id),
            );
            println!(
                "  login_customer_id  : {}",
                show_or("(missing GOOGLE_ADS_LOGIN_CUSTOMER_ID)", &login_customer_id),
            );
            println!(
                "  developer_token    : {}",
                if developer_token_present {
                    "set"
                } else {
                    "(missing GOOGLE_ADS_DEVELOPER_TOKEN)"
                },
            );
            if customer_id.is_empty() || !developer_token_present {
                eprintln!(
                    "plan: auth is ready but the API call envelope is incomplete — \
                     set the missing env var(s) before the next checkpoint.",
                );
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let report = miette::Report::msg(e.to_string());
            eprintln!("{report:?}");
            ExitCode::from(1)
        }
    }
}

fn env_or_blank(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn show_or<'a>(missing: &'a str, value: &'a str) -> &'a str {
    if value.is_empty() {
        missing
    } else {
        value
    }
}

fn head(s: &str, n: usize) -> &str {
    &s[..s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())]
}

fn tail(s: &str, n: usize) -> &str {
    let start = s
        .char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    &s[start..]
}

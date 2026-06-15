use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use crate::api::live_state::CacheMode;
use crate::api::{auth, client, diff, import, live_state, mutate};
use crate::commands::export::ExportInput;
use crate::commands::vars;
use crate::diagnostics::Diag;
use crate::program::{collect_bid_files, Program};
use crate::schema::{InputBindings, validate_files};

pub fn run(
    path: Option<&str>,
    whoami: bool,
    read_live: bool,
    refresh_state: bool,
    offline: bool,
    verbose: bool,
    cli_vars: &[String],
) -> ExitCode {
    if whoami {
        return run_whoami();
    }
    if read_live {
        if offline {
            eprintln!("plan: --offline is not supported with --read-live (which exists to inspect a fresh fetch)");
            return ExitCode::from(2);
        }
        return run_read_live(refresh_state, verbose);
    }

    let Some(path) = path else {
        eprintln!("plan: provide a .bid file or directory, or pass --whoami / --read-live");
        return ExitCode::from(2);
    };

    let inputs = match vars::collect(cli_vars) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("plan: {e}");
            return ExitCode::from(2);
        }
    };

    let prepared = match prepare(path, "plan", refresh_state, offline, &inputs) {
        Ok(Some(p)) => p,
        Ok(None) => return ExitCode::SUCCESS,
        Err(code) => return code,
    };

    execute(&prepared, /* validate_only */ true, verbose, DisplayMode::PerResource)
}

/// State produced by the parse/import/diff stages, ready to be sent through
/// `googleAds:mutate` one or more times (e.g. validateOnly then real).
///
/// `client` and `token` are `None` in offline mode — `execute` then prints
/// the diff without contacting the API.
pub struct Prepared {
    pub label: &'static str,
    pub client: Option<client::Client>,
    pub token: Option<auth::AccessToken>,
    pub imported: import::ImportResult,
    pub report: diff::DiffReport,
    pub width: usize,
    pub strip_module: bool,
}

// The module segment may contain dots (a `for_each` instance is `<label>.<key>`),
// but type and name never do, so the module ends at the 2nd-to-last dot.
fn split_module(qualified: &str) -> (&str, &str) {
    match qualified.rmatch_indices('.').nth(1) {
        Some((idx, _)) => (&qualified[..idx], &qualified[idx + 1..]),
        None => ("", qualified),
    }
}

fn display_address(qualified: &str, strip_module: bool) -> &str {
    if strip_module {
        split_module(qualified).1
    } else {
        qualified
    }
}

fn module_of(qualified: &str) -> &str {
    split_module(qualified).0
}

pub enum DisplayMode {
    /// Print each resource's diff row with its API outcome, then a summary.
    PerResource,
    /// Print only errors and a final one-line summary (used for the real-mutate
    /// pass of `apply`, where the user has already seen the diff).
    Summary,
}

fn run_read_live(refresh_state: bool, verbose: bool) -> ExitCode {
    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("plan: {e}");
            return ExitCode::from(1);
        }
    };

    let _ = verbose;
    let mode = if refresh_state { CacheMode::RefreshWrite } else { CacheMode::ReadWrite };
    let outcome = match live_state::fetch_with_cache(&client, &token.token, mode, "plan") {
        Ok(o) => o,
        Err(e) => {
            eprintln!("plan: live-state fetch failed: {e}");
            return ExitCode::from(1);
        }
    };
    let state = outcome.state;

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
pub fn prepare(
    path: &str,
    label: &'static str,
    refresh_state: bool,
    offline: bool,
    inputs: &InputBindings,
) -> Result<Option<Prepared>, ExitCode> {
    let program = load_and_validate(path, inputs, label)?;

    let mut imported = match import::import_program(&program) {
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

    let live = if offline {
        match load_live_from_cache(label, &mut imported.input) {
            Ok(input) => input,
            Err(code) => return Err(code),
        }
    } else {
        let client = match client::Client::for_target(
            &imported.input.customer_id,
            imported.input.login_customer_id.as_deref(),
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{label}: {e}");
                return Err(ExitCode::from(1));
            }
        };

        let token = match auth::get_access_token() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{label}: {e}");
                return Err(ExitCode::from(1));
            }
        };

        let mode = if refresh_state { CacheMode::RefreshWrite } else { CacheMode::ReadWrite };
        let outcome = match live_state::fetch_with_cache(&client, &token.token, mode, label) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{label}: live-state fetch failed: {e}");
                return Err(ExitCode::from(1));
            }
        };
        let live = outcome.state;

        return Ok(Some(build_prepared(label, Some(client), Some(token), imported, live)));
    };

    Ok(Some(build_prepared(label, None, None, imported, live)))
}

fn build_prepared(
    label: &'static str,
    client: Option<client::Client>,
    token: Option<auth::AccessToken>,
    imported: import::ImportResult,
    live: ExportInput,
) -> Prepared {
    let report = diff::diff(&imported.input, &live);
    let modules: std::collections::HashSet<&str> =
        report.diffs.iter().map(|d| module_of(&d.address)).collect();
    let strip_module = modules.len() <= 1;
    let width = report
        .diffs
        .iter()
        .map(|d| display_address(&d.address, strip_module).len())
        .max()
        .unwrap_or(0)
        .max(40);
    Prepared {
        label,
        client,
        token,
        imported,
        report,
        width,
        strip_module,
    }
}

fn load_live_from_cache(
    label: &'static str,
    declared: &mut ExportInput,
) -> Result<ExportInput, ExitCode> {
    use crate::api::cache;
    if declared.customer_id.is_empty() {
        eprintln!(
            "{label}: --offline still needs a customer id (provider block, bidsmith.toml, \
             GOOGLE_ADS_CUSTOMER_ID, or `bidsmith auth login`) to find the right cache entry."
        );
        return Err(ExitCode::from(1));
    }
    let customer_id = declared.customer_id.clone();
    let login = declared.login_customer_id.clone();

    let cache_dir = cache::project_cache_dir();
    let api_v = client::api_version();
    let ttl = cache::live_state_ttl_secs();
    let hit = match cache::load_live_state(&cache_dir, &customer_id, login.as_deref(), &api_v, ttl)
    {
        Some(h) => h,
        None => {
            eprintln!(
                "{label}: no fresh cached live state for customers/{customer_id}. \
                 Run `bidsmith pull` (or `bidsmith plan` without --offline) to warm the cache.",
            );
            return Err(ExitCode::from(1));
        }
    };
    eprintln!(
        "{label}: using cached live state from {} ago (offline — no API call).",
        cache::format_age(hit.age_secs),
    );
    let mega = Value::Array(hit.batches).to_string();
    crate::commands::adapt::from_search_response(&mega).map_err(|e| {
        eprintln!("{label}: cached live state failed to re-adapt: {e}");
        ExitCode::from(1)
    })
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
    let strip = prepared.strip_module;

    if report.create_count == 0 && report.update_count == 0 {
        if matches!(display, DisplayMode::PerResource) {
            for d in &report.diffs {
                println!(
                    "{addr:<width$}  no-op",
                    addr = display_address(&d.address, strip),
                    width = width
                );
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

    let Some((client, token)) = prepared.client.as_ref().zip(prepared.token.as_ref()) else {
        return display_offline_diff(prepared, validate_only);
    };

    let plan_body =
        match mutate::build_mutate_with_diff(&prepared.imported.input, report, validate_only) {
            Ok(b) => b,
            Err(errs) => {
                for e in errs {
                    eprintln!("{label}: {} — {}", display_address(&e.address, strip), e.message);
                }
                return ExitCode::from(1);
            }
        };

    if verbose {
        let mode = if validate_only { "validateOnly" } else { "real apply" };
        eprintln!(
            "{label}: POST /{}/customers/{}/googleAds:mutate ({} op(s), {mode})",
            client::api_version(),
            client.customer_id,
            plan_body.operations.len(),
        );
        eprintln!("--- request body ---");
        eprintln!("{}", serde_json::to_string_pretty(&plan_body.body).unwrap_or_default());
    }

    let response = match client.googleads_mutate(&token.token, &plan_body.body) {
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
                addr = display_address(&d.address, strip),
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

fn display_offline_diff(prepared: &Prepared, validate_only: bool) -> ExitCode {
    let report = &prepared.report;
    let width = prepared.width;
    let strip = prepared.strip_module;
    for d in &report.diffs {
        let (verb, detail): (&str, String) = match &d.action {
            diff::Action::NoOp { .. } => ("no-op", String::new()),
            diff::Action::Create => ("+ create", String::new()),
            diff::Action::Update { changed_fields, .. } => {
                ("~ update", format!(" ({})", changed_fields.join(", ")))
            }
        };
        println!(
            "{addr:<width$}  {verb}{detail}",
            addr = display_address(&d.address, strip),
            width = width,
        );
    }
    println!();
    let title = summary_title(validate_only);
    println!(
        "{title}: {} to create, {} to update, {} unchanged. (offline — diff only, not server-validated)",
        report.create_count, report.update_count, report.noop_count,
    );
    ExitCode::SUCCESS
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

fn load_and_validate(
    path: &str,
    inputs: &InputBindings,
    label: &'static str,
) -> Result<Program, ExitCode> {
    let target = Path::new(path);
    if !target.exists() {
        eprintln!("no such file or directory: {}", target.display());
        return Err(ExitCode::from(1));
    }

    let files = match collect_bid_files(target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{label}: {e}");
            return Err(ExitCode::from(1));
        }
    };

    if files.is_empty() {
        eprintln!("{label}: no .bid files under {}", target.display());
        return Err(ExitCode::from(1));
    }

    let loaded = Program::load(&files, inputs.clone());
    let program = loaded.program;
    let mut diags: Vec<Diag> = loaded.diagnostics;
    for scope in &program.scopes {
        diags.extend(validate_files(&scope.files, &scope.inputs));
    }

    let blocking_errors: Vec<Diag> = diags.into_iter().filter(|d| d.is_error()).collect();
    if !blocking_errors.is_empty() {
        for d in blocking_errors {
            let report = miette::Report::new(d);
            eprintln!("{report:?}");
        }
        eprintln!("{label}: refusing to plan an invalid .bid (fix `validate` errors first).");
        return Err(ExitCode::from(1));
    }

    Ok(program)
}

fn run_whoami() -> ExitCode {
    let resolved = crate::api::creds::Resolved::load();
    let customer_id = resolved.customer_id().unwrap_or_default();
    let login_customer_id = resolved.login_customer_id().unwrap_or_default();
    let developer_token_present = resolved.developer_token().is_some();

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
                show_or("(missing — run `bidsmith auth login`)", &customer_id),
            );
            println!(
                "  login_customer_id  : {}",
                show_or("(missing — run `bidsmith auth login`)", &login_customer_id),
            );
            println!(
                "  developer_token    : {}",
                if developer_token_present {
                    "set"
                } else {
                    "(missing — run `bidsmith auth login`)"
                },
            );
            if customer_id.is_empty() || !developer_token_present {
                eprintln!(
                    "plan: auth is ready but the API call envelope is incomplete — \
                     run `bidsmith auth login` (or set the missing GOOGLE_ADS_* env var).",
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

#[cfg(test)]
mod tests {
    use super::{display_address, module_of, split_module};

    #[test]
    fn split_module_handles_plain_address() {
        assert_eq!(
            split_module("summer.google_ads_campaign.search"),
            ("summer", "google_ads_campaign.search")
        );
        assert_eq!(module_of("summer.google_ads_campaign.search"), "summer");
        assert_eq!(
            display_address("summer.google_ads_campaign.search", true),
            "google_ads_campaign.search"
        );
    }

    #[test]
    fn split_module_handles_for_each_instance_address() {
        let addr = "ghostery_search.privacy.google_ads_campaign.search";
        assert_eq!(
            split_module(addr),
            ("ghostery_search.privacy", "google_ads_campaign.search")
        );
        assert_eq!(module_of(addr), "ghostery_search.privacy");
        assert_eq!(display_address(addr, true), "google_ads_campaign.search");
        assert_eq!(display_address(addr, false), addr);
    }

    #[test]
    fn for_each_siblings_are_distinct_modules() {
        let a = "ghostery_search.privacy.google_ads_campaign.search";
        let b = "ghostery_search.adblock.google_ads_campaign.search";
        assert_ne!(module_of(a), module_of(b));
    }
}

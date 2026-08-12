use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use crate::api::live_state::CacheMode;
use crate::api::spend::SpendSummary;
use crate::api::{auth, client, diff, import, live_state, mutate, spend};
use crate::commands::export::ExportInput;
use crate::commands::vars;
use crate::diagnostics::Diag;
use crate::program::{collect_bid_files, Program};
use crate::schema::{InputBindings, validate_files};

/// How the diff is rendered. `Text` is the aligned per-resource listing;
/// `Markdown` is a table suited to a pull-request comment.
#[derive(Copy, Clone, PartialEq)]
pub enum Format {
    Text,
    Markdown,
}

pub fn run(
    path: Option<&str>,
    whoami: bool,
    read_live: bool,
    refresh_state: bool,
    offline: bool,
    verbose: bool,
    show_unchanged: bool,
    format: Format,
    detailed_exitcode: bool,
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

    execute(
        &prepared,
        /* validate_only */ true,
        verbose,
        show_unchanged,
        DisplayMode::PerResource,
        format,
        detailed_exitcode,
    )
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
    pub spend: SpendSummary,
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
    println!(
        "  currency            : {}",
        state.currency_code.as_deref().unwrap_or("(unknown)"),
    );
    println!("  campaign_budgets    : {}", state.campaign_budgets.len());
    println!("  campaigns           : {}", state.campaigns.len());
    println!("  ad_groups           : {}", state.ad_groups.len());
    println!("  ad_group_ads        : {}", state.ad_group_ads.len());
    println!("  ad_group_criteria   : {}", state.ad_group_criteria.len());
    println!(
        "  campaign_criteria   : {} (keyword / location / language / proximity / device only)",
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

    if let Some(notice) = crate::commands::export::video_upload_notice(&imported.input) {
        eprintln!("{label}: {notice}");
    }

    let live = if offline {
        load_live_from_cache(label, &mut imported.input)?
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
    mut imported: import::ImportResult,
    mut live: ExportInput,
) -> Prepared {
    // Normalize both sides so an omitted attribute carrying a schema default is
    // compared (and mutated) as that default — "omitted" means "managed at the
    // default", not "unmanaged". Filling is None→default only, so real values
    // are never masked.
    imported.input.apply_schema_defaults();
    live.apply_schema_defaults();
    let report = diff::diff(&imported.input, &live);
    let spend = spend::summarize(&imported.input, &live, &report);
    for w in &report.warnings {
        eprintln!("{label}: warning: {w}");
    }
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
        spend,
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
    let queries_fp = live_state::queries_fingerprint();
    let hit = match cache::load_live_state(
        &cache_dir,
        &customer_id,
        login.as_deref(),
        &api_v,
        &queries_fp,
        ttl,
    ) {
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
    show_unchanged: bool,
    display: DisplayMode,
    format: Format,
    detailed_exitcode: bool,
) -> ExitCode {
    let report = &prepared.report;
    let width = prepared.width;
    let label = prepared.label;
    let strip = prepared.strip_module;

    if report.create_count == 0
        && report.update_count == 0
        && report.delete_count == 0
        && report.adopt_count == 0
    {
        if format == Format::Markdown {
            println!("## bidsmith {}\n", summary_title(validate_only).to_lowercase());
            println!("**No changes.** Your `.bid` files match the live Google Ads account.");
            print_markdown_spend(&prepared.spend);
            return ExitCode::SUCCESS;
        }
        if show_unchanged && matches!(display, DisplayMode::PerResource) {
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
            "{title}: 0 to create, 0 to update, 0 to destroy, {} unchanged. (no API call needed)",
            report.noop_count,
        );
        print_text_spend(&prepared.spend);
        return ExitCode::SUCCESS;
    }

    // The batch is atomic, so one operation the account can never accept
    // rejects every unrelated one with it. Those are knowable from the live
    // state alone, so the plan stops here and nothing is sent (issue #116).
    if !report.blockers.is_empty() {
        display_offline_diff(
            prepared,
            validate_only,
            show_unchanged,
            format,
            /* detailed_exitcode */ false,
            "not sent — see the blocking errors below",
        );
        eprintln!();
        for b in &report.blockers {
            eprintln!("{label}: error: {b}");
        }
        eprintln!();
        eprintln!(
            "{label}: {} operation(s) cannot succeed against this account, and the batch is \
             all-or-nothing — nothing was sent. Resolve them, or drop them from the plan.",
            report.blockers.len()
        );
        return ExitCode::from(1);
    }

    let Some((client, token)) = prepared.client.as_ref().zip(prepared.token.as_ref()) else {
        return display_offline_diff(
            prepared,
            validate_only,
            show_unchanged,
            format,
            detailed_exitcode,
            "offline — diff only, not server-validated",
        );
    };

    // Resources their own service owns go first: the unified batch below
    // references the resource names they return.
    let pre = match run_service_mutations(prepared, client, &token.token, validate_only, verbose) {
        Ok(p) => p,
        Err(code) => return code,
    };

    let plan_body = match mutate::build_mutate_with_diff(
        &prepared.imported.input,
        report,
        validate_only,
        &pre.created,
    ) {
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
    for (address, msgs) in &pre.errors_by_address {
        errors_by_address.insert(address.clone(), msgs.iter().map(String::as_str).collect());
    }
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

    let deferred: HashSet<&str> = plan_body.deferred.iter().map(String::as_str).collect();
    let adopted = adopted_addresses(report);
    let claims = claim_details(report);
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    let mut collateral = 0usize;
    let mut deferred_count = 0usize;
    let mut md_rows: Vec<(String, String, String)> = Vec::new();
    for d in &report.diffs {
        let is_adopt = adopted.contains(d.address.as_str());
        let claim = claims
            .get(d.address.as_str())
            .filter(|_| matches!(d.action, diff::Action::NoOp { .. }));
        let (verb, detail) = verb_detail(&d.action, is_adopt, claim);
        let is_mutating =
            is_adopt || claim.is_some() || !matches!(d.action, diff::Action::NoOp { .. });
        let has_err = errors_by_address.contains_key(&d.address);
        // A row a pre-batch service handled lives or dies by that call's status,
        // not the unified batch's.
        let batch_ok = pre.handled.get(&d.address).copied().unwrap_or(success);
        let is_deferred = deferred.contains(d.address.as_str());
        if is_mutating {
            if has_err {
                rejected += 1;
            } else if !batch_ok && !is_deferred {
                // No error of its own: this operation was fine and went down
                // with the batch. Counting it as "rejected" is what sends a PR
                // author bisecting an account they never touched (issue #116).
                collateral += 1;
            } else if is_deferred {
                deferred_count += 1;
            } else {
                accepted += 1;
            }
        }
        let printable = ((is_adopt || claim.is_some())
            && matches!(display, DisplayMode::PerResource))
            || row_is_visible(&d.action, &display, show_unchanged, has_err);
        if !printable {
            continue;
        }
        let addr = display_address(&d.address, strip);
        let first_err = errors_by_address
            .get(&d.address)
            .map(|msgs| msgs.first().copied().unwrap_or("(unknown)"));
        match format {
            Format::Text => {
                let outcome = if !is_mutating {
                    String::new()
                } else if let Some(msg) = first_err {
                    format!("  err: {msg}")
                } else if is_deferred {
                    "  (not validated — waits on its custom audience)".to_string()
                } else if batch_ok {
                    "  ok".to_string()
                } else {
                    "  (not applied — another operation in the batch failed)".to_string()
                };
                println!("{addr:<width$}  {verb}{detail}{outcome}", width = width);
            }
            Format::Markdown => {
                let result = if !is_mutating {
                    String::new()
                } else if let Some(msg) = first_err {
                    format!("❌ {}", md_cell(msg))
                } else if is_deferred {
                    "⏳ waits on its custom audience".to_string()
                } else if batch_ok {
                    "✅".to_string()
                } else {
                    "⚠️ blocked by another failure".to_string()
                };
                let action = format!("{}{}", md_action(verb), detail);
                md_rows.push((addr.to_string(), action, result));
            }
        }
    }

    let unattributed: Vec<&GoogleAdsErrorEntry> = parsed_errors
        .iter()
        .filter(|e| {
            e.op_index
                .and_then(|i| plan_body.operations.get(i))
                .map(|op| !errors_by_address.contains_key(&op.address))
                .unwrap_or(true)
        })
        .collect();

    let all_ok = rejected == 0 && success && pre.handled.values().all(|ok| *ok);
    // A plan's spend figures are a forecast either way, but a *failed* apply
    // changed nothing — reporting what the account would now run would read as
    // an accomplished fact.
    let spend = (validate_only || all_ok).then_some(&prepared.spend);

    match format {
        Format::Text => {
            if matches!(display, DisplayMode::PerResource) {
                println!();
            }
            print_text_summary(
                report, validate_only, accepted, rejected, collateral, deferred_count,
            );
            if let Some(spend) = spend {
                print_text_spend(spend);
            }
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
        }
        Format::Markdown => {
            print_markdown(
                report,
                validate_only,
                accepted,
                rejected,
                collateral,
                deferred_count,
                &md_rows,
                &unattributed,
                spend,
            );
        }
    }

    if !all_ok {
        ExitCode::from(1)
    } else if detailed_exitcode {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_text_summary(
    report: &diff::DiffReport,
    validate_only: bool,
    accepted: usize,
    rejected: usize,
    collateral: usize,
    deferred: usize,
) {
    let title = summary_title(validate_only);
    let tail = format!("{}{}", collateral_clause(collateral), deferred_clause(deferred));
    if validate_only {
        println!(
            "{title}: {} to create, {} to update, {} to destroy{}, {}{} unchanged. ({} accepted, {} rejected{tail})",
            report.create_count, report.update_count, report.delete_count,
            skipped_clause(report),
            adopt_clause(report, false), report.noop_count - report.adopt_count, accepted, rejected,
        );
    } else {
        println!(
            "{title}: {} created, {} updated, {} destroyed{}, {}{} unchanged. ({} succeeded, {} failed{tail})",
            report.create_count, report.update_count, report.delete_count,
            skipped_clause(report),
            adopt_clause(report, true), report.noop_count - report.adopt_count, accepted, rejected,
        );
    }
}

/// The money question an operation count can't answer: what this changeset
/// commits per day, and what the account runs on once it lands (issue #117).
fn print_text_spend(spend: &SpendSummary) {
    if let Some(line) = spend.line() {
        println!("Budget: {line}");
    }
}

fn print_markdown_spend(spend: &SpendSummary) {
    if let Some(line) = spend.line() {
        println!("\n**Budget:** {line}");
    }
}

/// Only shown when something was actually held back, so the common summary
/// line keeps its familiar shape.
fn deferred_clause(deferred: usize) -> String {
    if deferred == 0 {
        String::new()
    } else {
        format!(", {deferred} deferred")
    }
}

/// Removals the API refuses and bidsmith therefore never sent. Shown so a
/// skipped row reads as a decision rather than an omission (issue #116).
fn skipped_clause(report: &diff::DiffReport) -> String {
    if report.skipped_removal_count == 0 {
        String::new()
    } else {
        format!(" ({} skipped)", report.skipped_removal_count)
    }
}

/// Operations that drew no error of their own and failed only because the
/// batch is atomic. Kept out of the `rejected` count so a red plan says how
/// much of it is actually the author's to fix (issue #116).
fn collateral_clause(collateral: usize) -> String {
    if collateral == 0 {
        String::new()
    } else {
        format!(", {collateral} blocked by those failures")
    }
}

/// Render the diff as a GitHub-flavoured Markdown table — the shape the
/// scaffolded CI posts as a pull-request comment.
fn print_markdown(
    report: &diff::DiffReport,
    validate_only: bool,
    accepted: usize,
    rejected: usize,
    collateral: usize,
    deferred: usize,
    rows: &[(String, String, String)],
    unattributed: &[&GoogleAdsErrorEntry],
    spend: Option<&SpendSummary>,
) {
    println!("## bidsmith {}\n", summary_title(validate_only).to_lowercase());
    if !rows.is_empty() {
        println!("| Resource | Action | Result |");
        println!("| --- | --- | --- |");
        for (addr, action, result) in rows {
            println!("| `{addr}` | {action} | {result} |");
        }
        println!();
    }
    println!(
        "**Plan:** {} to create, {} to update, {} to destroy{}, {}{} unchanged. ({} accepted, {} rejected{}{})",
        report.create_count, report.update_count, report.delete_count,
        skipped_clause(report),
        adopt_clause(report, false), report.noop_count - report.adopt_count, accepted, rejected,
        collateral_clause(collateral),
        deferred_clause(deferred),
    );
    if let Some(spend) = spend {
        print_markdown_spend(spend);
    }
    if !unattributed.is_empty() {
        println!("\n### Other errors\n");
        for err in unattributed {
            if err.path.is_empty() {
                println!("- {}", md_cell(&err.message));
            } else {
                println!("- {} _(at {})_", md_cell(&err.message), md_cell(&err.path));
            }
            for topic in &err.policy_topics {
                println!("  - policy: {}", md_cell(topic));
            }
        }
    }
}

/// Escape a string for a single Markdown table cell / list item: pipes break
/// table columns, newlines break rows.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// The text-listing verb (`+ create`, `~ update`, `- destroy`, `~ adopt`,
/// `no-op`) without its leading diff symbol, for the Markdown "Action" column.
fn md_action(verb: &str) -> &str {
    verb.trim_start_matches(['+', '~', '-', ' '])
}

fn summary_title(validate_only: bool) -> &'static str {
    if validate_only { "Plan" } else { "Apply" }
}

/// Addresses that match live unchanged but still need their `bidsmith:address`
/// label written — first-run adoption. Rendered as a visible `~ adopt` row and
/// counted separately from `no-op`.
fn adopted_addresses(report: &diff::DiffReport) -> std::collections::HashSet<&str> {
    if report.adopt_count == 0 {
        return std::collections::HashSet::new();
    }
    let noop: std::collections::HashSet<&str> = report
        .diffs
        .iter()
        .filter(|d| matches!(d.action, diff::Action::NoOp { .. }))
        .map(|d| d.address.as_str())
        .collect();
    report
        .label_plans
        .iter()
        .map(|p| p.address.as_str())
        .filter(|a| noop.contains(a))
        .collect()
}

/// Per-address summary of pending `bidsmith:owns` claim work, e.g.
/// `+negative keywords, -locations` — rendered as a `~ claim` row on parents
/// that otherwise match live unchanged.
fn claim_details(report: &diff::DiffReport) -> HashMap<&str, String> {
    let mut parts: HashMap<&str, Vec<String>> = HashMap::new();
    for p in &report.claim_plans {
        let sign = if p.stale_assoc_rn.is_some() { '-' } else { '+' };
        parts
            .entry(p.address.as_str())
            .or_default()
            .push(format!("{sign}{}", claim_category_display(p.category)));
    }
    parts.into_iter().map(|(a, v)| (a, v.join(", "))).collect()
}

fn verb_detail(
    action: &diff::Action,
    is_adopt: bool,
    claim: Option<&String>,
) -> (&'static str, String) {
    match (action, claim) {
        (diff::Action::NoOp { .. }, Some(c)) if is_adopt => ("~ adopt", format!(" (label; {c})")),
        (diff::Action::NoOp { .. }, _) if is_adopt => ("~ adopt", " (label)".to_string()),
        (diff::Action::NoOp { .. }, Some(c)) => ("~ claim", format!(" ({c})")),
        (diff::Action::NoOp { .. }, None) => ("no-op", String::new()),
        (diff::Action::Create, _) => ("+ create", String::new()),
        (diff::Action::Update { changed_fields, .. }, _) => {
            ("~ update", format!(" ({})", changed_fields.join(", ")))
        }
        (diff::Action::Delete { .. }, _) => ("- destroy", String::new()),
    }
}

fn claim_category_display(category: &str) -> &'static str {
    match category {
        "keyword_positive" => "keywords",
        "keyword_negative" => "negative keywords",
        "location" => "locations",
        "language" => "languages",
        "proximity" => "proximity",
        "frequency_caps" => "frequency caps",
        _ => "criteria",
    }
}

/// The `", N to adopt"` (or `", N adopted"`) clause, empty when nothing adopts.
fn adopt_clause(report: &diff::DiffReport, past: bool) -> String {
    if report.adopt_count == 0 {
        String::new()
    } else if past {
        format!("{} adopted, ", report.adopt_count)
    } else {
        format!("{} to adopt, ", report.adopt_count)
    }
}

/// Whether a resource's row appears in the listing. By default the per-resource
/// listing is focused on changes: unchanged (`no-op`) rows are hidden unless
/// `show_unchanged` is set. In `Summary` mode only rejected rows surface.
fn row_is_visible(
    action: &diff::Action,
    display: &DisplayMode,
    show_unchanged: bool,
    has_error: bool,
) -> bool {
    match display {
        DisplayMode::PerResource => show_unchanged || !matches!(action, diff::Action::NoOp { .. }),
        DisplayMode::Summary => has_error,
    }
}

fn display_offline_diff(
    prepared: &Prepared,
    validate_only: bool,
    show_unchanged: bool,
    format: Format,
    detailed_exitcode: bool,
    note: &str,
) -> ExitCode {
    let report = &prepared.report;
    let width = prepared.width;
    let strip = prepared.strip_module;
    let adopted = adopted_addresses(report);
    let claims = claim_details(report);
    let mut md_rows: Vec<(String, String)> = Vec::new();
    for d in &report.diffs {
        let is_adopt = adopted.contains(d.address.as_str());
        let claim = claims
            .get(d.address.as_str())
            .filter(|_| matches!(d.action, diff::Action::NoOp { .. }));
        if !is_adopt
            && claim.is_none()
            && !row_is_visible(&d.action, &DisplayMode::PerResource, show_unchanged, false)
        {
            continue;
        }
        let (verb, detail) = verb_detail(&d.action, is_adopt, claim);
        let addr = display_address(&d.address, strip);
        match format {
            Format::Text => {
                println!("{addr:<width$}  {verb}{detail}", width = width);
            }
            Format::Markdown => {
                let action = format!("{}{}", md_action(verb), detail);
                md_rows.push((addr.to_string(), action));
            }
        }
    }
    match format {
        Format::Text => {
            println!();
            let title = summary_title(validate_only);
            println!(
                "{title}: {} to create, {} to update, {} to destroy{}, {}{} unchanged. ({note})",
                report.create_count, report.update_count, report.delete_count,
                skipped_clause(report),
                adopt_clause(report, false), report.noop_count - report.adopt_count,
            );
            print_text_spend(&prepared.spend);
        }
        Format::Markdown => {
            println!("## bidsmith {}\n", summary_title(validate_only).to_lowercase());
            if !md_rows.is_empty() {
                println!("| Resource | Action |");
                println!("| --- | --- |");
                for (addr, action) in &md_rows {
                    println!("| `{addr}` | {action} |");
                }
                println!();
            }
            println!(
                "**Plan:** {} to create, {} to update, {} to destroy{}, {}{} unchanged. _({note})_",
                report.create_count, report.update_count, report.delete_count,
                skipped_clause(report),
                adopt_clause(report, false), report.noop_count - report.adopt_count,
            );
            print_markdown_spend(&prepared.spend);
        }
    }
    if detailed_exitcode {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

/// Convenience: returns `true` iff the prepared diff would touch anything.
pub fn has_pending_changes(prepared: &Prepared) -> bool {
    prepared.report.create_count > 0
        || prepared.report.update_count > 0
        || prepared.report.delete_count > 0
        || prepared.report.adopt_count > 0
}

/// Reference to the underlying ExportInput, exposed so apply can print
/// pre-prompt context (e.g. customer id) without re-routing through `client`.
#[allow(dead_code)]
pub fn declared_input(prepared: &Prepared) -> &ExportInput {
    &prepared.imported.input
}

/// What the pre-batch service calls left behind for the unified batch and the
/// result table: real resource names for what they created, and their own
/// per-address outcomes (those rows are not covered by the batch's status).
#[derive(Default)]
struct ServiceMutationOutcome {
    created: HashMap<String, String>,
    errors_by_address: HashMap<String, Vec<String>>,
    /// Addresses this pass mutated, with whether their call succeeded.
    handled: HashMap<String, bool>,
}

fn run_service_mutations(
    prepared: &Prepared,
    client: &client::Client,
    access_token: &str,
    validate_only: bool,
    verbose: bool,
) -> Result<ServiceMutationOutcome, ExitCode> {
    let label = prepared.label;
    let mut outcome = ServiceMutationOutcome::default();
    let Some(pass) = mutate::build_custom_audience_mutate(
        &prepared.imported.input,
        &prepared.report,
        validate_only,
    ) else {
        return Ok(outcome);
    };

    if verbose {
        let mode = if validate_only { "validateOnly" } else { "real apply" };
        eprintln!(
            "{label}: POST /{}/customers/{}/{} ({} op(s), {mode})",
            client::api_version(),
            client.customer_id,
            pass.endpoint,
            pass.operations.len(),
        );
        eprintln!("--- request body ---");
        eprintln!("{}", serde_json::to_string_pretty(&pass.body).unwrap_or_default());
    }

    let response = match client.service_mutate(access_token, pass.endpoint, &pass.body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{label}: {} — {e}", pass.label);
            return Err(ExitCode::from(1));
        }
    };

    if verbose {
        eprintln!("--- response (HTTP {}) ---", response.status);
        eprintln!("{}", response.body_raw);
    }

    let success = response.status >= 200 && response.status < 300;
    for op in &pass.operations {
        outcome.handled.insert(op.address.clone(), success);
    }
    if success {
        // A `validateOnly` call returns errors but no results, so nothing to map.
        if let Some(results) = response.body.get("results").and_then(Value::as_array) {
            for (op, result) in pass.operations.iter().zip(results) {
                if let Some(rn) = result.get("resourceName").and_then(Value::as_str) {
                    outcome.created.insert(op.address.clone(), rn.to_string());
                }
            }
        }
        return Ok(outcome);
    }

    let mut attributed = false;
    for err in extract_google_ads_errors(&response.body) {
        match err.op_index.and_then(|i| pass.operations.get(i)) {
            Some(op) => {
                outcome
                    .errors_by_address
                    .entry(op.address.clone())
                    .or_default()
                    .push(err.message);
                attributed = true;
            }
            None => eprintln!("{label}: {} — {}", pass.label, err.message),
        }
    }
    if !attributed && outcome.errors_by_address.is_empty() {
        eprintln!(
            "{label}: {} rejected (HTTP {}): {}",
            pass.label, response.status, response.body_raw
        );
    }
    Ok(outcome)
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
                            } else if name == "operations" && op_index.is_none() {
                                // A resource-specific service names its list
                                // plain `operations`; inside the unified batch
                                // the outer `mutate_operations` already won.
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
    use super::{display_address, md_action, md_cell, module_of, row_is_visible, split_module, DisplayMode};
    use crate::api::diff::Action;

    #[test]
    fn md_cell_escapes_table_breakers() {
        assert_eq!(md_cell("a|b"), "a\\|b");
        assert_eq!(md_cell("line1\nline2"), "line1 line2");
        assert_eq!(md_cell("plain text"), "plain text");
    }

    #[test]
    fn md_action_drops_the_diff_symbol() {
        assert_eq!(md_action("+ create"), "create");
        assert_eq!(md_action("~ update"), "update");
        assert_eq!(md_action("- destroy"), "destroy");
        assert_eq!(md_action("~ adopt"), "adopt");
        assert_eq!(md_action("no-op"), "no-op");
    }

    fn noop() -> Action {
        Action::NoOp { live_id: "123".into() }
    }

    fn update() -> Action {
        Action::Update { live_id: "123".into(), changed_fields: vec!["amount_micros".into()] }
    }

    #[test]
    fn per_resource_hides_noops_by_default() {
        assert!(!row_is_visible(&noop(), &DisplayMode::PerResource, false, false));
        assert!(row_is_visible(&update(), &DisplayMode::PerResource, false, false));
        assert!(row_is_visible(&Action::Create, &DisplayMode::PerResource, false, false));
        assert!(row_is_visible(
            &Action::Delete { live_id: "1".into() },
            &DisplayMode::PerResource,
            false,
            false
        ));
    }

    #[test]
    fn show_unchanged_reveals_noops() {
        assert!(row_is_visible(&noop(), &DisplayMode::PerResource, true, false));
        assert!(row_is_visible(&update(), &DisplayMode::PerResource, true, false));
    }

    #[test]
    fn summary_mode_only_shows_errored_rows() {
        assert!(!row_is_visible(&update(), &DisplayMode::Summary, false, false));
        assert!(row_is_visible(&update(), &DisplayMode::Summary, false, true));
        assert!(!row_is_visible(&noop(), &DisplayMode::Summary, true, false));
    }

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

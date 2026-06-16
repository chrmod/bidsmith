use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::expr::Expression;
use hcl_edit::structure::Body;

use crate::api::diff::{Action, DiffReport};
use crate::api::import::import_files;
use crate::api::live_state::CacheMode;
use crate::api::{auth, client, diff, live_state};
use crate::commands::export::{
    canonicalize, filter_removed, prune_orphans, render_split, report_orphans, ExportInput,
};
use crate::diagnostics::Diag;
use crate::parser::{parse_file, parse_str, ParsedFile};
use crate::program::collect_bid_files;
use crate::schema::{validate_files, InputBindings, ResourceRegistry};

pub fn run(
    output: Option<&str>,
    dir: Option<&str>,
    include_removed: bool,
    verbose: bool,
) -> ExitCode {
    if output.is_some() && dir.is_some() {
        eprintln!("refresh: --output and --dir are mutually exclusive");
        return ExitCode::from(2);
    }

    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };

    if verbose {
        eprintln!(
            "refresh: customers/{} via /{}/googleAds:searchStream",
            client.customer_id,
            client::api_version(),
        );
    }

    let outcome = match live_state::fetch_with_cache(
        &client,
        &token.token,
        CacheMode::ReadWrite,
        "refresh",
    ) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("refresh: live-state fetch failed: {e}");
            return ExitCode::from(1);
        }
    };
    let mut input: ExportInput = outcome.state;
    if !include_removed {
        filter_removed(&mut input);
    }
    report_orphans("refresh", prune_orphans(&mut input));

    let (account_raw, campaigns_raw) = render_split(&input);
    let account = canonicalize(&account_raw);
    let campaigns = canonicalize(&campaigns_raw);

    match (output, dir) {
        (Some(path), None) => write_single(path, &account, &campaigns),
        (None, Some(d)) => write_split(d, &account, &campaigns, verbose),
        (None, None) => {
            print!("{}", account);
            if !campaigns.is_empty() {
                if !account.is_empty() && !account.ends_with("\n\n") {
                    println!();
                }
                print!("{}", campaigns);
            }
            ExitCode::SUCCESS
        }
        (Some(_), Some(_)) => unreachable!(),
    }
}

fn write_single(path: &str, account: &str, campaigns: &str) -> ExitCode {
    let mut combined = String::new();
    combined.push_str(account);
    if !campaigns.is_empty() {
        if !account.is_empty() && !account.ends_with("\n\n") {
            combined.push('\n');
        }
        combined.push_str(campaigns);
    }
    match std::fs::write(path, &combined) {
        Ok(()) => {
            eprintln!("refresh: wrote {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("refresh: failed to write {path}: {e}");
            ExitCode::from(1)
        }
    }
}

fn write_split(dir: &str, account: &str, campaigns: &str, verbose: bool) -> ExitCode {
    let dir_path = Path::new(dir);
    if let Err(e) = std::fs::create_dir_all(dir_path) {
        eprintln!("refresh: failed to create {dir}: {e}");
        return ExitCode::from(1);
    }

    let account_path = dir_path.join("account.bid");
    if let Err(e) = std::fs::write(&account_path, account) {
        eprintln!(
            "refresh: failed to write {}: {e}",
            account_path.display()
        );
        return ExitCode::from(1);
    }
    if verbose {
        eprintln!("refresh: wrote {}", account_path.display());
    }

    if campaigns.is_empty() {
        eprintln!(
            "refresh: wrote {} (no campaign-scoped resources to write)",
            account_path.display()
        );
        return ExitCode::SUCCESS;
    }

    let campaigns_path = dir_path.join("campaigns.bid");
    if let Err(e) = std::fs::write(&campaigns_path, campaigns) {
        eprintln!(
            "refresh: failed to write {}: {e}",
            campaigns_path.display()
        );
        return ExitCode::from(1);
    }
    eprintln!(
        "refresh: wrote {} and {}",
        account_path.display(),
        campaigns_path.display()
    );
    ExitCode::SUCCESS
}

// ---- reconcile mode (--in-place) -----------------------------------------
//
// Instead of rendering fresh .bid from live (bootstrap mode), reconcile reads
// the existing .bid files, diffs them against live, and writes back only the
// drifted *scalar* fields on resources bidsmith manages — leaving comments,
// block structure, ordering, and unmanaged resources untouched. Structural
// drift (ad copy, keyword sets, criteria membership) is reported, not edited:
// the diff engine only ever produces scalar `Update`s, so a changed RSA reads
// as a create+destroy elsewhere, not as a field this pass can patch.

pub fn run_reconcile(path: Option<&str>, check: bool, verbose: bool) -> ExitCode {
    let path = path.unwrap_or(".");
    let target = Path::new(path);
    if !target.exists() {
        eprintln!("refresh: no such file or directory: {path}");
        return ExitCode::from(1);
    }
    let paths = match collect_bid_files(target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };
    if paths.is_empty() {
        eprintln!(
            "refresh: no .bid files under {path} — use `bidsmith refresh -d {path}` to create them from live state."
        );
        return ExitCode::from(1);
    }

    // Parse once: the same ParsedFiles back both the declared model (read) and
    // the in-place edit (write).
    let mut files: Vec<ParsedFile> = Vec::new();
    for p in &paths {
        match parse_file(p) {
            Ok(pf) => files.push(pf),
            Err(d) => {
                eprintln!("{:?}", miette::Report::new(d));
                return ExitCode::from(1);
            }
        }
    }

    let inputs = InputBindings::default();
    let validation = validate_files(&files, &inputs);
    if validation.iter().any(Diag::is_error) {
        for d in validation.into_iter().filter(|d| d.is_error()) {
            eprintln!("{:?}", miette::Report::new(d));
        }
        eprintln!("refresh: refusing to reconcile an invalid .bid (fix `validate` errors first).");
        return ExitCode::from(1);
    }
    let baseline = error_signatures(&validation);

    let mut declared = match import_files(&files, &inputs) {
        Ok(r) => r.input,
        Err(diags) => {
            for d in diags {
                eprintln!("{:?}", miette::Report::new(d));
            }
            return ExitCode::from(1);
        }
    };
    if declared.customer_id.is_empty() {
        eprintln!(
            "refresh: no customer id — set it in the provider block, bidsmith.toml, \
             GOOGLE_ADS_CUSTOMER_ID, or run `bidsmith auth login`."
        );
        return ExitCode::from(1);
    }

    let client = match client::Client::for_target(
        &declared.customer_id,
        declared.login_customer_id.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };
    if verbose {
        eprintln!(
            "refresh: reconciling {} file(s) against customers/{} via /{}/googleAds:searchStream",
            files.len(),
            client.customer_id,
            client::api_version(),
        );
    }
    let mut live = match live_state::fetch_with_cache(
        &client,
        &token.token,
        CacheMode::ReadWrite,
        "refresh",
    ) {
        Ok(o) => o.state,
        Err(e) => {
            eprintln!("refresh: live-state fetch failed: {e}");
            return ExitCode::from(1);
        }
    };

    // Normalize both sides so an omitted attribute carrying a schema default
    // isn't read as drift (same as plan's diff prep).
    declared.apply_schema_defaults();
    live.apply_schema_defaults();
    let report = diff::diff(&declared, &live);

    let outcome = reconcile_sources(&mut files, &live, &report);

    // Re-serialize the changed files now, then re-validate the mutated tree
    // before touching disk — a malformed edit must never be written.
    let rendered: Vec<(PathBuf, String)> = outcome
        .changed_files
        .iter()
        .map(|&i| (files[i].path.clone(), files[i].body.to_string()))
        .collect();
    if let Err(code) = revalidate_reconcile(&files, &baseline) {
        return code;
    }

    report_reconcile(&outcome, check);

    if check {
        return ExitCode::SUCCESS;
    }
    for (p, content) in &rendered {
        if let Err(e) = std::fs::write(p, content) {
            eprintln!("refresh: failed to write {}: {e}", p.display());
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}

struct Edit {
    path: Vec<&'static str>,
    value: Expression,
}

struct ReconcileOutcome {
    /// (address, dotted field paths) actually written.
    applied: Vec<(String, Vec<String>)>,
    /// Human-readable lines for fields that drifted but couldn't be patched.
    skipped: Vec<String>,
    changed_files: Vec<usize>,
    create_count: usize,
    delete_count: usize,
}

/// Apply the diff's scalar `Update`s to the parsed source in place. Pure (no IO
/// / network) so it can be unit-tested with a synthetic live state.
fn reconcile_sources(
    files: &mut [ParsedFile],
    live: &ExportInput,
    report: &DiffReport,
) -> ReconcileOutcome {
    let mut planned: HashMap<String, Vec<Edit>> = HashMap::new();
    let mut skipped: Vec<String> = Vec::new();
    for d in &report.diffs {
        let Action::Update { live_id, changed_fields } = &d.action else {
            continue;
        };
        let (edits, unsupported) = collect_edits(d.kind, live, live_id, changed_fields);
        for u in unsupported {
            skipped.push(format!("{}: {u}", d.address));
        }
        if !edits.is_empty() {
            planned.entry(d.address.clone()).or_default().extend(edits);
        }
    }

    let mut applied: Vec<(String, Vec<String>)> = Vec::new();
    let mut changed_files: Vec<usize> = Vec::new();
    for (i, f) in files.iter_mut().enumerate() {
        let module = f.module.clone();
        let mut file_changed = false;
        for mut s in f.body.iter_mut() {
            let Some(b) = s.as_block_mut() else { continue };
            if b.ident.as_str() != "resource" || b.labels.len() != 2 {
                continue;
            }
            let addr =
                ResourceRegistry::qualified(&module, b.labels[0].as_str(), b.labels[1].as_str());
            let Some(edits) = planned.get(&addr) else { continue };
            let mut done: Vec<String> = Vec::new();
            for e in edits {
                if set_existing_scalar(&mut b.body, &e.path, e.value.clone()) {
                    done.push(e.path.join("."));
                    file_changed = true;
                } else {
                    skipped.push(format!(
                        "{addr}: {} (not set in your file — add it by hand or run a bootstrap refresh)",
                        e.path.join(".")
                    ));
                }
            }
            if !done.is_empty() {
                applied.push((addr.clone(), done));
            }
        }
        if file_changed {
            changed_files.push(i);
        }
    }

    ReconcileOutcome {
        applied,
        skipped,
        changed_files,
        create_count: report.create_count,
        delete_count: report.delete_count,
    }
}

/// Set a scalar at `path` within `body`, descending one block per non-terminal
/// segment. Only patches attributes (and nested blocks) that already exist —
/// returns false when the target attribute or its containing block is absent,
/// so the caller can report it rather than guess at formatting for an insert.
fn set_existing_scalar(body: &mut Body, path: &[&str], value: Expression) -> bool {
    match path {
        [key] => {
            if let Some(mut attr) = body.get_attribute_mut(key) {
                *attr.value_mut() = value;
                true
            } else {
                false
            }
        }
        [head, rest @ ..] => match body.get_blocks_mut(head).next() {
            Some(block) => set_existing_scalar(&mut block.body, rest, value),
            None => false,
        },
        [] => false,
    }
}

fn s(v: &str) -> Expression {
    Expression::from(v.to_string())
}

/// Map a resource's drifted field names to (source path, live value) edits.
/// The second tuple element is the list of fields we can't reconcile in place
/// (an unsupported resource kind, or a value cleared upstream).
fn collect_edits(
    kind: &str,
    live: &ExportInput,
    live_id: &str,
    fields: &[String],
) -> (Vec<Edit>, Vec<String>) {
    let mut e: Vec<Edit> = Vec::new();
    let mut skip: Vec<String> = Vec::new();

    macro_rules! push {
        ($path:expr, $val:expr) => {
            e.push(Edit { path: $path, value: $val })
        };
    }
    macro_rules! opt {
        ($field:expr, $path:expr, $val:expr) => {
            match $val {
                Some(v) => push!($path, v),
                None => skip.push(format!("{} (cleared upstream)", $field)),
            }
        };
    }

    match kind {
        "campaign_budget" => {
            let Some(b) = live.campaign_budgets.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "name" => push!(vec!["name"], s(&b.name)),
                    "amount_micros" => push!(vec!["amount_micros"], Expression::from(b.amount_micros)),
                    "delivery_method" => {
                        opt!(f, vec!["delivery_method"], b.delivery_method.as_deref().map(s))
                    }
                    "explicitly_shared" => {
                        opt!(f, vec!["explicitly_shared"], b.explicitly_shared.map(Expression::from))
                    }
                    other => skip.push(other.to_string()),
                }
            }
        }
        "campaign" => {
            let Some(c) = live.campaigns.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "name" => push!(vec!["name"], s(&c.name)),
                    "status" => opt!(f, vec!["status"], c.status.as_deref().map(s)),
                    "contains_eu_political_advertising" => opt!(
                        f,
                        vec!["contains_eu_political_advertising"],
                        c.contains_eu_political_advertising.as_deref().map(s)
                    ),
                    "manual_cpc.enhanced_cpc_enabled" => opt!(
                        f,
                        vec!["manual_cpc", "enhanced_cpc_enabled"],
                        c.manual_cpc
                            .as_ref()
                            .and_then(|m| m.enhanced_cpc_enabled)
                            .map(Expression::from)
                    ),
                    "network_settings.target_google_search" => opt!(
                        f,
                        vec!["network_settings", "target_google_search"],
                        c.network_settings.as_ref().and_then(|n| n.target_google_search).map(Expression::from)
                    ),
                    "network_settings.target_search_network" => opt!(
                        f,
                        vec!["network_settings", "target_search_network"],
                        c.network_settings.as_ref().and_then(|n| n.target_search_network).map(Expression::from)
                    ),
                    "network_settings.target_content_network" => opt!(
                        f,
                        vec!["network_settings", "target_content_network"],
                        c.network_settings.as_ref().and_then(|n| n.target_content_network).map(Expression::from)
                    ),
                    "network_settings.target_partner_search_network" => opt!(
                        f,
                        vec!["network_settings", "target_partner_search_network"],
                        c.network_settings.as_ref().and_then(|n| n.target_partner_search_network).map(Expression::from)
                    ),
                    other => skip.push(other.to_string()),
                }
            }
        }
        "ad_group" => {
            let Some(g) = live.ad_groups.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "name" => push!(vec!["name"], s(&g.name)),
                    "status" => opt!(f, vec!["status"], g.status.as_deref().map(s)),
                    "type" => opt!(f, vec!["type"], g.ty.as_deref().map(s)),
                    "cpc_bid_micros" => {
                        opt!(f, vec!["cpc_bid_micros"], g.cpc_bid_micros.map(Expression::from))
                    }
                    other => skip.push(other.to_string()),
                }
            }
        }
        "ad_group_ad" => {
            let Some(a) = live.ad_group_ads.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "status" => opt!(f, vec!["status"], a.status.as_deref().map(s)),
                    other => skip.push(other.to_string()),
                }
            }
        }
        "conversion_action" => {
            let Some(c) = live.conversion_actions.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "status" => opt!(f, vec!["status"], c.status.as_deref().map(s)),
                    "counting_type" => opt!(f, vec!["counting_type"], c.counting_type.as_deref().map(s)),
                    "click_through_lookback_window_days" => opt!(
                        f,
                        vec!["click_through_lookback_window_days"],
                        c.click_through_lookback_window_days.map(Expression::from)
                    ),
                    "view_through_lookback_window_days" => opt!(
                        f,
                        vec!["view_through_lookback_window_days"],
                        c.view_through_lookback_window_days.map(Expression::from)
                    ),
                    "value_settings.default_value" => opt!(
                        f,
                        vec!["value_settings", "default_value"],
                        c.value_settings.as_ref().and_then(|v| v.default_value).map(Expression::from)
                    ),
                    "value_settings.default_currency_code" => opt!(
                        f,
                        vec!["value_settings", "default_currency_code"],
                        c.value_settings.as_ref().and_then(|v| v.default_currency_code.as_deref()).map(s)
                    ),
                    "value_settings.always_use_default_value" => opt!(
                        f,
                        vec!["value_settings", "always_use_default_value"],
                        c.value_settings.as_ref().and_then(|v| v.always_use_default_value).map(Expression::from)
                    ),
                    other => skip.push(other.to_string()),
                }
            }
        }
        "customer_asset" => {
            let Some(a) = live.customer_assets.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "status" => opt!(f, vec!["status"], a.status.as_deref().map(s)),
                    other => skip.push(other.to_string()),
                }
            }
        }
        "shared_set" => {
            let Some(set) = live.shared_sets.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "status" => opt!(f, vec!["status"], set.status.as_deref().map(s)),
                    "type" => opt!(f, vec!["type"], set.ty.as_deref().map(s)),
                    other => skip.push(other.to_string()),
                }
            }
        }
        "campaign_shared_set" => {
            let Some(css) = live.campaign_shared_sets.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "status" => opt!(f, vec!["status"], css.status.as_deref().map(s)),
                    other => skip.push(other.to_string()),
                }
            }
        }
        // Keyword/criterion membership and call assets don't map to a single
        // scalar attribute on a 1:1 source block; leave them to bootstrap.
        _ => {
            for f in fields {
                skip.push(format!("{f} ({kind} reconcile not yet supported)"));
            }
        }
    }

    (e, skip)
}

fn report_reconcile(o: &ReconcileOutcome, check: bool) {
    if o.applied.is_empty() {
        println!(
            "refresh: no scalar drift to reconcile — your .bid files already match the managed resources in the account."
        );
    } else {
        for (addr, fields) in &o.applied {
            println!("  ~ {addr} ({})", fields.join(", "));
        }
        println!();
        let nfields: usize = o.applied.iter().map(|(_, f)| f.len()).sum();
        let verb = if check { "Would update" } else { "Updated" };
        println!(
            "{verb} {} resource{}, {nfields} field{} from live.",
            o.applied.len(),
            plural(o.applied.len()),
            plural(nfields),
        );
    }

    if !o.skipped.is_empty() {
        println!();
        println!("Skipped (structural or not set in your files):");
        for line in &o.skipped {
            println!("  - {line}");
        }
    }
    if o.create_count > 0 {
        println!(
            "note: {} declared resource(s) have no live match — `bidsmith plan` shows them.",
            o.create_count
        );
    }
    if o.delete_count > 0 {
        println!(
            "note: {} managed live resource(s) are no longer in your files — `bidsmith plan` shows them as `- destroy`.",
            o.delete_count
        );
    }
    if check && !o.applied.is_empty() {
        println!();
        println!("(--check: nothing written. Re-run without --check to apply.)");
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Re-parse the mutated bodies and re-validate, refusing to proceed if the edit
/// introduced an error not present before (mirrors `mv`'s baseline guard).
fn revalidate_reconcile(
    files: &[ParsedFile],
    baseline: &HashMap<(String, String), usize>,
) -> Result<(), ExitCode> {
    let mut reparsed: Vec<ParsedFile> = Vec::with_capacity(files.len());
    for f in files {
        let content = f.body.to_string();
        match parse_str(&f.path, &content) {
            Ok(pf) => reparsed.push(pf),
            Err(d) => {
                eprintln!("{:?}", miette::Report::new(d));
                eprintln!("refresh: the reconcile would produce an unparseable file; nothing was written.");
                return Err(ExitCode::from(1));
            }
        }
    }
    let errors = validate_files(&reparsed, &InputBindings::default());
    let after = error_signatures(&errors);
    let regressed = after
        .iter()
        .any(|(sig, &n)| n > baseline.get(sig).copied().unwrap_or(0));
    if regressed {
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        for d in errors.into_iter().filter(|d| d.is_error()) {
            let sig = (d.src.name().to_string(), d.message.clone());
            let allowed = baseline.get(&sig).copied().unwrap_or(0);
            let count = seen.entry(sig).or_insert(0);
            *count += 1;
            if *count > allowed {
                eprintln!("{:?}", miette::Report::new(d));
            }
        }
        eprintln!("refresh: the reconcile would break the project; nothing was written.");
        return Err(ExitCode::from(1));
    }
    Ok(())
}

fn error_signatures(diags: &[Diag]) -> HashMap<(String, String), usize> {
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for d in diags.iter().filter(|d| d.is_error()) {
        *counts
            .entry((d.src.name().to_string(), d.message.clone()))
            .or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn declared_from(src: &str) -> (Vec<ParsedFile>, ExportInput) {
        let pf = parse_str(&PathBuf::from("main.bid"), src).expect("parse");
        let files = vec![pf];
        let imported = import_files(&files, &InputBindings::default()).expect("import");
        (files, imported.input)
    }

    fn run(src: &str, live_json: &str) -> (String, ReconcileOutcome) {
        let (mut files, mut declared) = declared_from(src);
        let mut live: ExportInput = serde_json::from_str(live_json).expect("live json");
        declared.apply_schema_defaults();
        live.apply_schema_defaults();
        let report = diff::diff(&declared, &live);
        let outcome = reconcile_sources(&mut files, &live, &report);
        (files[0].body.to_string(), outcome)
    }

    const CAMPAIGN_SRC: &str = r#"provider "google_ads" {
  customer_id = "1234567890"
}

resource "google_ads_campaign_budget" "budget" {
  name            = "Budget"
  amount_micros   = 10000000
  delivery_method = "STANDARD"
}

# keep this comment
resource "google_ads_campaign" "summer_search" {
  name                     = "Summer 2026"
  status                   = "ENABLED"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.budget.id

  manual_cpc {
    enhanced_cpc_enabled = false
  }
}
"#;

    #[test]
    fn updates_drifted_scalars_in_place_preserving_structure() {
        let live = r#"{
          "customer_id": "1234567890",
          "campaign_budgets": [
            {"id":"111","name":"Budget","amount_micros":10000000,"delivery_method":"STANDARD"}
          ],
          "campaigns": [
            {"id":"555","name":"Summer 2026 — Sale","status":"PAUSED",
             "advertising_channel_type":"SEARCH","campaign_budget":"111",
             "managed_address":"main.google_ads_campaign.summer_search",
             "manual_cpc":{"enhanced_cpc_enabled":true}}
          ]
        }"#;
        let (out, outcome) = run(CAMPAIGN_SRC, live);

        assert!(out.contains(r#"name                     = "Summer 2026 — Sale""#), "{out}");
        assert!(out.contains(r#"status                   = "PAUSED""#), "{out}");
        assert!(out.contains("enhanced_cpc_enabled = true"), "{out}");
        // untouched structure
        assert!(out.contains("# keep this comment"), "{out}");
        assert!(out.contains(r#"resource "google_ads_campaign_budget" "budget""#), "{out}");
        assert!(out.contains("campaign_budget          = google_ads_campaign_budget.budget.id"), "{out}");

        assert_eq!(outcome.applied.len(), 1, "one resource updated");
        assert_eq!(outcome.changed_files, vec![0]);
        let (_, fields) = &outcome.applied[0];
        assert!(fields.contains(&"name".to_string()));
        assert!(fields.contains(&"status".to_string()));
        assert!(fields.contains(&"manual_cpc.enhanced_cpc_enabled".to_string()));
    }

    #[test]
    fn no_drift_leaves_source_byte_identical() {
        let live = r#"{
          "customer_id": "1234567890",
          "campaign_budgets": [
            {"id":"111","name":"Budget","amount_micros":10000000,"delivery_method":"STANDARD"}
          ],
          "campaigns": [
            {"id":"555","name":"Summer 2026","status":"ENABLED",
             "advertising_channel_type":"SEARCH","campaign_budget":"111",
             "managed_address":"main.google_ads_campaign.summer_search",
             "manual_cpc":{"enhanced_cpc_enabled":false}}
          ]
        }"#;
        let (out, outcome) = run(CAMPAIGN_SRC, live);
        assert_eq!(out, CAMPAIGN_SRC, "no-op reconcile must not rewrite source");
        assert!(outcome.applied.is_empty());
        assert!(outcome.changed_files.is_empty());
    }

    #[test]
    fn drift_on_field_absent_from_source_is_skipped_not_inserted() {
        // The campaign omits `status` (defaults ENABLED); live drifted to PAUSED.
        let src = r#"provider "google_ads" {
  customer_id = "1234567890"
}

resource "google_ads_campaign_budget" "budget" {
  name            = "Budget"
  amount_micros   = 10000000
  delivery_method = "STANDARD"
}

resource "google_ads_campaign" "summer_search" {
  name                     = "Summer 2026"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.budget.id
}
"#;
        let live = r#"{
          "customer_id": "1234567890",
          "campaign_budgets": [
            {"id":"111","name":"Budget","amount_micros":10000000,"delivery_method":"STANDARD"}
          ],
          "campaigns": [
            {"id":"555","name":"Summer 2026","status":"PAUSED",
             "advertising_channel_type":"SEARCH","campaign_budget":"111",
             "managed_address":"main.google_ads_campaign.summer_search"}
          ]
        }"#;
        let (out, outcome) = run(src, live);
        assert_eq!(out, src, "an absent attribute must not be inserted");
        assert!(outcome.applied.is_empty());
        assert!(
            outcome.skipped.iter().any(|s| s.contains("status") && s.contains("not set")),
            "expected a skip note for status, got {:?}",
            outcome.skipped
        );
    }
}

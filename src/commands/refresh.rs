use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::expr::Expression;
use hcl_edit::structure::{Block, Body, Structure};

use crate::api::diff::{Action, DiffReport};
use crate::api::import::import_program;
use crate::api::live_state::CacheMode;
use crate::api::{auth, client, diff, live_state};
use crate::commands::export::{
    canonicalize, filter_removed, fmt_string, prune_orphans, render_split, report_orphans,
    ExportInput, JsonFrequencyCap,
};
use crate::commands::vars;
use crate::diagnostics::Diag;
use crate::parser::{parse_str, ParsedFile};
use crate::program::{collect_bid_files, Program, Scope};
use crate::schema::{validate_files, ResourceRegistry};

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

pub fn run_reconcile(
    path: Option<&str>,
    check: bool,
    verbose: bool,
    cli_vars: &[String],
) -> ExitCode {
    let inputs = match vars::collect(cli_vars) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(2);
        }
    };
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

    // Load the same way validate/plan do: template files reached through a
    // `module` block are instance scopes, not standalone roots, so their
    // `var.*` references resolve against the caller's inputs.
    let loaded = Program::load(&paths, inputs);
    let program = loaded.program;
    let mut diags: Vec<Diag> = loaded.diagnostics;
    for scope in &program.scopes {
        diags.extend(validate_files(&scope.files, &scope.inputs));
    }
    if diags.iter().any(Diag::is_error) {
        for d in diags.into_iter().filter(|d| d.is_error()) {
            eprintln!("{:?}", miette::Report::new(d));
        }
        eprintln!("refresh: refusing to reconcile an invalid .bid (fix `validate` errors first).");
        return ExitCode::from(1);
    }

    // One editable copy per file on disk, plus the module instances whose
    // resources that copy backs — a template shared by N `module` instances
    // appears once here and carries N owners.
    let (mut files, owners) = editable_files(&program);

    let mut declared = match import_program(&program) {
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

    let outcome = reconcile_sources(&mut files, &owners, &live, &report);

    // Re-serialize the changed files now, then re-validate the mutated tree
    // before touching disk — a malformed edit must never be written.
    let rendered: Vec<(PathBuf, String)> = outcome
        .changed_files
        .iter()
        .map(|&i| (files[i].path.clone(), files[i].body.to_string()))
        .collect();
    if let Err(code) = revalidate_reconcile(&program.scopes, &rendered) {
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
    value: EditValue,
}

enum EditValue {
    Scalar(Expression),
    /// A whole repeated block set (`frequency_caps`), rendered from live and
    /// swapped in as a unit — the entries carry no identity of their own, so
    /// there is nothing to patch entry by entry.
    Blocks(Vec<Block>),
}

impl EditValue {
    /// Comparable form, used to decide whether every module instance behind one
    /// template drifted the same way.
    fn fingerprint(&self) -> String {
        match self {
            EditValue::Scalar(e) => e.to_string(),
            EditValue::Blocks(bs) => {
                let mut body = Body::new();
                for b in bs {
                    body.push(Structure::Block(b.clone()));
                }
                body.to_string()
            }
        }
    }
}

impl From<Expression> for EditValue {
    fn from(e: Expression) -> Self {
        EditValue::Scalar(e)
    }
}

impl From<Vec<Block>> for EditValue {
    fn from(b: Vec<Block>) -> Self {
        EditValue::Blocks(b)
    }
}

struct ReconcileOutcome {
    /// (source-block label, dotted field paths) actually written.
    applied: Vec<(String, Vec<String>)>,
    /// Human-readable lines for fields that drifted but couldn't be patched.
    skipped: Vec<String>,
    changed_files: Vec<usize>,
    create_count: usize,
    delete_count: usize,
}

/// One editable `ParsedFile` per file on disk, paired with the module instances
/// whose resources it backs. Root files own exactly themselves; a template
/// reached through `module` blocks owns one entry per instance, so an edit there
/// is only safe when every instance drifted the same way.
fn editable_files(program: &Program) -> (Vec<ParsedFile>, Vec<Vec<String>>) {
    let mut files: Vec<ParsedFile> = Vec::new();
    let mut owners: Vec<Vec<String>> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    for scope in &program.scopes {
        for f in &scope.files {
            match index.get(&canonical(&f.path)) {
                Some(&i) => owners[i].push(f.module.clone()),
                None => {
                    index.insert(canonical(&f.path), files.len());
                    owners.push(vec![f.module.clone()]);
                    files.push(f.clone());
                }
            }
        }
    }
    (files, owners)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Apply the diff's scalar `Update`s to the parsed source in place. Pure (no IO
/// / network) so it can be unit-tested with a synthetic live state.
fn reconcile_sources(
    files: &mut [ParsedFile],
    owners: &[Vec<String>],
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
    let mut visited: HashSet<String> = HashSet::new();
    for (i, f) in files.iter_mut().enumerate() {
        let modules = &owners[i];
        let path = f.path.clone();
        let mut file_changed = false;
        for mut s in f.body.iter_mut() {
            let Some(b) = s.as_block_mut() else { continue };
            if b.ident.as_str() != "resource" || b.labels.len() != 2 {
                continue;
            }
            let ty = b.labels[0].as_str().to_string();
            let name = b.labels[1].as_str().to_string();
            let addrs: Vec<String> = modules
                .iter()
                .map(|m| ResourceRegistry::qualified(m, &ty, &name))
                .collect();
            let mut touched = false;
            for addr in &addrs {
                if planned.contains_key(addr) {
                    visited.insert(addr.clone());
                    touched = true;
                }
            }
            if !touched {
                continue;
            }
            let label = block_label(&addrs, &path, &ty, &name);
            let (edits, conflicts) = agreed_edits(&planned, &addrs);
            for (field, why) in conflicts {
                skipped.push(format!("{label}: {field} ({why})"));
            }
            let mut done: Vec<String> = Vec::new();
            for e in edits {
                match apply_edit(&mut b.body, &e.path, &e.value) {
                    SetOutcome::Applied => {
                        done.push(e.path.join("."));
                        file_changed = true;
                    }
                    SetOutcome::Missing => skipped.push(format!(
                        "{label}: {} (not set in your file — add it by hand or run a bootstrap refresh)",
                        e.path.join(".")
                    )),
                    SetOutcome::NonLiteral => skipped.push(format!(
                        "{label}: {} (computed from a variable or reference — change it at the source)",
                        e.path.join(".")
                    )),
                }
            }
            if !done.is_empty() {
                applied.push((label, done));
            }
        }
        if file_changed {
            changed_files.push(i);
        }
    }

    // A planned address with no source block is a for_each instance — the
    // block only exists after expansion, so the drift can't be written back.
    let mut unmatched: Vec<&String> = planned
        .keys()
        .filter(|addr| !visited.contains(*addr))
        .collect();
    unmatched.sort();
    for addr in unmatched {
        for e in &planned[addr] {
            skipped.push(format!(
                "{addr}: {} (instance is generated by for_each — edit the source block or its entry list by hand)",
                e.path.join(".")
            ));
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

fn block_label(addrs: &[String], path: &Path, ty: &str, name: &str) -> String {
    match addrs {
        [only] => only.clone(),
        many => format!(
            "{ty}.{name} in {} ({} module instances)",
            path.display(),
            many.len()
        ),
    }
}

/// The edits every one of `addrs` agrees on. A template shared by several module
/// instances has one source block behind N live resources, so a field is only
/// writable when all N drifted to the same value; the rest come back as reasons
/// to report instead.
fn agreed_edits<'a>(
    planned: &'a HashMap<String, Vec<Edit>>,
    addrs: &[String],
) -> (Vec<&'a Edit>, Vec<(String, String)>) {
    let mut paths: Vec<Vec<&'static str>> = Vec::new();
    for addr in addrs {
        for e in planned.get(addr).map(Vec::as_slice).unwrap_or(&[]) {
            if !paths.contains(&e.path) {
                paths.push(e.path.clone());
            }
        }
    }

    let mut edits: Vec<&Edit> = Vec::new();
    let mut conflicts: Vec<(String, String)> = Vec::new();
    for p in paths {
        let found: Vec<&Edit> = addrs
            .iter()
            .filter_map(|a| planned.get(a)?.iter().find(|e| e.path == p))
            .collect();
        if found.len() != addrs.len() {
            conflicts.push((
                p.join("."),
                format!(
                    "only {} of {} module instances drifted — edit the template or its inputs by hand",
                    found.len(),
                    addrs.len()
                ),
            ));
            continue;
        }
        let first = found[0].value.fingerprint();
        if found.iter().any(|e| e.value.fingerprint() != first) {
            conflicts.push((
                p.join("."),
                "module instances drifted to different values — edit the template or its inputs by hand"
                    .to_string(),
            ));
            continue;
        }
        edits.push(found[0]);
    }
    (edits, conflicts)
}

enum SetOutcome {
    Applied,
    Missing,
    NonLiteral,
}

fn apply_edit(body: &mut Body, path: &[&str], value: &EditValue) -> SetOutcome {
    match value {
        EditValue::Scalar(v) => set_existing_scalar(body, path, v.clone()),
        EditValue::Blocks(blocks) => set_repeated_blocks(body, path, blocks),
    }
}

/// Swap every `path` block in `body` for `blocks`. The live set is one API
/// field, so it round-trips whole: the replacements land where the first old
/// block was, or at the end of the body when the file declared none — unlike a
/// scalar, a repeated block has a canonical rendering, so materializing one
/// isn't guesswork.
fn set_repeated_blocks(body: &mut Body, path: &[&str], blocks: &[Block]) -> SetOutcome {
    let [ident] = path else {
        return SetOutcome::Missing;
    };
    let at = body
        .iter()
        .position(|s| s.as_block().is_some_and(|b| b.ident.as_str() == *ident));
    body.remove_blocks(ident);
    for (n, block) in blocks.iter().enumerate() {
        match at {
            Some(i) => body.insert(i + n, Structure::Block(block.clone())),
            None => body.push(Structure::Block(block.clone())),
        }
    }
    SetOutcome::Applied
}

/// Set a scalar at `path` within `body`, descending one block per non-terminal
/// segment. Only overwrites literals that already exist — an absent attribute or
/// one computed from `var.`/`local.`/a reference is reported back, so the caller
/// can surface it rather than guess at formatting or erase the indirection.
fn set_existing_scalar(body: &mut Body, path: &[&str], value: Expression) -> SetOutcome {
    match path {
        [key] => match body.get_attribute_mut(key) {
            Some(mut attr) => {
                if !is_literal(&attr.value) {
                    return SetOutcome::NonLiteral;
                }
                *attr.value_mut() = value;
                SetOutcome::Applied
            }
            None => SetOutcome::Missing,
        },
        [head, rest @ ..] => match body.get_blocks_mut(head).next() {
            Some(block) => set_existing_scalar(&mut block.body, rest, value),
            None => SetOutcome::Missing,
        },
        [] => SetOutcome::Missing,
    }
}

fn is_literal(e: &Expression) -> bool {
    matches!(
        e,
        Expression::String(_) | Expression::Number(_) | Expression::Bool(_)
    )
}

fn s(v: &str) -> Expression {
    Expression::from(v.to_string())
}

/// Render the live frequency caps as source blocks in `export`'s shape (one
/// blank-line-separated block per cap, indented for a resource body). Built by
/// parsing rendered text rather than assembling structures, so the whitespace
/// decor hcl-edit needs comes from the parser. `None` when the round-trip
/// doesn't yield one block per cap — better to report the drift than to write a
/// half-rendered set.
fn frequency_cap_blocks(caps: &[JsonFrequencyCap]) -> Option<Vec<Block>> {
    let mut src = String::new();
    for f in caps {
        src.push_str("\n  frequency_caps {\n");
        src.push_str(&format!("    event_type = {}\n", fmt_string(&f.event_type)));
        src.push_str(&format!("    time_unit = {}\n", fmt_string(&f.time_unit)));
        src.push_str(&format!("    time_length = {}\n", f.time_length));
        src.push_str(&format!("    cap = {}\n", f.cap));
        if f.level_or_default() != crate::schema::DEFAULT_FREQUENCY_CAP_LEVEL {
            src.push_str(&format!("    level = {}\n", fmt_string(f.level_or_default())));
        }
        src.push_str("  }\n");
    }
    let blocks: Vec<Block> = src.parse::<Body>().ok()?.into_blocks().collect();
    (blocks.len() == caps.len()).then_some(blocks)
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
            e.push(Edit { path: $path, value: $val.into() })
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
                    // A strategy switch replaces one block with another, which
                    // the scalar-path edit model can't express.
                    "manual_cpc" | "manual_cpm" | "manual_cpv" | "target_cpm" | "target_cpv" => {
                        let live_block = c.bidding_strategy().unwrap_or("none");
                        skip.push(format!(
                            "{f} (live campaign bids with {live_block} — swap the block by hand)"
                        ));
                    }
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
                    "frequency_caps" => match frequency_cap_blocks(&c.frequency_caps) {
                        Some(blocks) => push!(vec!["frequency_caps"], blocks),
                        None => skip.push(
                            "frequency_caps (live caps did not render as valid blocks — edit them by hand)"
                                .to_string(),
                        ),
                    },
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
                    other => match crate::schema::AD_GROUP_BID_FIELDS
                        .iter()
                        .find(|(field, _)| *field == other)
                    {
                        Some((field, _)) => {
                            opt!(f, vec![*field], g.bid(field).map(Expression::from))
                        }
                        None => skip.push(other.to_string()),
                    },
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
        "custom_audience" => {
            let Some(a) = live.custom_audiences.iter().find(|x| x.id == live_id) else {
                return (e, fields.to_vec());
            };
            for f in fields {
                match f.as_str() {
                    "status" => opt!(f, vec!["status"], a.status.as_deref().map(s)),
                    "description" => {
                        opt!(f, vec!["description"], a.description.as_deref().map(s))
                    }
                    "members" => skip.push(
                        "members (repeated block — edit the blocks by hand or run a bootstrap refresh)"
                            .to_string(),
                    ),
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

/// Re-validate every scope against the mutated sources, refusing to write if the
/// edit introduced an error. The pre-check already refused on any error, so
/// anything reported here is a regression the reconcile caused.
fn revalidate_reconcile(scopes: &[Scope], rendered: &[(PathBuf, String)]) -> Result<(), ExitCode> {
    let edited: HashMap<PathBuf, &String> = rendered
        .iter()
        .map(|(p, content)| (canonical(p), content))
        .collect();

    for scope in scopes {
        let mut reparsed: Vec<ParsedFile> = Vec::with_capacity(scope.files.len());
        for f in &scope.files {
            let content = match edited.get(&canonical(&f.path)) {
                Some(c) => (*c).clone(),
                None => f.body.to_string(),
            };
            match parse_str(&f.path, &content) {
                Ok(mut pf) => {
                    pf.module = f.module.clone();
                    reparsed.push(pf);
                }
                Err(d) => {
                    eprintln!("{:?}", miette::Report::new(d));
                    eprintln!("refresh: the reconcile would produce an unparseable file; nothing was written.");
                    return Err(ExitCode::from(1));
                }
            }
        }
        let errors = validate_files(&reparsed, &scope.inputs);
        if errors.iter().any(Diag::is_error) {
            for d in errors.into_iter().filter(|d| d.is_error()) {
                eprintln!("{:?}", miette::Report::new(d));
            }
            eprintln!("refresh: the reconcile would break the project; nothing was written.");
            return Err(ExitCode::from(1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::api::import::import_files;
    use crate::schema::InputBindings;

    fn declared_from(src: &str) -> (Vec<ParsedFile>, ExportInput) {
        let pf = parse_str(&PathBuf::from("main.bid"), src).expect("parse");
        let files = vec![pf];
        let imported = import_files(&files, &InputBindings::default()).expect("import");
        (files, imported.input)
    }

    fn run(src: &str, live_json: &str) -> (String, ReconcileOutcome) {
        let (mut files, mut declared) = declared_from(src);
        let owners: Vec<Vec<String>> = files.iter().map(|f| vec![f.module.clone()]).collect();
        let mut live: ExportInput = serde_json::from_str(live_json).expect("live json");
        declared.apply_schema_defaults();
        live.apply_schema_defaults();
        let report = diff::diff(&declared, &live);
        let outcome = reconcile_sources(&mut files, &owners, &live, &report);
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

    const CAPS_SRC: &str = r#"provider "google_ads" {
  customer_id = "1234567890"
}

resource "google_ads_campaign_budget" "budget" {
  name          = "Budget"
  amount_micros = 10000000
}

# keep this comment
resource "google_ads_campaign" "brand_video" {
  name                     = "Brand video"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.budget.id

  frequency_caps {
    event_type  = "IMPRESSION"
    time_unit   = "DAY"
    time_length = 1
    cap         = 3
  }
}
"#;

    /// `CAPS_SRC` with the whole `frequency_caps` block (and the blank line
    /// that sets it off) gone — the shape a cleared set must reduce to.
    fn caps_src_without_blocks() -> String {
        let head = CAPS_SRC
            .split("\n\n  frequency_caps {")
            .next()
            .expect("split source");
        format!("{head}\n}}\n")
    }

    fn caps_live(caps_json: &str) -> String {
        format!(
            r#"{{
              "customer_id": "1234567890",
              "campaign_budgets": [
                {{"id":"111","name":"Budget","amount_micros":10000000,"delivery_method":"STANDARD"}}
              ],
              "campaigns": [
                {{"id":"555","name":"Brand video","status":"ENABLED",
                 "advertising_channel_type":"VIDEO","campaign_budget":"111",
                 "managed_address":"main.google_ads_campaign.brand_video",
                 "frequency_caps":[{caps_json}]}}
              ]
            }}"#
        )
    }

    #[test]
    fn drifted_frequency_caps_round_trip_into_the_blocks() {
        let live = caps_live(
            r#"{"event_type":"IMPRESSION","time_unit":"DAY","time_length":1,"cap":5},
               {"event_type":"VIDEO_VIEW","time_unit":"WEEK","time_length":2,"cap":1,"level":"AD_GROUP"}"#,
        );
        let (out, outcome) = run(CAPS_SRC, &live);

        assert!(
            out.contains(
                "\n  frequency_caps {\n    event_type = \"IMPRESSION\"\n    time_unit = \"DAY\"\n    \
                 time_length = 1\n    cap = 5\n  }\n"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "\n  frequency_caps {\n    event_type = \"VIDEO_VIEW\"\n    time_unit = \"WEEK\"\n    \
                 time_length = 2\n    cap = 1\n    level = \"AD_GROUP\"\n  }\n"
            ),
            "{out}"
        );
        assert_eq!(out.matches("frequency_caps {").count(), 2, "{out}");
        assert!(out.contains("# keep this comment"), "{out}");
        assert!(out.contains("campaign_budget          = google_ads_campaign_budget.budget.id"), "{out}");
        assert_eq!(outcome.changed_files, vec![0]);
        let (_, fields) = &outcome.applied[0];
        assert_eq!(fields, &vec!["frequency_caps".to_string()]);
        assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
    }

    #[test]
    fn caps_cleared_upstream_drop_the_blocks() {
        let (out, outcome) = run(CAPS_SRC, &caps_live(""));
        assert_eq!(out, caps_src_without_blocks(), "{out}");
        assert_eq!(outcome.changed_files, vec![0]);
    }

    #[test]
    fn undeclared_caps_are_left_alone() {
        // Issue #102: the field is unmanaged until the file declares a cap, so
        // reconcile has nothing to write back either.
        let src = caps_src_without_blocks();
        let live = caps_live(r#"{"event_type":"IMPRESSION","time_unit":"DAY","time_length":1,"cap":3}"#);
        let (out, outcome) = run(&src, &live);
        assert_eq!(out, src, "an unmanaged field must not be materialized");
        assert!(outcome.applied.is_empty());
        assert!(outcome.skipped.is_empty(), "{:?}", outcome.skipped);
    }

    // ---- module / template trees ------------------------------------------

    const TEMPLATE: &str = r#"variable "campaign_name" {
  type = string
}

resource "google_ads_campaign_budget" "budget" {
  name          = var.campaign_name
  amount_micros = 10000000
}

resource "google_ads_campaign" "c" {
  name                     = var.campaign_name
  status                   = "ENABLED"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.budget.id
}
"#;

    /// Load a whole tree the way `refresh --in-place` does, assert it validates,
    /// then reconcile it against a synthetic live state.
    fn run_tree(
        dir_name: &str,
        tree: &[(&str, &str)],
        live_json: &str,
    ) -> (HashMap<String, String>, ReconcileOutcome) {
        let root = std::env::temp_dir().join(dir_name);
        let _ = std::fs::remove_dir_all(&root);
        for (rel, content) in tree {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }

        let paths = collect_bid_files(&root).expect("collect");
        let loaded = Program::load(&paths, InputBindings::default());
        let program = loaded.program;
        let mut diags = loaded.diagnostics;
        for scope in &program.scopes {
            diags.extend(validate_files(&scope.files, &scope.inputs));
        }
        let errors: Vec<String> = diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect();
        assert!(errors.is_empty(), "tree must load cleanly, got {errors:?}");

        let (mut files, owners) = editable_files(&program);
        let mut declared = import_program(&program).expect("import").input;
        let mut live: ExportInput = serde_json::from_str(live_json).expect("live json");
        declared.apply_schema_defaults();
        live.apply_schema_defaults();
        let report = diff::diff(&declared, &live);
        let outcome = reconcile_sources(&mut files, &owners, &live, &report);

        let rendered = files
            .iter()
            .map(|f| {
                (
                    f.path.file_name().unwrap().to_string_lossy().to_string(),
                    f.body.to_string(),
                )
            })
            .collect();
        let _ = std::fs::remove_dir_all(&root);
        (rendered, outcome)
    }

    const FOR_EACH_MAIN: &str = r#"provider "google_ads" {
  customer_id = "1234567890"
}

module "m" {
  source = "./t.bid"
  for_each = {
    a = { campaign_name = "Alpha" }
    b = { campaign_name = "Beta" }
  }
}
"#;

    fn for_each_live(status_a: &str, status_b: &str) -> String {
        format!(
            r#"{{
              "customer_id": "1234567890",
              "campaign_budgets": [
                {{"id":"111","name":"Alpha","amount_micros":10000000,"delivery_method":"STANDARD"}},
                {{"id":"222","name":"Beta","amount_micros":10000000,"delivery_method":"STANDARD"}}
              ],
              "campaigns": [
                {{"id":"555","name":"Alpha","status":"{status_a}",
                 "advertising_channel_type":"SEARCH","campaign_budget":"111",
                 "managed_address":"m.a.google_ads_campaign.c"}},
                {{"id":"666","name":"Beta","status":"{status_b}",
                 "advertising_channel_type":"SEARCH","campaign_budget":"222",
                 "managed_address":"m.b.google_ads_campaign.c"}}
              ]
            }}"#
        )
    }

    #[test]
    fn template_reached_through_a_module_is_reconciled_not_rejected() {
        let (out, outcome) = run_tree(
            "bidsmith-refresh-module-agree",
            &[("main.bid", FOR_EACH_MAIN), ("t.bid", TEMPLATE)],
            &for_each_live("PAUSED", "PAUSED"),
        );
        assert!(
            out["t.bid"].contains(r#"status                   = "PAUSED""#),
            "{}",
            out["t.bid"]
        );
        assert_eq!(out["main.bid"], FOR_EACH_MAIN, "caller must be untouched");
        assert_eq!(outcome.applied.len(), 1, "{:?}", outcome.applied);
        let (label, fields) = &outcome.applied[0];
        assert!(label.contains("2 module instances"), "{label}");
        assert_eq!(fields, &vec!["status".to_string()]);
    }

    #[test]
    fn divergent_drift_across_module_instances_is_skipped() {
        let (out, outcome) = run_tree(
            "bidsmith-refresh-module-diverge",
            &[("main.bid", FOR_EACH_MAIN), ("t.bid", TEMPLATE)],
            &for_each_live("PAUSED", "ENABLED"),
        );
        assert_eq!(out["t.bid"], TEMPLATE, "shared template must not be rewritten");
        assert!(outcome.applied.is_empty(), "{:?}", outcome.applied);
        assert!(
            outcome
                .skipped
                .iter()
                .any(|s| s.contains("status") && s.contains("module instances")),
            "expected a divergence note, got {:?}",
            outcome.skipped
        );
    }

    #[test]
    fn var_driven_attribute_is_reported_not_overwritten() {
        let main = r#"provider "google_ads" {
  customer_id = "1234567890"
}

module "m" {
  source        = "./t.bid"
  campaign_name = "Alpha"
}
"#;
        let live = r#"{
          "customer_id": "1234567890",
          "campaign_budgets": [
            {"id":"111","name":"Alpha","amount_micros":10000000,"delivery_method":"STANDARD"}
          ],
          "campaigns": [
            {"id":"555","name":"Alpha Live","status":"ENABLED",
             "advertising_channel_type":"SEARCH","campaign_budget":"111",
             "managed_address":"m.google_ads_campaign.c"}
          ]
        }"#;
        let (out, outcome) = run_tree(
            "bidsmith-refresh-module-var",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
            live,
        );
        assert_eq!(out["t.bid"], TEMPLATE, "var indirection must survive");
        assert!(outcome.applied.is_empty(), "{:?}", outcome.applied);
        assert!(
            outcome
                .skipped
                .iter()
                .any(|s| s.contains("name") && s.contains("variable or reference")),
            "expected a var note, got {:?}",
            outcome.skipped
        );
    }
}

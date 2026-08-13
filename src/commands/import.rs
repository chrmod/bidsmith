use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::structure::Structure;

use crate::api::diff::DiffReport;
use crate::api::import::import_program;
use crate::api::live_state::CacheMode;
use crate::api::{auth, client, diff, live_state};
use crate::commands::export::{
    canonicalize, render_import, ExportInput, Imported, KnownAddresses, IMPORTABLE_TYPES,
};
use crate::commands::vars;
use crate::diagnostics::Diag;
use crate::parser::{parse_str, ParsedFile};
use crate::program::{collect_bid_files, Program, Scope};
use crate::schema::{validate_files, ResourceRegistry};

pub fn run(
    address: &str,
    resource: &str,
    path: &str,
    check: bool,
    refresh_state: bool,
    offline: bool,
    verbose: bool,
    cli_vars: &[String],
) -> ExitCode {
    let addr = match parse_addr(address) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("import: {e}");
            return ExitCode::from(2);
        }
    };
    if !IMPORTABLE_TYPES.contains(&addr.ty.as_str()) {
        eprintln!("import: {}", unsupported_type(&addr.ty));
        return ExitCode::from(2);
    }
    let inputs = match vars::collect(cli_vars) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("import: {e}");
            return ExitCode::from(2);
        }
    };

    let target = Path::new(path);
    if !target.exists() {
        eprintln!("import: no such file or directory: {path}");
        return ExitCode::from(1);
    }
    let paths = match collect_bid_files(target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("import: {e}");
            return ExitCode::from(1);
        }
    };
    if paths.is_empty() {
        eprintln!(
            "import: no .bid files under {path} — use `bidsmith refresh -d {path}` to \
             create them from live state first."
        );
        return ExitCode::from(1);
    }

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
        eprintln!("import: refusing to write into an invalid .bid (fix `validate` errors first).");
        return ExitCode::from(1);
    }

    // Only a root file can receive an import: a `module` source is a template
    // instantiated N times, and one live resource declared there would be
    // claimed by every instance at once.
    let root_files: &[ParsedFile] = match program.scopes.first() {
        Some(s) => &s.files,
        None => &[],
    };
    if root_files.is_empty() {
        eprintln!(
            "import: every .bid file under {path} is a `module` source. A template stands \
             behind N instances, so a live resource declared there would be claimed N \
             times — point --path at the root files that call the modules."
        );
        return ExitCode::from(1);
    }
    let file_index = match target_file(&addr, root_files) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("import: {e}");
            return ExitCode::from(1);
        }
    };
    let module = root_files[file_index].module.clone();

    let (registry, _) = ResourceRegistry::build(root_files);
    if registry.declared(&module, &addr.ty, &addr.name) {
        eprintln!(
            "import: {}.{}.{} is already declared — pick a free address, or `bidsmith mv` \
             the existing one out of the way.",
            module, addr.ty, addr.name
        );
        return ExitCode::from(1);
    }

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
            "import: no customer id — set it in the provider block, bidsmith.toml, \
             GOOGLE_ADS_CUSTOMER_ID, or run `bidsmith auth login`."
        );
        return ExitCode::from(1);
    }

    let mut live = if offline {
        match crate::commands::plan::load_live_from_cache("import", &mut declared) {
            Ok((state, _)) => state,
            Err(code) => return code,
        }
    } else {
        let client = match client::Client::for_target(
            &declared.customer_id,
            declared.login_customer_id.as_deref(),
        ) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("import: {e}");
                return ExitCode::from(1);
            }
        };
        let token = match auth::get_access_token() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("import: {e}");
                return ExitCode::from(1);
            }
        };
        if verbose {
            eprintln!(
                "import: reading customers/{} via /{}/googleAds:searchStream",
                client.customer_id,
                client::api_version(),
            );
        }
        let mode = match refresh_state {
            true => CacheMode::RefreshWrite,
            false => CacheMode::ReadWrite,
        };
        match live_state::fetch_with_cache(&client, &token.token, mode, "import") {
            Ok(o) => o.state,
            Err(e) => {
                eprintln!("import: live-state fetch failed: {e}");
                return ExitCode::from(1);
            }
        }
    };

    declared.apply_schema_defaults();
    live.apply_schema_defaults();
    let report = diff::diff(&declared, &live);

    let taken = declared_names(&root_files[file_index]);
    let plan = match plan_import(&addr, resource, &live, &report, taken) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("import: {e}");
            return ExitCode::from(1);
        }
    };

    let file_path = root_files[file_index].path.clone();
    let existing = match std::fs::read_to_string(&file_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("import: failed to read {}: {e}", file_path.display());
            return ExitCode::from(1);
        }
    };
    let snippet = canonicalize(&plan.text);
    let updated = append_blocks(&existing, &snippet);

    if let Err(code) = revalidate(&program.scopes, &file_path, &updated) {
        return code;
    }

    if check {
        println!("{}", snippet.trim_end());
        eprintln!(
            "import: would add {} to {} (--check, nothing written).",
            describe(&plan.added),
            file_path.display(),
        );
        return ExitCode::SUCCESS;
    }
    if let Err(e) = std::fs::write(&file_path, &updated) {
        eprintln!("import: failed to write {}: {e}", file_path.display());
        return ExitCode::from(1);
    }
    eprintln!(
        "import: added {} to {}.",
        describe(&plan.added),
        file_path.display(),
    );
    eprintln!("import: run `bidsmith plan` — the adopted resource should show as unchanged.");
    ExitCode::SUCCESS
}

struct Addr {
    module: Option<String>,
    ty: String,
    name: String,
}

fn parse_addr(s: &str) -> Result<Addr, String> {
    let parts: Vec<&str> = s.split('.').collect();
    match parts.as_slice() {
        [ty, name] => Ok(Addr {
            module: None,
            ty: (*ty).to_string(),
            name: (*name).to_string(),
        }),
        [m, ty, name] => Ok(Addr {
            module: Some((*m).to_string()),
            ty: (*ty).to_string(),
            name: (*name).to_string(),
        }),
        _ => Err(format!(
            "'{s}' is not a resource address; expected '<type>.<name>' \
             (optionally '<module>.<type>.<name>')"
        )),
    }
}

fn unsupported_type(ty: &str) -> String {
    if crate::schema::resource_schema(ty).is_some() {
        return format!(
            "'{ty}' is not one of the types import adopts. Campaigns, ad groups, and ads \
             carry a bidsmith:address label, so `apply` adopts a matching live one on its \
             own; `bidsmith refresh -d <dir>` writes the whole account out as .bid. \
             import handles: {}",
            IMPORTABLE_TYPES.join(", ")
        );
    }
    format!(
        "'{ty}' is not a bidsmith resource type. import handles: {}",
        IMPORTABLE_TYPES.join(", ")
    )
}

/// The file an imported block is appended to. The module segment of the address
/// names it — that is the files-as-modules rule read backwards.
fn target_file(addr: &Addr, root_files: &[ParsedFile]) -> Result<usize, String> {
    let mut modules: Vec<&str> = root_files.iter().map(|f| f.module.as_str()).collect();
    modules.sort_unstable();
    modules.dedup();
    match &addr.module {
        Some(m) => root_files
            .iter()
            .position(|f| &f.module == m)
            .ok_or_else(|| {
                format!(
                    "no root .bid file is module '{m}'; available: {}",
                    modules.join(", ")
                )
            }),
        None if root_files.len() == 1 => Ok(0),
        None => Err(format!(
            "'{}.{}' does not say which file to write into — qualify it as \
             '<module>.{}.{}' (one of: {})",
            addr.ty,
            addr.name,
            addr.ty,
            addr.name,
            modules.join(", ")
        )),
    }
}

fn declared_names(file: &ParsedFile) -> HashSet<(String, String)> {
    let mut out = HashSet::new();
    for s in file.body.iter() {
        let Structure::Block(b) = s else { continue };
        if b.ident.as_str() != "resource" || b.labels.len() != 2 {
            continue;
        }
        out.insert((b.labels[0].as_str().to_string(), b.labels[1].as_str().to_string()));
    }
    out
}

/// Work out the block(s) to append. Pure: the live snapshot and the plan's
/// declared/live matching are the only inputs, so the resolution rules are
/// unit-testable without an account.
fn plan_import(
    addr: &Addr,
    resource: &str,
    live: &ExportInput,
    report: &DiffReport,
    mut taken: HashSet<(String, String)>,
) -> Result<Imported, String> {
    let id = live_id(&addr.ty, resource)?;
    let kind = addr.ty.trim_start_matches("google_ads_");
    taken.insert((addr.ty.clone(), addr.name.clone()));

    let mut known = KnownAddresses { taken, ..Default::default() };
    for d in &report.diffs {
        let Some(live_id) = d.action.live_id() else {
            continue;
        };
        if d.kind == kind && live_id == id {
            return Err(format!(
                "{resource} is already managed by {} — plan already matches it, \
                 so there is nothing to adopt.",
                d.address
            ));
        }
        let reference = reference_form(&d.address);
        let bucket = match d.kind {
            "campaign" => &mut known.campaigns,
            "ad_group" => &mut known.ad_groups,
            "conversion_action" => &mut known.conversion_actions,
            "custom_audience" => &mut known.custom_audiences,
            "sitelink_asset" | "callout_asset" | "structured_snippet_asset" | "call_asset"
            | "youtube_video_asset" => &mut known.assets,
            _ => continue,
        };
        bucket.insert(live_id.to_string(), reference);
    }

    render_import(live, &addr.ty, &id, &addr.name, &known)
}

/// The id half of a Google Ads resource name, checked against the collection the
/// address's type lives in — `customers/1/assets/2` is not a campaign criterion,
/// and saying so beats "no live resource with that id".
fn live_id(ty: &str, resource: &str) -> Result<String, String> {
    let expected = collection_for(ty);
    let Some((head, id)) = resource.rsplit_once('/') else {
        return Ok(resource.to_string());
    };
    let collection = head.rsplit('/').next().unwrap_or("");
    if collection != expected {
        return Err(format!(
            "'{resource}' names something under '{collection}', but {ty} lives under \
             '{expected}' — pass the {expected} resource name, or its bare id"
        ));
    }
    Ok(id.to_string())
}

fn collection_for(ty: &str) -> &'static str {
    match ty {
        "google_ads_customer_asset" => "customerAssets",
        "google_ads_campaign_asset" => "campaignAssets",
        "google_ads_ad_group_asset" => "adGroupAssets",
        "google_ads_campaign_criterion" => "campaignCriteria",
        "google_ads_ad_group_criterion" => "adGroupCriteria",
        _ => "assets",
    }
}

/// The reference spelling of a declared address. Source writes references as
/// `<type>.<name>` and resolves them same-module-first, so the module segment an
/// address carries is dropped here.
fn reference_form(address: &str) -> String {
    let parts: Vec<&str> = address.split('.').collect();
    match parts.iter().position(|p| p.starts_with("google_ads_")) {
        Some(i) => parts[i..].join("."),
        None => address.to_string(),
    }
}

fn append_blocks(existing: &str, blocks: &str) -> String {
    let mut out = existing.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(blocks.trim_end());
    out.push('\n');
    out
}

fn describe(added: &[String]) -> String {
    match added {
        [one] => one.clone(),
        many => format!("{} ({} blocks)", many.join(", "), many.len()),
    }
}

fn revalidate(scopes: &[Scope], edited: &Path, content: &str) -> Result<(), ExitCode> {
    let edited = canonical(edited);
    for scope in scopes {
        let mut reparsed: Vec<ParsedFile> = Vec::with_capacity(scope.files.len());
        for f in &scope.files {
            let src = match canonical(&f.path) == edited {
                true => content.to_string(),
                false => f.body.to_string(),
            };
            match parse_str(&f.path, &src) {
                Ok(mut pf) => {
                    pf.module = f.module.clone();
                    pf.inherited_defaults = f.inherited_defaults.clone();
                    reparsed.push(pf);
                }
                Err(d) => {
                    eprintln!("{:?}", miette::Report::new(d));
                    eprintln!("import: the import would produce an unparseable file; nothing was written.");
                    return Err(ExitCode::from(1));
                }
            }
        }
        let errors = validate_files(&reparsed, &scope.inputs);
        if errors.iter().any(Diag::is_error) {
            for d in errors.into_iter().filter(|d| d.is_error()) {
                eprintln!("{:?}", miette::Report::new(d));
            }
            eprintln!("import: the import would break the project; nothing was written.");
            return Err(ExitCode::from(1));
        }
    }
    Ok(())
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::import::import_files;
    use crate::schema::InputBindings;

    fn plan(src: &str, live_json: &str, address: &str, resource: &str) -> Result<Imported, String> {
        let file = parse_str(&PathBuf::from("account.bid"), src).expect("parse");
        let files = vec![file];
        let mut declared = import_files(&files, &InputBindings::default())
            .expect("import")
            .input;
        let mut live: ExportInput = serde_json::from_str(live_json).expect("live json");
        declared.apply_schema_defaults();
        live.apply_schema_defaults();
        let report = diff::diff(&declared, &live);
        let addr = parse_addr(address).expect("address");
        plan_import(&addr, resource, &live, &report, declared_names(&files[0]))
    }

    const ACCOUNT: &str = r#"provider "google_ads" {
  customer_id = "9"
}

resource "google_ads_sitelink_asset" "shop" {
  link_text  = "Shop"
  final_urls = ["https://example.com/shop"]
}
"#;

    const LIVE: &str = r#"{
        "customer_id": "9",
        "sitelink_assets": [
            {"id": "4001", "link_text": "Shop", "final_urls": ["https://example.com/shop"]},
            {"id": "4002", "link_text": "Support", "final_urls": ["https://example.com/help"]}
        ],
        "customer_assets": [
            {"id": "4001~SITELINK", "asset": "4001", "field_type": "SITELINK", "status": "ENABLED"},
            {"id": "4002~SITELINK", "asset": "4002", "field_type": "SITELINK", "status": "ENABLED"}
        ]
    }"#;

    #[test]
    fn account_level_link_references_the_asset_the_file_already_declares() {
        let out = plan(
            ACCOUNT,
            LIVE,
            "google_ads_customer_asset.shop_link",
            "customers/9/customerAssets/4001~SITELINK",
        )
        .expect("plan");
        assert_eq!(out.added, vec!["google_ads_customer_asset.shop_link"]);
        assert!(
            out.text.contains("asset = google_ads_sitelink_asset.shop.id"),
            "the link should point at the declared asset, not re-declare it:\n{}",
            out.text
        );
    }

    #[test]
    fn an_undeclared_asset_is_imported_alongside_its_link() {
        let out = plan(
            ACCOUNT,
            LIVE,
            "google_ads_customer_asset.support_link",
            "customers/9/customerAssets/4002~SITELINK",
        )
        .expect("plan");
        assert_eq!(
            out.added,
            vec![
                "google_ads_sitelink_asset.sitelink_support",
                "google_ads_customer_asset.support_link",
            ],
            "the asset a link needs has to come with it",
        );
        assert!(out.text.contains(r#"link_text = "Support""#), "{}", out.text);
        assert!(
            out.text.contains("asset = google_ads_sitelink_asset.sitelink_support.id"),
            "{}",
            out.text
        );
    }

    #[test]
    fn a_dependency_name_never_collides_with_one_already_in_the_file() {
        let src = format!(
            "{ACCOUNT}\nresource \"google_ads_sitelink_asset\" \"sitelink_support\" {{\n  \
             link_text  = \"Other\"\n  final_urls = [\"https://example.com/other\"]\n}}\n"
        );
        let out = plan(
            &src,
            LIVE,
            "google_ads_customer_asset.support_link",
            "customers/9/customerAssets/4002~SITELINK",
        )
        .expect("plan");
        assert_eq!(out.added[0], "google_ads_sitelink_asset.sitelink_support_2");
    }

    #[test]
    fn an_already_managed_resource_is_refused() {
        let src = format!(
            "{ACCOUNT}\nresource \"google_ads_customer_asset\" \"shop_link\" {{\n  \
             asset = google_ads_sitelink_asset.shop.id\n}}\n"
        );
        let err = plan(
            &src,
            LIVE,
            "google_ads_customer_asset.dup",
            "customers/9/customerAssets/4001~SITELINK",
        )
        .expect_err("already managed");
        assert!(err.contains("already managed by"), "{err}");
    }

    #[test]
    fn a_resource_name_from_the_wrong_collection_is_named_as_such() {
        let err = plan(
            ACCOUNT,
            LIVE,
            "google_ads_customer_asset.x",
            "customers/9/assets/4001",
        )
        .expect_err("wrong collection");
        assert!(err.contains("customerAssets"), "{err}");
    }

    #[test]
    fn a_bare_id_is_accepted() {
        let out = plan(ACCOUNT, LIVE, "google_ads_sitelink_asset.support", "4002")
            .expect("plan");
        assert!(out.text.contains(r#"link_text = "Support""#), "{}", out.text);
    }

    #[test]
    fn a_missing_live_resource_says_so() {
        let err = plan(
            ACCOUNT,
            LIVE,
            "google_ads_sitelink_asset.nope",
            "customers/9/assets/9999",
        )
        .expect_err("missing");
        assert!(err.contains("9999"), "{err}");
    }

    #[test]
    fn a_criterion_needs_its_parent_declared() {
        let live = r#"{
            "customer_id": "9",
            "campaign_criteria": [
                {"id": "2001~333", "campaign": "2001", "negative": true,
                 "target": {"keyword": {"text": "free", "match_type": "BROAD"}}}
            ]
        }"#;
        let err = plan(
            ACCOUNT,
            live,
            "google_ads_campaign_criterion.no_free",
            "customers/9/campaignCriteria/2001~333",
        )
        .expect_err("undeclared parent");
        assert!(err.contains("not declared"), "{err}");
    }

    #[test]
    fn a_module_qualified_address_is_written_as_a_bare_reference() {
        assert_eq!(
            reference_form("account.google_ads_sitelink_asset.shop"),
            "google_ads_sitelink_asset.shop"
        );
        assert_eq!(
            reference_form("google_ads_sitelink_asset.shop"),
            "google_ads_sitelink_asset.shop"
        );
    }

    #[test]
    fn appending_keeps_the_file_that_was_there() {
        let out = append_blocks(ACCOUNT, "resource \"google_ads_callout_asset\" \"x\" {\n  text = \"Free delivery\"\n}\n\n");
        assert!(out.starts_with("provider \"google_ads\""), "{out}");
        assert!(out.contains("\"shop\""), "the existing resource survives:\n{out}");
        assert!(out.trim_end().ends_with('}'), "{out}");
        assert!(!out.contains("\n\n\n"), "no run of blank lines:\n{out}");
    }
}

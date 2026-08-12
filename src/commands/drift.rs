//! `bidsmith drift` — what `plan` is not looking at.
//!
//! A field bidsmith does not model is not merely unmanaged, it is *undiffed*:
//! `plan` counts the resource as `unchanged`, which reads as "the repo matches
//! live" when the accurate statement is "the repo matches live on the fields
//! bidsmith models". Every schema gap is therefore a silent hole rather than a
//! visible one, and the failure is directional — it always reports more
//! agreement than exists (issue #111).
//!
//! This verb makes the gap visible. It asks the API which fields each resource
//! has and which of those a mutate could write (`api::catalog`), subtracts what
//! the live-state fetch actually selects, and then reads the remainder off the
//! account so an unmodelled field that is merely *possible* reads differently
//! from one that is *set on a campaign you are running*.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::process::ExitCode;

use serde_json::Value;

use crate::api::catalog::{self, FieldCatalog};
use crate::api::client::Client;
use crate::api::{diff, live_state};
use crate::commands::plan;
use crate::commands::vars;

/// How the report is rendered. Mirrors `plan`'s two shapes so a CI job can post
/// the same audit as a pull-request comment.
#[derive(Copy, Clone, PartialEq)]
pub enum Format {
    Text,
    Markdown,
}

/// How many unmodelled fields go into one `SELECT`. Large enough to keep the
/// call count down, small enough that one field the API refuses only costs a
/// short bisect to isolate.
const FIELDS_PER_QUERY: usize = 40;
/// Longest value the report prints before eliding, matching a plan row.
const MAX_SHOWN_VALUE: usize = 60;

pub fn run(
    path: &str,
    refresh_state: bool,
    refresh_catalog: bool,
    show_all: bool,
    format: Format,
    detailed_exitcode: bool,
    verbose: bool,
    cli_vars: &[String],
) -> ExitCode {
    let inputs = match vars::collect(cli_vars) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("drift: {e}");
            return ExitCode::from(2);
        }
    };

    let prepared = match plan::prepare(path, "drift", refresh_state, /* offline */ false, &inputs)
    {
        Ok(Some(p)) => p,
        Ok(None) => return ExitCode::SUCCESS,
        Err(code) => return code,
    };

    let Some((client, token)) = prepared.client.as_ref().zip(prepared.token.as_ref()) else {
        eprintln!("drift: needs a live account — it reports the fields plan never fetches.");
        return ExitCode::from(1);
    };

    let managed = managed_resources(&prepared.report);
    if managed.is_empty() {
        println!("drift: no live resources matched, so there is nothing to audit yet.");
        return ExitCode::SUCCESS;
    }

    let resources: Vec<&str> = managed.keys().map(|r| r.as_str()).collect();
    let mode = if refresh_catalog {
        catalog::CacheMode::RefreshWrite
    } else {
        catalog::CacheMode::ReadWrite
    };
    let catalog = match catalog::load(client, &token.token, &resources, mode, "drift") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("drift: could not read the API field catalog: {e}");
            return ExitCode::from(1);
        }
    };

    let coverage = coverage(&catalog, &managed, &live_state::selected_fields());
    let report = scan(client, &token.token, &coverage, &managed, verbose);

    print!(
        "{}",
        match format {
            Format::Text => render_text(&coverage, &report, show_all),
            Format::Markdown => render_markdown(&coverage, &report, show_all),
        }
    );

    if detailed_exitcode && report.sightings.values().any(|s| !s.is_empty()) {
        ExitCode::from(2)
    } else {
        ExitCode::SUCCESS
    }
}

/// The live resources `plan` makes a claim about, keyed by API resource:
/// live id -> the bidsmith address that claims it. A resource `plan` would
/// create does not exist yet, so it has nothing to audit.
fn managed_resources(report: &diff::DiffReport) -> BTreeMap<String, HashMap<String, String>> {
    let mut out: BTreeMap<String, HashMap<String, String>> = BTreeMap::new();
    for d in &report.diffs {
        let Some(live_id) = d.action.live_id() else {
            continue;
        };
        let Some(resource) = api_resource(d.kind) else {
            continue;
        };
        out.entry(resource.to_string())
            .or_default()
            .insert(live_id.to_string(), d.address.clone());
    }
    out
}

/// The API resource a diff row's `kind` lives on. The asset subtypes are five
/// bidsmith kinds over one API resource, so they audit as one.
fn api_resource(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "campaign_budget" => "campaign_budget",
        "campaign" => "campaign",
        "ad_group" => "ad_group",
        "ad_group_ad" => "ad_group_ad",
        "ad_group_criterion" => "ad_group_criterion",
        "campaign_criterion" => "campaign_criterion",
        "conversion_action" => "conversion_action",
        "custom_audience" => "custom_audience",
        "shared_set" => "shared_set",
        "shared_criterion" => "shared_criterion",
        "campaign_shared_set" => "campaign_shared_set",
        "customer_asset" => "customer_asset",
        "campaign_asset" => "campaign_asset",
        "ad_group_asset" => "ad_group_asset",
        "call_asset" | "sitelink_asset" | "callout_asset" | "structured_snippet_asset"
        | "youtube_video_asset" => "asset",
        _ => return None,
    })
}

/// Per-resource split of the settable surface into what bidsmith compares and
/// what it does not.
pub struct Coverage {
    pub by_resource: BTreeMap<String, ResourceCoverage>,
}

pub struct ResourceCoverage {
    pub settable: usize,
    pub modelled: usize,
    /// Settable fields no `SELECT` in the live-state fetch names.
    pub unmodelled: Vec<String>,
}

fn coverage(
    catalog: &FieldCatalog,
    managed: &BTreeMap<String, HashMap<String, String>>,
    selected: &BTreeSet<String>,
) -> Coverage {
    let mut by_resource = BTreeMap::new();
    for resource in managed.keys() {
        let settable = catalog.settable_fields(resource);
        let (modelled, unmodelled): (Vec<&str>, Vec<&str>) = settable
            .iter()
            .partition(|field| is_compared(field, selected));
        by_resource.insert(
            resource.clone(),
            ResourceCoverage {
                settable: settable.len(),
                modelled: modelled.len(),
                unmodelled: unmodelled.into_iter().map(str::to_string).collect(),
            },
        );
    }
    Coverage { by_resource }
}

/// Whether `plan` compares `field`. A `SELECT` may name a message whole or name
/// its leaves, and either form puts the value in front of the diff — so a
/// selected path that contains `field`, or that `field` contains, counts.
fn is_compared(field: &str, selected: &BTreeSet<String>) -> bool {
    selected.iter().any(|s| {
        s == field
            || field.strip_prefix(s.as_str()).is_some_and(|r| r.starts_with('.'))
            || s.strip_prefix(field).is_some_and(|r| r.starts_with('.'))
    })
}

/// One unmodelled field found carrying a value on the account.
pub struct Sighting {
    pub field: String,
    pub resources: usize,
    pub example_address: String,
    pub example_value: String,
}

#[derive(Default)]
pub struct ScanReport {
    /// Resource -> the unmodelled fields that are actually set, most-common first.
    pub sightings: BTreeMap<String, Vec<Sighting>>,
    /// Fields the catalog offered but a `SELECT` refused, so they were not read.
    pub unreadable: Vec<String>,
}

fn scan(
    client: &Client,
    access_token: &str,
    coverage: &Coverage,
    managed: &BTreeMap<String, HashMap<String, String>>,
    verbose: bool,
) -> ScanReport {
    let mut report = ScanReport::default();
    for (resource, cov) in &coverage.by_resource {
        let empty = HashMap::new();
        let owners = managed.get(resource).unwrap_or(&empty);
        let fields: Vec<&str> = cov.unmodelled.iter().map(String::as_str).collect();
        let mut found: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
        if !fields.is_empty() {
            eprintln!(
                "drift: reading {} unmodelled field(s) on {resource}...",
                fields.len(),
            );
            read_chunk(
                client,
                access_token,
                resource,
                &fields,
                owners,
                verbose,
                &mut found,
                &mut report.unreadable,
            );
        }
        let mut sightings: Vec<Sighting> = found
            .into_iter()
            .map(|(field, (resources, example_address, example_value))| Sighting {
                field,
                resources,
                example_address,
                example_value,
            })
            .collect();
        sightings.sort_by(|a, b| b.resources.cmp(&a.resources).then(a.field.cmp(&b.field)));
        report.sightings.insert(resource.clone(), sightings);
    }
    report.unreadable.sort();
    report
}

/// Read one batch of fields, halving on rejection so a single field the API
/// will not put in a `SELECT` costs a short bisect instead of the whole batch.
#[allow(clippy::too_many_arguments)]
fn read_chunk(
    client: &Client,
    access_token: &str,
    resource: &str,
    fields: &[&str],
    owners: &HashMap<String, String>,
    verbose: bool,
    found: &mut BTreeMap<String, (usize, String, String)>,
    unreadable: &mut Vec<String>,
) {
    if fields.is_empty() {
        return;
    }
    if fields.len() > FIELDS_PER_QUERY {
        let (head, tail) = fields.split_at(FIELDS_PER_QUERY);
        read_chunk(client, access_token, resource, head, owners, verbose, found, unreadable);
        read_chunk(client, access_token, resource, tail, owners, verbose, found, unreadable);
        return;
    }

    let query = format!(
        "SELECT {}.resource_name, {} FROM {resource}",
        resource,
        fields.join(", "),
    );
    if verbose {
        eprintln!("drift: {query}");
    }
    let response = match client.search_stream(access_token, &query) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("drift: {resource}: {e}");
            unreadable.extend(fields.iter().map(|f| (*f).to_string()));
            return;
        }
    };
    if response.status < 200 || response.status >= 300 {
        // Only a rejected *query* is worth narrowing down. Bisecting a quota or
        // auth failure would turn one dead batch into hundreds of dead calls.
        if response.status != 400 {
            eprintln!(
                "drift: {resource}: HTTP {} — skipping {} field(s) of this batch.",
                response.status,
                fields.len(),
            );
            unreadable.extend(fields.iter().map(|f| (*f).to_string()));
            return;
        }
        if fields.len() == 1 {
            // Selectable per the catalog but not in this combination — record
            // it rather than dropping it, so the coverage number stays honest.
            unreadable.push(fields[0].to_string());
            return;
        }
        let mid = fields.len() / 2;
        read_chunk(
            client, access_token, resource, &fields[..mid], owners, verbose, found, unreadable,
        );
        read_chunk(
            client, access_token, resource, &fields[mid..], owners, verbose, found, unreadable,
        );
        return;
    }

    let row_key = lower_camel(resource);
    for row in rows(&response.body) {
        let Some(body) = row.get(&row_key) else {
            continue;
        };
        let Some(id) = body
            .get("resourceName")
            .and_then(Value::as_str)
            .and_then(|rn| rn.rsplit('/').next())
        else {
            continue;
        };
        let Some(address) = owners.get(id) else {
            continue;
        };
        for field in fields {
            let Some(value) = lookup(body, resource, field) else {
                continue;
            };
            // A REST response omits every field left at its default, so a key
            // that is present is a value someone set.
            if is_unreported(value) {
                continue;
            }
            let entry = found.entry((*field).to_string()).or_insert_with(|| {
                (0, address.clone(), render_value(value))
            });
            entry.0 += 1;
        }
    }
}

fn rows(body: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    let batches: Vec<&Value> = match body.as_array() {
        Some(arr) => arr.iter().collect(),
        None => vec![body],
    };
    for batch in batches {
        if let Some(results) = batch.get("results").and_then(Value::as_array) {
            out.extend(results.iter());
        }
    }
    out
}

/// Walk `campaign.geo_target_type_setting.positive_geo_target_type` into the
/// row body, which is keyed in lowerCamelCase and has the resource stripped.
fn lookup<'a>(body: &'a Value, resource: &str, field: &str) -> Option<&'a Value> {
    let rest = field.strip_prefix(resource)?.strip_prefix('.')?;
    let mut cur = body;
    for segment in rest.split('.') {
        cur = cur.get(lower_camel(segment))?;
    }
    Some(cur)
}

/// Values that are present but say nothing: Google reports an enum it has no
/// value for as `UNSPECIFIED` / `UNKNOWN`, which is a report, not a setting.
fn is_unreported(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s == "UNSPECIFIED" || s == "UNKNOWN",
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn render_value(value: &Value) -> String {
    let shown = match value {
        Value::String(s) => format!("\"{s}\""),
        other => other.to_string(),
    };
    match shown.char_indices().nth(MAX_SHOWN_VALUE) {
        Some((i, _)) => format!("{}…", &shown[..i]),
        None => shown,
    }
}

/// `ad_group_ad` -> `adGroupAd`, the casing a REST row uses.
fn lower_camel(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = false;
    for ch in snake.chars() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            out.push(ch.to_ascii_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

struct Totals {
    settable: usize,
    modelled: usize,
    set_fields: usize,
}

fn totals(coverage: &Coverage, report: &ScanReport) -> Totals {
    Totals {
        settable: coverage.by_resource.values().map(|c| c.settable).sum(),
        modelled: coverage.by_resource.values().map(|c| c.modelled).sum(),
        set_fields: report.sightings.values().map(Vec::len).sum(),
    }
}

/// The sentence the whole verb exists to be able to say.
fn guarantee_line(t: &Totals) -> String {
    format!(
        "`unchanged` in a plan means unchanged on the {} field(s) bidsmith models, \
         out of {} the API would let a .bid set.",
        t.modelled, t.settable,
    )
}

fn verdict_line(t: &Totals) -> String {
    if t.set_fields == 0 {
        "No unmodelled field carries a value on the resources bidsmith manages.".to_string()
    } else {
        format!(
            "{} unmodelled field(s) carry a value bidsmith never compares — a plan \
             calling these resources unchanged is not speaking about them.",
            t.set_fields,
        )
    }
}

fn render_text(coverage: &Coverage, report: &ScanReport, show_all: bool) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (resource, cov) in &coverage.by_resource {
        if cov.settable == 0 {
            // Zero is not "fully modelled" — it is the catalog telling us
            // nothing, which is the one thing this report must not render as
            // reassurance.
            let _ = writeln!(
                out,
                "{resource} — not audited: the API field catalog returned no settable fields",
            );
            let _ = writeln!(out);
            continue;
        }
        let _ = writeln!(
            out,
            "{resource} — {} of {} settable field(s) modelled",
            cov.modelled, cov.settable,
        );
        let sightings = report.sightings.get(resource).map(Vec::as_slice).unwrap_or(&[]);
        if sightings.is_empty() {
            let _ = writeln!(out, "  nothing set outside the modelled fields.");
        } else {
            let _ = writeln!(out, "  set on managed resources, never compared:");
            let width = sightings.iter().map(|s| s.field.len()).max().unwrap_or(0);
            for s in sightings {
                let _ = writeln!(
                    out,
                    "    {field:<width$}  {count} resource(s)  e.g. {addr} = {value}",
                    field = s.field,
                    width = width,
                    count = s.resources,
                    addr = s.example_address,
                    value = s.example_value,
                );
            }
        }
        if show_all {
            let unset: Vec<&String> = cov
                .unmodelled
                .iter()
                .filter(|f| !sightings.iter().any(|s| &&s.field == f))
                .collect();
            if !unset.is_empty() {
                let _ = writeln!(out, "  unmodelled and unset ({}):", unset.len());
                for field in unset {
                    let _ = writeln!(out, "    {field}");
                }
            }
        }
        let _ = writeln!(out);
    }

    let t = totals(coverage, report);
    let _ = writeln!(out, "{}", guarantee_line(&t));
    let _ = writeln!(out, "{}", verdict_line(&t));
    if !report.unreadable.is_empty() {
        let _ = writeln!(
            out,
            "\n{} field(s) the catalog lists could not be read and were not audited:",
            report.unreadable.len(),
        );
        for field in &report.unreadable {
            let _ = writeln!(out, "  {field}");
        }
    }
    out
}

fn render_markdown(coverage: &Coverage, report: &ScanReport, show_all: bool) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "## bidsmith drift\n");
    let mut any_rows = false;
    for (resource, cov) in &coverage.by_resource {
        let sightings = report.sightings.get(resource).map(Vec::as_slice).unwrap_or(&[]);
        if sightings.is_empty() && !show_all {
            continue;
        }
        any_rows = true;
        let _ = writeln!(
            out,
            "### `{resource}` — {} of {} settable field(s) modelled\n",
            cov.modelled, cov.settable,
        );
        if sightings.is_empty() {
            let _ = writeln!(out, "Nothing set outside the modelled fields.\n");
        } else {
            let _ = writeln!(out, "| Field | Resources | Example |");
            let _ = writeln!(out, "| --- | --- | --- |");
            for s in sightings {
                let _ = writeln!(
                    out,
                    "| `{}` | {} | `{}` = {} |",
                    md_cell(&s.field),
                    s.resources,
                    md_cell(&s.example_address),
                    md_cell(&s.example_value),
                );
            }
            let _ = writeln!(out);
        }
        if show_all {
            let unset: Vec<&String> = cov
                .unmodelled
                .iter()
                .filter(|f| !sightings.iter().any(|s| &&s.field == f))
                .collect();
            if !unset.is_empty() {
                // Folded away: this list runs to hundreds of fields and would
                // otherwise be the whole pull-request comment.
                let _ = writeln!(
                    out,
                    "<details><summary>{} unmodelled field(s) nothing has set</summary>\n",
                    unset.len(),
                );
                for field in unset {
                    let _ = writeln!(out, "- `{}`", md_cell(field));
                }
                let _ = writeln!(out, "\n</details>\n");
            }
        }
    }
    if !any_rows {
        let _ = writeln!(
            out,
            "No unmodelled field carries a value on the resources bidsmith manages.\n",
        );
    }
    let t = totals(coverage, report);
    let _ = writeln!(out, "**Coverage:** {}", guarantee_line(&t));
    let _ = writeln!(out, "\n{}", verdict_line(&t));
    if !report.unreadable.is_empty() {
        let _ = writeln!(
            out,
            "\n_{} field(s) the catalog lists could not be read and were not audited._",
            report.unreadable.len(),
        );
    }
    out
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(fields: &[&str]) -> BTreeSet<String> {
        fields.iter().map(|f| f.to_string()).collect()
    }

    #[test]
    fn a_selected_field_is_compared() {
        let sel = selected(&["campaign.name"]);
        assert!(is_compared("campaign.name", &sel));
        assert!(!is_compared("campaign.start_date", &sel));
    }

    #[test]
    fn selecting_a_message_whole_covers_its_leaves() {
        // `SELECT campaign.frequency_caps` puts every cap in front of the diff.
        let sel = selected(&["campaign.frequency_caps"]);
        assert!(is_compared("campaign.frequency_caps.key.level", &sel));
    }

    #[test]
    fn selecting_a_leaf_covers_the_message_that_holds_it() {
        // The catalog can offer the parent path too; a plan that reads a leaf
        // is already comparing part of that message, so it is not a gap of its
        // own — the sibling leaves still are.
        let sel = selected(&["campaign.network_settings.target_google_search"]);
        assert!(is_compared("campaign.network_settings", &sel));
        assert!(!is_compared("campaign.network_settings.target_youtube", &sel));
    }

    #[test]
    fn a_shared_prefix_that_is_not_a_path_boundary_is_not_coverage() {
        let sel = selected(&["ad_group.cpc_bid_micros"]);
        assert!(!is_compared("ad_group.cpc_bid_micros_extra", &sel));
        assert!(!is_compared("ad_group_ad.status", &selected(&["ad_group.status"])));
    }

    #[test]
    fn lower_camel_matches_rest_row_keys() {
        assert_eq!(lower_camel("ad_group_ad"), "adGroupAd");
        assert_eq!(lower_camel("campaign"), "campaign");
        assert_eq!(lower_camel("positive_geo_target_type"), "positiveGeoTargetType");
    }

    #[test]
    fn lookup_walks_a_dotted_path_into_the_row_body() {
        let body = serde_json::json!({
            "geoTargetTypeSetting": { "positiveGeoTargetType": "PRESENCE" },
            "name": "Winter",
        });
        assert_eq!(
            lookup(&body, "campaign", "campaign.geo_target_type_setting.positive_geo_target_type"),
            Some(&Value::String("PRESENCE".into())),
        );
        assert_eq!(lookup(&body, "campaign", "campaign.missing"), None);
        assert_eq!(lookup(&body, "campaign", "ad_group.name"), None);
    }

    #[test]
    fn placeholder_enum_reports_are_not_settings() {
        assert!(is_unreported(&Value::String("UNSPECIFIED".into())));
        assert!(is_unreported(&Value::String("UNKNOWN".into())));
        assert!(is_unreported(&serde_json::json!([])));
        assert!(!is_unreported(&Value::String("PRESENCE".into())));
        assert!(!is_unreported(&Value::Bool(false)));
    }

    #[test]
    fn a_value_renders_the_way_a_plan_row_does() {
        assert_eq!(render_value(&Value::String("OPTIMIZE".into())), "\"OPTIMIZE\"");
        assert_eq!(render_value(&Value::Bool(true)), "true");
        let long = Value::String("x".repeat(200));
        assert!(render_value(&long).ends_with('…'));
        assert!(render_value(&long).chars().count() <= MAX_SHOWN_VALUE + 1);
    }

    #[test]
    fn asset_subtypes_audit_as_one_api_resource() {
        assert_eq!(api_resource("call_asset"), Some("asset"));
        assert_eq!(api_resource("youtube_video_asset"), Some("asset"));
        assert_eq!(api_resource("campaign_asset"), Some("campaign_asset"));
        assert_eq!(api_resource("not_a_kind"), None);
    }

    #[test]
    fn only_resources_that_exist_live_are_audited() {
        let report = diff::DiffReport {
            diffs: vec![
                diff::ResourceDiff {
                    address: "google_ads_campaign.a".into(),
                    kind: "campaign",
                    action: diff::Action::NoOp { live_id: "111".into() },
                },
                diff::ResourceDiff {
                    address: "google_ads_campaign.b".into(),
                    kind: "campaign",
                    action: diff::Action::Create,
                },
            ],
            ..Default::default()
        };
        let managed = managed_resources(&report);
        let campaigns = managed.get("campaign").unwrap();
        assert_eq!(campaigns.len(), 1, "a pending create has nothing to audit");
        assert_eq!(campaigns.get("111").map(String::as_str), Some("google_ads_campaign.a"));
    }

    fn coverage_fixture() -> Coverage {
        let mut by_resource = BTreeMap::new();
        by_resource.insert(
            "campaign".to_string(),
            ResourceCoverage {
                settable: 10,
                modelled: 4,
                unmodelled: vec!["campaign.tracking_url_template".into()],
            },
        );
        Coverage { by_resource }
    }

    #[test]
    fn the_verdict_says_whether_anything_is_actually_set() {
        let clean = ScanReport::default();
        let t = totals(&coverage_fixture(), &clean);
        assert_eq!(t.settable, 10);
        assert_eq!(t.modelled, 4);
        assert!(verdict_line(&t).starts_with("No unmodelled field"));

        let mut dirty = ScanReport::default();
        dirty.sightings.insert(
            "campaign".to_string(),
            vec![Sighting {
                field: "campaign.tracking_url_template".into(),
                resources: 3,
                example_address: "google_ads_campaign.a".into(),
                example_value: "\"{lpurl}\"".into(),
            }],
        );
        let t = totals(&coverage_fixture(), &dirty);
        assert_eq!(t.set_fields, 1);
        assert!(verdict_line(&t).contains("never compares"));
    }

    #[test]
    fn the_guarantee_line_names_both_sides_of_the_ratio() {
        let t = totals(&coverage_fixture(), &ScanReport::default());
        let line = guarantee_line(&t);
        assert!(line.contains("4 field(s)"));
        assert!(line.contains("out of 10"));
    }

    fn scan_fixture() -> ScanReport {
        let mut report = ScanReport::default();
        report.sightings.insert(
            "campaign".to_string(),
            vec![Sighting {
                field: "campaign.tracking_url_template".into(),
                resources: 3,
                example_address: "google_ads_campaign.brand".into(),
                example_value: "\"{lpurl}?src=g\"".into(),
            }],
        );
        report
    }

    #[test]
    fn the_text_report_names_the_field_its_reach_and_an_example() {
        let out = render_text(&coverage_fixture(), &scan_fixture(), false);
        assert!(out.contains("campaign — 4 of 10 settable field(s) modelled"));
        assert!(out.contains("campaign.tracking_url_template"));
        assert!(out.contains("3 resource(s)"));
        assert!(out.contains("google_ads_campaign.brand = \"{lpurl}?src=g\""));
        assert!(out.contains("never compares"));
    }

    #[test]
    fn a_clean_account_still_reports_its_coverage() {
        let out = render_text(&coverage_fixture(), &ScanReport::default(), false);
        assert!(out.contains("nothing set outside the modelled fields."));
        assert!(out.contains("No unmodelled field carries a value"));
        assert!(out.contains("out of 10"));
    }

    #[test]
    fn all_lists_the_unmodelled_fields_nothing_has_set() {
        let quiet = render_text(&coverage_fixture(), &ScanReport::default(), false);
        assert!(!quiet.contains("unmodelled and unset"));
        let verbose = render_text(&coverage_fixture(), &ScanReport::default(), true);
        assert!(verbose.contains("unmodelled and unset (1)"));
        assert!(verbose.contains("campaign.tracking_url_template"));
    }

    #[test]
    fn a_field_that_is_set_is_not_repeated_in_the_unset_list() {
        let out = render_text(&coverage_fixture(), &scan_fixture(), true);
        assert!(!out.contains("unmodelled and unset"));
    }

    #[test]
    fn the_markdown_report_is_a_table_a_pr_comment_can_carry() {
        let out = render_markdown(&coverage_fixture(), &scan_fixture(), false);
        assert!(out.starts_with("## bidsmith drift"));
        assert!(out.contains("| Field | Resources | Example |"));
        assert!(out.contains("| `campaign.tracking_url_template` | 3 |"));
        assert!(out.contains("**Coverage:**"));
    }

    #[test]
    fn markdown_folds_the_long_unset_list_away() {
        let out = render_markdown(&coverage_fixture(), &ScanReport::default(), true);
        assert!(out.contains("<details><summary>1 unmodelled field(s) nothing has set"));
        assert!(out.contains("- `campaign.tracking_url_template`"));
        assert!(out.contains("</details>"));
    }

    #[test]
    fn a_clean_markdown_report_skips_the_per_resource_sections() {
        let out = render_markdown(&coverage_fixture(), &ScanReport::default(), false);
        assert!(!out.contains("| Field |"));
        assert!(out.contains("No unmodelled field carries a value"));
        assert!(out.contains("**Coverage:**"));
    }

    #[test]
    fn a_resource_the_catalog_knows_nothing_about_is_not_reported_as_covered() {
        let mut by_resource = BTreeMap::new();
        by_resource.insert(
            "campaign".to_string(),
            ResourceCoverage { settable: 0, modelled: 0, unmodelled: Vec::new() },
        );
        let out = render_text(&Coverage { by_resource }, &ScanReport::default(), false);
        assert!(out.contains("not audited"));
        assert!(
            !out.contains("0 of 0 settable"),
            "an empty catalog must not read as complete coverage",
        );
    }

    #[test]
    fn a_field_the_api_refused_is_reported_not_silently_dropped() {
        let mut report = scan_fixture();
        report.unreadable.push("campaign.hotel_setting.hotel_center_id".into());
        let text = render_text(&coverage_fixture(), &report, false);
        assert!(text.contains("could not be read and were not audited"));
        assert!(text.contains("campaign.hotel_setting.hotel_center_id"));
        let md = render_markdown(&coverage_fixture(), &report, false);
        assert!(md.contains("could not be read and were not audited"));
    }
}

use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::api::client;
use crate::api::live_state;

const TEMPLATE: &str = include_str!("../../templates/design-doc.html.tmpl");
const INIT_TOML: &str = include_str!("../../templates/design-doc.toml.tmpl");

#[derive(Debug, Deserialize)]
pub struct DesignDocConfig {
    pub applicant_legal_entity: String,
    pub manager_account_id: String,
    #[serde(default)]
    pub manager_account_name: String,
    pub contact_name: String,
    pub contact_email: String,
    #[serde(default)]
    pub company_url: String,

    #[serde(default = "default_tool_name")]
    pub tool_name: String,
    #[serde(default = "default_source_repo_url")]
    pub source_repo_url: String,

    pub why_this_tool_exists: String,
    pub who_uses_it_operators: String,

    #[serde(default = "default_volume_typical")]
    pub volume_typical_per_day: String,
    #[serde(default = "default_volume_ceiling")]
    pub volume_ceiling: String,

    #[serde(default)]
    pub document_date: String,
    #[serde(default)]
    pub closing_note: String,
}

fn default_tool_name() -> String {
    "bidsmith".into()
}

fn default_source_repo_url() -> String {
    "https://github.com/chrmod/bidsmith".into()
}

fn default_volume_typical() -> String {
    "well under 1,000 operations".into()
}

fn default_volume_ceiling() -> String {
    "~5,000 operations".into()
}

pub fn run_init(output: &str, force: bool) -> ExitCode {
    let path = Path::new(output);
    if path.exists() && !force {
        eprintln!(
            "design-doc init: {output} already exists. Pass --force to overwrite.",
        );
        return ExitCode::from(1);
    }
    if let Err(e) = fs::write(path, INIT_TOML) {
        eprintln!("design-doc init: failed to write {output}: {e}");
        return ExitCode::from(1);
    }
    eprintln!(
        "Wrote {output}. Edit it, then run `bidsmith design-doc render`.",
    );
    ExitCode::SUCCESS
}

pub fn run_render(config_path: &str, output: &str) -> ExitCode {
    let raw = match fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("design-doc render: failed to read {config_path}: {e}");
            return ExitCode::from(1);
        }
    };
    let mut cfg: DesignDocConfig = match toml::from_str(&raw) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("design-doc render: {config_path} is not valid TOML:\n{e}");
            return ExitCode::from(1);
        }
    };
    if let Err(e) = validate_config(&cfg) {
        eprintln!("design-doc render: {e}");
        return ExitCode::from(1);
    }
    if cfg.document_date.is_empty() {
        cfg.document_date = today_ymd();
    }

    let ctx = build_context(&cfg);
    let rendered = match render(&ctx) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("design-doc render: template error: {e}");
            return ExitCode::from(1);
        }
    };

    if output == "-" {
        if let Err(e) = io::stdout().write_all(rendered.as_bytes()) {
            eprintln!("design-doc render: failed to write stdout: {e}");
            return ExitCode::from(1);
        }
    } else if let Err(e) = fs::write(output, &rendered) {
        eprintln!("design-doc render: failed to write {output}: {e}");
        return ExitCode::from(1);
    }
    if output != "-" {
        eprintln!(
            "Wrote {output}. Open it in a browser and print to PDF to attach to Google's application form.",
        );
    }
    ExitCode::SUCCESS
}

fn validate_config(cfg: &DesignDocConfig) -> Result<(), String> {
    let required = [
        ("applicant_legal_entity", &cfg.applicant_legal_entity),
        ("manager_account_id", &cfg.manager_account_id),
        ("contact_name", &cfg.contact_name),
        ("contact_email", &cfg.contact_email),
        ("why_this_tool_exists", &cfg.why_this_tool_exists),
        ("who_uses_it_operators", &cfg.who_uses_it_operators),
    ];
    let missing: Vec<&str> = required
        .iter()
        .filter(|(_, v)| v.trim().is_empty() || v.trim().starts_with("<FILL IN"))
        .map(|(k, _)| *k)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "required fields not filled in: {}",
            missing.join(", "),
        ));
    }
    if !cfg.manager_account_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "manager_account_id must be 10 digits, no dashes; got '{}'",
            cfg.manager_account_id,
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct Endpoint {
    endpoint: String,
    method: &'static str,
    used_by: &'static str,
    validate_only: &'static str,
}

#[derive(Serialize)]
struct GaqlQuery {
    label: &'static str,
}

#[derive(Serialize)]
struct RmfRow {
    requirement: &'static str,
    satisfied_by: &'static str,
}

#[derive(Serialize)]
struct Context {
    applicant_legal_entity: String,
    manager_account_id: String,
    manager_account_name: String,
    contact_name: String,
    contact_email: String,
    company_url: String,
    company_url_label: String,
    tool_name: String,
    source_repo_url: String,
    source_repo_url_label: String,
    bidsmith_version: &'static str,
    api_version: String,
    oauth_scope: &'static str,
    document_date: String,
    closing_note: String,
    why_this_tool_exists_paragraphs: Vec<String>,
    who_uses_it_operators: String,
    volume_typical_per_day: String,
    volume_ceiling: String,
    endpoints: Vec<Endpoint>,
    gaql_queries: Vec<GaqlQuery>,
    gaql_query_count: usize,
    rmf_table: Vec<RmfRow>,
}

fn build_context(cfg: &DesignDocConfig) -> Context {
    let api_v = client::api_version();
    let endpoints = vec![
        Endpoint {
            endpoint: "oauth2.googleapis.com/token".into(),
            method: "POST",
            used_by: "auth (every run, plus the one-time authorization-code exchange in `auth login`)",
            validate_only: "—",
        },
        Endpoint {
            endpoint: format!("/{api_v}/customers:listAccessibleCustomers"),
            method: "GET",
            used_by: "auth login / auth status (account discovery)",
            validate_only: "n/a (read)",
        },
        Endpoint {
            endpoint: format!(
                "/{api_v}/customers/{{id}}/googleAds:searchStream"
            ),
            method: "POST",
            used_by: "plan, apply, pull, refresh, query, drift",
            validate_only: "n/a (read)",
        },
        Endpoint {
            endpoint: format!("/{api_v}/googleAdsFields:search"),
            method: "POST",
            used_by: "drift (GoogleAdsFieldService — which fields a resource exposes; account-independent metadata)",
            validate_only: "n/a (read)",
        },
        Endpoint {
            endpoint: "googleads.googleapis.com/$discovery/rest".into(),
            method: "GET",
            used_by: "drift (public discovery document — which fields are settable rather than output-only; unauthenticated)",
            validate_only: "n/a (read)",
        },
        Endpoint {
            endpoint: format!("/{api_v}/customers/{{id}}:generateKeywordIdeas"),
            method: "POST",
            used_by: "keyword-ideas (KeywordPlanIdeaService — read-only keyword research)",
            validate_only: "n/a (read)",
        },
        Endpoint {
            endpoint: format!("/{api_v}/customers/{{id}}/googleAds:mutate"),
            method: "POST",
            used_by: "plan (validateOnly=true), apply (validateOnly=false only after prompt)",
            validate_only: "both branches",
        },
        Endpoint {
            endpoint: format!("/{api_v}/customers/{{id}}/customAudiences:mutate"),
            method: "POST",
            used_by: "plan / apply — custom audiences, which GoogleAdsService.Mutate cannot batch",
            validate_only: "both branches",
        },
    ];

    let gaql_queries: Vec<GaqlQuery> = live_state::QUERIES
        .iter()
        .map(|(label, _)| GaqlQuery { label })
        .collect();
    let gaql_query_count = gaql_queries.len();

    Context {
        applicant_legal_entity: html_escape(&cfg.applicant_legal_entity),
        manager_account_id: html_escape(&cfg.manager_account_id),
        manager_account_name: html_escape(&cfg.manager_account_name),
        contact_name: html_escape(&cfg.contact_name),
        contact_email: html_escape(&cfg.contact_email),
        company_url_label: html_escape(&domain_of(&cfg.company_url)),
        company_url: html_escape(&cfg.company_url),
        tool_name: html_escape(&cfg.tool_name),
        source_repo_url_label: html_escape(&strip_scheme(&cfg.source_repo_url)),
        source_repo_url: html_escape(&cfg.source_repo_url),
        bidsmith_version: env!("CARGO_PKG_VERSION"),
        api_version: api_v,
        oauth_scope: "https://www.googleapis.com/auth/adwords",
        document_date: html_escape(&cfg.document_date),
        closing_note: html_escape(&cfg.closing_note),
        why_this_tool_exists_paragraphs: split_paragraphs(&cfg.why_this_tool_exists)
            .into_iter()
            .map(|p| html_escape(&p))
            .collect(),
        who_uses_it_operators: html_escape(&cfg.who_uses_it_operators),
        volume_typical_per_day: html_escape(&cfg.volume_typical_per_day),
        volume_ceiling: html_escape(&cfg.volume_ceiling),
        endpoints,
        gaql_queries,
        gaql_query_count,
        rmf_table: rmf_table(),
    }
}

fn render(ctx: &Context) -> Result<String, minijinja::Error> {
    let mut env = minijinja::Environment::new();
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::None);
    env.add_template("design-doc", TEMPLATE)?;
    let tmpl = env.get_template("design-doc")?;
    tmpl.render(ctx)
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn split_paragraphs(s: &str) -> Vec<String> {
    s.split("\n\n")
        .map(|p| p.trim().replace('\n', " "))
        .filter(|p| !p.is_empty())
        .collect()
}

fn strip_scheme(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

fn domain_of(url: &str) -> String {
    let s = strip_scheme(url);
    s.split('/').next().unwrap_or("").to_string()
}

fn today_ymd() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let (y, m, d) = unix_days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn unix_days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

fn rmf_table() -> Vec<RmfRow> {
    vec![
        RmfRow {
            requirement: "Display account-level information (name, currency, time zone, status)",
            satisfied_by: "<code>bidsmith query 'SELECT customer.descriptive_name, customer.currency_code, customer.time_zone, customer.status FROM customer'</code>; the account ids are surfaced by <code>plan --whoami</code> and the currency by <code>plan --read-live</code>, which every <code>plan</code> also uses to report committed daily spend as money.",
        },
        RmfRow {
            requirement: "Show campaigns: name, status, budget, type, start/end dates",
            satisfied_by: "Every <code>plan</code> reads and prints these fields; <code>refresh</code> materialises them into <code>.bid</code> source where they're directly editable.",
        },
        RmfRow {
            requirement: "Show ad groups: name, status, default bid",
            satisfied_by: "Fetched via the <code>ad_group</code> GAQL query, surfaced in <code>plan</code> output and <code>.bid</code> source.",
        },
        RmfRow {
            requirement: "Show ads (RSA): headlines, descriptions, final URLs, status",
            satisfied_by: "Modelled as <code>google_ads_ad_group_ad</code> with repeating <code>headline { text, pin? }</code> and <code>description { text, pin? }</code> sub-blocks. Edited and displayed in <code>.bid</code> source.",
        },
        RmfRow {
            requirement: "Show keywords: text, match type, status, bid",
            satisfied_by: "Modelled as <code>google_ads_ad_group_criterion</code> (positive/negative keyword with <code>match_type</code> and optional <code>cpc_bid_micros</code>).",
        },
        RmfRow {
            requirement: "Show and edit audience / placement targeting",
            satisfied_by: "Modelled as sub-blocks on <code>google_ads_campaign_criterion</code> and <code>google_ads_ad_group_criterion</code> (<code>audience</code>, <code>youtube_channel</code>, <code>youtube_video</code>, <code>topic</code>, <code>user_interest</code>, <code>age_range</code>, <code>gender</code>, plus <code>placement</code> / <code>parental_status</code> / <code>income_range</code> / <code>location</code> / <code>language</code> at ad-group scope), each usable as an exclusion via <code>negative = true</code>. Search-intent segments are built declaratively as <code>google_ads_custom_audience</code> via <code>CustomAudienceService</code>.",
        },
        RmfRow {
            requirement: "Pause / enable campaigns and ad groups",
            satisfied_by: "The <code>status</code> attribute on every campaign / ad-group / ad / criterion resource is editable. Toggling it produces a <code>~ update (status: &quot;PAUSED&quot; -&gt; &quot;ENABLED&quot;)</code> diff in <code>plan</code> and is applied via the normal mutate flow.",
        },
        RmfRow {
            requirement: "Adjust budgets",
            satisfied_by: "<code>google_ads_campaign_budget.amount_micros</code> is a first-class scalar field, mutable via the same plan/apply flow.",
        },
        RmfRow {
            requirement: "Respect Google Ads policies",
            satisfied_by: "The <code>plan</code> stage uses Google Ads' own <code>validateOnly=true</code> to surface policy violations <em>before</em> any mutate. Real mutate is skipped if validateOnly rejected anything.",
        },
        RmfRow {
            requirement: "Required disclosures for political advertising (EU)",
            satisfied_by: "The schema includes <code>contains_eu_political_advertising</code> on every campaign and defaults to <code>DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING</code> at mutate time when omitted, matching Google's API requirement.",
        },
        RmfRow {
            requirement: "Reporting access",
            satisfied_by: "<code>bidsmith query</code> is a read-only GAQL passthrough that exposes the full reporting surface (campaign / ad-group / keyword performance metrics, conversion stats, etc.) via the same <code>googleAds:searchStream</code> endpoint.",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_config() -> DesignDocConfig {
        DesignDocConfig {
            applicant_legal_entity: "Acme GmbH".into(),
            manager_account_id: "1234567890".into(),
            manager_account_name: "Acme MCC".into(),
            contact_name: "Jane Doe".into(),
            contact_email: "jane@acme.example".into(),
            company_url: "https://acme.example".into(),
            tool_name: "bidsmith".into(),
            source_repo_url: "https://github.com/chrmod/bidsmith".into(),
            why_this_tool_exists: "First paragraph.\n\nSecond paragraph.".into(),
            who_uses_it_operators: "two named ops engineers".into(),
            volume_typical_per_day: default_volume_typical(),
            volume_ceiling: default_volume_ceiling(),
            document_date: "2026-01-15".into(),
            closing_note: String::new(),
        }
    }

    #[test]
    fn unix_days_to_ymd_known_dates() {
        assert_eq!(unix_days_to_ymd(0), (1970, 1, 1));
        assert_eq!(unix_days_to_ymd(20_453), (2025, 12, 31));
        assert_eq!(unix_days_to_ymd(20_454), (2026, 1, 1));
        assert_eq!(unix_days_to_ymd(11_016), (2000, 2, 29));
    }

    #[test]
    fn split_paragraphs_splits_on_blank_lines() {
        let out = split_paragraphs("one\nline\n\ntwo\n\nthree");
        assert_eq!(out, vec!["one line", "two", "three"]);
    }

    #[test]
    fn strip_scheme_drops_https_and_trailing_slash() {
        assert_eq!(strip_scheme("https://example.com/"), "example.com");
        assert_eq!(strip_scheme("http://example.com/path"), "example.com/path");
    }

    #[test]
    fn domain_of_returns_host_only() {
        assert_eq!(domain_of("https://example.com/path"), "example.com");
    }

    #[test]
    fn html_escape_handles_xss_set() {
        assert_eq!(html_escape("a<b>c&d\"e'f"), "a&lt;b&gt;c&amp;d&quot;e&#39;f");
        assert_eq!(html_escape("plain/slash"), "plain/slash");
    }

    #[test]
    fn validate_rejects_empty_required_fields() {
        let mut cfg = good_config();
        cfg.applicant_legal_entity.clear();
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("applicant_legal_entity"), "{err}");
    }

    #[test]
    fn validate_rejects_fill_in_placeholder() {
        let mut cfg = good_config();
        cfg.contact_name = "<FILL IN: name>".into();
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("contact_name"), "{err}");
    }

    #[test]
    fn validate_rejects_non_numeric_manager_account() {
        let mut cfg = good_config();
        cfg.manager_account_id = "123-456-7890".into();
        let err = validate_config(&cfg).unwrap_err();
        assert!(err.contains("10 digits"), "{err}");
    }

    #[test]
    fn validate_accepts_good_config() {
        assert!(validate_config(&good_config()).is_ok());
    }

    #[test]
    fn render_with_fixture_produces_personalized_html() {
        let cfg = good_config();
        let ctx = build_context(&cfg);
        let html = render(&ctx).expect("render");

        assert!(html.contains("Acme GmbH"));
        assert!(html.contains("1234567890"));
        assert!(html.contains("Acme MCC"));
        assert!(html.contains("jane@acme.example"));
        assert!(html.contains("2026-01-15"));
        assert!(html.contains("First paragraph."));
        assert!(html.contains("Second paragraph."));

        assert!(!html.contains("<FILL IN"));
        assert!(!html.contains("{{"));
        assert!(!html.contains("{%"));
    }

    #[test]
    fn render_lists_every_endpoint_the_client_can_call() {
        // The lockstep rule: a new API call site has to reach the applicant's
        // design document in the same change, or the document understates what
        // the tool does with the token it is asking for.
        let html = render(&build_context(&good_config())).expect("render");
        for endpoint in [
            "customers:listAccessibleCustomers",
            "googleAds:searchStream",
            "googleAdsFields:search",
            "$discovery/rest",
            ":generateKeywordIdeas",
            "googleAds:mutate",
            "customAudiences:mutate",
        ] {
            assert!(html.contains(endpoint), "missing endpoint '{endpoint}' in output");
        }
    }

    #[test]
    fn render_includes_all_introspected_queries() {
        let html = render(&build_context(&good_config())).expect("render");
        for (label, _) in live_state::QUERIES {
            assert!(
                html.contains(&format!("<code>{label}</code>")),
                "missing GAQL query '{label}' in output",
            );
        }
    }

    #[test]
    fn render_includes_current_api_version() {
        let html = render(&build_context(&good_config())).expect("render");
        assert!(
            html.contains(&format!("currently <code>{}</code>", client::api_version())),
            "API version not rendered in §4.1 lead paragraph",
        );
    }

    #[test]
    fn render_does_not_double_escape_slashes() {
        let html = render(&build_context(&good_config())).expect("render");
        assert!(!html.contains("&#x2f;"));
        assert!(html.contains("googleapis.com/auth/adwords"));
    }

    #[test]
    fn init_template_round_trips_through_validator() {
        let parsed: Result<DesignDocConfig, _> = toml::from_str(INIT_TOML);
        let cfg = parsed.expect("init template parses as TOML");
        let err = validate_config(&cfg)
            .expect_err("unfilled init template should fail validation");
        assert!(err.contains("applicant_legal_entity"), "{err}");
    }
}

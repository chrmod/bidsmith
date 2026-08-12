use std::borrow::Cow;

use hcl_edit::Span;
use hcl_edit::expr::Expression;
use hcl_edit::structure::{Attribute, Block, Body, Structure};

use crate::diagnostics::Diag;
use crate::parser::ParsedFile;
use crate::schema::{Bindings, InputBindings};

const MIN_HEADLINES: usize = 3;
const MIN_DESCRIPTIONS: usize = 2;
const MAX_HEADLINE_LEN: usize = 30;
const MAX_DESCRIPTION_LEN: usize = 90;
const MAX_PATH_LEN: usize = 15;

const SUSPICIOUS_LANGUAGE_CONSTANTS: &[(&str, &str, &str)] = &[
    (
        "languageConstants/1045",
        "Afar",
        "did you mean languageConstants/1030 (Polish)?",
    ),
];

pub fn lint_files(files: &[ParsedFile], inputs: &InputBindings) -> Vec<Diag> {
    // Expansion + binding diags belong to `validate`; lint only reports its own warnings.
    let (expanded, _expand_diags) = crate::expand::expand_resource_for_each(files, inputs);
    let (bindings, _) = Bindings::build(&expanded, inputs);
    let mut diags = Vec::new();
    for f in &expanded {
        lint_file(f, &bindings, &mut diags);
    }
    diags
}

fn lint_file(file: &ParsedFile, bindings: &Bindings, diags: &mut Vec<Diag>) {
    for s in file.body.iter() {
        let Structure::Block(b) = s else { continue };
        match b.ident.as_str() {
            "resource" if b.labels.len() == 2 => lint_resource(file, b, bindings, diags),
            // Lint the template's RSA once at its declaration; referencing ad_group_ads carry no `ad` block.
            "ad_template" if b.labels.len() == 1 => {
                if let Some(rsa) = find_block(&b.body, "responsive_search_ad") {
                    let address = format!("ad_template.{}", b.labels[0].as_str());
                    lint_rsa(file, rsa, &address, bindings, diags);
                }
            }
            _ => {}
        }
    }
}

fn lint_resource(file: &ParsedFile, block: &Block, bindings: &Bindings, diags: &mut Vec<Diag>) {
    let ty = block.labels[0].as_str();
    let name = block.labels[1].as_str();
    let address = format!("{ty}.{name}");

    if ty == "google_ads_ad_group_ad" {
        if let Some(ad_block) = find_block(&block.body, "ad") {
            if let Some(rsa_block) = find_block(&ad_block.body, "responsive_search_ad") {
                lint_rsa(file, rsa_block, &address, bindings, diags);
            }
            if let Some(dg_block) = find_block(&ad_block.body, "demand_gen_video_responsive_ad") {
                lint_demand_gen_video_ad(file, dg_block, &address, diags);
            }
        }
        // path1/path2 set at the resource level override a template's RSA paths; lint them too.
        lint_rsa_paths(file, &block.body, &address, bindings, diags);
    }

    if ty == "google_ads_campaign" {
        lint_frequency_caps(file, block, &address, bindings, diags);
        lint_flight_window(file, block, &address, bindings, diags);
    }

    if matches!(
        ty,
        "google_ads_campaign_criterion" | "google_ads_ad_group_criterion"
    ) {
        if let Some(lang_block) = find_block(&block.body, "language") {
            lint_language(file, lang_block, &address, bindings, diags);
        }
        lint_undetermined_demographic(file, block, &address, bindings, diags);
    }
}

/// A flight that ends before it starts is a typo with a silent consequence:
/// Google accepts the campaign and it simply never delivers. Dates sort
/// lexically in `YYYY-MM-DD`, and both are validated as real dates before this
/// runs, so a string compare is the whole check.
fn lint_flight_window(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    let Some(end_attr) = find_attr(&block.body, "end_date") else {
        return;
    };
    let (Some(start), Some(end)) = (
        find_attr(&block.body, "start_date")
            .and_then(|a| eval_str(bindings, &file.module, &a.value)),
        eval_str(bindings, &file.module, &end_attr.value),
    ) else {
        return;
    };
    if end.as_str() < start.as_str() {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(end_attr.value.span()),
            format!(
                "{address} ends on {end} but starts on {start}: Google Ads accepts the campaign and it never delivers"
            ),
        ));
    }
}

/// Google can't classify age or gender for a large share of viewers, so the
/// `UNDETERMINED` bucket is not an edge case — excluding it is usually an
/// accidental reach cut rather than the narrowing the author intended.
fn lint_undetermined_demographic(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    let excluded = find_attr(&block.body, "negative")
        .map(|a| matches!(&a.value, Expression::Bool(b) if *b.as_ref()))
        .unwrap_or(false);
    if !excluded {
        return;
    }
    for (name, noun, undetermined) in [
        ("age_range", "age", "AGE_RANGE_UNDETERMINED"),
        ("gender", "gender", "UNDETERMINED"),
        ("parental_status", "parental status", "UNDETERMINED"),
        ("income_range", "household income", "INCOME_RANGE_UNDETERMINED"),
    ] {
        let Some(inner) = find_block(&block.body, name) else {
            continue;
        };
        let Some(attr) = find_attr(&inner.body, "type") else {
            continue;
        };
        if eval_str(bindings, &file.module, &attr.value).as_deref() != Some(undetermined) {
            continue;
        }
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(attr.value.span()),
            format!(
                "{address} excludes {undetermined}: Google Ads cannot determine {noun} for a large share of viewers, so this removes most of your reach rather than a small slice"
            ),
        ));
    }
}

fn lint_frequency_caps(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    let Some(caps) = find_block(&block.body, "frequency_caps") else {
        return;
    };
    let channel = find_attr(&block.body, "advertising_channel_type")
        .and_then(|a| eval_str(bindings, &file.module, &a.value));
    if channel.as_deref() == Some("DEMAND_GEN") {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(caps.ident.span()),
            format!(
                "{address} sets frequency_caps on a Demand Gen campaign: Google Ads does not support frequency capping for this channel, so the setting has no effect"
            ),
        ));
    }
}

fn lint_demand_gen_video_ad(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    diags: &mut Vec<Diag>,
) {
    if find_attr(&block.body, "business_name").is_none() {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "{address} omits business_name: the Google Ads API requires the advertiser/brand name on a Demand Gen video ad, so `apply` cannot create this ad without it"
            ),
        ));
    }
    if let Some(cta) = find_attr(&block.body, "call_to_actions") {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(cta.key.span()),
            format!(
                "{address} sets call_to_actions on a Demand Gen video ad: the API takes CALL_TO_ACTION asset references here rather than text, and bidsmith does not model that asset type yet — `apply` cannot create the ad while the attribute is set"
            ),
        ));
    }
}

fn lint_language(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    let Some(constant_attr) = find_attr(&block.body, "language_constant") else {
        return;
    };
    let Some(value) = eval_str(bindings, &file.module, &constant_attr.value) else {
        return;
    };
    let value = value.as_str();
    for (constant, name, hint) in SUSPICIOUS_LANGUAGE_CONSTANTS {
        if value == *constant {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(constant_attr.value.span()),
                format!(
                    "language_constant '{value}' in {address} is {name}, a rarely-targeted language — {hint}"
                ),
            ));
        }
    }
}

fn lint_rsa(file: &ParsedFile, rsa: &Block, address: &str, bindings: &Bindings, diags: &mut Vec<Diag>) {
    lint_rsa_block(
        file,
        rsa,
        address,
        "headline",
        MIN_HEADLINES,
        MAX_HEADLINE_LEN,
        bindings,
        diags,
    );
    lint_rsa_block(
        file,
        rsa,
        address,
        "description",
        MIN_DESCRIPTIONS,
        MAX_DESCRIPTION_LEN,
        bindings,
        diags,
    );
    lint_rsa_paths(file, &rsa.body, address, bindings, diags);
}

fn lint_rsa_paths(
    file: &ParsedFile,
    body: &Body,
    address: &str,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    for name in ["path1", "path2"] {
        let Some(attr) = find_attr(body, name) else {
            continue;
        };
        let Some(value) = eval_str(bindings, &file.module, &attr.value) else {
            continue;
        };
        let value = value.as_str();
        if value.chars().count() > MAX_PATH_LEN {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(attr.value.span()),
                format!(
                    "{name} in {address} is {len} characters; Google Ads truncates at {MAX_PATH_LEN}",
                    len = value.chars().count(),
                ),
            ));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(attr.value.span()),
                format!(
                    "{name} in {address} contains characters outside [a-z0-9-]; Google Ads display URLs only render lowercase letters, digits, and hyphens"
                ),
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lint_rsa_block(
    file: &ParsedFile,
    rsa: &Block,
    address: &str,
    label: &str,
    minimum: usize,
    max_len: usize,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    let blocks: Vec<&Block> = rsa
        .body
        .iter()
        .filter_map(|s| match s {
            Structure::Block(b) if b.ident.as_str() == label => Some(b),
            _ => None,
        })
        .collect();

    let list_attr_name = match label {
        "headline" => "headlines",
        "description" => "descriptions",
        _ => "",
    };
    let list_items: Vec<(Option<std::ops::Range<usize>>, Option<String>)> =
        find_attr(&rsa.body, list_attr_name)
            .map(|attr| collect_rsa_list_items(&attr.value, bindings, &file.module))
            .unwrap_or_default();

    let total = blocks.len() + list_items.len();
    if total < minimum {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(rsa.ident.span()),
            format!(
                "responsive_search_ad in {address} has only {total} {label}{plural} — Google Ads requires at least {minimum}",
                plural = if total == 1 { "" } else { "s" },
            ),
        ));
    }

    let mut idx = 0;
    for b in blocks.iter() {
        let Some(text_attr) = find_attr(&b.body, "text") else {
            idx += 1;
            continue;
        };
        let Some(text) = eval_str(bindings, &file.module, &text_attr.value) else {
            idx += 1;
            continue;
        };
        lint_rsa_text(
            file,
            address,
            label,
            idx,
            &text,
            max_len,
            span_of(text_attr.key.span()),
            span_of(text_attr.value.span()),
            diags,
        );
        idx += 1;
    }
    for (span, text) in &list_items {
        let Some(text) = text else {
            idx += 1;
            continue;
        };
        let s = span_of(span.clone());
        lint_rsa_text(
            file,
            address,
            label,
            idx,
            text,
            max_len,
            s.clone(),
            s,
            diags,
        );
        idx += 1;
    }
}

fn lint_rsa_text(
    file: &ParsedFile,
    address: &str,
    label: &str,
    idx: usize,
    text: &str,
    max_len: usize,
    key_span: std::ops::Range<usize>,
    value_span: std::ops::Range<usize>,
    diags: &mut Vec<Diag>,
) {
    if looks_like_phone(text) {
        diags.push(Diag::warning(
            file.src.clone(),
            key_span,
            format!(
                "{label}[{idx}] in {address} looks like a phone number; Google Ads policy disallows phone numbers in ad copy (use call extensions)"
            ),
        ));
    }
    let char_count = text.chars().count();
    if char_count > max_len {
        diags.push(Diag::warning(
            file.src.clone(),
            value_span,
            format!(
                "{label}[{idx}] in {address} is {char_count} characters; Google Ads rejects {label}s over {max_len}"
            ),
        ));
    }
}

fn collect_rsa_list_items(
    expr: &Expression,
    bindings: &Bindings,
    module: &str,
) -> Vec<(Option<std::ops::Range<usize>>, Option<String>)> {
    let resolved = bindings.resolve_value(module, expr);
    // A referenced list's item spans point into the (maybe other-file) declaration, so fall back to the use-site span.
    let from_ref = !matches!(&resolved, Cow::Borrowed(v) if std::ptr::eq(*v, expr));
    let fallback = expr.span();
    let Expression::Array(arr) = resolved.as_ref() else {
        return Vec::new();
    };
    arr.iter()
        .map(|item| {
            let span = if from_ref { fallback.clone() } else { item.span() };
            let text = match bindings.resolve_value(module, item).as_ref() {
                Expression::String(s) => Some(s.as_str().to_string()),
                Expression::Object(obj) => {
                    let mut text = None;
                    for (key, value) in obj.iter() {
                        let Some(ident) = key.as_ident() else { continue };
                        if ident.as_str() == "text" {
                            if let Expression::String(s) =
                                bindings.resolve_value(module, value.expr()).as_ref()
                            {
                                text = Some(s.as_str().to_string());
                            }
                        }
                    }
                    text
                }
                _ => None,
            };
            (span, text)
        })
        .collect()
}

fn eval_str(bindings: &Bindings, module: &str, expr: &Expression) -> Option<String> {
    match bindings.resolve_value(module, expr).as_ref() {
        Expression::String(s) => Some(s.as_str().to_string()),
        _ => None,
    }
}

fn looks_like_phone(s: &str) -> bool {
    let mut run = 0usize;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            run += 1;
            if run >= 7 {
                return true;
            }
        } else if matches!(ch, ' ' | '-' | '.' | '(' | ')' | '+' | '\u{a0}') {
            continue;
        } else {
            run = 0;
        }
    }
    false
}

fn find_attr<'a>(body: &'a Body, name: &str) -> Option<&'a Attribute> {
    body.iter().find_map(|s| match s {
        Structure::Attribute(a) if a.key.as_str() == name => Some(a),
        _ => None,
    })
}

fn find_block<'a>(body: &'a Body, name: &str) -> Option<&'a Block> {
    body.iter().find_map(|s| match s {
        Structure::Block(b) if b.ident.as_str() == name => Some(b),
        _ => None,
    })
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use std::io::Write;

    fn lint_str(name: &str, content: &str) -> Vec<String> {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("bidsmith-lint-test-{name}.bid"));
        {
            let mut f = std::fs::File::create(&tmp).expect("create tmp");
            f.write_all(content.as_bytes()).expect("write tmp");
        }
        let pf = parse_file(&tmp).expect("parse");
        lint_files(std::slice::from_ref(&pf), &InputBindings::default())
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    fn campaign_with_flight(name: &str, dates: &str) -> Vec<String> {
        lint_str(
            name,
            &format!(
                r#"
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
{dates}
}}
"#
            ),
        )
    }

    #[test]
    fn a_flight_that_ends_before_it_starts_warns() {
        let msgs = campaign_with_flight(
            "flight_backwards",
            "  start_date = \"2026-08-25\"\n  end_date   = \"2026-08-11\"",
        );
        assert!(
            msgs.iter().any(|m| m.contains("never delivers")),
            "{msgs:?}"
        );
    }

    #[test]
    fn an_ordinary_flight_window_is_quiet() {
        let msgs = campaign_with_flight(
            "flight_ok",
            "  start_date = \"2026-08-11\"\n  end_date   = \"2026-08-25\"",
        );
        assert!(
            !msgs.iter().any(|m| m.contains("never delivers")),
            "{msgs:?}"
        );
    }

    /// Same day is a legitimate one-day flight, not an error.
    #[test]
    fn a_single_day_flight_is_quiet() {
        let msgs = campaign_with_flight(
            "flight_one_day",
            "  start_date = \"2026-08-11\"\n  end_date   = \"2026-08-11\"",
        );
        assert!(
            !msgs.iter().any(|m| m.contains("never delivers")),
            "{msgs:?}"
        );
    }

    /// An open-ended campaign declares a start and no end; nothing to compare.
    #[test]
    fn a_start_without_an_end_is_quiet() {
        let msgs = campaign_with_flight("flight_open", "  start_date = \"2026-08-11\"");
        assert!(
            !msgs.iter().any(|m| m.contains("never delivers")),
            "{msgs:?}"
        );
    }

    fn campaign_with_caps(name: &str, channel: &str) -> Vec<String> {
        lint_str(
            name,
            &format!(
                r#"
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "{channel}"
  campaign_budget          = google_ads_campaign_budget.b.id

  frequency_caps {{
    event_type  = "IMPRESSION"
    time_unit   = "DAY"
    time_length = 1
    cap         = 3
  }}
}}
"#
            ),
        )
    }

    #[test]
    fn frequency_caps_on_demand_gen_warn() {
        let msgs = campaign_with_caps("caps_demand_gen", "DEMAND_GEN");
        assert!(
            msgs.iter().any(|m| m.contains("does not support frequency capping")),
            "{msgs:?}"
        );
    }

    #[test]
    fn frequency_caps_on_video_are_quiet() {
        let msgs = campaign_with_caps("caps_video", "VIDEO");
        assert!(
            !msgs.iter().any(|m| m.contains("frequency capping")),
            "{msgs:?}"
        );
    }

    fn video_campaign_with(name: &str, bidding: &str) -> Vec<String> {
        lint_str(
            name,
            &format!(
                r#"
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
{bidding}
}}
"#
            ),
        )
    }

    #[test]
    fn video_campaign_bidding_is_not_linted_offline() {
        // Whether a video campaign is new (uncreatable) or adopted (fine as-is)
        // is a question only `diff` can answer, so nothing is flagged here.
        for block in ["", "  manual_cpc {}", "  manual_cpv {}"] {
            let msgs = video_campaign_with("video_bidding_quiet", block);
            assert!(msgs.is_empty(), "{block}: {msgs:?}");
        }
    }

    #[test]
    fn video_campaign_with_a_video_strategy_is_quiet() {
        for (name, block) in [
            ("video_manual_cpv", "  manual_cpv {}"),
            ("video_target_cpv", "  target_cpv {}"),
            ("video_target_cpm", "  target_cpm {}"),
            ("video_manual_cpm", "  manual_cpm {}"),
        ] {
            let msgs = video_campaign_with(name, block);
            assert!(msgs.is_empty(), "{block}: {msgs:?}");
        }
    }

    #[test]
    fn search_campaign_without_bidding_is_quiet() {
        let msgs = lint_str(
            "search_no_bidding",
            r#"
resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}
"#,
        );
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    #[test]
    fn excluding_the_undetermined_bucket_warns() {
        let msgs = lint_str(
            "undetermined_gender",
            r#"
resource "google_ads_campaign_criterion" "no_unknown" {
  campaign = google_ads_campaign.v.id
  negative = true

  gender { type = "UNDETERMINED" }
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("removes most of your reach")),
            "{msgs:?}"
        );
    }

    #[test]
    fn the_undetermined_warning_covers_ad_group_targeting_too() {
        let msgs = lint_str(
            "undetermined_ad_group",
            r#"
resource "google_ads_ad_group_criterion" "no_unknown_income" {
  ad_group = google_ads_ad_group.g.id
  negative = true

  income_range { type = "INCOME_RANGE_UNDETERMINED" }
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("removes most of your reach")),
            "{msgs:?}"
        );
    }

    #[test]
    fn a_named_bracket_and_a_positive_undetermined_are_quiet() {
        let msgs = lint_str(
            "named_bracket",
            r#"
resource "google_ads_campaign_criterion" "no_kids" {
  campaign = google_ads_campaign.v.id
  negative = true

  age_range { type = "AGE_RANGE_18_24" }
}

resource "google_ads_campaign_criterion" "reach_unknown" {
  campaign = google_ads_campaign.v.id

  gender { type = "UNDETERMINED" }
}
"#,
        );
        assert!(msgs.is_empty(), "{msgs:?}");
    }

    fn rsa(headlines: &str, descriptions: &str) -> String {
        format!(
            r#"
resource "google_ads_ad_group_ad" "rsa" {{
  ad_group = google_ads_ad_group.g.id

  ad {{
    final_urls = ["https://example.com"]
    responsive_search_ad {{
      headlines    = {headlines}
      descriptions = {descriptions}
    }}
  }}
}}
"#
        )
    }

    #[test]
    fn list_local_meets_rsa_minimums_no_warning() {
        let content = format!(
            r#"
locals {{
  headlines    = ["One Headline", "Two Headline", "Three Headline"]
  descriptions = ["First description", "Second description"]
}}
{}"#,
            rsa("local.headlines", "local.descriptions")
        );
        let msgs = lint_str("list_local_ok", &content);
        assert!(
            !msgs.iter().any(|m| m.contains("Google Ads requires at least")),
            "unexpected min-count warning: {msgs:?}"
        );
    }

    #[test]
    fn list_local_below_minimum_still_warns() {
        let content = format!(
            r#"
locals {{
  headlines    = ["Only One Headline"]
  descriptions = ["First description", "Second description"]
}}
{}"#,
            rsa("local.headlines", "local.descriptions")
        );
        let msgs = lint_str("list_local_short", &content);
        assert!(
            msgs.iter()
                .any(|m| m.contains("has only 1 headline") && m.contains("at least 3")),
            "expected min-count warning: {msgs:?}"
        );
    }

    #[test]
    fn thin_ad_template_warns_at_declaration() {
        let msgs = lint_str(
            "thin_template",
            r#"
ad_template "thin" {
  final_urls = ["https://example.com"]
  responsive_search_ad {
    headlines    = ["Only One"]
    descriptions = ["Just one description here"]
  }
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("ad_template.thin")
                && m.contains("has only 1 headline")),
            "expected template min-headline warning: {msgs:?}"
        );
    }

    #[test]
    fn top_level_path_override_is_linted() {
        let msgs = lint_str(
            "path_override",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.shared
  path1    = "Get_Started"
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("path1 in google_ads_ad_group_ad.rsa")
                && m.contains("outside [a-z0-9-]")),
            "expected path-override charset warning: {msgs:?}"
        );
    }

    #[test]
    fn over_length_headline_in_referenced_list_warns() {
        let content = format!(
            r#"
locals {{
  headlines    = ["This headline is far too long for Google Ads to accept it", "Short Two", "Short Three"]
  descriptions = ["First description", "Second description"]
}}
{}"#,
            rsa("local.headlines", "local.descriptions")
        );
        let msgs = lint_str("list_local_long", &content);
        assert!(
            msgs.iter().any(|m| m.contains("characters; Google Ads rejects headlines over 30")),
            "expected length warning: {msgs:?}"
        );
    }

    #[test]
    fn concat_result_counts_toward_rsa_minimums() {
        let content = format!(
            r#"
locals {{
  specific = ["Stop Cookie Pop-Ups"]
  tail     = ["Add to Chrome, Free", "Open Source & Private"]
}}
{}"#,
            rsa(
                "concat(local.specific, local.tail)",
                r#"["First description", "Second description"]"#
            )
        );
        let msgs = lint_str("concat_min_ok", &content);
        assert!(
            !msgs.iter().any(|m| m.contains("Google Ads requires at least")),
            "unexpected min-count warning: {msgs:?}"
        );
    }

    #[test]
    fn over_length_item_in_concat_tail_warns() {
        let content = format!(
            r#"
locals {{
  specific = ["Stop Cookie Pop-Ups", "Short Two"]
  tail     = ["This shared brand tail headline is much too long to pass"]
}}
{}"#,
            rsa(
                "concat(local.specific, local.tail)",
                r#"["First description", "Second description"]"#
            )
        );
        let msgs = lint_str("concat_too_long", &content);
        assert!(
            msgs.iter()
                .any(|m| m.contains("characters; Google Ads rejects headlines over 30")),
            "expected length warning in concat tail: {msgs:?}"
        );
    }

    #[test]
    fn over_length_rendered_template_headline_warns() {
        let content = format!(
            r#"
locals {{
  brand = "The Extremely Long Brand Name Company"
}}
{}"#,
            rsa(
                r#"["Try ${local.brand} Today", "Short Two", "Short Three"]"#,
                r#"["First description", "Second description"]"#
            )
        );
        let msgs = lint_str("tmpl_headline_long", &content);
        assert!(
            msgs.iter()
                .any(|m| m.contains("characters; Google Ads rejects headlines over 30")),
            "expected length warning on rendered template: {msgs:?}"
        );
    }

    #[test]
    fn rendered_template_path_is_linted() {
        let msgs = lint_str(
            "tmpl_path",
            r#"
locals {
  slug = "Get_Started"
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.shared
  path1    = "x-${local.slug}"
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("path1 in google_ads_ad_group_ad.rsa")
                && m.contains("outside [a-z0-9-]")),
            "expected charset warning on rendered path: {msgs:?}"
        );
    }
}
